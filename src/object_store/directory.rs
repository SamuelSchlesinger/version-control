use std::{
    collections::HashMap,
    fs::create_dir,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use crate::object_id::ObjectId;

use super::ObjectStore;

// Cache entry with object data and expiration time
#[derive(Debug, Clone)]
struct CacheEntry {
    data: Vec<u8>,
    last_accessed: Instant,
}

// Cache configuration
const CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes
const CACHE_MAX_SIZE: usize = 50 * 1024 * 1024; // 50 MB

/// A persistent [`ObjectStore`] stored in a directory,
/// using the first two hexadecimal characters of the [`ObjectId`]
/// to determine which directory to place the binary object in
/// and creating a file with the rest of the hexadecimal characters
/// as the file name.
#[derive(Debug, Clone)]
pub struct DirectoryObjectStore {
    root: PathBuf,
    // Cache for objects to improve read performance
    cache: Arc<RwLock<HashMap<ObjectId, CacheEntry>>>,
    // Track current cache size in bytes
    cache_size: Arc<Mutex<usize>>,
}

impl DirectoryObjectStore {
    pub fn new(root: PathBuf) -> Result<Self, std::io::Error> {
        if !Path::try_exists(&root)? {
            log::info!("creating directory store root: {:?}", root);
            create_dir(&root)?;
        }
        Ok(Self {
            root,
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_size: Arc::new(Mutex::new(0)),
        })
    }

    // Clean up expired cache entries to free memory
    fn cleanup_cache(&self) {
        let now = Instant::now();
        let mut cache = self.cache.write().unwrap();
        let mut cache_size = self.cache_size.lock().unwrap();

        let mut to_remove = Vec::new();

        // Find expired entries
        for (id, entry) in cache.iter() {
            if now.duration_since(entry.last_accessed) > CACHE_TTL {
                to_remove.push(*id);
                *cache_size = cache_size.saturating_sub(entry.data.len());
            }
        }

        // Remove expired entries
        for id in to_remove {
            cache.remove(&id);
        }
    }

    // Add an object to the cache
    fn cache_object(&self, id: ObjectId, data: Vec<u8>) {
        // Clean up expired entries first
        self.cleanup_cache();

        let data_size = data.len();
        let mut cache = self.cache.write().unwrap();
        let mut cache_size = self.cache_size.lock().unwrap();

        // If adding this object would exceed max cache size,
        // evict entries until we have enough space
        if *cache_size + data_size > CACHE_MAX_SIZE {
            // Find and remove the oldest entries until we have enough space
            // Create a vector of IDs to remove
            let mut entries_to_remove = Vec::new();
            let mut size_freed = 0;

            {
                // Sort by last accessed time (oldest first)
                let mut entries: Vec<_> = cache.iter().collect();
                entries.sort_by_key(|(_, entry)| entry.last_accessed);

                // Find oldest entries to remove
                for (id, entry) in entries {
                    entries_to_remove.push(*id);
                    size_freed += entry.data.len();

                    if *cache_size - size_freed + data_size <= CACHE_MAX_SIZE {
                        break;
                    }
                }
            }

            // Now remove the entries from the cache
            for id in entries_to_remove {
                if let Some(entry) = cache.remove(&id) {
                    *cache_size = cache_size.saturating_sub(entry.data.len());
                }
            }
        }

        // Add the new entry
        cache.insert(id, CacheEntry {
            data,
            last_accessed: Instant::now(),
        });
        *cache_size += data_size;
    }

    // Get an object from the cache
    fn get_cached_object(&self, id: ObjectId) -> Option<Vec<u8>> {
        let mut cache = self.cache.write().unwrap();
        if let Some(entry) = cache.get_mut(&id) {
            // Update the last accessed time
            entry.last_accessed = Instant::now();
            Some(entry.data.clone())
        } else {
            None
        }
    }
}

impl ObjectStore for DirectoryObjectStore {
    type Error = std::io::Error;

    fn has(&self, id: ObjectId) -> Result<bool, Self::Error> {
        log::info!("checking whether {id} is contained in {:?}", self.root);

        // Check if the object is in the cache first
        if self.get_cached_object(id).is_some() {
            return Ok(true);
        }

        // Otherwise check on disk
        let s: String = format!("{}", id);
        let subdir: &str = &s[0..2];
        let filename: &str = &s[2..];
        let path = self.root.join(format!("{}/{}", subdir, filename));
        Path::try_exists(&path)
    }

    fn read(&self, id: ObjectId) -> Result<Option<Vec<u8>>, Self::Error> {
        log::info!("reading {id} from {:?}", self.root);

        // Try to get from cache first
        if let Some(data) = self.get_cached_object(id) {
            log::info!("cache hit for {id}");
            return Ok(Some(data));
        }

        // Otherwise read from disk
        log::info!("cache miss for {id}, reading from disk");
        let s: String = format!("{id}");
        let subdir: &str = &s[0..2];
        let filename: &str = &s[2..];
        let path = self.root.join(format!("{subdir}/{filename}"));
        match std::fs::File::options().read(true).open(path) {
            Ok(mut f) => {
                let mut v = Vec::new();
                f.read_to_end(&mut v)?;

                // Cache the object for future reads
                self.cache_object(id, v.clone());

                Ok(Some(v))
            }
            Err(err) => {
                if err.kind() == ErrorKind::NotFound {
                    Ok(None)
                } else {
                    Err(err)
                }
            }
        }
    }

    fn insert(&mut self, object: &[u8]) -> Result<ObjectId, Self::Error> {
        let id: ObjectId = object.into();
        log::info!("inserting {id} into {:?}", self.root);

        // Check if already in cache
        if self.get_cached_object(id).is_some() {
            log::info!("{id} already exists in cache");
            return Ok(id);
        }

        // Check if already on disk
        let s: String = format!("{id}");
        let subdir: &str = &s[0..2];
        let filename: &str = &s[2..];
        let subdir_path = self.root.join(subdir);
        let path = subdir_path.join(filename);
        if Path::try_exists(&path)? {
            log::info!("{path:?} already exists on disk");

            // The object is already stored; content addressing guarantees the
            // bytes are identical, so there is nothing to write. We deliberately
            // do NOT read it back to warm the cache: during a snapshot this is
            // the common case for every unchanged file, and reading each stored
            // object off disk just to populate a cache the snapshot never reads
            // doubled the I/O of the whole operation. A later read() will cache
            // it on demand.
            return Ok(id);
        }

        // Create the fan-out subdirectory, tolerating a concurrent creator
        // (check-then-create races otherwise fail with AlreadyExists).
        match std::fs::create_dir(&subdir_path) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }

        // Write atomically: fill a temp file in the same directory, flush it,
        // then rename into place. A crash or a concurrent writer can therefore
        // never leave a truncated file at the content-addressed path (whose name
        // must always match its bytes). The rename is atomic on the same
        // filesystem, and if two processes store the same id concurrently the
        // loser simply overwrites with byte-identical content.
        let mut tmp = tempfile::NamedTempFile::new_in(&subdir_path)?;
        tmp.write_all(object)?;
        tmp.as_file().sync_all()?;
        tmp.persist(&path).map_err(|e| e.error)?;

        // Add to cache
        self.cache_object(id, object.to_vec());

        Ok(id)
    }
}

#[test]
fn test_directory_object_store() {
    let tempdir = tempfile::tempdir().unwrap();
    let mut store = DirectoryObjectStore::new(tempdir.path().into()).unwrap();
    store.insert(b"hello, world").unwrap();
    let b: &[u8] = b"hello, world";
    assert!(store.has(b.into()).unwrap());
    assert_eq!(store.read(b.into()).unwrap(), Some(Vec::from(b)));
}
