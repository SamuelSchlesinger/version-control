use std::{
    collections::BTreeSet,
    env::current_dir,
    fs::{create_dir, create_dir_all, read_dir, read_to_string, remove_file, File},
    io::Write,
    path::{Path, PathBuf},
};

use derive_more::From;
use serde::{Deserialize, Serialize};

use crate::{
    directory::{Directory, Ignores},
    merge::MergeResult,
    object_id::ObjectId,
    object_store::{directory::DirectoryObjectStore, ObjectStore},
    snapshot::SnapShot,
    remote::RemoteConfig,
};

/// A wrapper for the path of the .rev directory which has a number of utilities defined on it.
pub struct DotRev {
    root: PathBuf,
}

#[derive(Debug, From)]
pub enum Error {
    #[from]
    IO(std::io::Error),
    #[from]
    Serde(serde_json::Error),
    MissingObject(ObjectId),
    BranchNotFound(String),
    RepositoryNotInitialized,
    CorruptRepository(String),
    /// A merge is already in progress
    MergeInProgress,
    /// No merge is in progress
    NoMergeInProgress,
    /// A branch name is not usable as a single filesystem component
    InvalidBranchName {
        name: String,
        reason: &'static str,
    },
    /// A remote with the given name already exists
    RemoteExists(String),
}

/// Maximum length of a branch name, in bytes.
///
/// Branch names become file names under `.rev/branches`, so this stays well
/// below the 255-byte limit common to ext4/APFS/NTFS.
const MAX_BRANCH_NAME_LEN: usize = 200;

/// Checks that `name` is safe to use as a single path component under
/// `.rev/branches`.
///
/// Branch names reach this crate from the CLI and from remotes, and every
/// branch operation resolves to `.rev/branches/<name>`. Without this check a
/// name like `../../x` escapes the repository entirely, and an empty name
/// resolves to the `branches` directory itself. Rejecting the name is the only
/// thing standing between a remote and an arbitrary file write, so this is
/// deliberately a strict allowlist rather than a sanitiser: nothing is
/// rewritten or stripped, because silently accepting a *different* branch than
/// the user typed is its own bug.
pub fn validate_branch_name(name: &str) -> Result<(), Error> {
    let reject = |reason: &'static str| {
        Err(Error::InvalidBranchName {
            name: name.to_string(),
            reason,
        })
    };

    if name.is_empty() {
        return reject("it is empty");
    }
    if name.len() > MAX_BRANCH_NAME_LEN {
        return reject("it is longer than 200 bytes");
    }
    if name == "." || name == ".." {
        return reject("it refers to a directory rather than a branch");
    }
    if name.contains('/') || name.contains('\\') {
        return reject("it contains a path separator");
    }
    if name.contains('\0') {
        return reject("it contains a null byte");
    }
    if name.chars().any(|c| c.is_control()) {
        return reject("it contains a control character");
    }
    // Leading '-' would be swallowed as a flag by any CLI that echoes the name
    // back into a command line, and leading/trailing whitespace is invisible in
    // `revtool branch` output.
    if name.starts_with('-') {
        return reject("it starts with '-'");
    }
    if name.trim() != name {
        return reject("it has leading or trailing whitespace");
    }

    // Belt and braces: whatever the rules above allow must still be exactly one
    // ordinary path component. This catches anything platform-specific the
    // explicit checks miss (e.g. Windows drive-relative names like `C:x`).
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(c)), None) if c == name => Ok(()),
        _ => reject("it is not a simple name"),
    }
}

