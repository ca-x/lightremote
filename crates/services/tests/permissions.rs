use lightremote_services::{
    HostConfig, HostServices,
    permissions::{ApprovalMode, Scope},
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[test]
fn queued_deletion_is_cancelled_across_revoke_and_regrant() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let root = tempfile::tempdir().unwrap();
        let victim = root.path().join("keep");
        std::fs::write(&victim, "existing data").unwrap();
        let host = HostServices::new(HostConfig { share_dir: Some(root.path().to_owned()), allow_write: true, ..HostConfig::default() }).unwrap();
        let (started, running) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || { let _ = started.send(()); let _ = blocked.recv(); });
        running.await.unwrap();
        let mut deletion = Box::pin(host.control(json!({"id":1,"method":"fs.remove","params":{"path":"keep"}})));
        tokio::select! { biased; result = &mut deletion => panic!("blocking deletion unexpectedly completed: {result:?}"), _ = tokio::task::yield_now() => {} }
        host.permissions().revoke(&[Scope::FileWrite]).unwrap();
        let denied = deletion.await.unwrap();
        assert_eq!(denied["error"]["code"], "permission_denied");
        let id = host.permissions().request(&[Scope::FileWrite]).unwrap().pending.unwrap().id;
        host.permissions().approve(id, None).unwrap();
        release.send(()).unwrap();
        blocker.await.unwrap();
        tokio::task::spawn_blocking(|| ()).await.unwrap();
        assert!(victim.exists(), "queued work executed after revoke/regrant");
        let fresh = host.control(json!({"id":2,"method":"fs.remove","params":{"path":"keep"}})).await.unwrap();
        assert!(fresh.get("error").is_none());
        assert!(!victim.exists(), "newly authorized work failed");
        host.shutdown().await.unwrap();
    });
}

