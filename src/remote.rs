use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use crate::object_id::ObjectId;
use crate::snapshot::SnapShot;
use crate::directory::Directory;

/// HTTP client implementation
pub mod http_client;

/// HTTP server implementation
pub mod http_server;

/// A fetched object: its id and its bytes, or `None` if the remote didn't have it.
pub type FetchedObject = (ObjectId, Option<Vec<u8>>);

/// Object bytes on the wire.
///
/// Serialized as a base64 string rather than serde_json's default `Vec<u8>`
/// encoding (a JSON array of integers), which is ~4x larger and emits one JSON
/// token per byte — dominating bandwidth and CPU for multi-MB blobs. Converts
/// freely to/from `Vec<u8>` so call sites keep working with raw bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blob(pub Vec<u8>);

impl From<Vec<u8>> for Blob {
    fn from(v: Vec<u8>) -> Self {
        Blob(v)
    }
}

impl From<Blob> for Vec<u8> {
    fn from(b: Blob) -> Self {
        b.0
    }
}

impl Serialize for Blob {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Blob {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .map(Blob)
            .map_err(serde::de::Error::custom)
    }
}

/// Configuration for a remote repository
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    /// Name of the remote (e.g., "origin")
    pub name: String,
    /// URL of the remote repository
    pub url: String,
}

/// Request types for the HTTP API
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RemoteRequest {
    /// Get information about the repository
    GetInfo,
    
    /// List all branches
    ListBranches,
    
    /// Get the snapshot ID for a branch
    GetBranchSnapshot { branch: String },
    
    /// Check if an object exists
    HasObject { id: ObjectId },
    
    /// Get an object by ID
    GetObject { id: ObjectId },
    
    /// Get multiple objects by IDs
    GetObjects { ids: Vec<ObjectId> },
    
    /// Push a new snapshot to a branch
    PushSnapshot {
        branch: String,
        snapshot_id: ObjectId,
        force: bool,
    },
    
    /// Upload an object
    UploadObject {
        id: ObjectId,
        data: Blob,
    },

    /// Upload multiple objects
    UploadObjects {
        objects: Vec<(ObjectId, Blob)>,
    },

    /// Upload one in-order chunk of a large object. Chunks for an object start
    /// at offset 0 and must arrive contiguously; when `offset + data.len()`
    /// reaches `total_size` the server verifies the assembled bytes hash to
    /// `id` before storing. This is how objects too large for a single request
    /// body get pushed at all.
    UploadObjectChunk {
        id: ObjectId,
        offset: u64,
        total_size: u64,
        data: Blob,
    },
}

/// Response types for the HTTP API
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RemoteResponse {
    /// Repository information
    Info {
        name: String,
        branches: Vec<String>,
    },
    
    /// List of branches
    Branches {
        branches: Vec<String>,
    },
    
    /// Branch snapshot information
    BranchSnapshot {
        branch: String,
        snapshot_id: Option<ObjectId>,
    },
    
    /// Object existence check result
    ObjectExists {
        id: ObjectId,
        exists: bool,
    },
    
    /// Object data
    Object {
        id: ObjectId,
        data: Option<Blob>,
    },
    
    /// Multiple objects data
    Objects {
        objects: Vec<(ObjectId, Option<Blob>)>,
    },
    
    /// Push result
    PushResult {
        success: bool,
        message: String,
        new_snapshot_id: Option<ObjectId>,
    },
    
    /// Upload result
    UploadResult {
        success: bool,
        message: String,
    },
    
    /// Error response
    Error {
        message: String,
    },
}

/// Remote repository trait for client operations
pub trait RemoteRepository {
    type Error: std::error::Error;
    
    /// Get repository information
    fn get_info(&self) -> Result<(String, Vec<String>), Self::Error>;
    
    /// List all branches
    fn list_branches(&self) -> Result<Vec<String>, Self::Error>;
    
    /// Get the snapshot ID for a branch
    fn get_branch_snapshot(&self, branch: &str) -> Result<Option<ObjectId>, Self::Error>;
    
    /// Check if an object exists
    fn has_object(&self, id: ObjectId) -> Result<bool, Self::Error>;
    
    /// Get an object by ID
    fn get_object(&self, id: ObjectId) -> Result<Option<Vec<u8>>, Self::Error>;
    
    /// Get multiple objects by IDs
    fn get_objects(&self, ids: &[ObjectId]) -> Result<Vec<FetchedObject>, Self::Error>;
    
    /// Push a snapshot to a branch
    fn push_snapshot(&self, branch: &str, snapshot_id: ObjectId, force: bool) -> Result<ObjectId, Self::Error>;
    
    /// Upload an object
    fn upload_object(&self, id: ObjectId, data: &[u8]) -> Result<(), Self::Error>;

