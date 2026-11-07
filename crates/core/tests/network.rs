use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use lightremote_core::{
    HostTicket, Invite, Room,
    crypto::{Cipher, Hello, Role},
    link::{self, LinkOptions, LinkPhase},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{io::AsyncReadExt, net::TcpListener, task::JoinHandle};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
    },
};
use tunnel_mux::{Event, Session, default_settings, framing};

async fn listener() -> Result<(TcpListener, Invite)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let ticket = HostTicket::new(
        &format!("http://{}", listener.local_addr()?),
        Room {
            room_id: "network_test".into(),
            host_token: "host_ticket".into(),
            client_token: "client_ticket".into(),
            expires_at: u64::MAX,
        },
    )?;
    Ok((listener, ticket.invite))
}
#[expect(
    clippy::result_large_err,
    reason = "tungstenite requires ErrorResponse by value in its handshake callback"
)]
fn protocol(
    _: &Request,
    mut response: Response,
) -> std::result::Result<Response, tokio_tungstenite::tungstenite::handshake::server::ErrorResponse>
{
    response
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", "lightremote.v1".parse().unwrap());
    Ok(response)
}
async fn peer(answer_pings: bool) -> Result<(Invite, Arc<AtomicUsize>, JoinHandle<Result<()>>)> {
    let (listener, invite) = listener().await?;
    let host = invite.clone();
    let probes = Arc::new(AtomicUsize::new(0));
    let count = probes.clone();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = accept_hdr_async(stream, protocol).await?;
        let message = socket.next().await.context("client hello")??;
        let client = Hello::parse(&message.into_data(), Role::Client)?;
        let hello = Hello::new(Role::Host);
        socket
            .send(Message::Binary(serde_json::to_vec(&hello)?.into()))
            .await?;
        let key = host.key()?;
        let mut tx = Cipher::derive(&key, &host.room_id, &client, &hello, Role::Host)?;
        let mut rx = Cipher::derive(&key, &host.room_id, &client, &hello, Role::Client)?;
        let mut session = Session::new(
            tunnel_mux::Role::Responder,
            default_settings(),
            lightremote_core::PROFILES
                .iter()
                .map(|profile| profile.to_string())
                .collect(),
        )?;
        let mut decoder = framing::Decoder::default();
        while let Some(message) = socket.next().await {
            let Ok(Message::Binary(bytes)) = message else {
                break;
            };
            let mut packets = Vec::new();
            decoder.feed(&rx.open(&bytes)?, |packet| {
                packets.push(packet);
                Ok(())
            })?;
            for packet in packets {
                let ping = matches!(packet.body, Some(tunnel_mux::pb::packet::Body::Ping(_)));
                if ping {
                    count.fetch_add(1, Ordering::Relaxed);
                }
                let outcome = session.receive(packet)?;
                let mut output = if ping && !answer_pings {
                    Vec::new()
                } else {
                    outcome.outbound
                };
                for event in outcome.events {
                    if let Event::Open(id, _) = event {
                        output.push(session.accept(id)?);
                    }
                }
                for packet in output {
                    socket
                        .send(Message::Binary(tx.seal(&framing::encode(&packet)?)?.into()))
                        .await?;
                }
            }
        }
        Ok(())
    });
    Ok((invite, probes, task))
}
fn options() -> LinkOptions {
    let mut options = LinkOptions::default();
    options.connect_timeout = Duration::from_secs(2);
    options.heartbeat_interval = Duration::from_millis(20);
    options.heartbeat_timeout = Duration::from_millis(150);
    options.degraded_after = Duration::from_millis(30);
    options
}

#[tokio::test]
async fn blackhole_fails_active_stream_and_publishes_terminal_health() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        let (invite, probes, peer) = peer(false).await?;
        let mut connection =
            link::connect_with_options(&invite, Role::Client, &invite.client_token, options())
                .await?;
        let mut stream = connection
            .handle
            .open("shell", serde_json::json!({}))
            .await?;
        let mut health = connection.handle.health();
        while health.borrow().phase != LinkPhase::Degraded {
            health.changed().await?;
        }
        let error = (&mut connection.task).await?.unwrap_err();
        assert!(error.to_string().contains("heartbeat timeout"));
        assert_eq!(health.borrow().phase, LinkPhase::Closed);
        assert!(
            stream.read(&mut [0; 1]).await.is_err(),
            "blackhole became successful EOF"
        );
        assert_eq!(
            probes.load(Ordering::Relaxed),
            1,
            "unanswered probe was replaced"
        );
        peer.await??;
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

#[tokio::test]
async fn live_pongs_allow_repeated_probes_and_abort_publishes_closed() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        let (invite, probes, peer) = peer(true).await?;
        let connection =
            link::connect_with_options(&invite, Role::Client, &invite.client_token, options())
                .await?;
        let mut health = connection.handle.health();
        while probes.load(Ordering::Relaxed) < 2 || health.borrow().rtt_ms.is_none() {
            health.changed().await?;
        }
        assert_eq!(health.borrow().phase, LinkPhase::Healthy);
        drop(connection);
        while health.borrow().phase != LinkPhase::Closed {
            health.changed().await?;
        }
        peer.await??;
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

#[tokio::test]
async fn negotiation_has_one_total_budget_and_never_retries_consumed_invite() -> Result<()> {
    let (listener, invite) = listener().await?;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = accept_hdr_async(stream, protocol).await?;
        assert!(matches!(socket.next().await, Some(Ok(Message::Binary(_)))));
        // No host hello: carrier remains open but negotiation cannot progress.
        let _ = socket.next().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err(),
            "invitation was retried"
        );
        Ok::<_, anyhow::Error>(())
    });
    let mut budgets = options();
    budgets.connect_timeout = Duration::from_millis(40);
    let error = link::connect_with_options(&invite, Role::Client, &invite.client_token, budgets)
        .await
        .err()
        .context("negotiation succeeded")?;
    assert!(
        error.to_string().contains("budget") || error.to_string().contains("handshake timeout")
    );
    tokio::time::timeout(Duration::from_secs(2), server).await???;
    Ok(())
}

#[tokio::test]
async fn cancelling_tmp_negotiation_drops_the_spawned_carrier_workers() -> Result<()> {
    let (listener, invite) = listener().await?;
    let host = invite.clone();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = accept_hdr_async(stream, protocol).await?;
        let message = socket.next().await.context("client hello")??;
        let client = Hello::parse(&message.into_data(), Role::Client)?;
        let hello = Hello::new(Role::Host);
        socket
            .send(Message::Binary(serde_json::to_vec(&hello)?.into()))
            .await?;
        let key = host.key()?;
        let mut rx = Cipher::derive(&key, &host.room_id, &client, &hello, Role::Client)?;
        let message = socket.next().await.context("TMP hello")??;
        assert!(!rx.open(&message.into_data())?.is_empty());
        let _ = started.send(());
        // Withhold WELCOME so connect owns a live runtime while still pending.
        assert!(
            !matches!(socket.next().await, Some(Ok(Message::Binary(_)))),
            "cancelled negotiation sent application work"
        );
        Ok::<_, anyhow::Error>(())
    });
    let connector = tokio::spawn(async move {
        link::connect_with_options(&invite, Role::Client, &invite.client_token, options()).await
    });
    tokio::time::timeout(Duration::from_secs(2), waiting).await??;
    connector.abort();
    assert!(matches!(connector.await, Err(error) if error.is_cancelled()));
    tokio::time::timeout(Duration::from_secs(1), server).await???;
    Ok(())
}
