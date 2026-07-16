use serde::{Deserialize, Serialize};

use crate::object_id::ObjectId;

/// A snapshot (commit) in the version history.
///
/// Captures the repository at a point in time — a message, a reference to the
/// directory tree, and its parent snapshots (more than one for a merge) — and
/// forms a node in the directed acyclic history graph. Snapshots are stored in
/// the object store as JSON, addressed by their `ObjectId`.
///
/// ```
/// use lib::snapshot::SnapShot;
/// use lib::object_id::ObjectId;
///
/// let snapshot = SnapShot {
///     message: "Initial commit".to_string(),
///     directory: ObjectId::from(&b"directory contents"[..]),
///     previous: vec![], // the first commit has no parent
/// };
/// assert!(snapshot.previous.is_empty());
/// ```
#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct SnapShot {
    /// The commit message describing the changes in this snapshot.
    pub message: String,

    /// Reference to the directory structure at this point in time.
    /// This is an `ObjectId` pointing to a serialized `Directory` in the object store.
    pub directory: ObjectId,

    /// References to parent snapshots, **in order**: the first element is the
    /// mainline (first) parent — the branch that was checked out when this
    /// snapshot was made. `log` and `HEAD~N` follow this first parent, so the
    /// ordering is load-bearing and must not be replaced with a set (which would
    /// order by hash and make "the first parent" meaningless).
    ///
    /// Empty for the initial commit; one entry for an ordinary snapshot; two or
    /// more for a merge (`[ours, theirs]`).
    pub previous: Vec<ObjectId>,
}
