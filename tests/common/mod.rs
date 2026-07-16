//! Shared helpers for the remote integration tests.
//!
//! These make the tests robust enough to run unattended in CI: they use an
//! OS-assigned port (no hard-coded numbers, so tests can run in parallel), poll
//! for the server to actually accept connections (instead of a fixed sleep),
//! and kill the server on drop (so a failing assertion can't leak the process).

use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Returns a currently-free localhost port.
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("failed to bind an ephemeral port")
        .local_addr()
        .expect("failed to read local addr")
        .port()
}

/// A `revtool serve` child process that is killed when dropped, even if a test
/// panics before reaching its end.
pub struct ServerGuard {
    child: Child,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts `revtool serve` on `port` in `repo` and waits until it is accepting
/// connections. Panics if the server does not come up within a few seconds.
pub fn start_server(revtool: &str, repo: &Path, port: u16) -> ServerGuard {
    start_server_opt(revtool, repo, port, None)
}

/// Like [`start_server`], but optionally requires a bearer token.
pub fn start_server_opt(revtool: &str, repo: &Path, port: u16, token: Option<&str>) -> ServerGuard {
    let mut args = vec!["serve".to_string(), "--port".to_string(), port.to_string()];
    if let Some(t) = token {
        args.push("--token".to_string());
        args.push(t.to_string());
    }
    let child = Command::new(revtool)
        .args(&args)
        .current_dir(repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start revtool serve");
    let guard = ServerGuard { child };

    let addr = format!("127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(&addr).is_ok() {
            return guard;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("server did not start listening on {addr} within 10s");
}
