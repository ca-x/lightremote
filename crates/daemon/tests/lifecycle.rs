//! Real child processes: config checking must be inert; SIGTERM must join host shutdown.
#![cfg(unix)]
use serde_json::json;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn command(path: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lightremote-daemon"));
    command
        .args(["--config"])
        .arg(path)
        .env_remove("LIGHTREMOTE_RELAY_TOKEN")
        // Service accounts need no login profile; configured state owns all locks.
        .env_remove("HOME")
        .env_remove("XDG_STATE_HOME");
    command
}
fn configuration(root: &Path, port: u16) -> std::path::PathBuf {
    let path = root.join("config.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "relay": format!("http://127.0.0.1:{port}"),
            "web_url": "http://127.0.0.1:5173/",
            "state_dir": root.join("state"), "policy_confirmed": true,
            "relay_token": "daemon-test-private-credential"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "daemon lifecycle deadline expired"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn check_config_is_read_only_and_never_contacts_relay_or_scm() {
    let root = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let path = configuration(root.path(), listener.local_addr().unwrap().port());
    let output = command(&path).arg("--check-config").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.path().join("state").exists());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("credential"));
    assert!(
        !command(&path)
            .args(["--check-config", "--service"])
            .status()
            .unwrap()
            .success()
    );
    let output = command(&path)
        .arg("--check-config")
        .env("LIGHTREMOTE_RELAY_TOKEN", "")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "an empty environment token must not fall back to a file token"
    );
}

#[test]
fn daemon_recovers_after_relay_outage_and_sigterm_cleans_private_invite() {
    let root = tempfile::tempdir().unwrap();
    let reserved = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reserved.local_addr().unwrap();
    drop(reserved);
    let path = configuration(root.path(), address.port());
    let mut daemon = Process(
        command(&path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    until(|| root.path().join("state").is_dir());
    // Exercise a refused connection before a local protocol fixture becomes available.
    thread::sleep(Duration::from_millis(300));
    let listener = TcpListener::bind(address).unwrap();
    listener.set_nonblocking(true).unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let fixture_done = done.clone();
    let fixture = thread::spawn(move || {
        let mut held = Vec::new();
        while !fixture_done.load(Ordering::Acquire) {
            if let Ok((mut stream, _)) = listener.accept() {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 8192];
                let size = stream.read(&mut request).unwrap_or(0);
                if request[..size].starts_with(b"POST /v1/rooms ") {
                    let body = r#"{"room_id":"fixture","host_token":"host-token","client_token":"client-token","expires_at":4102444800}"#;
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                } else {
                    // Hold WebSocket negotiation pending so the invitation remains available.
                    held.push(stream);
                }
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let invite_path = root.path().join("state/invite");
    until(|| invite_path.is_file());
    let invitation = fs::read_to_string(&invite_path).unwrap();
    let gate =
        lightremote_updater::SessionGate::new(&root.path().join("state/sessions.lock")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(gate.path())
        .unwrap();
    assert!(
        fs2::FileExt::try_lock_exclusive(&lock).is_err(),
        "the service and updater must exclude one another with the configured gate"
    );
    assert!(invitation.starts_with("http://127.0.0.1:5173/#"));
    assert_eq!(
        fs::metadata(&invite_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        Command::new("kill")
            .args(["-TERM", &daemon.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    until(|| daemon.0.try_wait().unwrap().is_some());
    let status = daemon.0.wait().unwrap();
    done.store(true, Ordering::Release);
    fixture.join().unwrap();
    let mut stdout = String::new();
    let mut stderr = String::new();
    daemon
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    daemon
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "{stderr}");
    assert!(!invite_path.exists());
    assert!(
        stderr.contains("Relay unavailable"),
        "outage recovery was not exercised: {stderr}"
    );
    assert!(stdout.is_empty());
    for secret in [
        invitation.trim(),
        "daemon-test-private-credential",
        "host-token",
        "client-token",
    ] {
        assert!(!stderr.contains(secret), "logs disclosed a secret");
    }
}
