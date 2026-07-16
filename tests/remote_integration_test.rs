use std::process::Command;
use tempfile::TempDir;

mod common;

/// End-to-end integration test for push/pull over HTTP.
#[test]
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

    // Initialize first repository with two files and a snapshot.
    run_revtool(&["init"], &repo1_path).expect("Failed to init repo1");
    std::fs::write(repo1_path.join("file1.txt"), "Hello from repo1").expect("Failed to write file1");
    std::fs::write(repo1_path.join("file2.txt"), "Test content").expect("Failed to write file2");
    run_revtool(&["snap", "-m", "Initial commit"], &repo1_path)
        .expect("Failed to create snapshot");

    // Start server on an ephemeral port; killed automatically on drop.
    let port = common::free_port();
    let _server = common::start_server(revtool, &repo1_path, port);
    let url = format!("http://127.0.0.1:{port}");

    // Second repository pulls from the first.
    run_revtool(&["init"], &repo2_path).expect("Failed to init repo2");
    run_revtool(&["remote", "add", "origin", &url], &repo2_path).expect("Failed to add remote");
    run_revtool(&["pull", "origin", "dev"], &repo2_path).expect("Failed to pull from origin");

    // Verify files were pulled with the right content.
    assert!(repo2_path.join("file1.txt").exists(), "file1.txt not pulled");
    assert!(repo2_path.join("file2.txt").exists(), "file2.txt not pulled");
    let content =
        std::fs::read_to_string(repo2_path.join("file1.txt")).expect("Failed to read file1.txt");
    assert_eq!(content, "Hello from repo1");

    // Make a change in repo2 and push it back.
    std::fs::write(repo2_path.join("file3.txt"), "New file from repo2")
        .expect("Failed to write file3");
    run_revtool(&["snap", "-m", "Add file3"], &repo2_path)
        .expect("Failed to create snapshot in repo2");
    run_revtool(&["push", "origin"], &repo2_path).expect("Failed to push to origin");

    // A third clone should now see the pushed file, proving the round-trip.
    let repo3_path = temp_dir.path().join("repo3");
    std::fs::create_dir(&repo3_path).expect("Failed to create repo3");
    run_revtool(&["init"], &repo3_path).expect("Failed to init repo3");
    run_revtool(&["remote", "add", "origin", &url], &repo3_path).expect("Failed to add remote");
    run_revtool(&["pull", "origin", "dev"], &repo3_path).expect("Failed to pull into repo3");
    assert!(
        repo3_path.join("file3.txt").exists(),
        "file3.txt did not round-trip through the server"
    );
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
/// The server enforces bearer-token auth: without the token a pull fails and
/// writes nothing; with it, the pull works.
#[test]
fn test_token_auth_enforced() {
    let temp = TempDir::new().unwrap();
    let server_repo = temp.path().join("server");
    let client_repo = temp.path().join("client");
    std::fs::create_dir(&server_repo).unwrap();
    std::fs::create_dir(&client_repo).unwrap();
    let revtool = env!("CARGO_BIN_EXE_revtool");

    let run = |args: &[&str], cwd: &std::path::Path| {
        Command::new(revtool).args(args).current_dir(cwd).output().unwrap()
    };
    let run_tok = |args: &[&str], cwd: &std::path::Path, tok: &str| {
        Command::new(revtool).args(args).current_dir(cwd).env("REVTOOL_TOKEN", tok).output().unwrap()
    };

    run(&["init"], &server_repo);
    std::fs::write(server_repo.join("secret.txt"), "top secret").unwrap();
    run(&["snap", "-m", "c1"], &server_repo);

    let port = common::free_port();
    let _server = common::start_server_opt(revtool, &server_repo, port, Some("s3cr3t"));
    let url = format!("http://127.0.0.1:{port}");

    run(&["init"], &client_repo);
    run(&["remote", "add", "origin", &url], &client_repo);

    // No token: pull must fail and write nothing.
    assert!(!run(&["pull", "origin", "dev"], &client_repo).status.success());
    assert!(!client_repo.join("secret.txt").exists(), "leaked data without a token");

    // Wrong token: same.
    assert!(!run_tok(&["pull", "origin", "dev"], &client_repo, "nope").status.success());
    assert!(!client_repo.join("secret.txt").exists());

    // Correct token: works.
    let ok = run_tok(&["pull", "origin", "dev"], &client_repo, "s3cr3t");
    assert!(ok.status.success(), "authed pull failed: {}", String::from_utf8_lossy(&ok.stderr));
    assert_eq!(std::fs::read_to_string(client_repo.join("secret.txt")).unwrap(), "top secret");
}

/// A divergent (non-fast-forward) push is rejected without --force and accepted
/// with it.
#[test]
fn test_non_fast_forward_push_rejected() {
    let temp = TempDir::new().unwrap();
    let server = temp.path().join("server");
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    for p in [&server, &a, &b] {
        std::fs::create_dir(p).unwrap();
    }
    let revtool = env!("CARGO_BIN_EXE_revtool");
    let run = |args: &[&str], cwd: &std::path::Path| {
        Command::new(revtool).args(args).current_dir(cwd).output().unwrap()
    };

    run(&["init"], &server);
    std::fs::write(server.join("f.txt"), "base").unwrap();
    run(&["snap", "-m", "base"], &server);
    let port = common::free_port();
    let _srv = common::start_server(revtool, &server, port);
    let url = format!("http://127.0.0.1:{port}");

    for c in [&a, &b] {
        run(&["init"], c);
        run(&["remote", "add", "origin", &url], c);
        run(&["pull", "origin", "dev"], c);
    }

    std::fs::write(a.join("a.txt"), "a").unwrap();
    run(&["snap", "-m", "a"], &a);
    assert!(run(&["push", "origin", "dev"], &a).status.success());

    // b is now stale; its divergent push must be refused.
    std::fs::write(b.join("b.txt"), "b").unwrap();
    run(&["snap", "-m", "b"], &b);
    assert!(!run(&["push", "origin", "dev"], &b).status.success(), "non-ff push accepted");

    // --force overrides.
    assert!(run(&["push", "origin", "dev", "--force"], &b).status.success());
}
