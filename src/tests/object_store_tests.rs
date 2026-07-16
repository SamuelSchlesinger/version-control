use crate::object_id::ObjectId;
use crate::object_store::in_memory::InMemoryObjectStore;
use crate::object_store::{InsertWithIdError, ObjectStore};

#[test]
fn test_insert_with_id_verifies_hash() {
    let mut store = InMemoryObjectStore::new();
    let data = b"authentic content";
    let real_id = ObjectId::from(&data[..]);

    // Correct id: stored.
    assert!(store.insert_with_id(real_id, data).is_ok());
    assert!(store.has(real_id).unwrap());

    // A lying id (the hash of different bytes) is rejected as a mismatch, and
    // nothing is stored under it. This is the server's receive-side integrity
    // check against a malicious or corrupt upload.
    let wrong_id = ObjectId::from(&b"something else"[..]);
    let err = store.insert_with_id(wrong_id, data);
    assert!(matches!(err, Err(InsertWithIdError::HashMismatch { .. })), "got {err:?}");
    assert!(!store.has(wrong_id).unwrap(), "stored under the wrong id");
}

#[test]
fn test_object_store_insert_and_read() {
    let mut store = InMemoryObjectStore::new();
    let data = b"test data";

    // Insert data and get its ObjectId
    let id = store.insert(data).unwrap();

    // Check that the store has this id
    assert!(store.has(id).unwrap());

    // Read the data back
    let retrieved_data = store.read(id).unwrap();
    assert_eq!(retrieved_data, Some(data.to_vec()));
}

#[test]
fn test_object_store_nonexistent_id() {
    let store = InMemoryObjectStore::new();
    // Create a unique ObjectId for a non-existent object
    let nonexistent_id = ObjectId::from(&[0u8; 32][..]);

    // Check that the store doesn't have this id
    assert!(!store.has(nonexistent_id).unwrap());

    // Read should return None for nonexistent id
    let retrieved_data = store.read(nonexistent_id).unwrap();
    assert_eq!(retrieved_data, None);
}

#[test]
fn test_object_store_multiple_inserts() {
    let mut store = InMemoryObjectStore::new();
    let data1 = b"first data";
    let data2 = b"second data";

    // Insert first piece of data
    let id1 = store.insert(data1).unwrap();

    // Insert second piece of data
    let id2 = store.insert(data2).unwrap();

    // IDs should be different
    assert_ne!(id1, id2);

    // Check that both data can be retrieved correctly
    assert_eq!(store.read(id1).unwrap(), Some(data1.to_vec()));
    assert_eq!(store.read(id2).unwrap(), Some(data2.to_vec()));
}

#[test]
fn test_object_store_idempotent_insert() {
    let mut store = InMemoryObjectStore::new();
    let data = b"duplicate data";

    // Insert the same data twice
    let id1 = store.insert(data).unwrap();
    let id2 = store.insert(data).unwrap();

    // Should get the same ID both times
    assert_eq!(id1, id2);

    // Reading from this ID should give the original data
    assert_eq!(store.read(id1).unwrap(), Some(data.to_vec()));
}

#[test]
fn test_object_store_empty_data() {
    let mut store = InMemoryObjectStore::new();
    let empty_data = b"";

    // Insert empty data
    let id = store.insert(empty_data).unwrap();

    // Check that the store has this id
    assert!(store.has(id).unwrap());

    // Reading should return empty vector
    assert_eq!(store.read(id).unwrap(), Some(Vec::new()));
}

#[test]
fn test_object_store_large_data() {
    let mut store = InMemoryObjectStore::new();
    let large_data = vec![0xAA; 10_000]; // 10KB of data

    // Insert large data
    let id = store.insert(&large_data).unwrap();

    // Check that the store has this id
    assert!(store.has(id).unwrap());

    // Reading should return the complete large data
    assert_eq!(store.read(id).unwrap(), Some(large_data));
}