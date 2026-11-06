use lightremote_services::{HostConfig, HostServices};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream},
    task::JoinHandle,
    time::timeout,
};

fn make_host(root: Option<&Path>, write: bool) -> HostServices {
    HostServices::new(HostConfig {
        share_dir: root.map(Path::to_path_buf),
        allow_write: write,
        max_file_size: 64 * 1024,
        io_timeout: Duration::from_secs(3),
        ..HostConfig::default()
    })
    .unwrap()
}
fn stream(
    host: &HostServices,
    route: &str,
    metadata: Value,
    capacity: usize,
) -> (DuplexStream, JoinHandle<anyhow::Result<()>>) {
    let (client, io) = tokio::io::duplex(capacity);
    let host = host.clone();
    let route = route.to_string();
    (
        client,
        tokio::spawn(async move { host.serve(&route, metadata, io).await }),
    )
}
// This adapter models the transport contract: a pipe ending without a
// verified remote FIN is RESET, not successful application EOF.
struct ResetOnEof {
    io: DuplexStream,
}
impl tokio::io::AsyncRead for ResetOnEof {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let capacity = buf.remaining();
        match std::pin::Pin::new(&mut self.io).poll_read(cx, buf) {
            std::task::Poll::Ready(Ok(())) if capacity > 0 && buf.filled().len() == before => {
                std::task::Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "transport reset before FIN",
                )))
            }
            other => other,
        }
    }
}
impl tokio::io::AsyncWrite for ResetOnEof {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.io).poll_write(cx, bytes)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
fn reset_stream(
    host: &HostServices,
    route: &str,
    metadata: Value,
    capacity: usize,
) -> (DuplexStream, JoinHandle<anyhow::Result<()>>) {
    let (client, io) = tokio::io::duplex(capacity);
    let host = host.clone();
    let route = route.to_string();
    (
        client,
        tokio::spawn(async move { host.serve(&route, metadata, ResetOnEof { io }).await }),
    )
}

async fn record(io: &mut DuplexStream, body: &[u8]) {
    io.write_all(&(body.len() as u32).to_be_bytes())
        .await
        .unwrap();
    io.write_all(body).await.unwrap();
}
async fn json_record(io: &mut DuplexStream) -> Value {
    let length = io.read_u32().await.unwrap();
    assert!(length <= 1024 * 1024);
    let mut body = vec![0; length as usize];
    io.read_exact(&mut body).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}
async fn joined(task: JoinHandle<anyhow::Result<()>>) -> anyhow::Result<()> {
    timeout(Duration::from_secs(5), task)
        .await
        .expect("service hung")
        .expect("service panicked")
}

#[tokio::test]
async fn uploads_commit_exact_size_with_receipt_and_download_over_tiny_pipe() {
    let dir = TempDir::new().unwrap();
    let host = make_host(Some(dir.path()), true);
    let bytes = vec![0x5a; 48 * 1024];
    let (mut client, task) = stream(
        &host,
        "file.put",
        json!({"path":"data","size":bytes.len()}),
        7,
    );
    client.write_all(&bytes).await.unwrap();
    client.shutdown().await.unwrap();
    let receipt = json_record(&mut client).await;
    assert_eq!(receipt["size"], bytes.len());
    assert_eq!(receipt["sha256"], format!("{:x}", Sha256::digest(&bytes)));
    let mut extra = Vec::new();
    client.read_to_end(&mut extra).await.unwrap();
    assert!(extra.is_empty());
    joined(task).await.unwrap();
    assert_eq!(std::fs::read(dir.path().join("data")).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    let (mut client, task) = stream(&host, "file.get", json!({"path":"data"}), 5);
    client.shutdown().await.unwrap();
    let mut result = Vec::new();
    client.read_to_end(&mut result).await.unwrap();
    joined(task).await.unwrap();
    assert_eq!(result, bytes);
}

#[tokio::test]
async fn reset_without_verified_fin_never_commits_zero_or_exact_size_upload() {
    for bytes in [b"".as_slice(), b"exact".as_slice()] {
        let dir = TempDir::new().unwrap();
        let host = make_host(Some(dir.path()), true);
        let (mut client, task) = reset_stream(
            &host,
            "file.put",
            json!({"path":"data","size":bytes.len()}),
            128,
        );
        client.write_all(bytes).await.unwrap();
        drop(client);
        assert!(
            joined(task).await.is_err(),
            "RESET was accepted as upload FIN"
        );
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "RESET committed destination or leaked staging"
        );
    }
}