/// Current state of an in-progress merge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeState {
    /// The current branch (ours)
    pub current_branch: String,
    /// The branch being merged (theirs)
    pub merge_branch: String,
    /// The original merge result with conflicts
    pub merge_result: MergeResult,
    /// Original state backup - the snapshot ID to reset to if aborting
    pub backup_snapshot_id: ObjectId,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::IO(err) => write!(f, "I/O error: {}", err),
            Error::Serde(err) => write!(f, "Serialization error: {}", err),
            Error::MissingObject(id) => write!(f, "Missing object: {}", id),
            Error::BranchNotFound(branch) => write!(f, "Branch not found: {}", branch),
            Error::RepositoryNotInitialized => write!(f, "Repository not initialized. Use 'revtool init' first"),
            Error::CorruptRepository(msg) => write!(f, "Corrupt repository: {}", msg),
            Error::MergeInProgress => write!(f, "A merge is already in progress. Resolve conflicts and use 'revtool merge --continue' or use 'revtool merge --abort' to cancel"),
            Error::NoMergeInProgress => write!(f, "No merge is in progress"),
            Error::InvalidBranchName { name, reason } => write!(
                f,
                "Invalid branch name {name:?}: {reason}. Branch names must be a single \
                 name without '/' or '..'"
            ),
            Error::RemoteExists(name) => write!(f, "A remote named '{name}' already exists"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::IO(e) => Some(e),
            Error::Serde(e) => Some(e),
            _ => None,
        }
    }
}
impl DotRev {
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn init(root: PathBuf) -> Result<Self, Error> {
        if read_dir(&root).is_ok() {
            return Ok(Self { root });
        }
        create_dir_all(&root)?;

        // Start out on the dev branch
        let mut file = File::options()
            .create(true)
            .write(true)
            .truncate(true)
            .open(root.join("branch"))?;
        file.write_all("dev".as_bytes())?;

        // Create the branches directory
        create_dir(&root.join("branches"))?;

        // Create the init commit on the dev branch
        let mut store = DirectoryObjectStore::new(root.join("store"))?;
        let directory = Directory::default();
        let directory = store.insert_json(&directory)?;
        let snapshot = SnapShot {
            directory,
            message: String::from("init"),
            previous: BTreeSet::new(),
        };
        let snapshot_id = store.insert_json(&snapshot)?;
        write_json(&snapshot_id, &root.join("branches").join("dev"))?;
        let ignores = Ignores::default();
        write_json(&ignores, &root.join("ignores"))?;

        Ok(DotRev { root })
    }

    /// Finds the repository that contains the current directory by walking up
    /// the directory tree looking for a `.rev`, like git's `.git` discovery.
    /// This lets every command work from any subdirectory of the repository.
    pub fn here() -> Result<Self, Error> {
        let start = current_dir().map_err(Error::IO)?;
        let mut dir: &Path = start.as_path();
        loop {
            let candidate = dir.join(".rev");
            if read_dir(&candidate).is_ok() {
                return Ok(DotRev { root: candidate });
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => return Err(Error::RepositoryNotInitialized),
            }
        }
    }

    /// The working-tree root: the directory that contains `.rev`. Snapshot and
    /// status operations are relative to this, not to the current directory, so
    /// they behave the same from anywhere inside the repository.
    pub fn work_dir(&self) -> &Path {
        self.root.parent().unwrap_or_else(|| Path::new("."))
    }

