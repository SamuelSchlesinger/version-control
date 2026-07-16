#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::*;
    use crate::dot_rev::DotRev;
    use tempfile::tempdir;
    
    #[test]
    fn test_remote_config_management() {
        let temp_dir = tempdir().unwrap();
        let rev_path = temp_dir.path().join(".rev");
        
        // Initialize repository
        let dot_rev = DotRev::init(rev_path).unwrap();
        
        // Initially no remotes
        let remotes = dot_rev.remotes().unwrap();
        assert_eq!(remotes.len(), 0);
        
        // Add a remote
        let remote1 = RemoteConfig {
            name: "origin".to_string(),
            url: "http://example.com:8080".to_string(),
        };
        dot_rev.add_remote(remote1.clone()).unwrap();
        
        // Check it was added
        let remotes = dot_rev.remotes().unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].url, "http://example.com:8080");
        
        // Get remote by name
        let origin = dot_rev.get_remote("origin").unwrap();
        assert!(origin.is_some());
        assert_eq!(origin.unwrap().url, "http://example.com:8080");
        
        // Add another remote
        let remote2 = RemoteConfig {
            name: "backup".to_string(),
            url: "http://backup.example.com:8080".to_string(),
        };
        dot_rev.add_remote(remote2).unwrap();
        
        // Check both exist
        let remotes = dot_rev.remotes().unwrap();
        assert_eq!(remotes.len(), 2);
        
        // Remove a remote
        dot_rev.remove_remote("origin").unwrap();
        
        // Check it was removed
        let remotes = dot_rev.remotes().unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "backup");
        
        // Try to add duplicate
        let duplicate = RemoteConfig {
            name: "backup".to_string(),
            url: "http://another.example.com:8080".to_string(),
        };
        assert!(dot_rev.add_remote(duplicate).is_err());
    }
    
    #[test]
    fn test_remote_request_response_serialization() {
        use serde_json;
        
        // Test GetInfo request
        let request = RemoteRequest::GetInfo;
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: RemoteRequest = serde_json::from_str(&json).unwrap();
        match deserialized {
            RemoteRequest::GetInfo => {},
            _ => panic!("Wrong request type"),
        }
        
        // Test Info response
        let response = RemoteResponse::Info {
            name: "test-repo".to_string(),
            branches: vec!["main".to_string(), "dev".to_string()],
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: RemoteResponse = serde_json::from_str(&json).unwrap();
        match deserialized {
            RemoteResponse::Info { name, branches } => {
                assert_eq!(name, "test-repo");
                assert_eq!(branches.len(), 2);
            },
            _ => panic!("Wrong response type"),
        }
    }
}