#[tokio::test]
async fn partial_excess_and_cancelled_uploads_never_commit_and_remove_staging() {
    for (size, bytes) in [(10, b"short".as_slice()), (2, b"long".as_slice())] {
        let dir = TempDir::new().unwrap();
        let host = make_host(Some(dir.path()), true);
        let (mut client, task) = stream(&host, "file.put", json!({"path":"data","size":size}), 128);
        client.write_all(bytes).await.unwrap();
        client.shutdown().await.unwrap();
        assert!(joined(task).await.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    let dir = TempDir::new().unwrap();
    let host = make_host(Some(dir.path()), true);
    let (mut client, task) = stream(&host, "file.put", json!({"path":"data","size":100}), 16);
    client.write_all(b"partial").await.unwrap();
    for _ in 0..100 {
        if std::fs::read_dir(dir.path()).unwrap().count() > 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn fin_is_required_and_destination_race_never_overwrites() {
    let dir = TempDir::new().unwrap();
    let host = make_host(Some(dir.path()), true);
    let (mut client, task) = stream(&host, "file.put", json!({"path":"data","size":3}), 16);
    client.write_all(b"new").await.unwrap();
    for _ in 0..100 {
        if std::fs::read_dir(dir.path()).unwrap().count() > 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(!dir.path().join("data").exists());
    std::fs::write(dir.path().join("data"), b"existing").unwrap();
    client.shutdown().await.unwrap();
    assert!(joined(task).await.is_err());
    assert_eq!(std::fs::read(dir.path().join("data")).unwrap(), b"existing");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn permissions_size_limits_and_traversal_are_enforced() {
    let dir = TempDir::new().unwrap();
    let host = make_host(Some(dir.path()), false);
    assert_eq!(host.capabilities().await["file_write"], false);
    for route in ["file.put", "shell", "input", "desktop"] {
        let (_, task) = stream(
            &host,
            route,
            json!({"path":"denied","size":0,"stream_id":1}),
            8,
        );
        assert!(joined(task).await.is_err());
    }
    for path in [
        "../escape",
        "/absolute",
        "nested/../../escape",
        "a//b",
        "./file",
        "a\\b",
        "C:/file",
    ] {
        let (_, task) = stream(&host, "file.get", json!({"path":path}), 8);
        assert!(joined(task).await.is_err(), "{path}");
        let response = host
            .control(json!({"id":1,"method":"fs.list","params":{"path":path}}))
            .await
            .unwrap();
        assert!(response.get("error").is_some(), "{path}");
        assert!(!response.to_string().contains(dir.path().to_str().unwrap()));
    }
    let host = make_host(Some(dir.path()), true);
    let (_, task) = stream(&host, "file.put", json!({"path":"big","size":64*1024+1}), 8);
    assert!(joined(task).await.is_err());
    std::fs::write(dir.path().join("big"), vec![0; 64 * 1024 + 1]).unwrap();
    let (_, task) = stream(&host, "file.get", json!({"path":"big"}), 8);
    assert!(joined(task).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_files_and_parents_cannot_escape_for_any_operation() {
    use std::os::unix::fs::symlink;
    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret"), b"private").unwrap();
    symlink(outside.path(), root.path().join("link")).unwrap();
    symlink(outside.path().join("secret"), root.path().join("direct")).unwrap();
    let host = make_host(Some(root.path()), true);
    for path in ["link/secret", "direct"] {
        let (_, task) = stream(&host, "file.get", json!({"path":path}), 8);
        assert!(joined(task).await.is_err());
    }
    let (_, task) = stream(&host, "file.put", json!({"path":"link/new","size":0}), 8);
    assert!(joined(task).await.is_err());
    for (method, path) in [
        ("fs.list", "link"),
        ("fs.mkdir", "link/new"),
        ("fs.remove", "link/secret"),
        ("fs.remove", "direct"),
    ] {
        let response = host
            .control(json!({"id":1,"method":method,"params":{"path":path}}))
            .await
            .unwrap();
        assert!(response.get("error").is_some(), "{response}");
    }
    assert_eq!(
        std::fs::read(outside.path().join("secret")).unwrap(),
        b"private"
    );
    assert!(!outside.path().join("new").exists());
    // Also disallow in-root symlinks; every opened component is no-follow.
    std::fs::create_dir(root.path().join("real")).unwrap();
    symlink("real", root.path().join("internal")).unwrap();
    let response = host
        .control(json!({"id":1,"method":"fs.list","params":{"path":"internal"}}))
        .await
        .unwrap();
    assert!(response.get("error").is_some());
}

#[tokio::test]
async fn real_filesystem_control_and_framed_request_fragmentation() {
    let dir = TempDir::new().unwrap();
    let host = make_host(Some(dir.path()), true);
    let (mut client, task) = stream(&host, "control", json!({}), 3);
    let request =
        serde_json::to_vec(&json!({"id":1,"method":"fs.mkdir","params":{"path":"sub"}})).unwrap();
    // A duplex pipe smaller than the record header exercises fragmentation.
    record(&mut client, &request).await;
    assert_eq!(json_record(&mut client).await["id"], 1);
    assert!(dir.path().join("sub").is_dir());
    record(&mut client, b"not json").await;
    assert_eq!(
        json_record(&mut client).await["error"]["code"],
        "invalid_request"
    );
    record(
        &mut client,
        &serde_json::to_vec(&json!({"id":"list","method":"fs.list","params":{"path":"."}}))
            .unwrap(),
    )
    .await;
    let response = json_record(&mut client).await;
    assert_eq!(response["result"]["entries"][0]["name"], "sub");
    client.shutdown().await.unwrap();
    let mut rest = Vec::new();
    client.read_to_end(&mut rest).await.unwrap();
    joined(task).await.unwrap();
    assert!(rest.is_empty());
    let response = host
        .control(json!({"id":2,"method":"fs.remove","params":{"path":"sub"}}))
        .await
        .unwrap();
    assert!(response.get("result").is_some());
    assert!(!dir.path().join("sub").exists());
}

#[tokio::test]
async fn overlong_and_truncated_records_fail_before_body_allocation_or_hang() {
    let host = make_host(None, false);
    for bytes in [vec![0, 16, 0, 1], vec![0, 0], vec![0, 0, 0, 5, b'{']] {
        let (mut client, task) = stream(&host, "control", json!({}), 16);
        client.write_all(&bytes).await.unwrap();
        client.shutdown().await.unwrap();
        assert!(joined(task).await.is_err());
    }
}

#[cfg(not(feature = "desktop"))]
#[tokio::test]
async fn headless_capabilities_never_claim_desktop_even_when_flags_enabled() {
    let host = HostServices::new(HostConfig {
        allow_desktop: true,
        allow_input: true,
        allow_clipboard: true,
        ..HostConfig::default()
    })
    .unwrap();
    let caps = host.capabilities().await;
    for flag in ["desktop_compiled", "desktop", "input", "clipboard"] {
        assert_eq!(caps[flag], false, "{flag}");
    }
    for method in ["desktop.displays", "clipboard.get"] {
        assert!(
            host.control(json!({"id":1,"method":method}))
                .await
                .unwrap()
                .get("error")
                .is_some()
        );
    }
}

#[tokio::test]
async fn stalled_upload_is_cancelled_and_stream_slots_are_bounded() {
    let dir = TempDir::new().unwrap();
    let host = HostServices::new(HostConfig {
        share_dir: Some(dir.path().to_path_buf()),
        allow_write: true,
        max_concurrent_streams: 1,
        io_timeout: Duration::from_millis(100),
        ..HostConfig::default()
    })
    .unwrap();
    let (mut client, task) = stream(&host, "file.put", json!({"path":"stalled","size":1}), 8);
    client.write_all(b"x").await.unwrap();
    for _ in 0..100 {
        if std::fs::read_dir(dir.path()).unwrap().count() > 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    let (_, other) = stream(&host, "control", json!({}), 8);
    assert!(joined(other).await.is_err());
    assert!(joined(task).await.is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[cfg(unix)]
mod pty {
    use super::*;
    fn shell_host(idle: Duration) -> HostServices {
        HostServices::new(HostConfig {
            allow_shell: true,
            shell: Some("/bin/sh".into()),
            io_timeout: idle,
            ..HostConfig::default()
        })
        .unwrap()
    }
    async fn until(client: &mut DuplexStream, needle: &str) -> String {
        timeout(Duration::from_secs(3), async {
            let mut collected = Vec::new();
            let mut buf = [0; 1024];
            loop {
                let n = client.read(&mut buf).await.unwrap();
                assert!(
                    n > 0,
                    "PTY closed before {needle}: {}",
                    String::from_utf8_lossy(&collected)
                );
                collected.extend_from_slice(&buf[..n]);
                assert!(collected.len() < 64 * 1024);
                let text = String::from_utf8_lossy(&collected);
                if text.contains(needle) {
                    return text.into_owned();
                }
            }
        })
        .await
        .expect("PTY timed out")
    }
    #[tokio::test]
    async fn shell_is_a_real_pty_resize_and_exit_notification() {
        let host = shell_host(Duration::from_secs(3));
        let mut events = host.subscribe();
        let (mut client, task) = stream(
            &host,
            "shell",
            json!({"stream_id":42,"cols":80,"rows":24}),
            64,
        );
        client
            .write_all(b"stty -echo\nprintf 'READY\\n'\n")
            .await
            .unwrap();
        until(&mut client, "READY\r\n").await;
        let response=host.control(json!({"id":1,"method":"shell.resize","params":{"stream_id":42,"cols":111,"rows":37}})).await.unwrap();
        assert!(response.get("result").is_some(), "{response}");
        client.write_all(b"stty size\n").await.unwrap();
        until(&mut client, "37 111").await;
        client.write_all(b"exit 7\n").await.unwrap();
        let mut output = Vec::new();
        timeout(Duration::from_secs(3), client.read_to_end(&mut output))
            .await
            .unwrap()
            .unwrap();
        joined(task).await.unwrap();
        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event["event"], "shell.exit");
        assert_eq!(event["data"]["stream_id"], 42);
        assert_eq!(event["data"]["code"], 7);
        assert!(
            host.control(
                json!({"id":2,"method":"shell.resize","params":{"stream_id":42,"cols":1,"rows":1}})
            )
            .await
            .unwrap()
            .get("error")
            .is_some()
        );
    }
    async fn pid(client: &mut DuplexStream) -> i32 {
        client
            .write_all(b"stty -echo\nprintf '\\nPID:%d:END\\n' $$\n")
            .await
            .unwrap();
        let mut text = String::new();
        timeout(Duration::from_secs(3), async {
            loop {
                let mut buf = [0; 1024];
                let n = client.read(&mut buf).await.unwrap();
                assert!(n > 0);
                text.push_str(&String::from_utf8_lossy(&buf[..n]));
                for line in text.lines() {
                    if let Some(value) = line
                        .trim()
                        .strip_prefix("PID:")
                        .and_then(|line| line.strip_suffix(":END"))
                        && let Ok(pid) = value.parse()
                    {
                        return pid;
                    }
                }
            }
        })
        .await
        .expect("PID not emitted")
    }
    async fn assert_gone(pid: i32) {
        timeout(Duration::from_secs(3), async {
            loop {
                // SAFETY: signal 0 checks existence without delivering a signal.
                if unsafe { libc::kill(pid, 0) } == -1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("PTY child survived disconnect/cancellation");
    }
    #[tokio::test]
    async fn shell_child_dies_when_peer_drops_or_service_is_aborted() {
        for abort in [false, true] {
            let host = shell_host(Duration::from_secs(3));
            let mut events = host.subscribe();
            let (mut client, task) = reset_stream(&host, "shell", json!({"stream_id":1}), 64);
            let child = pid(&mut client).await;
            if abort {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                drop(client);
                let _ = joined(task).await;
            }
            assert_gone(child).await;
            let event = timeout(Duration::from_secs(3), events.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(event["event"], "shell.exit");
            assert_eq!(event["data"]["stream_id"], 1);
        }
    }
    #[cfg(target_os = "linux")]
    fn running_process(pid: i32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                stat.rsplit_once(')')
                    .map(|(_, tail)| tail.trim_start().starts_with('Z'))
            })
            .is_some_and(|zombie| !zombie)
    }

    #[cfg(target_os = "linux")]
    struct Background(i32);
    #[cfg(target_os = "linux")]
    impl Drop for Background {
        fn drop(&mut self) {
            // SAFETY: this is the PID of our own test's HUP-ignoring sleep.
            unsafe {
                libc::kill(self.0, libc::SIGKILL);
            }
        }
    }

    #[cfg(target_os = "linux")]
    async fn background(client: &mut DuplexStream) -> (i32, Background) {
        client.write_all(b"stty -echo\nset -m\nprintf '\\nLEADER:%d:END\\n' $$\nsh -c 'trap \"\" HUP; printf \"\\nBACKGROUND:%d:END\\n\" \"$$\"; exec sleep 60' &\n").await.unwrap();
        let text = timeout(Duration::from_secs(3), async {
            let mut text = String::new();
            loop {
                let mut buffer = [0; 1024];
                let n = client.read(&mut buffer).await.unwrap();
                assert!(n > 0, "PTY closed: {text}");
                text.push_str(&String::from_utf8_lossy(&buffer[..n]));
                let marker = |prefix: &str| {
                    text.lines().find_map(|line| {
                        line.trim()
                            .strip_prefix(prefix)
                            .and_then(|line| line.strip_suffix(":END"))
                            .and_then(|number| number.parse::<i32>().ok())
                    })
                };
                if let (Some(leader), Some(background)) = (marker("LEADER:"), marker("BACKGROUND:"))
                {
                    return (leader, Background(background));
                }
            }
        })
        .await
        .expect("background job did not become ready");
        // SAFETY: getsid/getpgid only inspect these two owned test processes.
        unsafe {
            assert_eq!(libc::getsid(text.1.0), text.0);
            assert_ne!(libc::getpgid(text.1.0), libc::getpgid(text.0));
        }
        text
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn background_jobs_die_and_stream_slots_recover_after_disconnect() {
        background_slot_recovery(false, false).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn background_jobs_die_and_slots_recover_after_leader_already_exited() {
        background_slot_recovery(true, false).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn background_jobs_die_and_slots_recover_after_service_abort() {
        background_slot_recovery(false, true).await;
    }

    #[cfg(target_os = "linux")]
    async fn background_slot_recovery(exit_first: bool, abort: bool) {
        let host = HostServices::new(HostConfig {
            allow_shell: true,
            shell: Some("/bin/bash".into()),
            max_concurrent_streams: 1,
            io_timeout: Duration::from_secs(3),
            ..HostConfig::default()
        })
        .unwrap();
        for index in 0..20 {
            let (mut client, task) = reset_stream(&host, "shell", json!({"stream_id":index}), 1024);
            let (leader, background) = background(&mut client).await;
            if exit_first {
                client.write_all(b"exit\n").await.unwrap();
                timeout(Duration::from_secs(2), async {
                    while running_process(leader) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("shell leader did not exit");
            }
            drop(client);
            if abort {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                let _ = joined(task).await;
            }
            let dead = timeout(Duration::from_millis(800), async {
                while running_process(background.0) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
            assert!(
                dead.is_ok(),
                "background PID {} survived (leader exited: {exit_first})",
                background.0
            );
            timeout(Duration::from_secs(2), async {
                loop {
                    let (mut control, task) = stream(&host, "control", json!({}), 8);
                    control.shutdown().await.unwrap();
                    if joined(task).await.is_ok() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("service stream slot leaked after PTY teardown");
        }
    }

    #[tokio::test]
    async fn normal_shell_fin_drains_queued_commands_output_and_exit_status() {
        let host = shell_host(Duration::from_secs(3));
        let mut events = host.subscribe();
        let (mut client, task) = stream(&host, "shell", json!({"stream_id":77}), 128);
        // No literal marker appears in the input: terminal echo cannot satisfy it.
        client.write_all(b"stty -echo\nsleep 0.1; printf '\\114\\122\\106\\111\\116\\137\\104\\122\\101\\111\\116\\105\\104\\n'; exit 7\n").await.unwrap();
        client.shutdown().await.unwrap();
        let mut bytes = Vec::new();
        timeout(Duration::from_secs(4), client.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        joined(task).await.unwrap();
        assert!(
            String::from_utf8_lossy(&bytes).contains("LRFIN_DRAINED"),
            "shell lost output after input FIN: {}",
            String::from_utf8_lossy(&bytes)
        );
        let event = events.recv().await.unwrap();
        assert_eq!(
            event["data"]["code"], 7,
            "input FIN killed child instead of allowing exit"
        );
    }

    #[tokio::test]
    async fn canonical_shell_input_fin_delivers_eof_after_queued_input() {
        let host = shell_host(Duration::from_secs(3));
        let (mut client, task) = stream(&host, "shell", json!({"stream_id":78}), 128);
        // Wait until the shell has parsed the compound command and restored
        // canonical mode. Interactive readline can otherwise consume typeahead
        // intended for the command's stdin before that command is even running.
        // Both markers are escaped so terminal echo cannot satisfy this test.
        client.write_all(b"stty -echo; printf '\\122\\105\\101\\104\\131\\n'; count=$(wc -c); printf '\\105\\117\\106\\137\\104\\122\\101\\111\\116\\105\\104:%s\\n' \"$count\"; exit 0\n").await.unwrap();
        until(&mut client, "READY\r\n").await;
        let mut payload = b"payload\n".repeat(8192);
        payload.extend_from_slice(b"partial-final-line");
        client.write_all(&payload).await.unwrap();
        client.shutdown().await.unwrap();
        let mut bytes = Vec::new();
        timeout(Duration::from_secs(4), client.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let result = joined(task).await;
        assert!(
            result.is_ok(),
            "canonical input FIN failed: {result:?}; output {}",
            String::from_utf8_lossy(&bytes)
        );
        let text = String::from_utf8_lossy(&bytes);
        let count = text.lines().find_map(|line| {
            line.trim()
                .strip_prefix("EOF_DRAINED:")
                .and_then(|number| number.trim().parse::<usize>().ok())
        });
        assert_eq!(
            count,
            Some(payload.len()),
            "queued input was lost or extra input was injected: {text}"
        );
    }

    #[tokio::test]
    async fn shell_fin_output_drain_has_deadline_even_with_continuous_output() {
        let host = shell_host(Duration::from_millis(500));
        let (mut client, task) = stream(&host, "shell", json!({"stream_id":79}), 128);
        let child = pid(&mut client).await;
        client
            .write_all(b"while :; do printf x; done\n")
            .await
            .unwrap();
        let start = std::time::Instant::now();
        client.shutdown().await.unwrap();
        timeout(
            Duration::from_secs(3),
            tokio::io::copy(&mut client, &mut tokio::io::sink()),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            joined(task).await.is_err(),
            "infinite output unexpectedly completed successfully"
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_gone(child).await;
    }

    #[tokio::test]
    async fn stalled_output_kills_child_instead_of_accumulating_unbounded_bytes() {
        let host = shell_host(Duration::from_millis(300));
        let (mut client, task) = stream(&host, "shell", json!({"stream_id":1}), 64);
        let child = pid(&mut client).await;
        client.write_all(b"yes\n").await.unwrap();
        assert!(joined(task).await.is_err());
        assert_gone(child).await;
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_symlink_swaps_never_read_or_write_outside_share() {
    use std::{
        os::unix::fs::symlink,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("value"), b"outside-secret").unwrap();
    std::fs::create_dir(root.path().join("real")).unwrap();
    std::fs::write(root.path().join("real/value"), b"inside").unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let race_running = running.clone();
    let root_path = root.path().to_path_buf();
    let outside_path = outside.path().to_path_buf();
    let attacker = std::thread::spawn(move || {
        while race_running.load(Ordering::Acquire) {
            let _ = std::fs::rename(root_path.join("real"), root_path.join("held"));
            let _ = symlink(&outside_path, root_path.join("real"));
            let _ = std::fs::remove_file(root_path.join("real"));
            let _ = std::fs::rename(root_path.join("held"), root_path.join("real"));
        }
    });
    let host = make_host(Some(root.path()), true);
    for index in 0..100 {
        let (mut client, task) = stream(&host, "file.get", json!({"path":"real/value"}), 128);
        client.shutdown().await.unwrap();
        let mut data = Vec::new();
        client.read_to_end(&mut data).await.unwrap();
        let result = joined(task).await;
        assert!(
            !data
                .windows(b"outside-secret".len())
                .any(|bytes| bytes == b"outside-secret")
        );
        if result.is_ok() {
            assert_eq!(data, b"inside");
        }
        let (mut client, task) = stream(
            &host,
            "file.put",
            json!({"path":format!("real/new{index}"),"size":0}),
            128,
        );
        client.shutdown().await.unwrap();
        let mut data = Vec::new();
        client.read_to_end(&mut data).await.unwrap();
        let _ = joined(task).await;
    }
    running.store(false, Ordering::Release);
    attacker.join().unwrap();
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);
    assert_eq!(
        std::fs::read(outside.path().join("value")).unwrap(),
        b"outside-secret"
    );
}

#[cfg(feature = "desktop")]
#[tokio::test]
#[ignore = "requires an explicitly enabled, reachable desktop capture backend"]
async fn desktop_backend_emits_real_jpeg_single_fin_and_stream_disconnect() {
    let host = HostServices::new(HostConfig {
        allow_desktop: true,
        io_timeout: Duration::from_secs(5),
        ..HostConfig::default()
    })
    .unwrap();
    let response = host
        .control(json!({"id":1,"method":"desktop.displays","params":{}}))
        .await
        .unwrap();
    let display = &response["result"]["displays"][0];
    let width = display["width"]
        .as_u64()
        .expect("no actual desktop display");
    let height = display["height"]
        .as_u64()
        .expect("no actual desktop display");
    let (mut client, task) = stream(
        &host,
        "desktop",
        json!({"display":0,"single":true,"quality":70}),
        1024,
    );
    client.shutdown().await.unwrap();
    let length = timeout(Duration::from_secs(10), client.read_u32())
        .await
        .unwrap()
        .unwrap();
    assert!(length > 0 && length <= 8 * 1024 * 1024);
    let mut jpeg = vec![0; length as usize];
    client.read_exact(&mut jpeg).await.unwrap();
    let decoded = image::load_from_memory(&jpeg).unwrap();
    assert_eq!(u64::from(decoded.width()), width);
    assert_eq!(u64::from(decoded.height()), height);
    let mut tail = Vec::new();
    client.read_to_end(&mut tail).await.unwrap();
    assert!(tail.is_empty(), "single screenshot emitted extra records");
    joined(task).await.unwrap();
    let (mut client, task) = stream(
        &host,
        "desktop",
        json!({"display":0,"fps":10,"quality":50}),
        1024,
    );
    for _ in 0..2 {
        let length = timeout(Duration::from_secs(10), client.read_u32())
            .await
            .unwrap()
            .unwrap();
        assert!(length > 0 && length <= 8 * 1024 * 1024);
        let mut jpeg = vec![0; length as usize];
        client.read_exact(&mut jpeg).await.unwrap();
        assert!(image::load_from_memory(&jpeg).is_ok());
    }
    drop(client);
    assert!(joined(task).await.is_err());
}
