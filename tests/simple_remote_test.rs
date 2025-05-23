use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;

#[test]
#[ignore]
fn test_simple_remote_workflow() {
    // Create temp directory
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let repo1_path = temp_dir.path().join("repo1");
    let repo2_path = temp_dir.path().join("repo2");
    
    std::fs::create_dir(&repo1_path).expect("Failed to create repo1");
    std::fs::create_dir(&repo2_path).expect("Failed to create repo2");
    
    // Get revtool path
    let revtool = env!("CARGO_BIN_EXE_revtool");
    
    // Helper to run commands
    let run_cmd = |args: &[&str], cwd: &std::path::Path| -> Result<String, String> {
        let output = Command::new(revtool)
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("Failed to execute command");
        
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(format!("stderr: {}\nstdout: {}", 
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)))
        }
    };
    
    // Set up repo1
    println!("Setting up repo1...");
    run_cmd(&["init"], &repo1_path).expect("Failed to init repo1");
    std::fs::write(repo1_path.join("test.txt"), "Hello").expect("Failed to write file");
    run_cmd(&["snap", "-m", "Initial"], &repo1_path).expect("Failed to snap");
    
    // Start server using spawn (not thread)
    println!("Starting server...");
    let mut server = Command::new(revtool)
        .args(&["serve", "--port", "9999"])
        .current_dir(&repo1_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start server");
    
    // Wait for server
    std::thread::sleep(Duration::from_secs(3));
    
    // Check if server is still running
    match server.try_wait() {
        Ok(Some(status)) => {
            panic!("Server exited with status: {:?}", status);
        }
        Ok(None) => {
            println!("Server is running");
        }
        Err(e) => {
            panic!("Error checking server status: {}", e);
        }
    }
    
    // Set up repo2 and pull
    println!("Setting up repo2...");
    run_cmd(&["init"], &repo2_path).expect("Failed to init repo2");
    
    println!("Adding remote...");
    run_cmd(&["remote", "add", "origin", "http://127.0.0.1:9999"], &repo2_path)
        .expect("Failed to add remote");
    
    println!("Pulling from remote...");
    match run_cmd(&["pull", "origin", "dev"], &repo2_path) {
        Ok(output) => println!("Pull output: {}", output),
        Err(e) => {
            // Kill server before panicking
            let _ = server.kill();
            panic!("Pull failed: {}", e);
        }
    }
    
    // Check file exists
    assert!(repo2_path.join("test.txt").exists(), "File not pulled");
    
    // Kill server
    server.kill().expect("Failed to kill server");
}