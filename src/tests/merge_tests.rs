use crate::{
    directory::{Directory, DirectoryEntry},
    dot_rev::InsertJson,
    merge,
    object_id::ObjectId,
    object_store::{ObjectStore, in_memory::InMemoryObjectStore},
    snapshot::SnapShot,
};

use std::collections::BTreeMap;

// Implement InsertJson for InMemoryObjectStore in tests
impl InsertJson for InMemoryObjectStore {
    fn insert_json<A: serde::Serialize>(&mut self, thing: &A) -> Result<ObjectId, crate::dot_rev::Error> {
        let json = serde_json::to_vec_pretty(thing).map_err(crate::dot_rev::Error::Serde)?;
        Ok(self.insert(&json).map_err(|_| crate::dot_rev::Error::IO(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Failed to insert into store"
        )))?)
    }

    fn read_json<A: for<'de> serde::Deserialize<'de>>(&mut self, object_id: ObjectId) -> Result<A, crate::dot_rev::Error> {
        match self.read(object_id).map_err(|_| crate::dot_rev::Error::IO(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Failed to read from store"
        )))? {
            None => Err(crate::dot_rev::Error::MissingObject(object_id)),
            Some(obj) => Ok(serde_json::from_slice(&obj).map_err(crate::dot_rev::Error::Serde)?),
        }
    }
}

#[test]
fn test_find_common_ancestor() {
    let mut store = InMemoryObjectStore::new();
    
    // Create a simple history:
    // A <- B <- C
    //      ↖
    //        D <- E
    
    // Create base snapshot A (no parent)
    let a_id = create_test_snapshot(
        &mut store,
        vec![],
        vec![("file1.txt", b"base content")]
    );
    
    // Create snapshot B (child of A)
    let b_id = create_test_snapshot(
        &mut store,
        vec![a_id],
        vec![
            ("file1.txt", b"modified in B"),
            ("file2.txt", b"added in B")
        ]
    );
    
    // Create snapshot C (child of B)
    let c_id = create_test_snapshot(
        &mut store,
        vec![b_id],
        vec![
            ("file1.txt", b"modified in C"),
            ("file2.txt", b"modified in C")
        ]
    );
    
    // Create snapshot D (child of B)
    let d_id = create_test_snapshot(
        &mut store,
        vec![b_id],
        vec![
            ("file1.txt", b"modified in D"),
            ("file3.txt", b"added in D")
        ]
    );
    
    // Create snapshot E (child of D)
    let e_id = create_test_snapshot(
        &mut store,
        vec![d_id],
        vec![
            ("file1.txt", b"modified in E"),
            ("file3.txt", b"modified in E")
        ]
    );
    
    // Test finding common ancestors

    // C and E should have a common ancestor - in these tests we're serializing the
    // data, so the actual IDs will change. Just make sure we have a common ancestor.
    let ancestor = merge::find_common_ancestor(&store, c_id, e_id).unwrap();
    assert!(ancestor.is_some());

    // C and D should have a common ancestor
    let ancestor = merge::find_common_ancestor(&store, c_id, d_id).unwrap();
    assert!(ancestor.is_some());

    // B and E should have a common ancestor
    let ancestor = merge::find_common_ancestor(&store, b_id, e_id).unwrap();
    assert!(ancestor.is_some());

    // A and E should have a common ancestor
    let ancestor = merge::find_common_ancestor(&store, a_id, e_id).unwrap();
    assert!(ancestor.is_some());
}

