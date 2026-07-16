use std::process::Command;
use tempfile::TempDir;

mod common;

#[test]
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
            Err(format!(
                "stderr: {}\nstdout: {}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            ))
        }
    };

    // Set up repo1
    run_cmd(&["init"], &repo1_path).expect("Failed to init repo1");
    std::fs::write(repo1_path.join("test.txt"), "Hello").expect("Failed to write file");
    run_cmd(&["snap", "-m", "Initial"], &repo1_path).expect("Failed to snap");

    // Start server on an ephemeral port; killed automatically on drop.
    let port = common::free_port();
    let _server = common::start_server(revtool, &repo1_path, port);

    // Set up repo2 and pull
    run_cmd(&["init"], &repo2_path).expect("Failed to init repo2");
    run_cmd(
        &["remote", "add", "origin", &format!("http://127.0.0.1:{port}")],
        &repo2_path,
    )
    .expect("Failed to add remote");
    run_cmd(&["pull", "origin", "dev"], &repo2_path).expect("Pull failed");

    // Check file exists with the right content.
    assert!(repo2_path.join("test.txt").exists(), "File not pulled");
    let content =
        std::fs::read_to_string(repo2_path.join("test.txt")).expect("Failed to read test.txt");
    assert_eq!(content, "Hello");
}
