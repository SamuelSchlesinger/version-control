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
}

// Test modules
#[cfg(test)]
mod tests;

/// Operations for syncing with remote repositories
pub mod sync {
    use super::*;
    use crate::object_store::ObjectStore;
    use crate::dot_rev::InsertJson;
    use std::collections::{HashSet, VecDeque};
    
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
        
        // Collect all objects needed for this snapshot
        let needed_objects = collect_snapshot_objects(remote, store, remote_snapshot_id)?;
        
        // Download objects in batches
        download_objects(remote, store, &needed_objects)?;
        
        Ok(Some(remote_snapshot_id))
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
        
        // Upload objects in batches
        upload_objects(remote, &objects_to_upload)?;
        
        // Update the branch pointer
        remote.push_snapshot(branch, local_snapshot_id, force)
            .map_err(|e| SyncError::RemoteError(e.to_string()))?;
        
        Ok(())
    }
    
    /// Collect all objects needed for a snapshot (breadth-first traversal)
    fn collect_snapshot_objects<R, S>(
        remote: &R,
        store: &S,
        snapshot_id: ObjectId,
    ) -> Result<Vec<ObjectId>, SyncError>
    where
        R: RemoteRepository,
        S: ObjectStore,
        S::Error: std::fmt::Debug,
    {
        let mut visited = HashSet::new();
        let mut to_download = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(snapshot_id);

        while let Some(obj_id) = queue.pop_front() {
            if !visited.insert(obj_id) {
                continue;
            }

            // Get the object's bytes so we can inspect its children. Prefer the
            // local copy; if it isn't present, fetch it from the remote and
            // record that it must be downloaded. Crucially, we inspect children
            // even for objects we already have locally: a prior interrupted pull
            // can leave a snapshot present but a child blob missing, and the old
            // code's "already local, skip" shortcut meant a re-pull computed an
            // empty download set and could never repair the gap.
            let data = match store
                .read(obj_id)
                .map_err(|e| SyncError::LocalError(format!("Failed to check object: {:?}", e)))?
            {
                Some(local) => local,
                None => {
                    to_download.insert(obj_id);
                    remote
                        .get_object(obj_id)
                        .map_err(|e| SyncError::RemoteError(e.to_string()))?
                        .ok_or(SyncError::ObjectMissing(obj_id))?
                }
            };

            // Try to parse as snapshot
            if let Ok(snapshot) = serde_json::from_slice::<SnapShot>(&data) {
                queue.push_back(snapshot.directory);
                for parent in snapshot.previous {
                    queue.push_back(parent);
                }
            }

            // Try to parse as directory
            if let Ok(directory) = serde_json::from_slice::<Directory>(&data) {
                for (_, file_id) in directory.files() {
                    queue.push_back(file_id);
                }
            }
        }

        Ok(to_download.into_iter().collect())
    }
    
    /// Collect all objects needed for a local snapshot
    fn collect_local_objects<S>(
        store: &mut S,
        snapshot_id: ObjectId,
    ) -> Result<Vec<ObjectId>, SyncError>
    where
        S: ObjectStore + InsertJson,
        S::Error: std::fmt::Debug,
    {
        let mut needed = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(snapshot_id);
        needed.insert(snapshot_id);
        
        while let Some(obj_id) = queue.pop_front() {
            // Try to read as snapshot
            if let Ok(snapshot) = store.read_json::<SnapShot>(obj_id) {
                // Add directory object
                if needed.insert(snapshot.directory) {
                    queue.push_back(snapshot.directory);
                }
                
                // Add parent snapshots
                for parent in snapshot.previous {
                    if needed.insert(parent) {
                        queue.push_back(parent);
                    }
                }
            }
            
            // Try to read as directory
            if let Ok(directory) = store.read_json::<Directory>(obj_id) {
                // Add all file content objects
                for (_, file_id) in directory.files() {
                    needed.insert(file_id);
                }
            }
        }
        
        Ok(needed.into_iter().collect())
    }
    
    /// Download objects from remote in batches
    fn download_objects<R, S>(
        remote: &R,
        store: &mut S,
        object_ids: &[ObjectId],
    ) -> Result<(), SyncError>
    where
        R: RemoteRepository,
        S: ObjectStore,
        S::Error: std::fmt::Debug,
    {
        const BATCH_SIZE: usize = 100;
        
        for chunk in object_ids.chunks(BATCH_SIZE) {
            let objects = remote.get_objects(chunk)
                .map_err(|e| SyncError::RemoteError(e.to_string()))?;
            
            for (id, data) in objects {
                if let Some(data) = data {
                    store.insert_with_id(id, &data)
                        .map_err(|e| SyncError::LocalError(format!("Failed to store object: {:?}", e)))?;
                }
            }
        }
        
        Ok(())
    }
    
    /// Upload objects to remote in batches
    fn upload_objects<R>(
        remote: &R,
        objects: &[(ObjectId, Vec<u8>)],
    ) -> Result<(), SyncError>
    where
        R: RemoteRepository,
    {
        const BATCH_SIZE: usize = 100;
        
        for chunk in objects.chunks(BATCH_SIZE) {
            remote.upload_objects(chunk)
                .map_err(|e| SyncError::RemoteError(e.to_string()))?;
        }
        
        Ok(())
    }
}