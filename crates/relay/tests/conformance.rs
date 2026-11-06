use futures_util::{SinkExt, StreamExt};
use lightremote_relay::{Config, MAX_RECORD, QUEUE_RECORDS, router};
use serde_json::Value;
use std::time::Duration;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderValue, StatusCode},
    },
};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
struct Server {
    http: String,
    ws: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(max_rooms: usize, ttl: Duration, wait: Duration) -> Server {
    let mut c = Config::new("test-operator-secret".into()).unwrap();
    c.max_rooms = max_rooms;
    c.room_ttl = ttl;
    c.wait_timeout = wait;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(c).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server {
        http: format!("http://{addr}"),
        ws: format!("ws://{addr}"),
        task,
    }
}
async fn normal() -> Server {
    server(16, Duration::from_secs(60), Duration::from_secs(5)).await
}
async fn room(s: &Server) -> Value {
    reqwest::Client::new()
        .post(format!("{}/v1/rooms", s.http))
        .bearer_auth("test-operator-secret")
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}
fn request(
    s: &Server,
    r: &Value,
    role: &str,
    token: &str,
) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut req = format!("{}/v1/rooms/{}/ws", s.ws, r["room_id"].as_str().unwrap())
        .into_client_request()
        .unwrap();
    req.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("lightremote.v1, {role}.{token}")).unwrap(),
    );
    req
}
async fn join(s: &Server, r: &Value, role: &str) -> Socket {
    let (socket, response) = connect_async(request(
        s,
        r,
        role,
        r[format!("{role}_token")].as_str().unwrap(),
    ))
    .await
    .unwrap();
    assert_eq!(
        response.headers()["Sec-WebSocket-Protocol"],
        "lightremote.v1"
    );
    socket
}
async fn rejected(req: tokio_tungstenite::tungstenite::http::Request<()>, status: StatusCode) {
    match connect_async(req).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status(), status),
        _ => panic!("expected HTTP rejection"),
    }
}
async fn closed(socket: &mut Socket) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match socket.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                _ => {}
            }
        }
    })
    .await
    .expect("socket should close");
}
#[tokio::test]
async fn operator_auth_health_quota_and_origin() {
    assert!(Config::new("".into()).is_err());
    let s = server(1, Duration::from_secs(60), Duration::from_secs(5)).await;
    let client = reqwest::Client::new();
    let health: Value = client
        .get(format!("{}/healthz", s.http))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        health,
        serde_json::json!({"ok":true,"protocol":"lightremote-relay.v1"})
    );
    for token in [None, Some("wrong")] {
        let mut req = client
            .post(format!("{}/v1/rooms", s.http))
            .json(&serde_json::json!({}));
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        assert_eq!(req.send().await.unwrap().status(), 401);
    }
    assert_eq!(
        client
            .post(format!("{}/v1/rooms", s.http))
            .bearer_auth("test-operator-secret")
            .header("Origin", "https://evil.example")
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let r = room(&s).await;
    assert_eq!(r["host_token"].as_str().unwrap().len(), 43);
    assert_ne!(r["host_token"], r["client_token"]);
    assert_eq!(
        client
            .post(format!("{}/v1/rooms", s.http))
            .bearer_auth("test-operator-secret")
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        429
    );
}
#[tokio::test]
async fn tickets_duplicate_and_bidirectional_binary_forwarding() {
    let s = normal().await;
    let r = room(&s).await;
    rejected(
        request(&s, &r, "host", r["client_token"].as_str().unwrap()),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let mut req = request(&s, &r, "host", r["host_token"].as_str().unwrap());
    *req.uri_mut() = format!("{}?token=never", req.uri()).parse().unwrap();
    rejected(req, StatusCode::BAD_REQUEST).await;
    let mut host = join(&s, &r, "host").await;
    rejected(
        request(&s, &r, "host", r["host_token"].as_str().unwrap()),
        StatusCode::CONFLICT,
    )
    .await;
    host.send(Message::Binary(
        b"binary hello before client".to_vec().into(),
    ))
    .await
    .unwrap();
    let mut peer = join(&s, &r, "client").await;
    assert_eq!(
        peer.next().await.unwrap().unwrap().into_data(),
        b"binary hello before client"[..]
    );
    peer.send(Message::Binary(vec![1; MAX_RECORD].into()))
        .await
        .unwrap();
    assert_eq!(
        host.next().await.unwrap().unwrap().into_data().len(),
        MAX_RECORD
    );
    host.close(None).await.unwrap();
    closed(&mut peer).await;
    rejected(
        request(&s, &r, "client", r["client_token"].as_str().unwrap()),
        StatusCode::NOT_FOUND,
    )
    .await;
}
#[tokio::test]
async fn oversize_and_text_consume_both_peers() {
    for message in [
        Message::Binary(vec![0; MAX_RECORD + 1].into()),
        Message::Text("hello".into()),
    ] {
        let s = normal().await;
        let r = room(&s).await;
        let mut host = join(&s, &r, "host").await;
        let mut peer = join(&s, &r, "client").await;
        host.send(message).await.unwrap();
        closed(&mut host).await;
        closed(&mut peer).await;
        rejected(
            request(&s, &r, "host", r["host_token"].as_str().unwrap()),
            StatusCode::NOT_FOUND,
        )
        .await;
    }
}
#[tokio::test]
async fn pre_peer_overflow_consumes_room() {
    let s = normal().await;
    let r = room(&s).await;
    let mut host = join(&s, &r, "host").await;
    for _ in 0..=QUEUE_RECORDS {
        host.send(Message::Binary(vec![42].into())).await.unwrap();
    }
    closed(&mut host).await;
    rejected(
        request(&s, &r, "client", r["client_token"].as_str().unwrap()),
        StatusCode::NOT_FOUND,
    )
    .await;
}
#[tokio::test]
async fn expiration_and_wait_timeout_close_idle_sockets() {
    for (ttl, wait) in [
        (Duration::from_secs(1), Duration::from_secs(10)),
        (Duration::from_secs(60), Duration::from_millis(50)),
    ] {
        let s = server(1, ttl, wait).await;
        let r = room(&s).await;
        let mut host = join(&s, &r, "host").await;
        closed(&mut host).await;
        rejected(
            request(&s, &r, "client", r["client_token"].as_str().unwrap()),
            StatusCode::NOT_FOUND,
        )
        .await;
        room(&s).await; // Timers recover room capacity without incoming traffic.
    }
}
#[tokio::test]
async fn static_client_and_explicit_origin_allowlist() {
    let dir = std::env::temp_dir().join(format!(
        "lightremote-static-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<title>LightRemote fixture</title>").unwrap();
    let mut config = Config::new("operator".into()).unwrap();
    config.web_dir = Some(dir.clone());
    config.allowed_origins = vec!["https://client.example".into()];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router(config).unwrap())
            .await
            .unwrap();
    });
    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{addr}/"))
        .header("Origin", "https://client.example")
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.headers()["Access-Control-Allow-Origin"],
        "https://client.example"
    );
    assert_eq!(
        response.text().await.unwrap(),
        "<title>LightRemote fixture</title>"
    );
    assert_eq!(
        client
            .get(format!("http://{addr}/"))
            .header("Origin", "http://localhost:5173")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    task.abort();
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn malformed_protocol_offers_never_accept_tickets_elsewhere() {
    let s = normal().await;
    let r = room(&s).await;
    for protocol in [
        String::new(),
        "lightremote.v1".into(),
        format!(
            "lightremote.v1, host.{}, client.{}",
            r["host_token"].as_str().unwrap(),
            r["client_token"].as_str().unwrap()
        ),
        format!("other, host.{}", r["host_token"].as_str().unwrap()),
    ] {
        let mut req = request(&s, &r, "host", r["host_token"].as_str().unwrap());
        req.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_str(&protocol).unwrap(),
        );
        req.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {}", r["host_token"].as_str().unwrap()))
                .unwrap(),
        );
        rejected(req, StatusCode::UNAUTHORIZED).await;
    }
}

