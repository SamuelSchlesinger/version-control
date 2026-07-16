use crate::directory::{Directory, DirectoryEntry};
use crate::object_store::ObjectStore;
use crate::object_store::in_memory::InMemoryObjectStore;
use crate::snapshot::SnapShot;
use std::collections::BTreeMap;

/// This is an integration test that simulates basic version control operations
/// It creates a directory structure, commits it to a repository, then makes changes
/// and creates a new commit.
#[test]
fn test_basic_version_control_flow() {
    // Create an in-memory object store
    let mut store = InMemoryObjectStore::new();

    // Create initial file structure
    let mut root_entries = BTreeMap::new();
    let file1_content = b"Hello, world!".to_vec();
    let file1_id = store.insert(&file1_content).unwrap();
    root_entries.insert("file1.txt".to_string(), DirectoryEntry::File(file1_id));

    let file2_content = b"Another file".to_vec();
    let file2_id = store.insert(&file2_content).unwrap();
    root_entries.insert("file2.txt".to_string(), DirectoryEntry::File(file2_id));

    // Create a subdirectory with a file
    let mut subdir_entries = BTreeMap::new();
    let subfile_content = b"File in subdirectory".to_vec();
    let subfile_id = store.insert(&subfile_content).unwrap();
    subdir_entries.insert("subfile.txt".to_string(), DirectoryEntry::File(subfile_id));

    let subdir = Directory { root: subdir_entries };
    root_entries.insert("subdir".to_string(), DirectoryEntry::Directory(Box::new(subdir)));

    // Create the root directory structure
    let root_dir = Directory { root: root_entries };
    let root_serialized = serde_json::to_vec(&root_dir).unwrap();
    let root_dir_id = store.insert(&root_serialized).unwrap();

    // Create the initial snapshot/commit
    let initial_snapshot = SnapShot {
        message: "Initial commit".to_string(),
        directory: root_dir_id,
        previous: Vec::new(),
    };

    let snapshot_serialized = serde_json::to_vec(&initial_snapshot).unwrap();
    let snapshot_id = store.insert(&snapshot_serialized).unwrap();

    // In a real scenario, we would use DotRev::init() to create the repository
    // but for testing purposes we're just simulating branch references

    // Verify we can retrieve the committed files
    let retrieved_snapshot_bytes = store.read(snapshot_id).unwrap().unwrap();
    let retrieved_snapshot: SnapShot = serde_json::from_slice(&retrieved_snapshot_bytes).unwrap();

    assert_eq!(retrieved_snapshot.message, "Initial commit");
    assert_eq!(retrieved_snapshot.directory, root_dir_id);

    // Now make changes and create a second commit

    // 1. Modify file1.txt
    let new_file1_content = b"Hello, modified world!".to_vec();
    let new_file1_id = store.insert(&new_file1_content).unwrap();

    // 2. Create the new directory structure
    let mut new_root_entries = BTreeMap::new();
    new_root_entries.insert("file1.txt".to_string(), DirectoryEntry::File(new_file1_id)); // Modified file
    new_root_entries.insert("file2.txt".to_string(), DirectoryEntry::File(file2_id));     // Unchanged file

    // Copy the subdirectory from the root_dir
    match root_dir.root.get("subdir").unwrap() {
        DirectoryEntry::Directory(subdir) => {
            new_root_entries.insert("subdir".to_string(), DirectoryEntry::Directory(subdir.clone()));
        },
        _ => panic!("Expected subdir to be a directory"),
    }

    let new_root_dir = Directory { root: new_root_entries };
    let new_root_serialized = serde_json::to_vec(&new_root_dir).unwrap();
    let new_root_dir_id = store.insert(&new_root_serialized).unwrap();

    // 3. Create a new snapshot with the previous one as parent
    let previous = vec![snapshot_id];

    let second_snapshot = SnapShot {
        message: "Update file1.txt".to_string(),
        directory: new_root_dir_id,
        previous,
    };

    let second_snapshot_serialized = serde_json::to_vec(&second_snapshot).unwrap();
    let second_snapshot_id = store.insert(&second_snapshot_serialized).unwrap();

    // In a real scenario, we would update the branch reference in the DotRev

    // Verify the second commit
    let retrieved_second_snapshot_bytes = store.read(second_snapshot_id).unwrap().unwrap();
    let retrieved_second_snapshot: SnapShot = serde_json::from_slice(&retrieved_second_snapshot_bytes).unwrap();

    assert_eq!(retrieved_second_snapshot.message, "Update file1.txt");
    assert_eq!(retrieved_second_snapshot.directory, new_root_dir_id);
    assert!(retrieved_second_snapshot.previous.contains(&snapshot_id));

    // Verify we can retrieve the file content
    let retrieved_root_dir_bytes = store.read(new_root_dir_id).unwrap().unwrap();
    let retrieved_root_dir: Directory = serde_json::from_slice(&retrieved_root_dir_bytes).unwrap();

    // Get the modified file ID
    if let DirectoryEntry::File(file_id) = retrieved_root_dir.root.get("file1.txt").unwrap() {
        let retrieved_file1_content = store.read(*file_id).unwrap().unwrap();
        // Verify file content was updated
        assert_eq!(retrieved_file1_content, new_file1_content);
    } else {
        panic!("Expected file1.txt to be a file");
    }
}

