use roc_remote_core::{
    crypto::Role,
    invite::{HostTicket, Room},
    link,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn encrypted_mux_large_bidirectional_half_close() -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let relay = roc_remote_relay::router(
            roc_remote_relay::Config::new("test-operator".into()).unwrap(),
        )
        .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, relay).await });
        let base = format!("http://{address}");
        let room: Room = reqwest::Client::new()
            .post(format!("{base}/v1/rooms"))
            .bearer_auth("test-operator")
            .json(&serde_json::json!({}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let host = HostTicket::new(&base, room)?;
        let (h, c) = tokio::join!(
            link::connect(&host.invite, Role::Host, &host.host_token),
            link::connect(&host.invite, Role::Client, &host.invite.client_token)
        );
        let mut host = h?;
        let client = c?;
        let (done, complete) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(async move {
            let incoming = host.incoming.recv().await.unwrap();
            assert_eq!(incoming.route, "file.put");
            let mut io = incoming.stream.io;
            let mut payload = Vec::new();
            io.read_to_end(&mut payload).await?;
            assert_eq!(payload, vec![17; 1024 * 1024]);
            // Request FIN must permit a response on the opposite direction.
            io.write_all(b"committed").await?;
            io.shutdown().await?;
            incoming.completion.finish(true);
            // Keep connection alive until receipt consumed.
            complete.await?;
            Ok::<_, anyhow::Error>(())
        });
        let mut stream = client
            .handle
            .open("file.put", serde_json::json!({"path":"x","size":1048576}))
            .await?;
        stream.io.write_all(&vec![17; 1024 * 1024]).await?;
        stream.io.shutdown().await?;
        let mut receipt = Vec::new();
        stream.io.read_to_end(&mut receipt).await?;
        assert_eq!(receipt, b"committed");
        stream.completed().await?;
        let _ = done.send(());
        peer.await??;
        drop(client);
        server.abort();
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}