    /// Upload multiple objects
    fn upload_objects(&self, objects: &[(ObjectId, Vec<u8>)]) -> Result<(), Self::Error>;

    /// Upload one in-order chunk of a large object (see
    /// [`RemoteRequest::UploadObjectChunk`]).
    fn upload_object_chunk(
        &self,
        id: ObjectId,
        offset: u64,
        total_size: u64,
        data: &[u8],
    ) -> Result<(), Self::Error>;
}

// Test modules
#[cfg(test)]
mod tests;

/// Operations for syncing with remote repositories
pub mod sync {
    use super::*;
    use crate::object_store::ObjectStore;
    use crate::dot_rev::InsertJson;
    use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

    #[derive(Debug)]
    pub enum SyncError {
        RemoteError(String),
        LocalError(String),
        ObjectMissing(ObjectId),
    }

    impl std::fmt::Display for SyncError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                SyncError::RemoteError(msg) => write!(f, "Remote error: {}", msg),
                SyncError::LocalError(msg) => write!(f, "Local error: {}", msg),
                SyncError::ObjectMissing(id) => write!(f, "Object missing: {}", id),
            }
        }
    }

    impl std::error::Error for SyncError {}

    /// The type an object must have, known from the graph edge that reached
    /// it: the traversal root is a snapshot, `snapshot.previous` entries are
    /// snapshots, `snapshot.directory` is a directory, and directory file
    /// entries are opaque blobs.
    ///
    /// Blobs are NEVER parsed. The store is untyped, so guessing an object's
    /// type from its bytes let a checked-in file that happened to parse as
    /// snapshot JSON inject phantom ids into the walk (failing push with
    /// "object missing" for an id that was never real), and let a malicious
    /// server steer a pull's traversal with crafted content.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum ObjectKind {
        Snapshot,
        Directory,
        Blob,
    }

    /// Parses `bytes` as the type the graph says it must be and appends the
    /// children to `out`. An object that fails to parse as its required type is
    /// corruption — reported as an error message for the caller to wrap, never
    /// silently skipped.
    fn enqueue_children(
        bytes: &[u8],
        id: ObjectId,
        kind: ObjectKind,
        out: &mut Vec<(ObjectId, ObjectKind)>,
    ) -> Result<(), String> {
        match kind {
            ObjectKind::Blob => {}
            ObjectKind::Snapshot => {
                let snapshot: SnapShot = serde_json::from_slice(bytes)
                    .map_err(|e| format!("object {id} is not a valid snapshot: {e}"))?;
                out.push((snapshot.directory, ObjectKind::Directory));
                for parent in snapshot.previous {
                    out.push((parent, ObjectKind::Snapshot));
                }
            }
            ObjectKind::Directory => {
                let directory: Directory = serde_json::from_slice(bytes)
                    .map_err(|e| format!("object {id} is not a valid directory: {e}"))?;
                for (_, file_id) in directory.files() {
                    out.push((file_id, ObjectKind::Blob));
                }
            }
        }
        Ok(())
    }

    /// Pull a branch from a remote repository
    pub fn pull_branch<R, S>(
        remote: &R,
        store: &mut S,
        branch: &str,
    ) -> Result<Option<ObjectId>, SyncError>
    where
        R: RemoteRepository,
        S: ObjectStore + InsertJson,
        S::Error: std::fmt::Debug,
    {
        // Get the remote branch snapshot
        let remote_snapshot_id = remote
            .get_branch_snapshot(branch)
            .map_err(|e| SyncError::RemoteError(e.to_string()))?;

        let remote_snapshot_id = match remote_snapshot_id {
            Some(id) => id,
            None => return Ok(None), // Branch doesn't exist on remote
        };

        fetch_graph(remote, store, remote_snapshot_id)?;

        Ok(Some(remote_snapshot_id))
    }

    /// Downloads every object reachable from `root` that is not already
    /// present locally — each exactly once, in batches.
    ///
    /// This is a frontier-batched BFS: locally-present snapshots/directories
    /// are read from the store, missing objects are batch-fetched with
    /// [`RemoteRepository::get_objects`], hash-verified and stored immediately
    /// via `insert_with_id`, and only then parsed for children (per their
    /// expected [`ObjectKind`]). Objects already present locally are still
    /// traversed for children, so a pull interrupted mid-transfer repairs its
    /// gaps on the next run. The old implementation fetched every missing
    /// object one-at-a-time during discovery (unverified, and without storing
    /// it) and then batch-downloaded everything a second time — a fresh clone
    /// transferred every byte twice.
    fn fetch_graph<R, S>(remote: &R, store: &mut S, root: ObjectId) -> Result<(), SyncError>
    where
        R: RemoteRepository,
        S: ObjectStore + InsertJson,
        S::Error: std::fmt::Debug,
    {
        const BATCH_SIZE: usize = 100;

        // Visited is keyed by (id, kind): content addressing means the same
        // bytes can legitimately be referenced both as a blob and as a
        // snapshot/directory (a user can check in a file whose content equals
        // a metadata object), and each role contributes different children.
        let mut visited: HashSet<(ObjectId, ObjectKind)> = HashSet::new();
        let mut frontier: Vec<(ObjectId, ObjectKind)> = vec![(root, ObjectKind::Snapshot)];

        while !frontier.is_empty() {
            let mut next: Vec<(ObjectId, ObjectKind)> = Vec::new();
            let mut missing: Vec<ObjectId> = Vec::new();
            let mut missing_kinds: HashMap<ObjectId, Vec<ObjectKind>> = HashMap::new();

            for (id, kind) in frontier.drain(..) {
                if !visited.insert((id, kind)) {
                    continue;
                }
                match kind {
                    // A blob has no children; if it's already stored there is
                    // nothing to do (and no reason to read its bytes).
                    ObjectKind::Blob => {
                        let present = store.has(id).map_err(|e| {
                            SyncError::LocalError(format!("Failed to check object: {:?}", e))
                        })?;
                        if !present {
                            let kinds = missing_kinds.entry(id).or_default();
                            if kinds.is_empty() {
                                missing.push(id);
                            }
                            kinds.push(kind);
                        }
                    }
                    ObjectKind::Snapshot | ObjectKind::Directory => {
                        let local = store.read(id).map_err(|e| {
                            SyncError::LocalError(format!("Failed to read object: {:?}", e))
                        })?;
                        match local {
                            Some(bytes) => enqueue_children(&bytes, id, kind, &mut next)
                                .map_err(SyncError::LocalError)?,
                            None => {
                                let kinds = missing_kinds.entry(id).or_default();
                                if kinds.is_empty() {
                                    missing.push(id);
                                }
                                kinds.push(kind);
                            }
                        }
                    }
                }
            }

            for chunk in missing.chunks(BATCH_SIZE) {
                // The server caps a response by total bytes and may return
                // fewer objects than requested, so loop until every id in this
                // chunk is accounted for. An empty response for a non-empty
                // request means no progress is possible — fail rather than
                // spin forever.
                let mut pending: Vec<ObjectId> = chunk.to_vec();
                while !pending.is_empty() {
                    let objects = remote
                        .get_objects(&pending)
                        .map_err(|e| SyncError::RemoteError(e.to_string()))?;
                    if objects.is_empty() {
                        return Err(SyncError::RemoteError(format!(
                            "server returned no objects for a request of {} id(s)",
                            pending.len()
                        )));
                    }

                    let mut returned = BTreeSet::new();
                    for (id, data) in objects {
                        returned.insert(id);
                        // Every requested id is required by the graph; a remote
                        // that lacks one cannot satisfy this pull. Treating
                        // (id, None) as delivered used to let a pull "succeed"
                        // with holes in the object graph.
                        let data = data.ok_or(SyncError::ObjectMissing(id))?;
                        // insert_with_id verifies the bytes hash to the claimed
                        // id BEFORE storing, so a lying server can neither
                        // poison the store nor steer the traversal.
                        store.insert_with_id(id, &data).map_err(|e| {
                            SyncError::LocalError(format!("Failed to store object: {:?}", e))
                        })?;
                        for kind in missing_kinds.get(&id).map(|v| v.as_slice()).unwrap_or(&[]) {
                            enqueue_children(&data, id, *kind, &mut next)
                                .map_err(SyncError::RemoteError)?;
                        }
                    }
                    pending.retain(|id| !returned.contains(id));
                }
            }

            frontier = next;
        }

        Ok(())
    }

    /// Raw-byte budget for a single `UploadObjects` request. Base64 inflates
    /// payloads 4/3 and the server buffers bodies only up to its body cap
    /// (64 MB); 24 MB of raw bytes leaves comfortable headroom for the JSON
    /// envelope. Counting objects alone (the old batching) made any single
    /// object over ~48 MB impossible to push, permanently.
    pub(crate) const UPLOAD_BATCH_BYTES: usize = 24 * 1024 * 1024;
    /// Object-count cap per `UploadObjects` request.
    pub(crate) const UPLOAD_BATCH_COUNT: usize = 100;
    /// Chunk size for objects larger than the batch budget.
    pub(crate) const UPLOAD_CHUNK_BYTES: usize = 16 * 1024 * 1024;

    /// Size limits for [`upload_in_batches`], parameterized so tests can
    /// exercise the batching and chunking logic with tiny values.
    pub(crate) struct UploadLimits {
        pub batch_bytes: usize,
        pub batch_count: usize,
        pub chunk_bytes: usize,
    }

    impl Default for UploadLimits {
        fn default() -> Self {
            UploadLimits {
                batch_bytes: UPLOAD_BATCH_BYTES,
                batch_count: UPLOAD_BATCH_COUNT,
                chunk_bytes: UPLOAD_CHUNK_BYTES,
            }
        }
    }

    /// Push a branch to a remote repository
    pub fn push_branch<R, S>(
        remote: &R,
        store: &mut S,
        branch: &str,
        local_snapshot_id: ObjectId,
        force: bool,
    ) -> Result<(), SyncError>
    where
        R: RemoteRepository,
        S: ObjectStore + InsertJson,
        S::Error: std::fmt::Debug,
    {
        // Collect all objects needed for this snapshot
        let needed_objects = collect_local_objects(store, local_snapshot_id)?;

        // Check which objects the remote already has
        let mut objects_to_upload = Vec::new();
        for &obj_id in &needed_objects {
            if !remote.has_object(obj_id).map_err(|e| SyncError::RemoteError(e.to_string()))? {
                let data = store.read(obj_id)
                    .map_err(|e| SyncError::LocalError(format!("Failed to read object: {:?}", e)))?
                    .ok_or(SyncError::ObjectMissing(obj_id))?;
                objects_to_upload.push((obj_id, data));
            }
        }

        // Upload objects in byte-bounded batches
        upload_in_batches(remote, &objects_to_upload, &UploadLimits::default())?;

        // Update the branch pointer
        remote.push_snapshot(branch, local_snapshot_id, force)
            .map_err(|e| SyncError::RemoteError(e.to_string()))?;

        Ok(())
    }

    /// Collect all objects needed for a local snapshot (typed breadth-first
    /// traversal — see [`ObjectKind`]). A missing or unparseable metadata
    /// object is a hard error: pushing a branch whose graph is incomplete
    /// would strand every puller.
    fn collect_local_objects<S>(
        store: &mut S,
        snapshot_id: ObjectId,
    ) -> Result<Vec<ObjectId>, SyncError>
    where
        S: ObjectStore + InsertJson,
        S::Error: std::fmt::Debug,
    {
        let mut needed: HashSet<ObjectId> = HashSet::new();
        let mut expanded: HashSet<(ObjectId, ObjectKind)> = HashSet::new();
        let mut queue: VecDeque<(ObjectId, ObjectKind)> = VecDeque::new();
        queue.push_back((snapshot_id, ObjectKind::Snapshot));

        while let Some((id, kind)) = queue.pop_front() {
            if !expanded.insert((id, kind)) {
                continue;
            }
            needed.insert(id);
            if kind == ObjectKind::Blob {
                continue;
            }
            let bytes = store
                .read(id)
                .map_err(|e| SyncError::LocalError(format!("Failed to read object: {:?}", e)))?
                .ok_or(SyncError::ObjectMissing(id))?;
            let mut children = Vec::new();
            enqueue_children(&bytes, id, kind, &mut children).map_err(SyncError::LocalError)?;
            queue.extend(children);
        }

        Ok(needed.into_iter().collect())
    }

    /// Uploads objects in batches bounded by both byte budget and count; an
    /// object larger than the byte budget goes through the chunked path.
    pub(crate) fn upload_in_batches<R>(
        remote: &R,
        objects: &[(ObjectId, Vec<u8>)],
        limits: &UploadLimits,
    ) -> Result<(), SyncError>
    where
        R: RemoteRepository,
    {
        let mut batch: Vec<(ObjectId, Vec<u8>)> = Vec::new();
        let mut batch_bytes = 0usize;

        for (id, data) in objects {
            if data.len() > limits.batch_bytes {
                upload_chunked(remote, *id, data, limits.chunk_bytes)?;
                continue;
            }
            if !batch.is_empty()
                && (batch_bytes + data.len() > limits.batch_bytes
                    || batch.len() >= limits.batch_count)
            {
                remote
                    .upload_objects(&batch)
                    .map_err(|e| SyncError::RemoteError(e.to_string()))?;
                batch.clear();
                batch_bytes = 0;
            }
            batch_bytes += data.len();
            batch.push((*id, data.clone()));
        }

        if !batch.is_empty() {
            remote
                .upload_objects(&batch)
                .map_err(|e| SyncError::RemoteError(e.to_string()))?;
        }
        Ok(())
    }

    /// Streams one oversized object to the remote in in-order chunks.
    fn upload_chunked<R>(
        remote: &R,
        id: ObjectId,
        data: &[u8],
        chunk_bytes: usize,
    ) -> Result<(), SyncError>
    where
        R: RemoteRepository,
    {
        let total = data.len() as u64;
        let mut offset = 0usize;
        while offset < data.len() {
            let end = (offset + chunk_bytes).min(data.len());
            remote
                .upload_object_chunk(id, offset as u64, total, &data[offset..end])
                .map_err(|e| SyncError::RemoteError(e.to_string()))?;
            offset = end;
        }
        Ok(())
    }
}