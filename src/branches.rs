use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branches {
    pub branches: BTreeSet<(String, ObjectId)>,
}
