use serde::{Deserialize, Serialize};

use crate::object_id::ObjectId;

/// A particular snapshot of a version.
#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct SnapShot {
    /// The [`ObjectId`] of the message added with the commit.
    pub message: ObjectId,
    /// The [`ObjectId`] of the directory structure.
    pub directory: ObjectId,
    /// The previous [`SnapShot`].
    pub previous: Option<ObjectId>,
}
