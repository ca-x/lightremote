//! Single-owner TMP runtime with independent carrier I/O and bounded per-stream buffers.
use crate::{
    PROFILES,
    crypto::{Cipher, Hello, MAX_WIRE, Role},
    invite::Invite,
};
use anyhow::{Context, Result, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    ops::Bound::{Excluded, Unbounded},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf},
    sync::{mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        Message, client::IntoClientRequest, http::HeaderValue, protocol::WebSocketConfig,
    },
};
use tunnel_mux::{Event, Session, default_settings, framing, pb};
const CHUNK: usize = 16384;
const MAX_STREAMS: u32 = 32;
const IO_CAPACITY: usize = 65536;
const QUEUE: usize = 128;

type Reply<T> = oneshot::Sender<std::result::Result<T, String>>;
#[derive(Clone)]
pub struct LinkHandle {
    commands: mpsc::Sender<Command>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamStatus {
    Active,
    Completed,
    Failed,
}
pub struct MuxStream {
    pub id: u32,
    pub io: DuplexStream,
    status: watch::Receiver<StreamStatus>,
    receive_fin: Arc<AtomicBool>,
}
impl MuxStream {
    /// Observe RESET/carrier failure independently after a normal input FIN.
    pub fn cancellation(&self) -> StreamCancellation {
        StreamCancellation(self.status.clone())
    }
    /// Validate terminal success after reading EOF; a RESET/transport loss is an error.
    pub async fn completed(&mut self) -> Result<()> {
        loop {
            match *self.status.borrow_and_update() {
                StreamStatus::Completed => return Ok(()),
                StreamStatus::Failed => bail!("stream reset or connection lost"),
                StreamStatus::Active => {}
            }
            self.status
                .changed()
                .await
                .context("stream closed without completion")?;
        }
    }
}
/// A host adapter must drop its service future when this completes, including
/// after the request half has cleanly closed while child/output work continues.
pub struct StreamCancellation(watch::Receiver<StreamStatus>);
impl StreamCancellation {
    pub async fn cancelled(&mut self) {
        loop {
            let state = *self.0.borrow_and_update();
            if state == StreamStatus::Failed {
                return;
            }
            if state == StreamStatus::Completed {
                std::future::pending::<()>().await;
            }
            if self.0.changed().await.is_err() {
                return;
            }
        }
    }
}
// Host services consume this error-aware boundary, not the raw duplex field.
// A bridge drop on RESET must never masquerade as the authenticated upload FIN.
impl AsyncRead for MuxStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if *self.status.borrow() == StreamStatus::Failed {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "TMP stream failed",
            )));
        }
        let before = buf.filled().len();
        let requested = buf.remaining();
        let result = Pin::new(&mut self.io).poll_read(cx, buf);
        if matches!(&result, Poll::Ready(Ok(())))
            && requested > 0
            && buf.filled().len() == before
            && !self.receive_fin.load(Ordering::Acquire)
        {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "missing authenticated TMP FIN",
            )));
        }
        result
    }
}
impl AsyncWrite for MuxStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if *self.status.borrow() == StreamStatus::Failed {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "TMP stream failed",
            )));
        }
        Pin::new(&mut self.io).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
pub struct IncomingStream {
    pub route: String,
    pub metadata: Value,
    pub stream: MuxStream,
    pub completion: StreamCompletion,
}
/// Complete only after service work (including upload commit/receipt) has finished.
/// Dropping this value or reporting failure resets the stream rather than successful EOF.
pub struct StreamCompletion(oneshot::Sender<bool>);
impl StreamCompletion {
    pub fn finish(self, success: bool) {
        let _ = self.0.send(success);
    }
}
pub struct Connection {
    pub handle: LinkHandle,
    pub incoming: mpsc::Receiver<IncomingStream>,
    pub task: JoinHandle<Result<()>>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.task.abort();
    }
}
enum Command {
    Open {
        route: String,
        metadata: Value,
        reply: Reply<MuxStream>,
    },
    Send {
        id: u32,
        data: Vec<u8>,
        fin: bool,
        reply: Reply<()>,
    },
    Consume {
        id: u32,
        count: u32,
    },
    Reset(u32),
    Drain,
}
impl LinkHandle {
    /// Cancel one stream without affecting unrelated streams.
    pub async fn reset(&self, id: u32) -> Result<()> {
        self.commands
            .send(Command::Reset(id))
            .await
            .context("connection closed")
    }
    /// Refuse new streams and finish already accepted work before closing the carrier.
    pub async fn drain(&self) -> Result<()> {
        self.commands
            .send(Command::Drain)
            .await
            .context("connection closed")
    }

