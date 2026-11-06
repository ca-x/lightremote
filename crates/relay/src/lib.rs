//! Authenticated, single-use, bounded WebSocket rooms. No payloads or secrets are logged.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{SinkExt, StreamExt};
use rand::{RngCore, rngs::OsRng};
use serde::Serialize;
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, watch};
use url::Url;

/// Maximum binary record size, including the encrypted carrier overhead.
pub const MAX_RECORD: usize = 150_000;
/// Maximum records buffered per destination, including before its socket connects.
pub const QUEUE_RECORDS: usize = 8;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Explicit server limits. Construct through [`Config::new`] or [`Config::from_env`].
#[derive(Clone)]
pub struct Config {
    operator_token: String,
    pub max_rooms: usize,
    pub room_ttl: Duration,
    pub wait_timeout: Duration,
    pub allowed_origins: Vec<String>,
    pub public_origin: Option<String>,
    pub web_dir: Option<std::path::PathBuf>,
}
impl Config {
    pub fn new(operator_token: String) -> Result<Self, String> {
        if operator_token.trim().is_empty() {
            return Err("LIGHTREMOTE_RELAY_TOKEN is mandatory".into());
        }
        Ok(Self {
            operator_token,
            max_rooms: 128,
            room_ttl: Duration::from_secs(3600),
            wait_timeout: Duration::from_secs(30),
            allowed_origins: vec![],
            public_origin: None,
            web_dir: None,
        })
    }
    pub fn from_env() -> Result<Self, String> {
        let mut c = Self::new(std::env::var("LIGHTREMOTE_RELAY_TOKEN").unwrap_or_default())?;
        c.max_rooms = env_number("LIGHTREMOTE_MAX_ROOMS", 128)? as usize;
        c.room_ttl = Duration::from_secs(env_number("LIGHTREMOTE_ROOM_TTL_SECONDS", 3600)?);
        c.wait_timeout = Duration::from_secs(env_number("LIGHTREMOTE_WAIT_SECONDS", 30)?);
        c.allowed_origins = std::env::var("LIGHTREMOTE_ALLOWED_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        c.public_origin = std::env::var("LIGHTREMOTE_PUBLIC_ORIGIN").ok();
        c.web_dir = std::env::var_os("LIGHTREMOTE_WEB_DIR").map(Into::into);
        c.validate()?;
        Ok(c)
    }
    fn validate(&self) -> Result<(), String> {
        if !(1..=4096).contains(&self.max_rooms)
            || self.room_ttl.is_zero()
            || self.room_ttl > Duration::from_secs(86400)
            || self.wait_timeout.is_zero()
            || self.wait_timeout > Duration::from_secs(86400)
        {
            return Err("invalid room limits (rooms 1..4096, TTL/wait 1..86400 seconds)".into());
        }
        for origin in self.allowed_origins.iter().chain(self.public_origin.iter()) {
            normalize_origin(origin)
                .ok_or("origins must be exact http(s) origins without paths")?;
        }
        Ok(())
    }
}
fn env_number(key: &str, default: u64) -> Result<u64, String> {
    match std::env::var(key) {
        Ok(v) => v.parse().map_err(|_| format!("invalid {key}")),
        Err(_) => Ok(default),
    }
}
fn normalize_origin(origin: &str) -> Option<String> {
    let u = Url::parse(origin).ok()?;
    if !matches!(u.scheme(), "http" | "https")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.path() != "/"
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return None;
    }
    Some(u.origin().ascii_serialization())
}
fn secret_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}
fn random_token() -> String {
    let mut bytes = [0; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

struct Room {
    tickets: [String; 2],
    joined: [bool; 2],
    senders: [mpsc::Sender<Message>; 2],
    receivers: [Option<mpsc::Receiver<Message>>; 2],
    cancelled: watch::Sender<bool>,
    expires_at: u64,
}
#[derive(Clone)]
struct Relay {
    config: Arc<Config>,
    rooms: Arc<Mutex<HashMap<String, Room>>>,
}
impl Relay {
    fn consume(&self, id: &str) {
        if let Some(room) = self.rooms.lock().expect("room lock poisoned").remove(id) {
            let _ = room.cancelled.send(true);
        }
    }
}
/// Construct the HTTP router, rejecting insecure or invalid configuration.
pub fn router(config: Config) -> Result<Router, String> {
    config.validate()?;
    let state = Relay {
        config: Arc::new(config),
        rooms: Arc::new(Mutex::new(HashMap::new())),
    };
    let mut app = Router::new()
        .route(
            "/healthz",
            get(|| async {
                Json(serde_json::json!({"ok":true,"protocol":"lightremote-relay.v1"}))
            }),
        )
        .route("/v1/rooms", post(create_room))
        .route("/v1/rooms/{room}/ws", get(join_room))
        .layer(DefaultBodyLimit::max(1024))
        .with_state(state.clone());
    if let Some(dir) = &state.config.web_dir {
        app = app.fallback_service(tower_http::services::ServeDir::new(dir));
    }
    Ok(app.layer(middleware::from_fn_with_state(state, origin_policy)))
}
async fn origin_policy(
    State(state): State<Relay>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let origin = request.headers().get(header::ORIGIN).cloned();
    if let Some(origin) = &origin {
        let allowed = origin
            .to_str()
            .ok()
            .is_some_and(|o| origin_allowed(o, request.uri(), request.headers(), &state.config));
        if !allowed {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let mut response = if request.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(origin) = origin {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Authorization, Content-Type"),
        );
    }
    response
}
fn origin_allowed(origin: &str, uri: &Uri, headers: &HeaderMap, config: &Config) -> bool {
    let Some(normalized) = normalize_origin(origin) else {
        return false;
    };
    if !config.allowed_origins.is_empty() {
        return config
            .allowed_origins
            .iter()
            .any(|o| normalize_origin(o).as_deref() == Some(&normalized));
    }
    let parsed = Url::parse(&normalized).expect("normalized URL");
    if matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        return true;
    }
    let expected = config.public_origin.clone().or_else(|| {
        let host = headers.get(header::HOST)?.to_str().ok()?;
        normalize_origin(&format!("{}://{host}", uri.scheme_str().unwrap_or("http")))
    });
    expected.and_then(|o| normalize_origin(&o)).as_deref() == Some(&normalized)
}
#[derive(Serialize)]
struct CreatedRoom {
    room_id: String,
    host_token: String,
    client_token: String,
    expires_at: u64,
}
async fn create_room(
    State(state): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<CreatedRoom>, StatusCode> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    if !secret_eq(bearer, &state.config.operator_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !body.as_object().is_some_and(|b| b.is_empty()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let id = random_token();
    let tickets = [random_token(), random_token()];
    let expires_at = now() + state.config.room_ttl.as_secs();
    let (host_tx, host_rx) = mpsc::channel(QUEUE_RECORDS);
    let (client_tx, client_rx) = mpsc::channel(QUEUE_RECORDS);
    let (cancelled, mut expiry_cancel) = watch::channel(false);
    {
        let mut rooms = state.rooms.lock().expect("room lock poisoned");
        if rooms.len() >= state.config.max_rooms {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        rooms.insert(
            id.clone(),
            Room {
                tickets: tickets.clone(),
                joined: [false; 2],
                senders: [host_tx, client_tx],
                receivers: [Some(host_rx), Some(client_rx)],
                cancelled,
                expires_at,
            },
        );
    }
    let timer_state = state.clone();
    let timer_id = id.clone();
    tokio::spawn(async move {
        tokio::select! { _ = tokio::time::sleep(timer_state.config.room_ttl) => timer_state.consume(&timer_id), _ = expiry_cancel.changed() => {} }
    });
    Ok(Json(CreatedRoom {
        room_id: id,
        host_token: tickets[0].clone(),
        client_token: tickets[1].clone(),
        expires_at,
    }))
}
fn role_ticket(headers: &HeaderMap) -> Option<(usize, &str)> {
    // Never read credentials from URLs, cookies or the Authorization header here.
    let mut protocol = false;
    let mut ticket = None;
    let mut count = 0;
    for h in headers.get_all(header::SEC_WEBSOCKET_PROTOCOL) {
        for p in h.to_str().ok()?.split(',').map(str::trim) {
            count += 1;
            if p == "lightremote.v1" {
                protocol = true;
            } else if let Some(t) = p.strip_prefix("host.") {
                if ticket.is_some() {
                    return None;
                }
                ticket = Some((0, t));
            } else {
                let t = p.strip_prefix("client.")?;
                if ticket.is_some() {
                    return None;
                }
                ticket = Some((1, t));
            }
        }
    }
    if !protocol || count != 2 {
        return None;
    }
    ticket.filter(|(_, t)| {
        t.len() == 43
            && t.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    })
}
async fn join_room(
    State(state): State<Relay>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    if uri.query().is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let (role, ticket) = role_ticket(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let (rx, cancel) = {
        let mut rooms = state.rooms.lock().expect("room lock poisoned");
        let room = rooms.get_mut(&id).ok_or(StatusCode::NOT_FOUND)?;
        if room.expires_at <= now() {
            return Err(StatusCode::GONE);
        }
        if !secret_eq(ticket, &room.tickets[role]) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        if room.joined[role] {
            return Err(StatusCode::CONFLICT);
        }
        room.joined[role] = true;
        (
            room.receivers[role].take().ok_or(StatusCode::CONFLICT)?,
            room.cancelled.subscribe(),
        )
    };
    let waiting_state = state.clone();
    let waiting_id = id.clone();
    let mut waiting_cancel = cancel.clone();
    tokio::spawn(async move {
        tokio::select! { _ = tokio::time::sleep(waiting_state.config.wait_timeout) => {}, _ = waiting_cancel.changed() => return }
        let waiting = waiting_state
            .rooms
            .lock()
            .expect("room lock poisoned")
            .get(&waiting_id)
            .is_some_and(|r| !r.joined.iter().all(|j| *j));
        if waiting {
            waiting_state.consume(&waiting_id);
        }
    });
    let failed_state = state.clone();
    let failed_id = id.clone();
    Ok(ws
        .protocols(["lightremote.v1"])
        .max_message_size(MAX_RECORD)
        .max_frame_size(MAX_RECORD)
        .max_write_buffer_size(MAX_RECORD * 2)
        .write_buffer_size(0)
        .on_failed_upgrade(move |_| failed_state.consume(&failed_id))
        .on_upgrade(move |socket| relay_socket(state, id, role, socket, rx, cancel)))
}
async fn relay_socket(
    state: Relay,
    id: String,
    role: usize,
    socket: WebSocket,
    mut rx: mpsc::Receiver<Message>,
    mut cancel: watch::Receiver<bool>,
) {
    let peer = state
        .rooms
        .lock()
        .expect("room lock poisoned")
        .get(&id)
        .map(|room| room.senders[1 - role].clone());
    let Some(peer) = peer else {
        return;
    };
    let (mut sink, mut stream) = socket.split();
    // Paired traffic applies TCP backpressure instead of rejecting transient
    // bursts. One pending record bounds what is read while awaiting queue space.
    let mut pending = None;
    let mut pending_deadline = tokio::time::Instant::now();
    loop {
        if *cancel.borrow()
            || (pending.is_some() && tokio::time::Instant::now() >= pending_deadline)
        {
            break;
        }
        // Fair selection keeps sustained output from starving inbound traffic.
        // Cancellation and the pending deadline are also checked on every turn.
        tokio::select! {
            _ = cancel.changed() => break,
            _ = tokio::time::sleep_until(pending_deadline), if pending.is_some() => break,
            permit = peer.reserve(), if pending.is_some() => {
                let Ok(permit) = permit else { break; };
                permit.send(pending.take().expect("pending branch is guarded"));
            }
            // Drain our destination queue while peer.reserve() is pending:
            // both sockets can free each other's queue capacity without deadlock.
            message = rx.recv() => {
                let Some(message) = message else { break; };
                let mut deadline = tokio::time::Instant::now() + WRITE_TIMEOUT;
                if pending.is_some() { deadline = deadline.min(pending_deadline); }
                tokio::select! {
                    biased;
                    _ = cancel.changed() => break,
                    result = tokio::time::timeout_at(deadline, sink.send(message)) => {
                        if !matches!(result, Ok(Ok(()))) { break; }
                    }
                }
            }
            incoming = stream.next(), if pending.is_none() => {
                match incoming {
                    Some(Ok(Message::Binary(bytes))) if bytes.len() <= MAX_RECORD => {
                        let paired = state.rooms.lock().expect("room lock poisoned").get(&id).map(|r| r.joined[1 - role]);
                        match paired {
                            Some(true) => {
                                pending = Some(Message::Binary(bytes));
                                pending_deadline = tokio::time::Instant::now() + WRITE_TIMEOUT;
                            }
                            Some(false) => {
                                // An absent peer cannot drain its queue. Retain
                                // the explicit eight-record waiting-phase cap.
                                if peer.try_send(Message::Binary(bytes)).is_err() { break; }
                            }
                            None => break,
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {},
                    _ => break,
                }
            }
        }
    }
    state.consume(&id);
    let _ = tokio::time::timeout(Duration::from_secs(1), sink.send(Message::Close(None))).await;
}