async fn paired_bidirectional_burst() {
    let s = normal().await;
    let r = room(&s).await;
    let mut host = join(&s, &r, "host").await;
    let mut peer = join(&s, &r, "client").await;
    for socket in [&mut host, &mut peer] {
        if let tokio_tungstenite::MaybeTlsStream::Plain(tcp) = socket.get_mut() {
            tcp.set_nodelay(true).unwrap();
        }
    }
    let (mut host_tx, mut host_rx) = host.split();
    let (mut peer_tx, mut peer_rx) = peer.split();
    let send_host = async {
        for sequence in 0_u32..512 {
            let mut body = vec![19; 1024];
            body[..4].copy_from_slice(&sequence.to_be_bytes());
            host_tx.send(Message::Binary(body.into())).await.unwrap();
        }
    };
    let send_peer = async {
        for sequence in 0_u32..512 {
            let mut body = vec![23; 1024];
            body[..4].copy_from_slice(&sequence.to_be_bytes());
            peer_tx.send(Message::Binary(body.into())).await.unwrap();
        }
    };
    let receive_host = async {
        for sequence in 0_u32..512 {
            let message = host_rx.next().await.unwrap().unwrap();
            let Message::Binary(body) = message else {
                panic!("host closed during burst: {message:?}");
            };
            assert_eq!(body.len(), 1024);
            assert_eq!(&body[..4], &sequence.to_be_bytes());
            assert!(body[4..].iter().all(|b| *b == 23));
        }
    };
    let receive_peer = async {
        for sequence in 0_u32..512 {
            let message = peer_rx.next().await.unwrap().unwrap();
            let Message::Binary(body) = message else {
                panic!("peer closed during burst: {message:?}");
            };
            assert_eq!(body.len(), 1024);
            assert_eq!(&body[..4], &sequence.to_be_bytes());
            assert!(body[4..].iter().all(|b| *b == 19));
        }
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(send_host, send_peer, receive_host, receive_peer);
    })
    .await
    .expect("paired burst must make progress in both directions");
    // The room remains reserved until an endpoint disconnects.
    rejected(
        request(&s, &r, "host", r["host_token"].as_str().unwrap()),
        StatusCode::CONFLICT,
    )
    .await;
    peer_tx.close().await.unwrap();
}