#[test]
fn test_branching() {
    // Create an in-memory object store
    let mut store = InMemoryObjectStore::new();

    // Create a simple file
    let file_content = b"Initial content".to_vec();
    let file_id = store.insert(&file_content).unwrap();

    let mut root_entries = BTreeMap::new();
    root_entries.insert("file.txt".to_string(), DirectoryEntry::File(file_id));

    // Create root directory
    let root_dir = Directory { root: root_entries };
    let root_serialized = serde_json::to_vec(&root_dir).unwrap();
    let root_dir_id = store.insert(&root_serialized).unwrap();

    // Create initial snapshot
    let initial_snapshot = SnapShot {
        message: "Initial commit".to_string(),
        directory: root_dir_id,
        previous: Vec::new(),
    };

    let snapshot_serialized = serde_json::to_vec(&initial_snapshot).unwrap();
    let snapshot_id = store.insert(&snapshot_serialized).unwrap();

    // In a real scenario, we would use DotRev::init() to create the repository

    // Create two different branches from this point

    // Branch 1: feature-a
    let feature_a_content = b"Feature A content".to_vec();
    let feature_a_id = store.insert(&feature_a_content).unwrap();

    let mut feature_a_entries = BTreeMap::new();
    feature_a_entries.insert("file.txt".to_string(), DirectoryEntry::File(feature_a_id));

    let feature_a_dir = Directory { root: feature_a_entries };
    let feature_a_serialized = serde_json::to_vec(&feature_a_dir).unwrap();
    let feature_a_dir_id = store.insert(&feature_a_serialized).unwrap();

    let previous = vec![snapshot_id];

    let feature_a_snapshot = SnapShot {
        message: "Feature A changes".to_string(),
        directory: feature_a_dir_id,
        previous: previous.clone(),
    };

    let feature_a_snapshot_serialized = serde_json::to_vec(&feature_a_snapshot).unwrap();
    let feature_a_snapshot_id = store.insert(&feature_a_snapshot_serialized).unwrap();

    // Branch 2: feature-b
    let feature_b_content = b"Feature B content".to_vec();
    let feature_b_id = store.insert(&feature_b_content).unwrap();

    let mut feature_b_entries = BTreeMap::new();
    feature_b_entries.insert("file.txt".to_string(), DirectoryEntry::File(feature_b_id));

    let feature_b_dir = Directory { root: feature_b_entries };
    let feature_b_serialized = serde_json::to_vec(&feature_b_dir).unwrap();
    let feature_b_dir_id = store.insert(&feature_b_serialized).unwrap();

    let feature_b_snapshot = SnapShot {
        message: "Feature B changes".to_string(),
        directory: feature_b_dir_id,
        previous,
    };

    let feature_b_snapshot_serialized = serde_json::to_vec(&feature_b_snapshot).unwrap();
    let feature_b_snapshot_id = store.insert(&feature_b_snapshot_serialized).unwrap();

    // In a real scenario, we would update branch references using DotRev methods

    // There's no need to verify branch references since we're not using actual DotRev functionality

    // Verify different content in each branch
    let feature_a_snapshot_bytes = store.read(feature_a_snapshot_id).unwrap().unwrap();
    let feature_a_snapshot: SnapShot = serde_json::from_slice(&feature_a_snapshot_bytes).unwrap();
    let feature_a_dir_bytes = store.read(feature_a_snapshot.directory).unwrap().unwrap();
    let feature_a_dir: Directory = serde_json::from_slice(&feature_a_dir_bytes).unwrap();

    let feature_b_snapshot_bytes = store.read(feature_b_snapshot_id).unwrap().unwrap();
    let feature_b_snapshot: SnapShot = serde_json::from_slice(&feature_b_snapshot_bytes).unwrap();
    let feature_b_dir_bytes = store.read(feature_b_snapshot.directory).unwrap().unwrap();
    let feature_b_dir: Directory = serde_json::from_slice(&feature_b_dir_bytes).unwrap();

    // Extract file content from directories
    let feature_a_file_id = match feature_a_dir.root.get("file.txt").unwrap() {
        DirectoryEntry::File(id) => id,
        _ => panic!("Expected file.txt to be a file"),
    };
    let feature_a_file_content = store.read(*feature_a_file_id).unwrap().unwrap();

    let feature_b_file_id = match feature_b_dir.root.get("file.txt").unwrap() {
        DirectoryEntry::File(id) => id,
        _ => panic!("Expected file.txt to be a file"),
    };
    let feature_b_file_content = store.read(*feature_b_file_id).unwrap().unwrap();

    // Content should be different between branches
    assert_ne!(feature_a_file_content, feature_b_file_content);
    assert_eq!(feature_a_file_content, feature_a_content);
    assert_eq!(feature_b_file_content, feature_b_content);
}

