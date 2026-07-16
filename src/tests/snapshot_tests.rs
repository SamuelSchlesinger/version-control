use crate::object_id::ObjectId;
use crate::snapshot::SnapShot;

#[test]
fn test_snapshot_creation() {
    let directory = ObjectId::from(&b"1234567890abcdef1234567890abcdef12345678"[..]);
    let snapshot = SnapShot {
        message: "Initial commit".to_string(),
        directory,
        previous: Vec::new(),
    };

    assert_eq!(snapshot.message, "Initial commit");
    assert_eq!(snapshot.directory, directory);
    assert!(snapshot.previous.is_empty());
}

#[test]
fn test_snapshot_with_previous() {
    let directory = ObjectId::from(&b"1234567890abcdef1234567890abcdef12345678"[..]);
    let previous_id = ObjectId::from(&b"abcdef1234567890abcdef1234567890abcdef12"[..]);

    let previous = vec![previous_id];

    let snapshot = SnapShot {
        message: "Second commit".to_string(),
        directory,
        previous,
    };

    assert_eq!(snapshot.message, "Second commit");
    assert_eq!(snapshot.directory, directory);
    assert_eq!(snapshot.previous.len(), 1);
    assert!(snapshot.previous.contains(&previous_id));
}

#[test]
fn test_snapshot_with_multiple_previous() {
    let directory = ObjectId::from(&b"1234567890abcdef1234567890abcdef12345678"[..]);
    let previous_id1 = ObjectId::from(&b"abcdef1234567890abcdef1234567890abcdef12"[..]);
    let previous_id2 = ObjectId::from(&b"fedcba0987654321fedcba0987654321fedcba09"[..]);

    let previous = vec![previous_id1, previous_id2];

    let snapshot = SnapShot {
        message: "Merge commit".to_string(),
        directory,
        previous,
    };

    assert_eq!(snapshot.message, "Merge commit");
    assert_eq!(snapshot.directory, directory);
    assert_eq!(snapshot.previous.len(), 2);
    assert!(snapshot.previous.contains(&previous_id1));
    assert!(snapshot.previous.contains(&previous_id2));
}

#[test]
fn test_snapshot_equality() {
    let directory = ObjectId::from(&b"1234567890abcdef1234567890abcdef12345678"[..]);

    let snapshot1 = SnapShot {
        message: "Same commit".to_string(),
        directory,
        previous: Vec::new(),
    };

    let snapshot2 = SnapShot {
        message: "Same commit".to_string(),
        directory,
        previous: Vec::new(),
    };

    let snapshot3 = SnapShot {
        message: "Different commit".to_string(),
        directory,
        previous: Vec::new(),
    };

    assert_eq!(snapshot1, snapshot2);
    assert_ne!(snapshot1, snapshot3);
}

#[test]
fn test_snapshot_clone() {
    let directory = ObjectId::from(&b"1234567890abcdef1234567890abcdef12345678"[..]);
    let previous_id = ObjectId::from(&b"abcdef1234567890abcdef1234567890abcdef12"[..]);

    let previous = vec![previous_id];

    let snapshot = SnapShot {
        message: "Original commit".to_string(),
        directory,
        previous: previous.clone(),
    };

    let cloned_snapshot = snapshot.clone();

    assert_eq!(snapshot, cloned_snapshot);
    assert_eq!(cloned_snapshot.message, "Original commit");
    assert_eq!(cloned_snapshot.directory, directory);
    assert_eq!(cloned_snapshot.previous, previous);
}