    pub fn existing(root: PathBuf) -> Result<Self, Error> {
        match read_dir(&root) {
            Ok(_) => Ok(DotRev { root }),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Err(Error::RepositoryNotInitialized)
                } else {
                    Err(Error::IO(e))
                }
            }
        }
    }

    /// The only place a branch name is turned into a path.
    ///
    /// Every branch operation goes through here, so validation cannot be
    /// bypassed by adding a new call site.
    fn branch_path(&self, branch: &str) -> Result<PathBuf, Error> {
        validate_branch_name(branch)?;
        Ok(self.root.join("branches").join(branch))
    }

    pub fn branch(&self) -> Result<String, Error> {
        let branch = read_to_string(&self.root.join("branch"))?;
        // A `branch` file that fails validation means the repository state is
        // damaged, not that the user passed something bad, so it maps to a
        // corruption error rather than InvalidBranchName.
        validate_branch_name(&branch).map_err(|_| {
            Error::CorruptRepository(format!(
                "the current branch file contains an unusable branch name {branch:?}"
            ))
        })?;
        Ok(branch)
    }

    pub fn set_branch(&self, new_branch: &str) -> Result<(), Error> {
        validate_branch_name(new_branch)?;
        let mut file = File::options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(self.root.join("branch"))?;
        file.write_all(new_branch.as_bytes())?;
        Ok(())
    }

    pub fn branch_snapshot_id(&self, branch: &str) -> Result<ObjectId, Error> {
        let branch_path = self.branch_path(branch)?;
        if !Path::try_exists(&branch_path)? {
            return Err(Error::BranchNotFound(branch.to_string()));
        }
        read_json(&branch_path)
    }

    pub fn set_branch_snapshot_id(&self, branch: &str, object_id: ObjectId) -> Result<(), Error> {
        write_json(&object_id, &self.branch_path(branch)?)
    }

    pub fn current_snapshot_id(&self) -> Result<ObjectId, Error> {
        let branch = self.branch()?;
        self.branch_snapshot_id(&branch)
    }

    pub fn create_branch(&self, new_branch: &str) -> Result<(), Error> {
        if !self.branch_exists(new_branch)? {
            let snapshot_id = self.current_snapshot_id()?;
            return write_json(&snapshot_id, &self.branch_path(new_branch)?);
        }
        Ok(())
    }

    pub fn branch_exists(&self, branch: &str) -> Result<bool, Error> {
        Ok(Path::try_exists(&self.branch_path(branch)?)?)
    }

    pub fn list_branches(&self) -> Result<Vec<String>, Error> {
        let branches_dir = self.root.join("branches");
        let mut branches = Vec::new();

        for entry in read_dir(&branches_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(branch_name) = entry.file_name().to_str() {
                    branches.push(branch_name.to_string());
                }
            }
        }

        branches.sort();
        Ok(branches)
    }

    pub fn store(&self) -> Result<DirectoryObjectStore, Error> {
        Ok(DirectoryObjectStore::new(self.root.join("store").clone())?)
    }

    pub fn ignores(&self) -> Result<Ignores, Error> {
        Ok(read_json(&self.root.join("ignores"))?)
    }

    pub fn set_ignores(&self, ignores: &Ignores) -> Result<(), Error> {
        write_json(ignores, &self.root.join("ignores"))
    }

    /// Checks if a merge is in progress
    pub fn is_merge_in_progress(&self) -> Result<bool, Error> {
        let merge_state_path = self.root.join("merge_state");
        Ok(Path::try_exists(&merge_state_path)?)
    }

    /// Saves the current merge state
    pub fn save_merge_state(&self, state: &MergeState) -> Result<(), Error> {
        if self.is_merge_in_progress()? {
            return Err(Error::MergeInProgress);
        }
        write_json(state, &self.root.join("merge_state"))
    }

    /// Gets the current merge state
    pub fn get_merge_state(&self) -> Result<MergeState, Error> {
        if !self.is_merge_in_progress()? {
            return Err(Error::NoMergeInProgress);
        }
        read_json(&self.root.join("merge_state"))
    }

    /// Removes the merge state file
    pub fn clear_merge_state(&self) -> Result<(), Error> {
        let merge_state_path = self.root.join("merge_state");
        if Path::try_exists(&merge_state_path)? {
            remove_file(&merge_state_path)?;
        }
        Ok(())
    }
    
    /// Get all configured remotes
    pub fn remotes(&self) -> Result<Vec<RemoteConfig>, Error> {
        let remotes_path = self.root.join("remotes");
        if !Path::try_exists(&remotes_path)? {
            return Ok(Vec::new());
        }
        read_json(&remotes_path)
    }
    
    /// Add a remote
    pub fn add_remote(&self, remote: RemoteConfig) -> Result<(), Error> {
        let mut remotes = self.remotes()?;
        
        // Check if remote with same name already exists
        if remotes.iter().any(|r| r.name == remote.name) {
            return Err(Error::RemoteExists(remote.name));
        }
        
        remotes.push(remote);
        write_json(&remotes, &self.root.join("remotes"))
    }
    
    /// Remove a remote by name
    pub fn remove_remote(&self, name: &str) -> Result<(), Error> {
        let mut remotes = self.remotes()?;
        remotes.retain(|r| r.name != name);
        write_json(&remotes, &self.root.join("remotes"))
    }
    
    /// Get a remote by name
    pub fn get_remote(&self, name: &str) -> Result<Option<RemoteConfig>, Error> {
        let remotes = self.remotes()?;
        Ok(remotes.into_iter().find(|r| r.name == name))
    }
}

/// A convenience trait for writing and reading JSON from the [`DirectoryObjectStore`].
pub trait InsertJson {
    /// Inserts a pretty JSON encoded version of the thing into the store.
    fn insert_json<A: Serialize>(&mut self, thing: &A) -> Result<ObjectId, Error>;

    /// Reads a JSON encoded thing of the given type from the store at that given [`ObjectId`].
    fn read_json<A: for<'de> Deserialize<'de>>(&mut self, object_id: ObjectId) -> Result<A, Error>;
}

impl InsertJson for DirectoryObjectStore {
    fn insert_json<A: Serialize>(&mut self, thing: &A) -> Result<ObjectId, Error> {
        Ok(self.insert(&serde_json::to_vec_pretty(thing)?)?)
    }

    fn read_json<A: for<'de> Deserialize<'de>>(&mut self, object_id: ObjectId) -> Result<A, Error> {
        match self.read(object_id)? {
            None => Err(Error::MissingObject(object_id)),
            Some(obj) => Ok(serde_json::from_slice(&obj)?),
        }
    }
}

fn read_json<A: for<'de> Deserialize<'de>>(path: &Path) -> Result<A, Error> {
    let file = File::options().read(true).open(path)?;
    Ok(serde_json::from_reader(file)?)
}