#[test]
fn test_merging() {
    // Create an in-memory object store
    let mut store = InMemoryObjectStore::new();

    // Create initial file structure
    let file1_content = b"Original content".to_vec();
    let file1_id = store.insert(&file1_content).unwrap();

    let file2_content = b"Common file".to_vec();
    let file2_id = store.insert(&file2_content).unwrap();

    let mut root_entries = BTreeMap::new();
    root_entries.insert("file1.txt".to_string(), DirectoryEntry::File(file1_id));
    root_entries.insert("file2.txt".to_string(), DirectoryEntry::File(file2_id));

    // Create root directory
    let root_dir = Directory { root: root_entries };
    let root_serialized = serde_json::to_vec(&root_dir).unwrap();
    let root_dir_id = store.insert(&root_serialized).unwrap();

    // Create initial snapshot
    let initial_snapshot = SnapShot {
        message: "Initial commit".to_string(),
        directory: root_dir_id,
        previous: Vec::new(),
    };

    let snapshot_serialized = serde_json::to_vec(&initial_snapshot).unwrap();
    let snapshot_id = store.insert(&snapshot_serialized).unwrap();

    // Create first branch: feature
    let feature_file1_content = b"Feature branch content".to_vec();
    let feature_file1_id = store.insert(&feature_file1_content).unwrap();

    let feature_file3_content = b"New file in feature".to_vec();
    let feature_file3_id = store.insert(&feature_file3_content).unwrap();

    let mut feature_entries = BTreeMap::new();
    feature_entries.insert("file1.txt".to_string(), DirectoryEntry::File(feature_file1_id));
    feature_entries.insert("file2.txt".to_string(), DirectoryEntry::File(file2_id)); // unchanged
    feature_entries.insert("file3.txt".to_string(), DirectoryEntry::File(feature_file3_id)); // new file

    let feature_dir = Directory { root: feature_entries };
    let feature_serialized = serde_json::to_vec(&feature_dir).unwrap();
    let feature_dir_id = store.insert(&feature_serialized).unwrap();

    let previous = vec![snapshot_id];

    let feature_snapshot = SnapShot {
        message: "Feature branch changes".to_string(),
        directory: feature_dir_id,
        previous: previous.clone(),
    };

    let feature_snapshot_serialized = serde_json::to_vec(&feature_snapshot).unwrap();
    let feature_snapshot_id = store.insert(&feature_snapshot_serialized).unwrap();

    // Create second branch: main continued
    let main_file1_content = b"Updated content on main".to_vec();
    let main_file1_id = store.insert(&main_file1_content).unwrap();

    let main_file4_content = b"New file in main".to_vec();
    let main_file4_id = store.insert(&main_file4_content).unwrap();

    let mut main_entries = BTreeMap::new();
    main_entries.insert("file1.txt".to_string(), DirectoryEntry::File(main_file1_id));
    main_entries.insert("file2.txt".to_string(), DirectoryEntry::File(file2_id)); // unchanged
    main_entries.insert("file4.txt".to_string(), DirectoryEntry::File(main_file4_id)); // new file

    let main_dir = Directory { root: main_entries };
    let main_serialized = serde_json::to_vec(&main_dir).unwrap();
    let main_dir_id = store.insert(&main_serialized).unwrap();

    let main_snapshot = SnapShot {
        message: "Main branch changes".to_string(),
        directory: main_dir_id,
        previous,
    };

    let main_snapshot_serialized = serde_json::to_vec(&main_snapshot).unwrap();
    let main_snapshot_id = store.insert(&main_snapshot_serialized).unwrap();

    // Create merge snapshot (simulating a merge)
    // In a real merge, we'd need to handle file conflicts, but for this test
    // we'll just create a merged directory structure

    // The merged file uses the main branch version of file1.txt
    let mut merged_entries = BTreeMap::new();
    merged_entries.insert("file1.txt".to_string(), DirectoryEntry::File(main_file1_id));
    merged_entries.insert("file2.txt".to_string(), DirectoryEntry::File(file2_id));
    merged_entries.insert("file3.txt".to_string(), DirectoryEntry::File(feature_file3_id)); // from feature branch
    merged_entries.insert("file4.txt".to_string(), DirectoryEntry::File(main_file4_id));    // from main branch

    let merged_dir = Directory { root: merged_entries };
    let merged_serialized = serde_json::to_vec(&merged_dir).unwrap();
    let merged_dir_id = store.insert(&merged_serialized).unwrap();

    // The merge snapshot has both feature and main as parents
    let merge_previous = vec![feature_snapshot_id, main_snapshot_id];

    let merge_snapshot = SnapShot {
        message: "Merge feature into main".to_string(),
        directory: merged_dir_id,
        previous: merge_previous,
    };

    let merge_snapshot_serialized = serde_json::to_vec(&merge_snapshot).unwrap();
    let merge_snapshot_id = store.insert(&merge_snapshot_serialized).unwrap();

    // Verify merge has two parents
    let retrieved_merge_snapshot_bytes = store.read(merge_snapshot_id).unwrap().unwrap();
    let retrieved_merge_snapshot: SnapShot = serde_json::from_slice(&retrieved_merge_snapshot_bytes).unwrap();

    assert_eq!(retrieved_merge_snapshot.previous.len(), 2);
    assert!(retrieved_merge_snapshot.previous.contains(&feature_snapshot_id));
    assert!(retrieved_merge_snapshot.previous.contains(&main_snapshot_id));

    // Verify merged directory contains files from both branches
    let merged_dir_bytes = store.read(retrieved_merge_snapshot.directory).unwrap().unwrap();
    let merged_dir: Directory = serde_json::from_slice(&merged_dir_bytes).unwrap();

    assert!(merged_dir.root.contains_key("file1.txt"));
    assert!(merged_dir.root.contains_key("file2.txt"));
    assert!(merged_dir.root.contains_key("file3.txt")); // from feature branch
    assert!(merged_dir.root.contains_key("file4.txt")); // from main branch

    // Verify content of the files
    let file1_in_merge_id = match merged_dir.root.get("file1.txt").unwrap() {
        DirectoryEntry::File(id) => id,
        _ => panic!("Expected file1.txt to be a file"),
    };
    let file1_in_merge_content = store.read(*file1_in_merge_id).unwrap().unwrap();
    assert_eq!(file1_in_merge_content, main_file1_content); // Merged content chose main

    let file3_in_merge_id = match merged_dir.root.get("file3.txt").unwrap() {
        DirectoryEntry::File(id) => id,
        _ => panic!("Expected file3.txt to be a file"),
    };
    let file3_in_merge_content = store.read(*file3_in_merge_id).unwrap().unwrap();
    assert_eq!(file3_in_merge_content, feature_file3_content); // From feature branch

    let file4_in_merge_id = match merged_dir.root.get("file4.txt").unwrap() {
        DirectoryEntry::File(id) => id,
        _ => panic!("Expected file4.txt to be a file"),
    };
    let file4_in_merge_content = store.read(*file4_in_merge_id).unwrap().unwrap();
    assert_eq!(file4_in_merge_content, main_file4_content); // From main branch
}