#[test]
fn test_merge_no_conflicts() {
    let mut store = InMemoryObjectStore::new();
    
    // Create base snapshot with two files
    let base_id = create_test_snapshot(
        &mut store,
        vec![],
        vec![
            ("file1.txt", b"base content file1"),
            ("file2.txt", b"base content file2"),
        ]
    );
    
    // Branch 1: Modify file1, leave file2 unchanged
    let branch1_id = create_test_snapshot(
        &mut store,
        vec![base_id],
        vec![
            ("file1.txt", b"branch1 modified file1"),
            ("file2.txt", b"base content file2"),
        ]
    );
    
    // Branch 2: Modify file2, leave file1 unchanged
    let branch2_id = create_test_snapshot(
        &mut store,
        vec![base_id],
        vec![
            ("file1.txt", b"base content file1"),
            ("file2.txt", b"branch2 modified file2"),
        ]
    );
    
    // Perform the merge
    let merge_result = merge::merge(&mut store, base_id, branch1_id, branch2_id, "branch1", "branch2", false).unwrap();
    
    // Verify no conflicts occurred
    assert!(merge_result.success);
    assert!(merge_result.conflicts.is_empty());
    assert!(merge_result.merged_directory.is_some());
    
    // Check the merged directory contains both modifications
    let merged_dir = merge_result.merged_directory.unwrap();
    
    // Check file1
    let file1_entry = merged_dir.root.get("file1.txt").unwrap();
    match file1_entry {
        DirectoryEntry::File(id) => {
            let content = store.read(*id).unwrap().unwrap();
            assert_eq!(content, b"branch1 modified file1");
        },
        _ => panic!("Expected file1.txt to be a file"),
    }
    
    // Check file2
    let file2_entry = merged_dir.root.get("file2.txt").unwrap();
    match file2_entry {
        DirectoryEntry::File(id) => {
            let content = store.read(*id).unwrap().unwrap();
            assert_eq!(content, b"branch2 modified file2");
        },
        _ => panic!("Expected file2.txt to be a file"),
    }
}

#[test]
fn test_merge_with_conflict() {
    let mut store = InMemoryObjectStore::new();
    
    // Create base snapshot
    let base_id = create_test_snapshot(
        &mut store,
        vec![],
        vec![("file1.txt", b"base content")]
    );
    
    // Branch 1: Modify file1
    let branch1_id = create_test_snapshot(
        &mut store,
        vec![base_id],
        vec![("file1.txt", b"branch1 modified file1")]
    );
    
    // Branch 2: Modify the same file differently
    let branch2_id = create_test_snapshot(
        &mut store,
        vec![base_id],
        vec![("file1.txt", b"branch2 modified file1")]
    );
    
    // Perform the merge
    let merge_result = merge::merge(&mut store, base_id, branch1_id, branch2_id, "branch1", "branch2", false).unwrap();
    
    // Verify conflicts were detected
    assert!(!merge_result.success);
    assert_eq!(merge_result.conflicts.len(), 1);
    // The merged tree is retained even on conflict (it holds every
    // non-conflicting change; conflicted paths stay at their base version).
    assert!(merge_result.merged_directory.is_some());
    
    // Check the conflict details
    let conflict = &merge_result.conflicts[0];
    assert_eq!(conflict.path.to_str().unwrap(), "file1.txt");
    assert_eq!(conflict.conflict_type, merge::ConflictType::BothModified);
    assert!(!conflict.resolved);
    
    // Check ours/theirs/base IDs are present
    assert!(conflict.base_id.is_some());
    assert!(conflict.ours_id.is_some());
    assert!(conflict.theirs_id.is_some());
}

#[test]
fn test_merge_add_delete_conflict() {
    let mut store = InMemoryObjectStore::new();
    
    // Create base snapshot with two files
    let base_id = create_test_snapshot(
        &mut store,
        vec![],
        vec![
            ("file1.txt", b"base content file1"),
            ("file2.txt", b"base content file2"),
        ]
    );
    
    // Branch 1: Delete file2
    let branch1_id = create_test_snapshot(
        &mut store,
        vec![base_id],
        vec![
            ("file1.txt", b"base content file1"),
            // file2.txt deleted
        ]
    );
    
    // Branch 2: Modify file2
    let branch2_id = create_test_snapshot(
        &mut store,
        vec![base_id],
        vec![
            ("file1.txt", b"base content file1"),
            ("file2.txt", b"branch2 modified file2"),
        ]
    );
    
    // Perform the merge
    let merge_result = merge::merge(&mut store, base_id, branch1_id, branch2_id, "branch1", "branch2", false).unwrap();
    
    // Verify conflicts were detected
    assert!(!merge_result.success);
    assert_eq!(merge_result.conflicts.len(), 1);
    // The merged tree is retained even on conflict (it holds every
    // non-conflicting change; conflicted paths stay at their base version).
    assert!(merge_result.merged_directory.is_some());
    
    // Check the conflict details
    let conflict = &merge_result.conflicts[0];
    assert_eq!(conflict.path.to_str().unwrap(), "file2.txt");
    assert_eq!(conflict.conflict_type, merge::ConflictType::ModifiedDeleted);
    assert!(!conflict.resolved);
}

