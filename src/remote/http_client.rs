use std::time::Duration;
use reqwest::blocking::Client;
use crate::object_id::ObjectId;
use super::{RemoteRequest, RemoteResponse, RemoteRepository};

/// HTTP client for communicating with remote repositories
pub struct HttpRemoteClient {
    client: Client,
    base_url: String,
}

#[derive(Debug)]
pub enum HttpClientError {
    Network(reqwest::Error),
    Serialization(serde_json::Error),
    ServerError(String),
    InvalidResponse(String),
}

impl std::fmt::Display for HttpClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpClientError::Network(e) => write!(f, "Network error: {}", e),
            HttpClientError::Serialization(e) => write!(f, "Serialization error: {}", e),
            HttpClientError::ServerError(msg) => write!(f, "Server error: {}", msg),
            HttpClientError::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
        }
    }
}

impl std::error::Error for HttpClientError {}

impl HttpRemoteClient {
    /// Create a new HTTP client for a remote repository
    pub fn new(base_url: String) -> Result<Self, HttpClientError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(HttpClientError::Network)?;
        
        Ok(HttpRemoteClient { client, base_url })
    }
    
    /// Send a request to the remote repository
    fn send_request(&self, request: &RemoteRequest) -> Result<RemoteResponse, HttpClientError> {
        let url = format!("{}/api", self.base_url);
        
        let response = self.client
            .post(&url)
            .json(request)
            .send()
            .map_err(HttpClientError::Network)?;
        
        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().unwrap_or_else(|_| "Unknown error".to_string());
            return Err(HttpClientError::ServerError(
                format!("HTTP {}: {}", status, error_text)
            ));
        }
        
        let response_body = response.bytes()
            .map_err(HttpClientError::Network)?;
        
        serde_json::from_slice(&response_body)
            .map_err(HttpClientError::Serialization)
    }
}

impl RemoteRepository for HttpRemoteClient {
    type Error = HttpClientError;
    
    fn get_info(&self) -> Result<(String, Vec<String>), Self::Error> {
        match self.send_request(&RemoteRequest::GetInfo)? {
            RemoteResponse::Info { name, branches } => Ok((name, branches)),
            RemoteResponse::Error { message } => Err(HttpClientError::ServerError(message)),
            _ => Err(HttpClientError::InvalidResponse("Expected Info response".to_string())),
        }
    }
    
    fn list_branches(&self) -> Result<Vec<String>, Self::Error> {
        match self.send_request(&RemoteRequest::ListBranches)? {
            RemoteResponse::Branches { branches } => Ok(branches),
            RemoteResponse::Error { message } => Err(HttpClientError::ServerError(message)),
            _ => Err(HttpClientError::InvalidResponse("Expected Branches response".to_string())),
        }
    }
    
    fn get_branch_snapshot(&self, branch: &str) -> Result<Option<ObjectId>, Self::Error> {
        let request = RemoteRequest::GetBranchSnapshot {
            branch: branch.to_string(),
        };
        
        match self.send_request(&request)? {
            RemoteResponse::BranchSnapshot { branch: _, snapshot_id } => Ok(snapshot_id),
            RemoteResponse::Error { message } => Err(HttpClientError::ServerError(message)),
            _ => Err(HttpClientError::InvalidResponse("Expected BranchSnapshot response".to_string())),
        }
    }
    
    fn has_object(&self, id: ObjectId) -> Result<bool, Self::Error> {
        let request = RemoteRequest::HasObject { id };
        
        match self.send_request(&request)? {
            RemoteResponse::ObjectExists { id: _, exists } => Ok(exists),
            RemoteResponse::Error { message } => Err(HttpClientError::ServerError(message)),
            _ => Err(HttpClientError::InvalidResponse("Expected ObjectExists response".to_string())),
        }
    }
    
    fn get_object(&self, id: ObjectId) -> Result<Option<Vec<u8>>, Self::Error> {
        let request = RemoteRequest::GetObject { id };
        
        match self.send_request(&request)? {
            RemoteResponse::Object { id: _, data } => Ok(data),
            RemoteResponse::Error { message } => Err(HttpClientError::ServerError(message)),
            _ => Err(HttpClientError::InvalidResponse("Expected Object response".to_string())),
        }
    }
    
    fn get_objects(&self, ids: &[ObjectId]) -> Result<Vec<(ObjectId, Option<Vec<u8>>)>, Self::Error> {
        let request = RemoteRequest::GetObjects {
            ids: ids.to_vec(),
        };
        
        match self.send_request(&request)? {
            RemoteResponse::Objects { objects } => Ok(objects),
            RemoteResponse::Error { message } => Err(HttpClientError::ServerError(message)),
            _ => Err(HttpClientError::InvalidResponse("Expected Objects response".to_string())),
        }
    }
    
    fn push_snapshot(&self, branch: &str, snapshot_id: ObjectId, force: bool) -> Result<ObjectId, Self::Error> {
        let request = RemoteRequest::PushSnapshot {
            branch: branch.to_string(),
            snapshot_id,
            force,
        };
        
        match self.send_request(&request)? {
            RemoteResponse::PushResult { success, message, new_snapshot_id } => {
                if success {
                    Ok(new_snapshot_id.unwrap_or(snapshot_id))
                } else {
                    Err(HttpClientError::ServerError(message))
                }
            },
            RemoteResponse::Error { message } => Err(HttpClientError::ServerError(message)),
            _ => Err(HttpClientError::InvalidResponse("Expected PushResult response".to_string())),
        }
    }
    
    fn upload_object(&self, id: ObjectId, data: &[u8]) -> Result<(), Self::Error> {
        let request = RemoteRequest::UploadObject {
            id,
            data: data.to_vec(),
        };
        
        match self.send_request(&request)? {
            RemoteResponse::UploadResult { success, message } => {
                if success {
                    Ok(())
                } else {
                    Err(HttpClientError::ServerError(message))
                }
            },
            RemoteResponse::Error { message } => Err(HttpClientError::ServerError(message)),
            _ => Err(HttpClientError::InvalidResponse("Expected UploadResult response".to_string())),
        }
    }
    
    fn upload_objects(&self, objects: &[(ObjectId, Vec<u8>)]) -> Result<(), Self::Error> {
        let request = RemoteRequest::UploadObjects {
            objects: objects.to_vec(),
        };
        
        match self.send_request(&request)? {
            RemoteResponse::UploadResult { success, message } => {
                if success {
                    Ok(())
                } else {
                    Err(HttpClientError::ServerError(message))
                }
            },
            RemoteResponse::Error { message } => Err(HttpClientError::ServerError(message)),
            _ => Err(HttpClientError::InvalidResponse("Expected UploadResult response".to_string())),
        }
    }
}