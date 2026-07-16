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

    /// Checks whether the given `ObjectId` is present in the store.
    ///
    /// # Arguments
    ///
    /// * `id` - The `ObjectId` to check
    ///
    /// # Returns
    ///
    /// * `Result<bool, Self::Error>` - `true` if the object exists, `false` otherwise
    fn has(&self, id: ObjectId) -> Result<bool, Self::Error>;

    /// Retrieves an object from the store by its `ObjectId`.
    ///
    /// # Arguments
    ///
    /// * `id` - The `ObjectId` of the object to retrieve
    ///
    /// # Returns
    ///
    /// * `Result<Option<Vec<u8>>, Self::Error>` - The object's data if found, `None` if not present
    fn read(&self, id: ObjectId) -> Result<Option<Vec<u8>>, Self::Error>;

    /// Inserts an object into the store.
    ///
    /// This method calculates the `ObjectId` (hash) of the object and stores the object
    /// under that identifier. If an object with the same content already exists, it will
    /// not be stored again, but the same `ObjectId` will be returned.
    ///
    /// # Arguments
    ///
    /// * `object` - The binary data to store
    ///
    /// # Returns
    ///
    /// * `Result<ObjectId, Self::Error>` - The `ObjectId` of the stored object
    fn insert(&mut self, object: &[u8]) -> Result<ObjectId, Self::Error>;

    /// Inserts an object into the store with a pre-computed `ObjectId`.
    ///
    /// This method is useful when syncing objects from a remote store where
    /// the ID has already been computed. The implementation should verify
    /// that the provided ID matches the content hash.
    ///
    /// # Arguments
    ///
    /// * `id` - The pre-computed `ObjectId` for the object
    /// * `object` - The binary data to store
    ///
    /// # Returns
    ///
    /// * `Result<(), Self::Error>` - Ok if stored successfully
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
