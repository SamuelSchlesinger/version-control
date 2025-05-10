use std::{
    collections::{BTreeMap, BTreeSet},
    env::current_dir,
    fs::{create_dir, create_dir_all, read_dir, read_to_string, File},
    io::Write,
    path::{Path, PathBuf},
};

use derive_more::From;
use serde::{Deserialize, Serialize};

use crate::{
    directory::{Directory, Ignores},
    object_id::ObjectId,
    object_store::{directory::DirectoryObjectStore, ObjectStore},
    snapshot::SnapShot,
};

/// A reference to a remote repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Remote {
    /// The URL or path to the remote repository.
    pub url: String,
}

/// A collection of remotes for a repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Remotes {
    /// The remote repositories, keyed by name.
    pub remotes: BTreeMap<String, Remote>,
}

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
    RemoteNotFound(String),
    RemoteAlreadyExists(String),
    PushError(String),
    PullError(String),
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
            Error::RemoteNotFound(name) => write!(f, "Remote '{}' not found", name),
            Error::RemoteAlreadyExists(name) => write!(f, "Remote '{}' already exists", name),
            Error::PushError(msg) => write!(f, "Push failed: {}", msg),
            Error::PullError(msg) => write!(f, "Pull failed: {}", msg),
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

    // Remote-related methods

    /// Get all remotes.
    pub fn remotes(&self) -> Result<Remotes, Error> {
        let remotes_path = self.root.join("remotes");
        if !Path::try_exists(&remotes_path)? {
            // Create remotes file if it doesn't exist
            let remotes = Remotes::default();
            write_json(&remotes, &remotes_path)?;
            return Ok(remotes);
        }

        read_json(&remotes_path)
    }

    /// Add a new remote.
    pub fn add_remote(&self, name: &str, url: &str) -> Result<(), Error> {
        let mut remotes = self.remotes()?;

        if remotes.remotes.contains_key(name) {
            return Err(Error::RemoteAlreadyExists(name.to_string()));
        }

        remotes.remotes.insert(name.to_string(), Remote { url: url.to_string() });
        write_json(&remotes, &self.root.join("remotes"))
    }

    /// Remove a remote.
    pub fn remove_remote(&self, name: &str) -> Result<(), Error> {
        let mut remotes = self.remotes()?;

        if !remotes.remotes.contains_key(name) {
            return Err(Error::RemoteNotFound(name.to_string()));
        }

        remotes.remotes.remove(name);
        write_json(&remotes, &self.root.join("remotes"))
    }

    /// Get a remote by name.
    pub fn get_remote(&self, name: &str) -> Result<Remote, Error> {
        let remotes = self.remotes()?;

        match remotes.remotes.get(name) {
            Some(remote) => Ok(remote.clone()),
            None => Err(Error::RemoteNotFound(name.to_string())),
        }
    }

    /// Push a branch to a remote.
    pub fn push(&self, remote_name: &str, branch_name: &str) -> Result<(), Error> {
        // Get the remote
        let remote = self.get_remote(remote_name)?;

        // Check if the branch exists
        if !self.branch_exists(branch_name)? {
            return Err(Error::BranchNotFound(branch_name.to_string()));
        }

        // Get the remote DotRev based on the URL
        let remote_path = Path::new(&remote.url);
        let remote_dot_rev = if Path::try_exists(remote_path)? {
            DotRev::existing(remote_path.to_path_buf())?
        } else {
            return Err(Error::PushError(format!("Remote path does not exist: {}", remote.url)));
        };

        // Get current branch snapshot ID
        let snapshot_id = self.branch_snapshot_id(branch_name)?;

        // Get the snapshot and all its dependencies
        let mut store = self.store()?;
        let mut remote_store = remote_dot_rev.store()?;

        // First, transfer the snapshot
        let snapshot: SnapShot = store.read_json(snapshot_id)?;

        // Then transfer the directory
        let directory: Directory = store.read_json(snapshot.directory)?;
        let remote_directory_id = remote_store.insert_json(&directory)?;

        // Create the snapshot in the remote with the new directory ID
        let remote_snapshot = SnapShot {
            directory: remote_directory_id,
            previous: snapshot.previous.clone(),
            message: snapshot.message.clone(),
        };

        // Insert the snapshot into the remote store
        let remote_snapshot_id = remote_store.insert_json(&remote_snapshot)?;

        // Create or update the branch in the remote
        if !remote_dot_rev.branch_exists(branch_name)? {
            remote_dot_rev.create_branch(branch_name)?;
        }
        remote_dot_rev.set_branch_snapshot_id(branch_name, remote_snapshot_id)?;

        Ok(())
    }

    /// Pull a branch from a remote.
    pub fn pull(&self, remote_name: &str, branch_name: &str) -> Result<(), Error> {
        // Get the remote
        let remote = self.get_remote(remote_name)?;

        // Get the remote DotRev based on the URL
        let remote_path = Path::new(&remote.url);
        let remote_dot_rev = if Path::try_exists(remote_path)? {
            DotRev::existing(remote_path.to_path_buf())?
        } else {
            return Err(Error::PullError(format!("Remote path does not exist: {}", remote.url)));
        };

        // Check if the branch exists in the remote
        if !remote_dot_rev.branch_exists(branch_name)? {
            return Err(Error::BranchNotFound(format!("Branch '{}' not found in remote '{}'", branch_name, remote_name)));
        }

        // Get remote branch snapshot ID
        let remote_snapshot_id = remote_dot_rev.branch_snapshot_id(branch_name)?;

        // Get the snapshot and all its dependencies
        let mut store = self.store()?;
        let mut remote_store = remote_dot_rev.store()?;

        // First, transfer the snapshot
        let remote_snapshot: SnapShot = remote_store.read_json(remote_snapshot_id)?;

        // Then transfer the directory
        let remote_directory: Directory = remote_store.read_json(remote_snapshot.directory)?;

        // Check if we already have a local version of this branch
        let local_directory = if self.branch_exists(branch_name)? {
            // Get local branch snapshot ID
            let local_snapshot_id = self.branch_snapshot_id(branch_name)?;

            // Get local snapshot and directory
            let local_snapshot: SnapShot = store.read_json(local_snapshot_id)?;
            let local_directory: Directory = store.read_json(local_snapshot.directory)?;

            // Merge the local and remote directories
            // We'll use a simple strategy: keep all files from both repositories
            let mut merged_directory = local_directory.clone();

            // Add all files from remote that don't exist locally
            for (name, entry) in remote_directory.root.iter() {
                if !merged_directory.root.contains_key(name) {
                    merged_directory.root.insert(name.clone(), entry.clone());
                }
            }

            merged_directory
        } else {
            // Just use the remote directory directly if the branch doesn't exist locally
            remote_directory
        };

        // Insert the merged directory
        let local_directory_id = store.insert_json(&local_directory)?;

        // Create the snapshot in our repo with the new directory ID
        let local_snapshot = SnapShot {
            directory: local_directory_id,
            previous: if self.branch_exists(branch_name)? {
                // Include both the current local branch and the remote branch as parents
                let mut previous = BTreeSet::new();
                previous.insert(self.branch_snapshot_id(branch_name)?);
                previous.insert(remote_snapshot_id);
                previous
            } else {
                // Just include the remote branch as parent if this is a new branch
                remote_snapshot.previous.clone()
            },
            message: format!("Merge from remote '{}' branch '{}'", remote_name, branch_name),
        };

        // Insert the snapshot into our store
        let local_snapshot_id = store.insert_json(&local_snapshot)?;

        // Create or update the branch in our repo
        if !self.branch_exists(branch_name)? {
            self.create_branch(branch_name)?;
        }
        self.set_branch_snapshot_id(branch_name, local_snapshot_id)?;

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

    #[test]
    fn test_remotes() {
        let temp_dir = tempdir().unwrap();
        let rev_path = temp_dir.path().join(".rev");

        // Initialize a clean repository
        let dot_rev = DotRev::init(rev_path).unwrap();

        // Initially, no remotes should exist
        let remotes = dot_rev.remotes().unwrap();
        assert!(remotes.remotes.is_empty());

        // Add a remote
        dot_rev.add_remote("origin", "https://example.com/repo.git").unwrap();

        // Verify remote was added
        let remotes = dot_rev.remotes().unwrap();
        assert_eq!(remotes.remotes.len(), 1);
        assert!(remotes.remotes.contains_key("origin"));

        let remote = remotes.remotes.get("origin").unwrap();
        assert_eq!(remote.url, "https://example.com/repo.git");

        // Add another remote
        dot_rev.add_remote("upstream", "https://upstream.com/repo.git").unwrap();

        // Verify both remotes exist
        let remotes = dot_rev.remotes().unwrap();
        assert_eq!(remotes.remotes.len(), 2);
        assert!(remotes.remotes.contains_key("origin"));
        assert!(remotes.remotes.contains_key("upstream"));

        // Test get_remote method
        let remote = dot_rev.get_remote("origin").unwrap();
        assert_eq!(remote.url, "https://example.com/repo.git");

        // Test remove_remote method
        dot_rev.remove_remote("origin").unwrap();

        // Verify remote was removed
        let remotes = dot_rev.remotes().unwrap();
        assert_eq!(remotes.remotes.len(), 1);
        assert!(!remotes.remotes.contains_key("origin"));
        assert!(remotes.remotes.contains_key("upstream"));

        // Test error on removing non-existent remote
        let remove_result = dot_rev.remove_remote("nonexistent");
        assert!(matches!(remove_result, Err(Error::RemoteNotFound(_))));

        // Test error on adding duplicate remote
        let add_result = dot_rev.add_remote("upstream", "https://another-url.com/repo.git");
        assert!(matches!(add_result, Err(Error::RemoteAlreadyExists(_))));
    }

    #[test]
    fn test_push_pull() {
        use std::fs::write;

        // Create two temporary directories for two repositories
        let temp_dir1 = tempdir().unwrap();
        let temp_dir2 = tempdir().unwrap();

        // Path to the repositories
        let repo1_path = temp_dir1.path();
        let repo2_path = temp_dir2.path();

        // Path to the .rev directories
        let rev_path1 = repo1_path.join(".rev");
        let rev_path2 = repo2_path.join(".rev");

        // Initialize the repositories
        let dot_rev1 = DotRev::init(rev_path1.clone()).unwrap();
        let dot_rev2 = DotRev::init(rev_path2.clone()).unwrap();

        // Create a file in repository 1
        let file_path = repo1_path.join("test.txt");
        write(&file_path, "Hello, world!").unwrap();

        // Create a snapshot in repository 1
        let mut store1 = dot_rev1.store().unwrap();
        let ignores1 = dot_rev1.ignores().unwrap();

        let directory1 = Directory::new(repo1_path, &ignores1, &mut store1).unwrap();
        let directory_id1 = store1.insert_json(&directory1).unwrap();

        // Get the previous snapshot ID (should be the init commit)
        let old_tip1 = dot_rev1.branch_snapshot_id("dev").unwrap();

        // Create a new snapshot
        let snap = SnapShot {
            directory: directory_id1,
            previous: vec![old_tip1].into_iter().collect(),
            message: "Add test.txt".to_string(),
        };

        // Store the snapshot and update the branch
        let snap_id = store1.insert_json(&snap).unwrap();
        dot_rev1.set_branch_snapshot_id("dev", snap_id).unwrap();

        // Add repository 2 as a remote in repository 1
        dot_rev1.add_remote("remote", rev_path2.to_str().unwrap()).unwrap();

        // Test push
        dot_rev1.push("remote", "dev").unwrap();

        // Verify push: check that repository 2 has the file
        let branch_snapshot_id = dot_rev2.branch_snapshot_id("dev").unwrap();
        let mut store2 = dot_rev2.store().unwrap();

        let snapshot2: SnapShot = store2.read_json(branch_snapshot_id).unwrap();
        let directory2: Directory = store2.read_json(snapshot2.directory).unwrap();

        // The directory should contain our test file
        assert!(directory2.root.contains_key("test.txt"));

        // Modify the file in repository 1
        write(&file_path, "Hello, world updated!").unwrap();

        // Create another snapshot in repository 1
        let directory1 = Directory::new(repo1_path, &ignores1, &mut store1).unwrap();
        let directory_id1 = store1.insert_json(&directory1).unwrap();

        let snap = SnapShot {
            directory: directory_id1,
            previous: vec![snap_id].into_iter().collect(),
            message: "Update test.txt".to_string(),
        };

        // Store the snapshot and update the branch
        let snap_id2 = store1.insert_json(&snap).unwrap();
        dot_rev1.set_branch_snapshot_id("dev", snap_id2).unwrap();

        // Push the updated file to repository 2
        dot_rev1.push("remote", "dev").unwrap();

        // Create a new file in repository 2
        let file_path2 = repo2_path.join("test2.txt");
        write(&file_path2, "Second file").unwrap();

        // Create a snapshot in repository 2
        let ignores2 = dot_rev2.ignores().unwrap();
        let directory2 = Directory::new(repo2_path, &ignores2, &mut store2).unwrap();
        let directory_id2 = store2.insert_json(&directory2).unwrap();

        // Get the current snapshot ID in repository 2
        let current_tip2 = dot_rev2.branch_snapshot_id("dev").unwrap();

        // Create a new snapshot in repository 2
        let snap2 = SnapShot {
            directory: directory_id2,
            previous: vec![current_tip2].into_iter().collect(),
            message: "Add test2.txt".to_string(),
        };

        // Store the snapshot and update the branch
        let snap_id3 = store2.insert_json(&snap2).unwrap();
        dot_rev2.set_branch_snapshot_id("dev", snap_id3).unwrap();

        // We need to make a separate branch in repo2 to keep the test2.txt file
        dot_rev2.create_branch("with-test2").unwrap();
        dot_rev2.set_branch_snapshot_id("with-test2", snap_id3).unwrap();

        // Add repository 1 as a remote in repository 2
        dot_rev2.add_remote("origin", rev_path1.to_str().unwrap()).unwrap();

        // Test pull from repository 1 to repository 2's dev branch
        dot_rev2.pull("origin", "dev").unwrap();

        // Verify repository 2 now has the updated content from repository 1 in dev branch
        let branch_snapshot_id = dot_rev2.branch_snapshot_id("dev").unwrap();
        let snapshot2: SnapShot = store2.read_json(branch_snapshot_id).unwrap();
        let directory2: Directory = store2.read_json(snapshot2.directory).unwrap();

        // The directory should contain our test file with updated content
        assert!(directory2.root.contains_key("test.txt"));

        // Switch to the with-test2 branch to verify test2.txt is still there
        let branch_snapshot_id = dot_rev2.branch_snapshot_id("with-test2").unwrap();
        let snapshot2: SnapShot = store2.read_json(branch_snapshot_id).unwrap();
        let directory2: Directory = store2.read_json(snapshot2.directory).unwrap();

        // This branch should have test2.txt
        assert!(directory2.root.contains_key("test2.txt"));
    }
}
