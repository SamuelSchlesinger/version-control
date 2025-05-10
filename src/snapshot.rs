use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::object_id::ObjectId;

/// Represents a snapshot (commit) in the version history.
///
/// A `SnapShot` is a point-in-time capture of a repository's state, forming a node
/// in the version history graph. Each snapshot contains:
///
/// - A commit message describing the changes
/// - A reference to the directory structure (contents) at this point in time
/// - References to parent snapshots (can be multiple in case of a merge)
///
/// The version history is structured as a directed acyclic graph (DAG), allowing
/// for complex version histories including branches and merges.
///
/// # Storage
///
/// Snapshots are stored in the object store as serialized JSON, and are referenced
/// by their `ObjectId`. Branch references simply store the `ObjectId` of their
/// most recent snapshot.
///
/// # Example
///
/// Creating a new snapshot:
///
/// ```
/// # fn main() {
/// # // Mock structs for doctest
/// # #[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
/// # struct ObjectId(u64);
/// # impl ObjectId {
/// #     fn from<T>(_: T) -> Self { ObjectId(0) }
/// # }
/// # struct SnapShot {
/// #     message: String,
/// #     directory: ObjectId,
/// #     previous: std::collections::BTreeSet<ObjectId>
/// # }
/// use std::collections::BTreeSet;
///
/// // Create a snapshot with a message, directory reference and parent references
/// let snapshot = SnapShot {
///     message: String::from("Initial commit"),
///     directory: ObjectId::from(&b"directory_id"[..]),
///     previous: BTreeSet::from([ObjectId::from(&b"parent_snapshot_id"[..])])
/// };
/// # }
/// ```
#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct SnapShot {
    /// The commit message describing the changes in this snapshot.
    pub message: String,

    /// Reference to the directory structure at this point in time.
    /// This is an `ObjectId` pointing to a serialized `Directory` in the object store.
    pub directory: ObjectId,

    /// References to parent snapshots.
    ///
    /// For a normal commit, this will contain a single parent.
    /// For a merge commit, this will contain multiple parents.
    /// For the first commit in a repository, this will be empty.
    ///
    /// Each element is an `ObjectId` pointing to a serialized `SnapShot` in the object store.
    pub previous: BTreeSet<ObjectId>,
}
