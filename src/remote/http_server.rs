use std::path::PathBuf;
use std::sync::Arc;
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::post,
    Router,
};
use tokio::sync::RwLock;
use crate::{
    dot_rev::{DotRev, InsertJson},
    object_store::ObjectStore,
    object_id::ObjectId,
    snapshot::SnapShot,
    directory::Directory,
};
use super::{RemoteRequest, RemoteResponse};

/// Largest request body the server will buffer. Bounds memory against a client
/// that streams an enormous payload. Object transfers are chunked below this.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Largest number of object ids a single GetObjects request may ask for.
/// Without a cap, a client could request one object millions of times and
/// amplify a small request into an unbounded response (memory-exhaustion DoS).
const MAX_OBJECTS_PER_REQUEST: usize = 10_000;

/// HTTP server for hosting a remote repository
pub struct HttpRemoteServer {
    dot_rev: Arc<RwLock<DotRev>>,
    /// If set, every request must present `Authorization: Bearer <token>`.
    token: Option<String>,
}

impl HttpRemoteServer {
    /// Create a new HTTP server for a repository
    pub fn new(repo_path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_token(repo_path, None)
    }

    /// Create a server that requires the given bearer token, if any.
    pub fn with_token(
        repo_path: PathBuf,
        token: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let dot_rev = DotRev::existing(repo_path)?;
        Ok(HttpRemoteServer {
            dot_rev: Arc::new(RwLock::new(dot_rev)),
            token,
        })
    }
    
    /// Build the router for the HTTP server
    pub fn router(self) -> Router {
        Router::new()
            .route("/api", post(handle_request))
            .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
            .with_state(Arc::new(self))
    }
    
    /// Run the server on the specified address
    pub async fn run(self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let app = self.router();
        let listener = tokio::net::TcpListener::bind(addr).await?;
        println!("Remote repository server listening on {}", addr);
        axum::serve(listener, app).await?;
        Ok(())
    }
}

