use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    directory::Diff,
    object_id::ObjectId,
    object_store::ObjectStore,
    snapshot::SnapShot,
};

/// Error types for snapshot diffing operations
#[derive(Debug)]
pub enum Error<Store: ObjectStore> {
    /// Object missing from store
    ObjectMissing(ObjectId),
    /// Store error
    Store(Store::Error),
    /// JSON parse error
    ParseError(serde_json::Error),
    /// I/O error
    IO(std::io::Error),
}

impl<Store: ObjectStore> From<std::io::Error> for Error<Store> {
    fn from(error: std::io::Error) -> Self {
        Error::IO(error)
    }
}

impl<Store: ObjectStore> From<serde_json::Error> for Error<Store> {
    fn from(error: serde_json::Error) -> Self {
        Error::ParseError(error)
    }
}

/// The result of comparing two snapshots
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapShotDiff {
    /// The source (older) snapshot ID
    pub source_id: ObjectId,
    /// The target (newer) snapshot ID
    pub target_id: ObjectId,
    /// The source snapshot message
    pub source_message: String,
    /// The target snapshot message
    pub target_message: String,
    /// The directory diff between the snapshots
    pub directory_diff: Diff,
    /// Whether content diffs are included
    #[serde(skip)]
    pub with_content_diffs: bool,
}

impl SnapShotDiff {
    /// Generates a diff from the `source_id` (older) snapshot to the `target_id`
    /// (newer) one, optionally including per-file content diffs.
    pub fn generate<Store: ObjectStore>(
        store: &Store,
        source_id: ObjectId,
        target_id: ObjectId,
        with_content_diffs: bool,
    ) -> Result<Self, Error<Store>> {
        // Get the snapshots from the store
        let source_bytes = store.read(source_id).map_err(Error::Store)?
            .ok_or(Error::ObjectMissing(source_id))?;
        let target_bytes = store.read(target_id).map_err(Error::Store)?
            .ok_or(Error::ObjectMissing(target_id))?;

        // Deserialize the snapshots
        let source: SnapShot = serde_json::from_slice(&source_bytes)?;
        let target: SnapShot = serde_json::from_slice(&target_bytes)?;

        // Get the directory structures
        let source_dir_bytes = store.read(source.directory).map_err(Error::Store)?
            .ok_or(Error::ObjectMissing(source.directory))?;
        let target_dir_bytes = store.read(target.directory).map_err(Error::Store)?
            .ok_or(Error::ObjectMissing(target.directory))?;

        // Deserialize the directories
        let source_dir: crate::directory::Directory = serde_json::from_slice(&source_dir_bytes)?;
        let target_dir: crate::directory::Directory = serde_json::from_slice(&target_dir_bytes)?;

        // Generate the diff
        let directory_diff = if with_content_diffs {
            source_dir.diff_with_content(&target_dir, true, store)
        } else {
            source_dir.diff(&target_dir)
        };

        Ok(SnapShotDiff {
            source_id,
            target_id,
            source_message: source.message,
            target_message: target.message,
            directory_diff,
            with_content_diffs,
        })
    }

    /// Returns true if there are changes between the snapshots
    pub fn has_changes(&self) -> bool {
        !self.directory_diff.added.is_empty() || 
        !self.directory_diff.deleted.is_empty() || 
        !self.directory_diff.modified.is_empty()
    }

    /// Counts the number of files added
    pub fn added_count(&self) -> usize {
        self.directory_diff.added.len()
    }

    /// Counts the number of files deleted
    pub fn deleted_count(&self) -> usize {
        self.directory_diff.deleted.len()
    }

    /// Counts the number of files modified
    pub fn modified_count(&self) -> usize {
        self.directory_diff.modified.len()
    }
}