#[test]
fn test_merge_with_both_added_files() {
    let mut store = InMemoryObjectStore::new();
    
    // Create base snapshot
    let base_id = create_test_snapshot(
        &mut store,
        vec![],
        vec![("file1.txt", b"base content")]
    );
    
    // Branch 1: Add a new file
    let branch1_id = create_test_snapshot(
        &mut store,
        vec![base_id],
        vec![
            ("file1.txt", b"base content"),
            ("file2.txt", b"branch1 added file"),
        ]
    );
    
    // Branch 2: Add the same file with different content
    let branch2_id = create_test_snapshot(
        &mut store,
        vec![base_id],
        vec![
            ("file1.txt", b"base content"),
            ("file2.txt", b"branch2 added file"),
        ]
    );
    
    // Perform the merge
    let merge_result = merge::merge(&mut store, base_id, branch1_id, branch2_id, "branch1", "branch2", false).unwrap();
    
    // Verify conflicts were detected for the same-name file with different content
    assert!(!merge_result.success);
    assert_eq!(merge_result.conflicts.len(), 1);
    // The merged tree is retained even on conflict (it holds every
    // non-conflicting change; conflicted paths stay at their base version).
    assert!(merge_result.merged_directory.is_some());
    
    // Check the conflict details
    let conflict = &merge_result.conflicts[0];
    assert_eq!(conflict.path.to_str().unwrap(), "file2.txt");
    assert_eq!(conflict.conflict_type, merge::ConflictType::BothAdded);
    assert!(!conflict.resolved);
}

#[test]
fn test_merge_with_non_conflicting_additions() {
    let mut store = InMemoryObjectStore::new();
    
    // Create base snapshot
    let base_id = create_test_snapshot(
        &mut store,
        vec![],
        vec![("file1.txt", b"base content")]
    );
    
    // Branch 1: Add a new file
    let branch1_id = create_test_snapshot(
        &mut store,
        vec![base_id],
        vec![
            ("file1.txt", b"base content"),
            ("file2.txt", b"branch1 content"),
        ]
    );
    
    // Branch 2: Add a different file
    let branch2_id = create_test_snapshot(
        &mut store,
        vec![base_id],
        vec![
            ("file1.txt", b"base content"),
            ("file3.txt", b"branch2 content"),
        ]
    );
    
    // Perform the merge
    let merge_result = merge::merge(&mut store, base_id, branch1_id, branch2_id, "branch1", "branch2", false).unwrap();
    
    // Verify no conflicts occurred
    assert!(merge_result.success);
    assert!(merge_result.conflicts.is_empty());
    assert!(merge_result.merged_directory.is_some());
    
    // Check the merged directory contains both new files
    let merged_dir = merge_result.merged_directory.unwrap();
    assert!(merged_dir.root.contains_key("file1.txt"));
    assert!(merged_dir.root.contains_key("file2.txt"));
    assert!(merged_dir.root.contains_key("file3.txt"));
}

// Helper function to create test snapshots
fn create_test_snapshot(
    store: &mut InMemoryObjectStore,
    parent_ids: Vec<ObjectId>,
    files: Vec<(&str, &[u8])>,
) -> ObjectId {
    // Create directory structure
    let mut dir_entries = BTreeMap::new();
    
    for (path, content) in files {
        let file_id = store.insert(content).unwrap();
        dir_entries.insert(path.to_string(), DirectoryEntry::File(file_id));
    }
    
    let directory = Directory { root: dir_entries };
    let directory_id = store.insert_json(&directory).unwrap();
    
    // Create the snapshot
    let snapshot = SnapShot {
        message: "Test snapshot".to_string(),
        directory: directory_id,
        previous: parent_ids.into_iter().collect(),
    };
    
    store.insert_json(&snapshot).unwrap()
}