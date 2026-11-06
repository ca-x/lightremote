use anyhow::{Context, Result, ensure};
use lightremote_core::{HostTicket, Room, crypto::Role, link};
use lightremote_services::{HostConfig, HostServices};
use serde_json::json;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn reset_after_exact_size_without_fin_never_commits_upload() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        let share = tempfile::tempdir()?;
        let services = HostServices::new(HostConfig {
            share_dir: Some(share.path().into()),
            allow_write: true,
            allow_shell: true,
            shell: Some("/bin/sh".into()),
            ..Default::default()
        })?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let relay = lightremote_relay::router(
            lightremote_relay::Config::new("test-operator".into()).unwrap(),
        )
        .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, relay).await });
        let room: Room = reqwest::Client::new()
            .post(format!("{base}/v1/rooms"))
            .bearer_auth("test-operator")
            .json(&json!({}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let ticket = HostTicket::new(&base, room)?;
        let (host, client) = tokio::join!(
            link::connect(&ticket.invite, Role::Host, &ticket.host_token),
            link::connect(&ticket.invite, Role::Client, &ticket.invite.client_token)
        );
        let mut host = host?;
        let client = client?;
        for size in [0usize, 128 * 1024] {
            let path = format!("cancelled-{size}");
            let mut stream = client
                .handle
                .open("file.put", json!({"path":path,"size":size}))
                .await?;
            let incoming = host.incoming.recv().await.context("incoming upload")?;
            let service = services.clone();
            let worker =
                tokio::spawn(async move { lightremote::serve_stream(service, incoming).await });
            stream.io.write_all(&vec![7; size]).await?;
            // Wait for the exact payload in staging; FIN is deliberately withheld.
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if std::fs::read_dir(share.path())?
                        .filter_map(|e| e.ok())
                        .any(|e| {
                            e.file_name()
                                .to_string_lossy()
                                .starts_with(".lightremote-upload-")
                                && e.metadata().is_ok_and(|m| m.len() == size as u64)
                        })
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await??;
            client.handle.reset(stream.id).await?;
            ensure!(
                tokio::time::timeout(Duration::from_secs(3), worker)
                    .await??
                    .is_err(),
                "RESET incorrectly became upload FIN"
            );
            ensure!(
                !share.path().join(path).exists(),
                "cancelled upload committed"
            );
            ensure!(
                std::fs::read_dir(share.path())?.next().is_none(),
                "upload staging leaked"
            );
        }
        // A reset is scoped: the same encrypted link can accept another stream.
        let mut stream = client
            .handle
            .open("file.put", json!({"path":"valid","size":0}))
            .await?;
        let incoming = host.incoming.recv().await.context("valid upload")?;
        let service = services.clone();
        let worker =
            tokio::spawn(async move { lightremote::serve_stream(service, incoming).await });
        stream.io.shutdown().await?;
        let receipt = lightremote_core::records::read(&mut stream.io, 1024)
            .await?
            .context("valid FIN receipt")?;
        ensure!(
            serde_json::from_slice::<serde_json::Value>(&receipt)?["size"] == 0,
            "invalid receipt"
        );
        worker.await??;
        ensure!(share.path().join("valid").exists(), "valid upload failed");
        #[cfg(target_os = "linux")]
        {
            use tokio::io::AsyncReadExt;
            let mut shell = client
                .handle
                .open("shell", json!({"cols":80,"rows":24}))
                .await?;
            let incoming = host.incoming.recv().await.context("shell open")?;
            let worker = tokio::spawn(lightremote::serve_stream(services.clone(), incoming));
            shell
                .io
                .write_all(b"printf 'LR_PID:%s\\n' \"$$\"; sleep 30\n")
                .await?;
            let pid = tokio::time::timeout(Duration::from_secs(3), async {
                let mut text = String::new();
                let mut bytes = [0u8; 1024];
                loop {
                    let n = shell.io.read(&mut bytes).await?;
                    ensure!(n > 0, "shell closed before pid");
                    text.push_str(&String::from_utf8_lossy(&bytes[..n]));
                    for line in text.lines() {
                        if let Some(index) = line.find("LR_PID:")
                            && let Ok(pid) = line[index + 7..].trim().parse::<u32>()
                        {
                            return Ok::<_, anyhow::Error>(pid);
                        }
                    }
                    ensure!(text.len() < 8192, "PID response too long");
                }
            })
            .await??;
            shell.io.shutdown().await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
            ensure!(
                !worker.is_finished(),
                "normal FIN prematurely killed shell output"
            );
            client.handle.reset(shell.id).await?;
            ensure!(
                tokio::time::timeout(Duration::from_secs(2), worker)
                    .await??
                    .is_err(),
                "RESET after FIN did not cancel service"
            );
            tokio::time::timeout(Duration::from_secs(2), async {
                while std::path::Path::new(&format!("/proc/{pid}")).exists() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .context("shell child survived FIN then RESET")?;
        }
        drop(client);
        drop(host);
        server.abort();
        Ok::<_, anyhow::Error>(())
    })
    .await?
}