#[tokio::test(start_paused = true)]
async fn a_healthy_idle_control_stream_outlives_the_record_io_budget() {
    use tokio::io::AsyncReadExt;
    let host = HostServices::new(HostConfig {
        io_timeout: Duration::from_millis(50),
        ..HostConfig::default()
    })
    .unwrap();
    let (mut client, service) = tokio::io::duplex(4096);
    let worker = tokio::spawn(async move { host.serve("control", json!({}), service).await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(120)).await;
    tokio::task::yield_now().await;
    assert!(
        !worker.is_finished(),
        "healthy silence killed the control stream"
    );
    let body = serde_json::to_vec(&json!({"id":1,"method":"capabilities","params":{}})).unwrap();
    client.write_u32(body.len() as u32).await.unwrap();
    client.write_all(&body).await.unwrap();
    let size = client.read_u32().await.unwrap();
    let mut response = vec![0; size as usize];
    client.read_exact(&mut response).await.unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&response).unwrap()["id"], 1);
    client.shutdown().await.unwrap();
    worker.await.unwrap().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn an_idle_interactive_shell_survives_and_revocation_still_terminates_it() {
    use tokio::io::AsyncReadExt;
    let host = HostServices::new(HostConfig {
        allow_shell: true,
        io_timeout: Duration::from_millis(300),
        ..HostConfig::default()
    })
    .unwrap();
    let (mut client, service) = tokio::io::duplex(4096);
    let service_host = host.clone();
    let worker = tokio::spawn(async move {
        service_host
            .serve("shell", json!({"stream_id":1,"cols":80,"rows":24}), service)
            .await
    });
    client
        .write_all(b"stty -echo\nprintf '%s_%s\\n' IDLE READY\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut output = Vec::new();
        loop {
            let byte = client.read_u8().await.unwrap();
            output.push(byte);
            if output
                .windows(b"IDLE_READY".len())
                .any(|bytes| bytes == b"IDLE_READY")
            {
                break;
            }
            assert!(output.len() < 65536);
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(
        !worker.is_finished(),
        "healthy silence killed the interactive shell"
    );
    host.permissions().revoke(&[Scope::Shell]).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn remote_requests_are_not_grants_and_control_methods_share_the_boundary() {
    let root = tempfile::tempdir().unwrap();
    let host = HostServices::with_approval(
        HostConfig {
            share_dir: Some(root.path().to_owned()),
            allow_write: true,
            allow_shell: true,
            ..HostConfig::default()
        },
        ApprovalMode::Attended,
    )
    .unwrap();
    let before = host.capabilities().await;
    assert_eq!(before["shell"], false);
    assert_eq!(before["file_read"], false);
    let request = host
        .control(json!({"id":1,"method":"permissions.request","params":{"scopes":["file_write"]}}))
        .await
        .unwrap();
    assert!(request.get("error").is_none());
    let denied = host
        .control(json!({"id":2,"method":"fs.mkdir","params":{"path":"folder"}}))
        .await
        .unwrap();
    assert_eq!(denied["error"]["code"], "permission_denied");
    assert!(!root.path().join("folder").exists());
    let id = request["result"]["pending"]["id"].as_u64().unwrap() as u32;
    host.permissions().approve(id, None).unwrap();
    let allowed = host
        .control(json!({"id":3,"method":"fs.mkdir","params":{"path":"folder"}}))
        .await
        .unwrap();
    assert!(allowed.get("error").is_none());
    assert!(root.path().join("folder").is_dir());
    // Upload permission does not silently grant directory listing or downloads.
    let listing = host
        .control(json!({"id":4,"method":"fs.list","params":{"path":"."}}))
        .await
        .unwrap();
    assert_eq!(listing["error"]["code"], "permission_denied");
    host.permissions().revoke(&[Scope::FileWrite]).unwrap();
    let removed = host
        .control(json!({"id":5,"method":"fs.remove","params":{"path":"folder"}}))
        .await
        .unwrap();
    assert_eq!(removed["error"]["code"], "permission_denied");
    assert!(root.path().join("folder").is_dir());
}

#[tokio::test]
async fn revoking_an_active_upload_removes_staging_and_never_commits() {
    let root = tempfile::tempdir().unwrap();
    let host = HostServices::new(HostConfig {
        share_dir: Some(root.path().to_owned()),
        allow_write: true,
        ..HostConfig::default()
    })
    .unwrap();
    let (mut client, service) = tokio::io::duplex(8);
    let worker_host = host.clone();
    let worker = tokio::spawn(async move {
        worker_host
            .serve(
                "file.put",
                json!({"path":"never-commit","size":100}),
                service,
            )
            .await
    });
    client.write_all(b"partial").await.unwrap();
    // Observe the real staging file before revocation, without a timing sleep.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if std::fs::read_dir(root.path()).unwrap().next().is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    host.permissions().revoke(&[Scope::FileWrite]).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(!root.path().join("never-commit").exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn control_notifications_report_local_denial_and_revocation() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let host = HostServices::with_approval(
        HostConfig {
            allow_shell: true,
            ..HostConfig::default()
        },
        ApprovalMode::Attended,
    )
    .unwrap();
    let (mut client, service) = tokio::io::duplex(4096);
    let worker_host = host.clone();
    let worker =
        tokio::spawn(async move { worker_host.serve("control", json!({}), service).await });
    let body = serde_json::to_vec(
        &json!({"id":7,"method":"permissions.request","params":{"scopes":["shell"]}}),
    )
    .unwrap();
    client.write_u32(body.len() as u32).await.unwrap();
    client.write_all(&body).await.unwrap();
    async fn read(client: &mut tokio::io::DuplexStream) -> Value {
        let len = client.read_u32().await.unwrap();
        assert!(len <= 65536);
        let mut body = vec![0; len as usize];
        client.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }
    let id = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let response = read(&mut client).await;
            if response["id"] == 7 {
                break response["result"]["pending"]["id"].as_u64().unwrap() as u32;
            }
        }
    })
    .await
    .unwrap();
    host.permissions().deny(id).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let message = read(&mut client).await;
            if message["event"] == "permissions.changed"
                && message["data"]["last_decision"]["outcome"] == "denied"
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    client.shutdown().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
