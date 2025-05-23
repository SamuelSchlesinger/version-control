use std::path::PathBuf;
use std::sync::Arc;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::post,
    Router,
};
use tokio::sync::RwLock;
use crate::{
    dot_rev::{DotRev, InsertJson},
    object_store::ObjectStore,
    object_id::ObjectId,
};
use super::{RemoteRequest, RemoteResponse};

/// HTTP server for hosting a remote repository
pub struct HttpRemoteServer {
    dot_rev: Arc<RwLock<DotRev>>,
}

impl HttpRemoteServer {
    /// Create a new HTTP server for a repository
    pub fn new(repo_path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let dot_rev = DotRev::existing(repo_path)?;
        Ok(HttpRemoteServer {
            dot_rev: Arc::new(RwLock::new(dot_rev)),
        })
    }
    
    /// Build the router for the HTTP server
    pub fn router(self) -> Router {
        Router::new()
            .route("/api", post(handle_request))
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
    Json(request): Json<RemoteRequest>,
) -> impl IntoResponse {
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
            let dot_rev = server.dot_rev.read().await;
            let store = dot_rev.store()?;
            let mut objects = Vec::new();
            
            for id in ids {
                let data = store.read(id)?;
                objects.push((id, data));
            }
            
            Ok(RemoteResponse::Objects { objects })
        },
        
        RemoteRequest::PushSnapshot { branch, snapshot_id, force } => {
            let dot_rev = server.dot_rev.write().await;
            
            // Check if branch exists and if we need to force push
            if dot_rev.branch_exists(&branch)? && !force {
                let current_id = dot_rev.branch_snapshot_id(&branch)?;
                
                // Check if the new snapshot is a descendant of the current one
                let mut store = dot_rev.store()?;
                if !is_ancestor(&mut store, current_id, snapshot_id)? {
                    return Ok(RemoteResponse::PushResult {
                        success: false,
                        message: "Push rejected: not a fast-forward. Use --force to override.".to_string(),
                        new_snapshot_id: None,
                    });
                }
            }
            
            // Update the branch
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