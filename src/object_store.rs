use crate::object_id::ObjectId;

pub mod directory;
pub mod in_memory;

pub trait ObjectStore {
    type Error;

    fn has(&self, id: ObjectId) -> Result<bool, Self::Error>;

    fn read(&self, id: ObjectId) -> Result<Option<Vec<u8>>, Self::Error>;

    fn insert(&mut self, object: &[u8]) -> Result<ObjectId, Self::Error>;
}