impl fmt::Display for SnapShotDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Snapshot Diff")?;
        writeln!(f, "=============")?;
        writeln!(f, "From: {} - {}", self.source_id, self.source_message)?;
        writeln!(f, "To:   {} - {}", self.target_id, self.target_message)?;
        writeln!(f)?;

        if !self.has_changes() {
            writeln!(f, "No changes detected between snapshots.")?;
            return Ok(());
        }

        let total = self.added_count() + self.deleted_count() + self.modified_count();
        writeln!(f, "Changes: {} files changed, {} added, {} deleted, {} modified", 
            total, self.added_count(), self.deleted_count(), self.modified_count())?;
        writeln!(f)?;
        
        // Display the directory diff
        write!(f, "{}", self.directory_diff)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        directory::{Directory, DirectoryEntry},
        object_store::in_memory::InMemoryObjectStore,
    };
    use std::collections::BTreeMap;

    #[test]
    fn test_snapshot_diff_no_changes() {
        let mut store = InMemoryObjectStore::new();
        
        // Create a simple directory
        let mut dir_entries = BTreeMap::new();
        let content = b"test content";
        let file_id = store.insert(content).unwrap();
        dir_entries.insert("test.txt".to_string(), DirectoryEntry::File(file_id));
        
        let directory = Directory {
            root: dir_entries,
        };
        
        // Serialize the directory
        let dir_bytes = serde_json::to_vec(&directory).unwrap();
        let dir_id = store.insert(&dir_bytes).unwrap();
        
        // Create a snapshot
        let snapshot = SnapShot {
            message: "Test snapshot".to_string(),
            directory: dir_id,
            previous: Vec::new(),
        };
        
        // Serialize the snapshot
        let snapshot_bytes = serde_json::to_vec(&snapshot).unwrap();
        let snapshot_id = store.insert(&snapshot_bytes).unwrap();
        
        // Create the same snapshot again for comparison
        let snapshot_id2 = snapshot_id;
        
        // Generate a diff
        let diff = SnapShotDiff::generate(&store, snapshot_id, snapshot_id2, false).unwrap();
        
        // Verify no changes
        assert!(!diff.has_changes());
        assert_eq!(diff.added_count(), 0);
        assert_eq!(diff.deleted_count(), 0);
        assert_eq!(diff.modified_count(), 0);
    }
    
    #[test]
    fn test_snapshot_diff_with_changes() {
        let mut store = InMemoryObjectStore::new();
        
        // Create first directory
        let mut dir1_entries = BTreeMap::new();
        let content1 = b"test content";
        let file1_id = store.insert(content1).unwrap();
        dir1_entries.insert("test1.txt".to_string(), DirectoryEntry::File(file1_id));
        
        let directory1 = Directory {
            root: dir1_entries,
        };
        
        // Serialize the first directory
        let dir1_bytes = serde_json::to_vec(&directory1).unwrap();
        let dir1_id = store.insert(&dir1_bytes).unwrap();
        
        // Create first snapshot
        let snapshot1 = SnapShot {
            message: "First snapshot".to_string(),
            directory: dir1_id,
            previous: Vec::new(),
        };
        
        // Serialize the first snapshot
        let snapshot1_bytes = serde_json::to_vec(&snapshot1).unwrap();
        let snapshot1_id = store.insert(&snapshot1_bytes).unwrap();
        
        // Create second directory with changes
        let mut dir2_entries = BTreeMap::new();
        // Keep the first file
        dir2_entries.insert("test1.txt".to_string(), DirectoryEntry::File(file1_id));
        // Add a new file
        let content2 = b"new content";
        let file2_id = store.insert(content2).unwrap();
        dir2_entries.insert("test2.txt".to_string(), DirectoryEntry::File(file2_id));
        
        let directory2 = Directory {
            root: dir2_entries,
        };
        
        // Serialize the second directory
        let dir2_bytes = serde_json::to_vec(&directory2).unwrap();
        let dir2_id = store.insert(&dir2_bytes).unwrap();
        
        // Create second snapshot
        let previous = vec![snapshot1_id];
        
        let snapshot2 = SnapShot {
            message: "Second snapshot".to_string(),
            directory: dir2_id,
            previous,
        };
        
        // Serialize the second snapshot
        let snapshot2_bytes = serde_json::to_vec(&snapshot2).unwrap();
        let snapshot2_id = store.insert(&snapshot2_bytes).unwrap();
        
        // Generate a diff
        let diff = SnapShotDiff::generate(&store, snapshot1_id, snapshot2_id, false).unwrap();
        
        // Verify changes
        assert!(diff.has_changes());
        assert_eq!(diff.added_count(), 1); // One file added
        assert_eq!(diff.deleted_count(), 0); // No files deleted
        assert_eq!(diff.modified_count(), 0); // No files modified
    }
}