/// Handle incoming requests
async fn handle_request(
    State(server): State<Arc<HttpRemoteServer>>,
    headers: HeaderMap,
    Json(request): Json<RemoteRequest>,
) -> impl IntoResponse {
    // Enforce bearer-token auth if the server was started with a token.
    if let Some(expected) = &server.token {
        if !bearer_token_matches(&headers, expected) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(RemoteResponse::Error {
                    message: "missing or invalid authentication token".to_string(),
                }),
            );
        }
    }

    match process_request(&server, request).await {
        Ok(response) => (StatusCode::OK, Json(response)),
        Err(e) => {
            let error_response = RemoteResponse::Error {
                message: e.to_string(),
            };
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

/// Checks the `Authorization: Bearer <token>` header against the expected token
/// in constant time (to avoid leaking the token via timing).
fn bearer_token_matches(headers: &HeaderMap, expected: &str) -> bool {
    let presented = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match presented {
        Some(token) => constant_time_eq(token.as_bytes(), expected.as_bytes()),
        None => false,
    }
}

/// Length-independent, data-independent byte comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Process a remote request
async fn process_request(
    server: &HttpRemoteServer,
    request: RemoteRequest,
) -> Result<RemoteResponse, Box<dyn std::error::Error + Send + Sync>> {
    match request {
        RemoteRequest::GetInfo => {
            let dot_rev = server.dot_rev.read().await;
            let branches = dot_rev.list_branches()?;
            let name = dot_rev.root().file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("repository")
                .to_string();
            
            Ok(RemoteResponse::Info { name, branches })
        },
        
        RemoteRequest::ListBranches => {
            let dot_rev = server.dot_rev.read().await;
            let branches = dot_rev.list_branches()?;
            Ok(RemoteResponse::Branches { branches })
        },
        
        RemoteRequest::GetBranchSnapshot { branch } => {
            let dot_rev = server.dot_rev.read().await;
            let snapshot_id = if dot_rev.branch_exists(&branch)? {
                Some(dot_rev.branch_snapshot_id(&branch)?)
            } else {
                None
            };
            
            Ok(RemoteResponse::BranchSnapshot { branch, snapshot_id })
        },
        
        RemoteRequest::HasObject { id } => {
            let dot_rev = server.dot_rev.read().await;
            let store = dot_rev.store()?;
            let exists = store.has(id)?;
            Ok(RemoteResponse::ObjectExists { id, exists })
        },
        
        RemoteRequest::GetObject { id } => {
            let dot_rev = server.dot_rev.read().await;
            let store = dot_rev.store()?;
            let data = store.read(id)?;
            Ok(RemoteResponse::Object { id, data })
        },
        
        RemoteRequest::GetObjects { ids } => {
            if ids.len() > MAX_OBJECTS_PER_REQUEST {
                return Ok(RemoteResponse::Error {
                    message: format!(
                        "too many objects requested ({}); the limit is {}",
                        ids.len(),
                        MAX_OBJECTS_PER_REQUEST
                    ),
                });
            }
            let dot_rev = server.dot_rev.read().await;
            let store = dot_rev.store()?;
            let mut objects = Vec::new();

            // De-duplicate so a request listing the same id thousands of times
            // cannot amplify into a huge response.
            let mut seen = std::collections::BTreeSet::new();
            for id in ids {
                if !seen.insert(id) {
                    continue;
                }
                let data = store.read(id)?;
                objects.push((id, data));
            }

            Ok(RemoteResponse::Objects { objects })
        },
        
        RemoteRequest::PushSnapshot { branch, snapshot_id, force } => {
            let dot_rev = server.dot_rev.write().await;
            let mut store = dot_rev.store()?;

            // Refuse to move a branch onto a snapshot whose object graph is not
            // fully present. Without this, a client could point a branch at a
            // dangling id and brick the branch for the host and every puller.
            if let Some(missing) = first_missing_object(&mut store, snapshot_id)? {
                return Ok(RemoteResponse::PushResult {
                    success: false,
                    message: format!(
                        "Push rejected: object {missing} is missing. Upload all objects before moving the branch."
                    ),
                    new_snapshot_id: None,
                });
            }

            // Check if branch exists and if we need to force push
            if dot_rev.branch_exists(&branch)? && !force {
                let current_id = dot_rev.branch_snapshot_id(&branch)?;

                // Check if the new snapshot is a descendant of the current one
                if !is_ancestor(&mut store, current_id, snapshot_id)? {
                    return Ok(RemoteResponse::PushResult {
                        success: false,
                        message: "Push rejected: not a fast-forward. Use --force to override.".to_string(),
                        new_snapshot_id: None,
                    });
                }
            }

            // Update the branch. Branch-name validation happens inside
            // set_branch_snapshot_id, so a traversal name is rejected here.
            dot_rev.set_branch_snapshot_id(&branch, snapshot_id)?;

            Ok(RemoteResponse::PushResult {
                success: true,
                message: format!("Branch '{}' updated to {}", branch, snapshot_id),
                new_snapshot_id: Some(snapshot_id),
            })
        },
        
        RemoteRequest::UploadObject { id, data } => {
            let dot_rev = server.dot_rev.read().await;
            let mut store = dot_rev.store()?;
            
            // Verify the object ID matches the data
            store.insert_with_id(id, &data)?;
            
            Ok(RemoteResponse::UploadResult {
                success: true,
                message: format!("Object {} uploaded successfully", id),
            })
        },
        
        RemoteRequest::UploadObjects { objects } => {
            let dot_rev = server.dot_rev.read().await;
            let mut store = dot_rev.store()?;
            let count = objects.len();
            
            for (id, data) in objects {
                store.insert_with_id(id, &data)?;
            }
            
            Ok(RemoteResponse::UploadResult {
                success: true,
                message: format!("{} objects uploaded successfully", count),
            })
        },
    }
}

/// Walks the entire object graph reachable from `root_snapshot` (ancestor
/// snapshots, their directory objects, and every file blob) and returns the
/// first object that is not present in the store, or `None` if the graph is
/// complete. Used to reject a push that would leave a branch pointing at a
/// dangling snapshot.
fn first_missing_object<S>(
    store: &mut S,
    root_snapshot: ObjectId,
) -> Result<Option<ObjectId>, Box<dyn std::error::Error + Send + Sync>>
where
    S: ObjectStore + InsertJson,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    use std::collections::{HashSet, VecDeque};

    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(root_snapshot);

    while let Some(sid) = queue.pop_front() {
        if !visited.insert(sid) {
            continue;
        }
        let snap_bytes = match store.read(sid)? {
            Some(b) => b,
            None => return Ok(Some(sid)),
        };
        let snapshot: SnapShot = serde_json::from_slice(&snap_bytes)?;

        let dir_bytes = match store.read(snapshot.directory)? {
            Some(b) => b,
            None => return Ok(Some(snapshot.directory)),
        };
        let directory: Directory = serde_json::from_slice(&dir_bytes)?;

        for (_, blob_id) in directory.files() {
            if !store.has(blob_id)? {
                return Ok(Some(blob_id));
            }
        }

        for parent in snapshot.previous {
            queue.push_back(parent);
        }
    }

    Ok(None)
}

/// Check if one snapshot is an ancestor of another
fn is_ancestor<S>(
    store: &mut S,
    ancestor_id: ObjectId,
    descendant_id: ObjectId,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>
where
    S: ObjectStore + InsertJson,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    use crate::snapshot::SnapShot;
    use std::collections::{HashSet, VecDeque};
    
    if ancestor_id == descendant_id {
        return Ok(true);
    }
    
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(descendant_id);
    
    while let Some(current_id) = queue.pop_front() {
        if current_id == ancestor_id {
            return Ok(true);
        }
        
        if visited.contains(&current_id) {
            continue;
        }
        visited.insert(current_id);
        
        let snapshot: SnapShot = store.read_json(current_id)?;
        for parent in snapshot.previous {
            queue.push_back(parent);
        }
    }
    
    Ok(false)
}