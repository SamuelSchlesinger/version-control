use std::fmt;

use crate::object_id::ObjectId;

/// A persistent implementation using a directory structure on the filesystem.
pub mod directory;

/// An ephemeral implementation using a [`BTreeMap`] for in-memory storage.
pub mod in_memory;

/// A trait defining the content-addressable object store interface.
///
/// The `ObjectStore` is a fundamental component of the version control system,
/// providing a way to store and retrieve binary objects based on their content hash.
/// This abstraction allows for different storage backends (filesystem, in-memory, etc.)
/// while maintaining a consistent interface.
///
/// # Design
///
/// The store implements a content-addressable storage system where:
/// - Objects are identified solely by their content via `ObjectId` (a BLAKE3 hash)
/// - Objects are immutable once stored
/// - The same content always maps to the same identifier
/// - Duplicate content is automatically deduplicated
///
/// # Available Implementations
///
/// - `directory`: A persistent filesystem-based implementation
/// - `in_memory`: An ephemeral in-memory implementation used primarily for testing
///
/// ```
/// use lib::object_store::ObjectStore;
/// use lib::object_store::in_memory::InMemoryObjectStore;
///
/// let mut store = InMemoryObjectStore::new();
/// let id = store.insert(b"Hello, world!").unwrap();
/// // Content is addressed by its hash, so it reads back byte-for-byte.
/// assert_eq!(store.read(id).unwrap().unwrap(), b"Hello, world!");
/// ```
pub trait ObjectStore {
    /// The error type returned by operations on this store.
    type Error;

    /// Returns whether an object with this id is present in the store.
    fn has(&self, id: ObjectId) -> Result<bool, Self::Error>;

    /// Retrieves an object by its id, or `None` if it is not present.
    fn read(&self, id: ObjectId) -> Result<Option<Vec<u8>>, Self::Error>;

    /// Stores an object and returns its content-hash id. Content that is already
    /// present is deduplicated: the same id is returned without storing again.
    fn insert(&mut self, object: &[u8]) -> Result<ObjectId, Self::Error>;

    /// Stores an object whose id was computed elsewhere (e.g. received from a
    /// remote). The default implementation verifies the bytes hash to the
    /// claimed `id` *before* storing, so a lying id is rejected rather than
    /// corrupting the store.
    fn insert_with_id(
        &mut self,
        id: ObjectId,
        object: &[u8],
    ) -> Result<(), InsertWithIdError<Self::Error>> {
        // Verify the hash BEFORE storing. A remote (client or server) can send
        // a lying id; the old code inserted the bytes first and then panicked,
        // both crashing the process and leaving orphaned data behind.
        let computed_id = ObjectId::from(object);
        if computed_id != id {
            return Err(InsertWithIdError::HashMismatch {
                expected: id,
                actual: computed_id,
            });
        }
        self.insert(object).map_err(InsertWithIdError::Store)?;
        Ok(())
    }
}

/// Failure modes of [`ObjectStore::insert_with_id`].
#[derive(Debug)]
pub enum InsertWithIdError<E> {
    /// The underlying store failed to store the object.
    Store(E),
    /// The provided id did not match the hash of the object. This means the
    /// data was corrupted in transit or the sender is misbehaving.
    HashMismatch { expected: ObjectId, actual: ObjectId },
}

impl<E: fmt::Display> fmt::Display for InsertWithIdError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InsertWithIdError::Store(e) => write!(f, "failed to store object: {e}"),
            InsertWithIdError::HashMismatch { expected, actual } => write!(
                f,
                "object id mismatch: claimed {expected} but content hashes to {actual}"
            ),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for InsertWithIdError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InsertWithIdError::Store(e) => Some(e),
            InsertWithIdError::HashMismatch { .. } => None,
        }
    }
}
