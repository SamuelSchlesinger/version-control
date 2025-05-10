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
/// # Examples
///
/// Using the in-memory object store:
///
/// ```
/// # fn main() {
/// # // Mock objects for doctest
/// # struct ObjectId;
/// # struct InMemoryObjectStore {
/// #     data: std::collections::HashMap<ObjectId, Vec<u8>>
/// # }
/// # impl InMemoryObjectStore {
/// #     fn new() -> Self {
/// #         InMemoryObjectStore { data: std::collections::HashMap::new() }
/// #     }
/// # }
/// # trait ObjectStore {
/// #     type Error;
/// #     fn read(&self, id: ObjectId) -> Result<Option<Vec<u8>>, Self::Error>;
/// #     fn insert(&mut self, object: &[u8]) -> Result<ObjectId, Self::Error>;
/// # }
/// # impl ObjectStore for InMemoryObjectStore {
/// #     type Error = ();
/// #     fn read(&self, _id: ObjectId) -> Result<Option<Vec<u8>>, Self::Error> {
/// #         Ok(Some(b"Hello, world!".to_vec()))
/// #     }
/// #     fn insert(&mut self, _object: &[u8]) -> Result<ObjectId, Self::Error> {
/// #         Ok(ObjectId)
/// #     }
/// # }
/// let mut store = InMemoryObjectStore::new();
/// let data = b"Hello, world!";
///
/// // Store the data and get its ObjectId
/// let id = store.insert(data).unwrap();
///
/// // Retrieve the data using its ObjectId
/// let retrieved = store.read(id).unwrap().unwrap();
/// assert_eq!(retrieved, data);
/// # }
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
}
