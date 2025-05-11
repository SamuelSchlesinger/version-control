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
            .open(&root.join("branch"))?;
        file.write("dev".as_bytes())?;

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

    pub fn here() -> Result<Self, Error> {
        match current_dir() {
            Ok(dir) => DotRev::existing(dir.join(".rev")),
            Err(e) => Err(Error::IO(e)),
        }
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

    pub fn branch(&self) -> Result<String, Error> {
        Ok(read_to_string(&self.root.join("branch"))?)
    }

    pub fn set_branch(&self, new_branch: &str) -> Result<(), Error> {
        let mut file = File::options()
            .write(true)
            .truncate(true)
            .open(&self.root.join("branch"))?;
        file.write(new_branch.as_bytes())?;
        Ok(())
    }

    pub fn branch_snapshot_id(&self, branch: &str) -> Result<ObjectId, Error> {
        let branch_path = self.root.join("branches").join(branch);
        if !Path::try_exists(&branch_path)? {
            return Err(Error::BranchNotFound(branch.to_string()));
        }
        read_json(&branch_path)
    }

    pub fn set_branch_snapshot_id(&self, branch: &str, object_id: ObjectId) -> Result<(), Error> {
        write_json(&object_id, &self.root.join("branches").join(&branch))
    }

    pub fn current_snapshot_id(&self) -> Result<ObjectId, Error> {
        let branch = self.branch()?;
        self.branch_snapshot_id(&branch)
    }

    pub fn create_branch(&self, new_branch: &str) -> Result<(), Error> {
        if !self.branch_exists(&new_branch)? {
            let snapshot_id = self.current_snapshot_id()?;
            return write_json(&snapshot_id, &self.root.join("branches").join(&new_branch));
        }
        Ok(())
    }

    pub fn branch_exists(&self, branch: &str) -> Result<bool, Error> {
        Ok(Path::try_exists(&self.root.join("branches").join(&branch))?)
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