fn write_json<A: Serialize>(thing: &A, path: &Path) -> Result<(), Error> {
    let file = File::options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;

    Ok(serde_json::to_writer_pretty(file, thing)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::Ignores;
    use tempfile::tempdir;

    #[test]
    fn test_init_and_existing() {
        let temp_dir = tempdir().unwrap();
        let rev_path = temp_dir.path().join(".rev");

        // Test initialization
        let dot_rev = DotRev::init(rev_path.clone()).unwrap();
        assert!(rev_path.exists());

        // Should have created the branch file with "dev"
        let branch_file = rev_path.join("branch");
        assert!(branch_file.exists());
        let branch_content = std::fs::read_to_string(&branch_file).unwrap();
        assert_eq!(branch_content, "dev");

        // Should have created branches directory
        let branches_dir = rev_path.join("branches");
        assert!(branches_dir.exists());

        // Should have created an ignores file
        let ignores_file = rev_path.join("ignores");
        assert!(ignores_file.exists());

        // Test existing
        let existing = DotRev::existing(rev_path.clone()).unwrap();
        assert_eq!(existing.root, dot_rev.root);
    }

    #[test]
    fn test_branch_operations() {
        let temp_dir = tempdir().unwrap();
        let rev_path = temp_dir.path().join(".rev");

        // Initialize
        let dot_rev = DotRev::init(rev_path).unwrap();

        // Test branch
        let branch = dot_rev.branch().unwrap();
        assert_eq!(branch, "dev");

        // Test create_branch first
        dot_rev.create_branch("feature").unwrap();

        // Then test set_branch
        dot_rev.set_branch("feature").unwrap();
        let branch = dot_rev.branch().unwrap();
        assert_eq!(branch, "feature");

        // Test branch_exists
        assert!(dot_rev.branch_exists("dev").unwrap());
        assert!(!dot_rev.branch_exists("nonexistent").unwrap());

        // Test create_branch
        dot_rev.create_branch("test-branch").unwrap();
        assert!(dot_rev.branch_exists("test-branch").unwrap());

        // Test list_branches
        let branches = dot_rev.list_branches().unwrap();
        assert!(branches.contains(&"dev".to_string()));
        assert!(branches.contains(&"test-branch".to_string()));
    }

    #[test]
    fn test_valid_branch_names_are_accepted() {
        for name in [
            "dev",
            "feature",
            "feature-1",
            "feature_1",
            "release/2.0".replace('/', "-").as_str(),
            "a",
            "ünïcode",
            "..dotted",
            "dot.ted",
        ] {
            assert!(
                validate_branch_name(name).is_ok(),
                "expected {name:?} to be a valid branch name"
            );
        }
    }

    #[test]
    fn test_branch_name_traversal_is_rejected() {
        // Each of these previously resolved to a path outside .rev/branches.
        for name in [
            "",
            ".",
            "..",
            "../evil",
            "../../pwned",
            "../branch",
            "a/b",
            "a\\b",
            "/abs",
            "with\nnewline",
            "trailing ",
            " leading",
            "-flag",
        ] {
            assert!(
                validate_branch_name(name).is_err(),
                "expected {name:?} to be rejected as a branch name"
            );
        }
        assert!(validate_branch_name(&"x".repeat(201)).is_err());
    }

    #[test]
    fn test_traversal_branch_cannot_escape_repository() {
        let temp_dir = tempdir().unwrap();
        let rev_path = temp_dir.path().join(".rev");
        let dot_rev = DotRev::init(rev_path).unwrap();

        // The regression this guards: `revtool branch ../../pwned` used to
        // write a file into the working tree, outside .rev entirely.
        assert!(dot_rev.create_branch("../../pwned").is_err());
        assert!(!temp_dir.path().join("pwned").exists());

        assert!(dot_rev.create_branch("../evil").is_err());
        assert!(!temp_dir.path().join(".rev").join("evil").exists());

        // And the empty name, which used to resolve to the branches directory
        // itself and brick every subsequent command.
        assert!(dot_rev.set_branch("").is_err());
        assert_eq!(dot_rev.branch().unwrap(), "dev");
    }

    #[test]
    fn test_ignores() {
        let temp_dir = tempdir().unwrap();
        let rev_path = temp_dir.path().join(".rev");

        // Initialize a clean repository
        let dot_rev = DotRev::init(rev_path).unwrap();

        // Test default ignores
        let ignores = dot_rev.ignores().unwrap();
        assert!(ignores.patterns.contains(&".rev".to_string()));
        assert!(ignores.patterns.contains(&".git".to_string()));

        // Create a new simple ignores object with a single pattern
        let simple_ignores = Ignores::new(vec!["*.log".to_string()]);

        // Write the simple ignores
        dot_rev.set_ignores(&simple_ignores).unwrap();

        // Read back and verify
        let read_ignores = dot_rev.ignores().unwrap();
        assert_eq!(read_ignores.patterns.len(), 1);
        assert_eq!(read_ignores.patterns[0], "*.log");
    }

}