    pub async fn open(&self, route: &str, metadata: Value) -> Result<MuxStream> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Open {
                route: route.into(),
                metadata,
                reply: tx,
            })
            .await
            .context("connection closed")?;
        let response = tokio::time::timeout(Duration::from_secs(20), rx)
            .await
            .context("stream acceptance timeout")??;
        response.map_err(anyhow::Error::msg)
    }
}
struct LocalStream {
    input: Option<mpsc::Sender<(Vec<u8>, bool)>>,
    opening: Option<(
        Reply<MuxStream>,
        DuplexStream,
        watch::Receiver<StreamStatus>,
    )>,
    pending: Option<(Vec<u8>, bool, Reply<()>)>,
    remote_fin: bool,
    local_fin: bool,
    consumed: u64,
    received: u64,
    tasks: Vec<tokio::task::AbortHandle>,
    status: watch::Sender<StreamStatus>,
    receive_fin: Arc<AtomicBool>,
}
impl Drop for LocalStream {
    fn drop(&mut self) {
        if *self.status.borrow() == StreamStatus::Active {
            self.status.send_replace(StreamStatus::Failed);
        }
        for task in &self.tasks {
            task.abort();
        }
    }
}
#[derive(Debug)]
struct CarrierClosed;
#[derive(Debug)]
struct CarrierWriteFailed(anyhow::Error);
impl std::fmt::Display for CarrierWriteFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "carrier write failed: {}", self.0)
    }
}
impl std::error::Error for CarrierWriteFailed {}
impl std::fmt::Display for CarrierClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("carrier closed")
    }
}
impl std::error::Error for CarrierClosed {}
enum Output {
    Packet { packet: pb::Packet, critical: bool },
    Finish(oneshot::Sender<()>),
}
struct Runtime {
    session: Session,
    commands: mpsc::Sender<Command>,
    streams: BTreeMap<u32, LocalStream>,
    incoming: mpsc::Sender<IncomingStream>,
    output: mpsc::Sender<Output>,
    workers: JoinSet<()>,
    ready: bool,
    draining: bool,
    carrier_closed: bool,
    drain_probe: Option<u64>,
    drain_ack: bool,
    cursor: u32,
    write_pending: Arc<AtomicUsize>,
    heartbeat: Option<(u64, tokio::time::Instant)>,
    last_ping: tokio::time::Instant,
}
impl Runtime {
    fn emit(&self, packets: impl IntoIterator<Item = pb::Packet>) -> Result<()> {
        for packet in packets {
            if self.carrier_closed {
                continue;
            }
            let critical = !matches!(
                &packet.body,
                Some(
                    pb::packet::Body::Window(_)
                        | pb::packet::Body::Ping(_)
                        | pb::packet::Body::Pong(_)
                )
            );
            if critical {
                self.write_pending.fetch_add(1, Ordering::AcqRel);
            }
            if self
                .output
                .try_send(Output::Packet { packet, critical })
                .is_err()
            {
                if critical {
                    self.write_pending.fetch_sub(1, Ordering::AcqRel);
                }
                bail!("carrier send queue exhausted");
            }
        }
        Ok(())
    }
    fn attach(
        &mut self,
        id: u32,
        opening: Option<Reply<MuxStream>>,
        completion: Option<oneshot::Receiver<bool>>,
    ) -> (DuplexStream, watch::Receiver<StreamStatus>) {
        let (status, status_rx) = watch::channel(StreamStatus::Active);
        let (application, bridge) = tokio::io::duplex(IO_CAPACITY);
        let (mut reader, mut writer) = tokio::io::split(bridge);
        let (input, mut rx) = mpsc::channel::<(Vec<u8>, bool)>(4);
        let chunk = self
            .session
            .peer_settings()
            .map(|s| s.max_data.min(s.stream_window) as usize)
            .unwrap_or(CHUNK)
            .min(CHUNK);
        let commands = self.commands.clone();
        let output_task = self.workers.spawn(async move {
            let result: Result<()> = async {
                let mut completion = completion;
                loop {
                    let mut bytes = vec![0; chunk];
                    let n = reader.read(&mut bytes).await?;
                    bytes.truncate(n);
                    if n == 0
                        && let Some(done) = completion.take()
                    {
                        ensure!(done.await.unwrap_or(false), "host service failed");
                    }
                    let (tx, rx) = oneshot::channel();
                    commands
                        .send(Command::Send {
                            id,
                            data: bytes,
                            fin: n == 0,
                            reply: tx,
                        })
                        .await?;
                    rx.await?.map_err(anyhow::Error::msg)?;
                    if n == 0 {
                        break;
                    }
                }
                Ok(())
            }
            .await;
            if result.is_err() {
                let _ = commands.send(Command::Reset(id)).await;
            }
        });
        let commands = self.commands.clone();
        let input_task = self.workers.spawn(async move {
            let result: Result<()> = async {
                while let Some((bytes, fin)) = rx.recv().await {
                    writer.write_all(&bytes).await?;
                    if !bytes.is_empty() {
                        commands
                            .send(Command::Consume {
                                id,
                                count: bytes.len() as u32,
                            })
                            .await?;
                    }
                    if fin {
                        writer.shutdown().await?;
                        break;
                    }
                }
                Ok(())
            }
            .await;
            if result.is_err() {
                let _ = commands.send(Command::Reset(id)).await;
            }
        });
        self.streams.insert(
            id,
            LocalStream {
                input: Some(input),
                opening: None,
                pending: None,
                remote_fin: false,
                local_fin: false,
                consumed: 0,
                received: 0,
                status,
                receive_fin: Arc::new(AtomicBool::new(false)),
                tasks: vec![output_task, input_task],
            },
        );
        // Opening responses retain their application half until OPEN_ACK.
        if let Some(reply) = opening {
            self.streams.get_mut(&id).expect("new stream").opening =
                Some((reply, application, status_rx.clone()));
            // This placeholder is never returned to an application.
            let (placeholder, _) = tokio::io::duplex(1);
            (placeholder, status_rx)
        } else {
            (application, status_rx)
        }
    }
    fn reset(&mut self, id: u32) -> Result<()> {
        if let Some(mut state) = self.streams.remove(&id) {
            if let Some((reply, _, _)) = state.opening.take() {
                let _ = reply.send(Err("stream refused or reset".into()));
            }
            if let Some((_, _, reply)) = state.pending.take() {
                let _ = reply.send(Err("stream reset".into()));
            }
            let packets =
                self.session
                    .reset(id, pb::ErrorCode::Cancelled, "stream closed".into())?;
            self.emit(packets)?;
        }
        Ok(())
    }
    fn retire(&mut self, id: u32) {
        if self
            .streams
            .get(&id)
            .is_some_and(|s| s.local_fin && s.remote_fin && s.consumed == s.received)
            && let Some(state) = self.streams.remove(&id)
        {
            state.status.send_replace(StreamStatus::Completed);
        }
    }
    fn event(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Ready => self.ready = true,
            Event::Open(id, open) => {
                let metadata = serde_json::from_slice(&open.metadata);
                if self.draining
                    || self.streams.len() >= MAX_STREAMS as usize
                    || crate::profile(&open.route).is_err()
                    || crate::profile(&open.route).ok() != Some(open.profile.as_str())
                    || metadata.is_err()
                {
                    let p = self.session.reset(
                        id,
                        pb::ErrorCode::Refused,
                        "unsupported or busy".into(),
                    )?;
                    self.emit(p)?;
                } else {
                    let (done, completion) = oneshot::channel();
                    let (io, status) = self.attach(id, None, Some(completion));
                    let receive_fin = self.streams[&id].receive_fin.clone();
                    if self
                        .incoming
                        .try_send(IncomingStream {
                            route: open.route,
                            metadata: metadata?,
                            stream: MuxStream {
                                id,
                                io,
                                status,
                                receive_fin,
                            },
                            completion: StreamCompletion(done),
                        })
                        .is_err()
                    {
                        self.reset(id)?;
                    } else {
                        let packet = self.session.accept(id)?;
                        self.emit([packet])?;
                    }
                }
            }
            Event::Accepted(id) => {
                if let Some(state) = self.streams.get_mut(&id)
                    && let Some((reply, io, status)) = state.opening.take()
                    && reply
                        .send(Ok(MuxStream {
                            id,
                            io,
                            status,
                            receive_fin: state.receive_fin.clone(),
                        }))
                        .is_err()
                {
                    self.reset(id)?;
                }
            }
            Event::Data(id, bytes, fin) => {
                if let Some(state) = self.streams.get_mut(&id) {
                    state.received += bytes.len() as u64;
                    state.remote_fin = fin;
                    if fin {
                        state.receive_fin.store(true, Ordering::Release);
                    }
                    let result = state
                        .input
                        .as_ref()
                        .context("stream receive half closed")?
                        .try_send((bytes, fin));
                    if fin {
                        state.input.take();
                    }
                    if result.is_err() {
                        self.reset(id)?;
                    } else {
                        self.retire(id);
                    }
                }
            }
            Event::Reset(id, _, _) => {
                if let Some(mut state) = self.streams.remove(&id) {
                    if let Some((reply, _, _)) = state.opening.take() {
                        let _ = reply.send(Err("peer reset stream".into()));
                    }
                    if let Some((_, _, reply)) = state.pending.take() {
                        let _ = reply.send(Err("peer reset stream".into()));
                    }
                }
            }
            Event::GoAway(_) => {
                self.draining = true;
            }
            Event::Pong(token) => {
                if self.drain_probe == Some(token) {
                    self.drain_ack = true;
                }
                if self.heartbeat.is_some_and(|(pending, _)| pending == token) {
                    self.heartbeat = None;
                }
            }
        }
        Ok(())
    }
    fn command(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Open {
                route,
                metadata,
                reply,
            } => {
                let open = (|| -> Result<_> {
                    ensure!(
                        self.ready && !self.draining,
                        "session not ready or draining"
                    );
                    ensure!(self.streams.len() < MAX_STREAMS as usize, "stream limit");
                    let profile = crate::profile(&route)?;
                    let metadata = serde_json::to_vec(&metadata)?;
                    ensure!(metadata.len() <= 4096, "open metadata length");
                    let class = if route == "desktop" {
                        1
                    } else if route.starts_with("file.") {
                        2
                    } else {
                        0
                    };
                    Ok(self.session.open(pb::Open {
                        route,
                        profile: profile.into(),
                        metadata,
                        class,
                    })?)
                })();
                match open {
                    Ok((id, packet)) => {
                        self.attach(id, Some(reply), None);
                        self.emit([packet])?;
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e.to_string()));
                    }
                }
            }
            Command::Send {
                id,
                data,
                fin,
                reply,
            } => {
                if let Some(state) = self.streams.get_mut(&id) {
                    ensure!(state.pending.is_none(), "duplicate pending send");
                    state.pending = Some((data, fin, reply));
                } else {
                    let _ = reply.send(Err("stream closed".into()));
                }
            }
            Command::Consume { id, count } => {
                if let Some(state) = self.streams.get_mut(&id) {
                    state.consumed += count as u64;
                    let p = self.session.consume(id, count)?;
                    self.emit(p)?;
                    self.retire(id);
                }
            }
            Command::Reset(id) => self.reset(id)?,
            Command::Drain => {
                self.draining = true;
                let packets = self
                    .session
                    .goaway(pb::ErrorCode::Cancelled, "session draining".into())?;
                self.emit(packets)?;
            }
        }
        Ok(())
    }
    fn carrier_fully_written(&self) -> bool {
        self.ready
            && self.write_pending.load(Ordering::Acquire) == 0
            && self.streams.values().all(|s| s.local_fin && s.remote_fin)
    }
    fn cancel_unclaimed_opens(&mut self) -> Result<()> {
        let cancelled: Vec<_> = self
            .streams
            .iter()
            .filter_map(|(id, s)| {
                s.opening
                    .as_ref()
                    .filter(|(reply, _, _)| reply.is_closed())
                    .map(|_| *id)
            })
            .collect();
        for id in cancelled {
            self.reset(id)?;
        }
        Ok(())
    }
    fn pump(&mut self) -> Result<()> {
        // One bounded chunk per stream per pass; every WINDOW packet calls pump.
        let ids: Vec<_> = self
            .streams
            .range((Excluded(self.cursor), Unbounded))
            .chain(self.streams.range(..=self.cursor))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let state = self.streams.get_mut(&id).context("stream state")?;
            if let Some((bytes, fin, reply)) = state.pending.take() {
                match self.session.send(id, bytes.clone(), fin) {
                    Ok(packet) => {
                        state.local_fin = fin;
                        self.cursor = id;
                        self.emit([packet])?;
                        let _ = reply.send(Ok(()));
                        self.retire(id);
                    }
                    Err(tunnel_mux::Error::WouldBlock) => {
                        state.pending = Some((bytes, fin, reply));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e.to_string()));
                        self.reset(id)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Connect as host or controller. `ticket` must be the corresponding role ticket.
/// Completion means the encrypted TMP negotiation succeeded; application streams may open.
pub async fn connect(invite: &Invite, role: Role, ticket: &str) -> Result<Connection> {
    connect_inner(invite, role, ticket, default_settings()).await
}
async fn connect_inner(
    invite: &Invite,
    role: Role,
    ticket: &str,
    mut settings: pb::Settings,
) -> Result<Connection> {
    invite.validate()?;
    ensure!(
        ticket.len() <= 128
            && ticket
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "ticket format"
    );
    let mut request = invite.websocket_url()?.as_str().into_client_request()?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("lightremote.v1, {}.{ticket}", role.name()))?,
    );
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_WIRE))
        .max_frame_size(Some(MAX_WIRE))
        .max_write_buffer_size(2 * MAX_WIRE);
    let (mut socket, response) = tokio::time::timeout(
        Duration::from_secs(20),
        connect_async_with_config(request, Some(config), true),
    )
    .await
    .context("relay connection timeout")?
    .map_err(|_| anyhow::anyhow!("relay connection failed"))?;
    ensure!(
        response
            .headers()
            .get("Sec-WebSocket-Protocol")
            .and_then(|s| s.to_str().ok())
            == Some("lightremote.v1"),
        "relay subprotocol"
    );
    let local = Hello::new(role);
    socket
        .send(Message::Binary(serde_json::to_vec(&local)?.into()))
        .await?;
    let remote = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match socket.next().await.context("handshake EOF")?? {
                Message::Binary(bytes) => break Hello::parse(&bytes, role.other()),
                Message::Ping(_) | Message::Pong(_) => continue,
                _ => bail!("invalid handshake record"),
            }
        }
    })
    .await
    .context("peer handshake timeout")??;
    let (client, host) = if role == Role::Client {
        (&local, &remote)
    } else {
        (&remote, &local)
    };
    let secret = invite.key()?;
    let mut encrypt = Cipher::derive(&secret, &invite.room_id, client, host, role)?;
    let mut decrypt = Cipher::derive(&secret, &invite.room_id, client, host, role.other())?;
    let (mut writer, mut reader) = socket.split();
    let (output, mut outgoing) = mpsc::channel::<Output>(QUEUE);
    let (input, mut received) = mpsc::channel::<Result<pb::Packet>>(32);
    let (command_tx, mut commands) = mpsc::channel(QUEUE);
    let (incoming, incoming_rx) = mpsc::channel(8);
    let (ready_tx, ready_rx) = oneshot::channel();
    let handle = LinkHandle {
        commands: command_tx.clone(),
    };
    settings.max_streams = MAX_STREAMS;
    let tmp_role = if role == Role::Client {
        tunnel_mux::Role::Initiator
    } else {
        tunnel_mux::Role::Responder
    };
    let mut runtime = Runtime {
        session: Session::new(
            tmp_role,
            settings,
            PROFILES.iter().map(|s| s.to_string()).collect(),
        )?,
        commands: command_tx,
        streams: BTreeMap::new(),
        incoming,
        output,
        workers: JoinSet::new(),
        ready: false,
        draining: false,
        carrier_closed: false,
        drain_probe: None,
        drain_ack: false,
        cursor: 0,
        write_pending: Arc::new(AtomicUsize::new(0)),
        heartbeat: None,
        last_ping: tokio::time::Instant::now(),
    };
    let task = tokio::spawn(async move {
        let mut io_tasks = JoinSet::new();
        let failures = input.clone();
        let write_pending = runtime.write_pending.clone();
        io_tasks.spawn(async move {
            let result: Result<()> = async {
                while let Some(output) = outgoing.recv().await {
                    let (packet, critical) = match output {
                        Output::Packet { packet, critical } => (packet, critical),
                        Output::Finish(done) => {
                            writer.close().await?;
                            let _ = done.send(());
                            return Ok(());
                        }
                    };
                    let frame = encrypt.seal(&framing::encode(&packet)?)?;
                    tokio::time::timeout(
                        Duration::from_secs(20),
                        writer.send(Message::Binary(frame.into())),
                    )
                    .await
                    .context("carrier write timeout")??;
                    if critical {
                        write_pending.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                Ok(())
            }
            .await;
            if let Err(error) = result {
                let _ = failures.send(Err(CarrierWriteFailed(error).into())).await;
            }
        });
        io_tasks.spawn(async move {
            let result: Result<()> = async {
                let mut decoder = framing::Decoder::default();
                while let Some(message) = reader.next().await {
                    match message? {
                        Message::Binary(bytes) => {
                            let plain = decrypt.open(&bytes)?;
                            let mut packets = VecDeque::new();
                            decoder.feed(&plain, |p| {
                                packets.push_back(p);
                                Ok(())
                            })?;
                            while let Some(p) = packets.pop_front() {
                                input.send(Ok(p)).await?;
                            }
                        }
                        Message::Ping(_) | Message::Pong(_) => {}
                        Message::Close(_) => {
                            decoder.finish()?;
                            return Err(CarrierClosed.into());
                        }
                        _ => bail!("invalid carrier record"),
                    }
                }
                decoder.finish()?;
                Err(CarrierClosed.into())
            }
            .await;
            if let Err(error) = result {
                let _ = input.send(Err(error)).await;
            }
        });
        if role == Role::Client {
            let p = runtime.session.start()?;
            runtime.emit([p])?;
        }
        let mut ready = Some(ready_tx);
        let mut maintenance = tokio::time::interval(Duration::from_secs(1));
        let mut drain_deadline = None;
        let result:Result<()> = async {
            loop {
                tokio::select! {
                    Some(packet)=received.recv() => {
                        match packet {
                            Ok(packet)=>{
                                let outcome=runtime.session.receive(packet)?;
                                runtime.emit(outcome.outbound)?;
                                for event in outcome.events {runtime.event(event)?;}
                            }
                            Err(error) if (error.is::<CarrierClosed>() || error.is::<CarrierWriteFailed>()) && runtime.carrier_fully_written()=>{
                                runtime.carrier_closed=true;runtime.draining=true;
                            }
                            Err(error)=>return Err(error),
                        }
                    }
                    Some(command)=commands.recv() => {runtime.command(command)?;}
                    _=maintenance.tick()=>{
                        runtime.cancel_unclaimed_opens()?;
                        if runtime.ready && !runtime.draining {
                            if let Some((_,started))=runtime.heartbeat {ensure!(started.elapsed()<Duration::from_secs(30),"peer heartbeat timeout");}
                            else if runtime.last_ping.elapsed()>=Duration::from_secs(10) {
                                let token=rand::random();let now=tokio::time::Instant::now();runtime.heartbeat=Some((token,now));runtime.last_ping=now;
                                runtime.emit([runtime.session.ping(token)?])?;
                            }
                        }
                    }
                    outcome=io_tasks.join_next(), if !io_tasks.is_empty() => {outcome.context("carrier worker missing")? .context("carrier worker panic")?;}
                    Some(_)=runtime.workers.join_next(), if !runtime.workers.is_empty() => {},
                }
                runtime.pump()?;
                if runtime.draining {
                    let deadline=drain_deadline.get_or_insert_with(||tokio::time::Instant::now()+Duration::from_secs(60));
                    ensure!(tokio::time::Instant::now()<*deadline,"session drain deadline exceeded");
                }
                if runtime.ready&& let Some(ready)=ready.take(){let _=ready.send(());}
                if runtime.draining && runtime.streams.is_empty() {
                    if runtime.carrier_closed || runtime.drain_ack {break;}
                    if runtime.drain_probe.is_none(){
                        let token=rand::random();runtime.drain_probe=Some(token);
                        runtime.emit([runtime.session.ping(token)?])?;
                    }
                }
            }
            Ok(())
        }.await;
        #[cfg(test)]
        if let Err(error) = &result {
            eprintln!("test actor {} failed: {error:#}", role.name());
        }
        if result.is_ok() && !runtime.carrier_closed {
            let (done, flushed) = oneshot::channel();
            runtime
                .output
                .try_send(Output::Finish(done))
                .map_err(|_| anyhow::anyhow!("cannot flush carrier"))?;
            tokio::time::timeout(Duration::from_secs(20), flushed)
                .await
                .context("carrier flush timeout")??;
        }
        runtime.session.close();
        result
    });
    let connection = Connection {
        handle,
        incoming: incoming_rx,
        task,
    };
    tokio::time::timeout(Duration::from_secs(20), ready_rx)
        .await
        .context("TMP handshake timeout")?
        .context("TMP handshake failed")?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HostTicket, Room};

    fn runtime_with_two_streams() -> (Runtime, mpsc::Receiver<Output>, [u32; 2]) {
        let mut settings = default_settings();
        settings.max_data = 65536;
        settings.stream_window = 65536;
        settings.connection_window = 65536;
        let profiles = PROFILES.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut session =
            Session::new(tunnel_mux::Role::Initiator, settings, profiles.clone()).unwrap();
        let mut peer = Session::new(tunnel_mux::Role::Responder, settings, profiles).unwrap();
        let welcome = peer
            .receive(session.start().unwrap())
            .unwrap()
            .outbound
            .remove(0);
        session.receive(welcome).unwrap();
        let (commands, _) = mpsc::channel(8);
        let (incoming, _) = mpsc::channel(8);
        let (output, rx) = mpsc::channel(8);
        let mut runtime = Runtime {
            session,
            commands,
            streams: BTreeMap::new(),
            incoming,
            output,
            workers: JoinSet::new(),
            ready: true,
            draining: false,
            carrier_closed: false,
            drain_probe: None,
            drain_ack: false,
            cursor: 0,
            write_pending: Arc::new(AtomicUsize::new(0)),
            heartbeat: None,
            last_ping: tokio::time::Instant::now(),
        };
        let mut ids = [0; 2];
        for id in &mut ids {
            let (stream, open) = runtime
                .session
                .open(pb::Open {
                    route: "file.get".into(),
                    profile: PROFILES[4].into(),
                    class: 2,
                    metadata: b"{}".to_vec(),
                })
                .unwrap();
            peer.receive(open).unwrap();
            runtime
                .session
                .receive(peer.accept(stream).unwrap())
                .unwrap();
            let (status, _) = watch::channel(StreamStatus::Active);
            runtime.streams.insert(
                stream,
                LocalStream {
                    input: None,
                    opening: None,
                    pending: None,
                    remote_fin: false,
                    local_fin: false,
                    consumed: 0,
                    received: 0,
                    tasks: Vec::new(),
                    status,
                    receive_fin: Arc::new(AtomicBool::new(false)),
                },
            );
            *id = stream;
        }
        (runtime, rx, ids)
    }
    #[tokio::test]
    async fn fair_credit_rotation_and_unclaimed_open_cleanup() -> Result<()> {
        let (mut runtime, mut output, [first, second]) = runtime_with_two_streams();
        runtime.session.send(first, vec![0; 64512], false)?;
        let pending = || {
            let (reply, _) = oneshot::channel();
            (vec![1; 1024], false, reply)
        };
        runtime.streams.get_mut(&first).unwrap().pending = Some(pending());
        runtime.streams.get_mut(&second).unwrap().pending = Some(pending());
        runtime.pump()?;
        assert!(
            matches!(output.recv().await,Some(Output::Packet{packet,..}) if packet.stream_id==first)
        );
        runtime.session.receive(pb::Packet {
            stream_id: 0,
            body: Some(pb::packet::Body::Window(pb::Window { credit: 1024 })),
        })?;
        runtime.streams.get_mut(&first).unwrap().pending = Some(pending());
        runtime.pump()?;
        assert!(
            matches!(output.recv().await,Some(Output::Packet{packet,..}) if packet.stream_id==second)
        );
        let (reply, unclaimed) = oneshot::channel();
        drop(unclaimed);
        let (io, _) = tokio::io::duplex(1);
        let (_, status) = watch::channel(StreamStatus::Active);
        runtime.streams.get_mut(&second).unwrap().opening = Some((reply, io, status));
        runtime.cancel_unclaimed_opens()?;
        assert!(!runtime.streams.contains_key(&second));
        Ok(())
    }
    #[tokio::test]
    async fn application_read_distinguishes_fin_reset_and_broken_bridge() {
        for mode in 0..3 {
            let (io, bridge) = tokio::io::duplex(4);
            let (status, rx) = watch::channel(StreamStatus::Active);
            let fin = Arc::new(AtomicBool::new(mode == 0));
            let mut stream = MuxStream {
                id: 1,
                io,
                status: rx,
                receive_fin: fin,
            };
            if mode == 1 {
                status.send_replace(StreamStatus::Failed);
            }
            drop(bridge);
            let result = stream.read(&mut [0; 1]).await;
            if mode == 0 {
                assert_eq!(result.unwrap(), 0);
            } else {
                assert!(result.is_err());
            }
        }
    }
    #[tokio::test]
    async fn empty_stream_table_cannot_hide_unwritten_data() -> Result<()> {
        let (mut runtime, _output, [id, _]) = runtime_with_two_streams();
        let data = runtime.session.send(id, b"receipt".to_vec(), true)?;
        runtime.emit([data])?;
        runtime.streams.clear();
        assert!(!runtime.carrier_fully_written());
        // Only the writer decrements after successful write/flush, never on error.
        runtime.write_pending.fetch_sub(1, Ordering::AcqRel);
        assert!(runtime.carrier_fully_written());
        Ok(())
    }
    #[tokio::test]
    async fn tiny_windows_and_goaway_preserve_large_half_closed_transfer() -> Result<()> {
        tokio::time::timeout(Duration::from_secs(30), async {
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
                .json(&serde_json::json!({}))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let ticket = HostTicket::new(&base, room)?;
            let mut settings = default_settings();
            settings.max_data = 1024;
            settings.stream_window = 4096;
            settings.connection_window = 65536;
            let (host, client) = tokio::join!(
                connect_inner(&ticket.invite, Role::Host, &ticket.host_token, settings),
                connect_inner(
                    &ticket.invite,
                    Role::Client,
                    &ticket.invite.client_token,
                    settings
                )
            );
            let mut host = host?;
            let client = client?;
            let mut stream = client
                .handle
                .open("file.put", serde_json::json!({}))
                .await?;
            let incoming = host.incoming.recv().await.context("incoming open")?;
            host.handle.drain().await?;
            let service = tokio::spawn(async move {
                let mut io = incoming.stream.io;
                let mut bytes = Vec::new();
                io.read_to_end(&mut bytes).await?;
                ensure!(bytes == vec![19; 512 * 1024], "large request truncated");
                io.write_all(&vec![23; 512 * 1024]).await?;
                io.shutdown().await?;
                incoming.completion.finish(true);
                Ok::<_, anyhow::Error>(())
            });
            stream.io.write_all(&vec![19; 512 * 1024]).await?;
            stream.io.shutdown().await?;
            let mut response = Vec::new();
            stream.io.read_to_end(&mut response).await?;
            stream.completed().await?;
            ensure!(
                response == vec![23; 512 * 1024],
                "GOAWAY truncated response"
            );
            service.await??;
            (&mut host.task).await??;
            server.abort();
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }
}
