use serde::{Deserialize, Serialize};

use crate::object_id::ObjectId;

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct SnapShot {
    pub message: ObjectId,
    pub directory: ObjectId,
    pub previous: ObjectId,
}
