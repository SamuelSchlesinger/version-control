use crate::object_id::ObjectId;

/// A persistent implementation using a directory.
pub mod directory;

/// An ephemeral implementation using a [`BTreeMap`].
pub mod in_memory;

/// A trait for maps which store binary objects based on their
/// [`ObjectId`].
pub trait ObjectStore {
    type Error;

    /// Checks whether the given [`ObjectId`] is present in the store.
    fn has(&self, id: ObjectId) -> Result<bool, Self::Error>;

    /// Read the [`ObjectId`] out of the store, if it is present.
    fn read(&self, id: ObjectId) -> Result<Option<Vec<u8>>, Self::Error>;

    /// Insert the [`ObjectId`] into the store.
    fn insert(&mut self, object: &[u8]) -> Result<ObjectId, Self::Error>;
}
