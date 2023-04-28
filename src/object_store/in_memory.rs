use std::{
    collections::{btree_map::Entry, BTreeMap},
    convert::Infallible,
};

use crate::object_id::ObjectId;

use super::ObjectStore;

/// An ephemeral [`ObjectStore`] stored in memory using a
/// [`BTreeMap`].
pub struct InMemoryObjectStore {
    objects: BTreeMap<ObjectId, Vec<u8>>,
}

impl InMemoryObjectStore {
    /// Create a new, ephemeral [`InMemoryObjectStore`].
    pub fn new() -> Self {
        Self {
            objects: BTreeMap::new(),
        }
    }
}

impl ObjectStore for InMemoryObjectStore {
    type Error = Infallible;

    fn has(&self, id: ObjectId) -> Result<bool, Self::Error> {
        Ok(self.objects.contains_key(&id))
    }

    fn read(&self, id: ObjectId) -> Result<Option<Vec<u8>>, Self::Error> {
        match self.objects.get(&id) {
            Some(v) => Ok(Some(v.clone())),
            None => Ok(None),
        }
    }

    fn insert(&mut self, object: &[u8]) -> Result<ObjectId, Self::Error> {
        let id: ObjectId = object.into();
        match self.objects.entry(id) {
            Entry::Vacant(v) => {
                v.insert(object.into());
                Ok(id)
            }
            Entry::Occupied(_o) => Ok(id),
        }
    }
}

#[test]
fn test_in_memory_object_store() {
    let mut store = InMemoryObjectStore::new();
    store.insert(b"hello, world").unwrap();
    let b: &[u8] = b"hello, world";
    assert!(store.has(b.into()).unwrap());
    assert_eq!(store.read(b.into()).unwrap(), Some(Vec::from(b)));
}
