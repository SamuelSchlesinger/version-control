use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::object_id::ObjectId;

/// A particular snapshot of a version.
#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct SnapShot {
    /// The message added with the commit.
    pub message: String,
    /// The [`ObjectId`] of the directory structure.
    pub directory: ObjectId,
    /// The previous [`SnapShot`]s' [`ObjectId`]s, if there were some.
    pub previous: BTreeSet<ObjectId>,
}
