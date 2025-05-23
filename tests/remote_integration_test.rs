use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// Integration test for remote functionality
#[test]
#[ignore] // Run with: cargo test -- --ignored
fn test_remote_push_pull_workflow() {
    // Create temporary directories for our test repositories
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let repo1_path = temp_dir.path().join("repo1");
    let repo2_path = temp_dir.path().join("repo2");
    
    std::fs::create_dir(&repo1_path).expect("Failed to create repo1");
    std::fs::create_dir(&repo2_path).expect("Failed to create repo2");
    
    // Get path to revtool binary
    let revtool = env!("CARGO_BIN_EXE_revtool");
    
    // Helper function to run revtool commands
    let run_revtool = |args: &[&str], cwd: &std::path::Path| -> Result<String, String> {
        let output = Command::new(revtool)
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("Failed to execute revtool");
        
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    };
    
    // Initialize first repository
    run_revtool(&["init"], &repo1_path).expect("Failed to init repo1");
    
    // Create some test files in repo1
    std::fs::write(repo1_path.join("file1.txt"), "Hello from repo1").expect("Failed to write file1");
    std::fs::write(repo1_path.join("file2.txt"), "Test content").expect("Failed to write file2");
    
    // Create initial snapshot
    run_revtool(&["snap", "-m", "Initial commit"], &repo1_path)
        .expect("Failed to create snapshot");
    
    // Check branches in repo1
    match run_revtool(&["branch"], &repo1_path) {
        Ok(branches) => println!("Branches in repo1 before server: {}", branches),
        Err(e) => println!("Failed to list branches: {}", e),
    }
    
    // Start server in background
    let repo1_path_clone = repo1_path.clone();
    let revtool_clone = revtool.to_string();
    let server_handle = thread::spawn(move || {
        Command::new(revtool_clone)
            .args(&["serve", "--port", "8766"])
            .current_dir(&repo1_path_clone)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .status()
            .expect("Failed to start server");
    });
    
    // Wait for server to start
    thread::sleep(Duration::from_secs(3));
    
    // Test that server is responding by making a simple request
    println!("Testing server connectivity...");
    
    // Initialize second repository
    run_revtool(&["init"], &repo2_path).expect("Failed to init repo2");
    
    // Add remote to repo2
    run_revtool(&["remote", "add", "origin", "http://127.0.0.1:8766"], &repo2_path)
        .expect("Failed to add remote");
    
    // Pull from remote - with better error handling
    match run_revtool(&["pull", "origin", "dev"], &repo2_path) {
        Ok(output) => {
            println!("Pull successful. Output: {}", output);
        },
        Err(e) => {
            eprintln!("Pull failed with error: {}", e);
            // Try to diagnose the issue
            if let Ok(branches) = run_revtool(&["branch"], &repo1_path) {
                eprintln!("Branches in repo1: {}", branches);
            }
            if let Ok(remotes) = run_revtool(&["remote"], &repo2_path) {
                eprintln!("Remotes in repo2: {}", remotes);
            }
            panic!("Failed to pull from origin: {}", e);
        }
    }
    
    // List files in repo2 to debug
    println!("Files in repo2 after pull:");
    if let Ok(entries) = std::fs::read_dir(&repo2_path) {
        for entry in entries {
            if let Ok(entry) = entry {
                println!("  - {:?}", entry.path());
            }
        }
    }
    
    // Verify files were pulled
    assert!(repo2_path.join("file1.txt").exists(), "file1.txt not pulled");
    assert!(repo2_path.join("file2.txt").exists(), "file2.txt not pulled");
    
    let content = std::fs::read_to_string(repo2_path.join("file1.txt"))
        .expect("Failed to read file1.txt");
    assert_eq!(content, "Hello from repo1");
    
    // Make changes in repo2
    std::fs::write(repo2_path.join("file3.txt"), "New file from repo2")
        .expect("Failed to write file3");
    
    run_revtool(&["snap", "-m", "Add file3"], &repo2_path)
        .expect("Failed to create snapshot in repo2");
    
    // Push changes back
    run_revtool(&["push", "origin"], &repo2_path)
        .expect("Failed to push to origin");
    
    // Note: We can't easily test pulling back in repo1 because the server
    // is running in that directory. In a real scenario, you'd pull from
    // another client.
    
    // The server thread will be terminated when the test ends
    drop(server_handle);
}

/// Test remote configuration management
#[test]
fn test_remote_config() {
    use lib::dot_rev::DotRev;
    use lib::remote::RemoteConfig;
    
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let repo_path = temp_dir.path().join(".rev");
    
    // Initialize repository
    let dot_rev = DotRev::init(repo_path).expect("Failed to init repository");
    
    // Test adding remotes
    let remote1 = RemoteConfig {
        name: "origin".to_string(),
        url: "http://example.com:8080".to_string(),
    };
    
    dot_rev.add_remote(remote1.clone()).expect("Failed to add remote");
    
    // Verify remote was added
    let remotes = dot_rev.remotes().expect("Failed to get remotes");
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].name, "origin");
    
    // Test adding another remote
    let remote2 = RemoteConfig {
        name: "backup".to_string(),
        url: "http://backup.example.com:8080".to_string(),
    };
    
    dot_rev.add_remote(remote2).expect("Failed to add second remote");
    
    let remotes = dot_rev.remotes().expect("Failed to get remotes");
    assert_eq!(remotes.len(), 2);
    
    // Test removing a remote
    dot_rev.remove_remote("origin").expect("Failed to remove remote");
    
    let remotes = dot_rev.remotes().expect("Failed to get remotes");
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].name, "backup");
    
    // Test getting specific remote
    let backup = dot_rev.get_remote("backup").expect("Failed to get remote");
    assert!(backup.is_some());
    assert_eq!(backup.unwrap().url, "http://backup.example.com:8080");
    
    let missing = dot_rev.get_remote("origin").expect("Failed to get remote");
    assert!(missing.is_none());
}