#[tokio::test]
async fn paired_bidirectional_bursts_apply_backpressure_without_consuming_room() {
    paired_bidirectional_burst().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paired_bidirectional_bursts_make_progress_across_workers() {
    paired_bidirectional_burst().await;
}

#[tokio::test]
async fn paired_stalled_reader_expires_instead_of_buffering_indefinitely() {
    let s = normal().await;
    let r = room(&s).await;
    let host = join(&s, &r, "host").await;
    let mut peer = join(&s, &r, "client").await;
    let (mut tx, mut rx) = host.split();
    let started = tokio::time::Instant::now();
    let sending = tokio::spawn(async move {
        for _ in 0..512 {
            if tx
                .send(Message::Binary(vec![7; MAX_RECORD].into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    // The peer deliberately never polls its socket, eventually filling the TCP
    // receive buffer. The relay must propagate pressure and enforce its deadline.
    let ended = tokio::time::timeout(Duration::from_secs(8), rx.next()).await;
    sending.abort();
    let ended = ended.expect("stalled paired destination must hit its five-second deadline");
    assert!(matches!(
        ended,
        None | Some(Err(_)) | Some(Ok(Message::Close(_)))
    ));
    assert!(
        started.elapsed() >= Duration::from_secs(4),
        "paired queue saturation must not immediately consume the room"
    );
    closed(&mut peer).await;
    rejected(
        request(&s, &r, "host", r["host_token"].as_str().unwrap()),
        StatusCode::NOT_FOUND,
    )
    .await;
}
