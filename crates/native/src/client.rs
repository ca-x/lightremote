//! Bounded, cancellable controller operations. NSH never polls network I/O on its UI thread.
use crate::{desktop::Display, input::Bridge};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use roc_remote_core::{
    crypto::Role,
    invite::Invite,
    link::{self, LinkHandle},
    records,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
};

const TIMEOUT: Duration = Duration::from_secs(25);
const MAX_TEXT: usize = 128 * 1024;
const MAX_FILE: u64 = 512 * 1024 * 1024;
pub const MAX_CLIPBOARD_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Operation {
    Listing,
    Transfer,
    Clipboard,
}

#[derive(Clone, Default, PartialEq)]
pub struct State {
    pub connected: bool,
    pub connecting: bool,
    pub status: String,
    pub error: String,
    pub capabilities: String,
    pub displays: String,
    pub listing: String,
    pub listing_path: String,
    pub pending: BTreeSet<Operation>,
    pub transfer_status: String,
    pub clipboard_status: String,
    pub clipboard: String,
    pub clipboard_revision: u64,
    pub terminal: String,
    pub terminal_total: u64,
    pub terminal_epoch: u64,
    pub shell_id: Option<u32>,
    pub desktop: Arc<str>,
    pub desktop_running: bool,
    pub shell_running: bool,
    pub display: Option<Display>,
    pub screenshot: Arc<[u8]>,
}

impl State {
    pub fn platform_guidance(&self) -> String {
        serde_json::from_str::<Value>(&self.capabilities)
            .ok()
            .and_then(|value| {
                value["platform"]["desktop_guidance"]
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    }

    pub fn allows(&self, capability: &str) -> bool {
        self.connected
            && serde_json::from_str::<Value>(&self.capabilities)
                .ok()
                .and_then(|value| value.get(capability).and_then(Value::as_bool))
                == Some(true)
    }
}

pub enum Command {
    Connect(String),
    Disconnect,
    Capabilities,
    Displays,
    Desktop { display: u32, single: bool },
    StopDesktop,
    OpenShell,
    CloseShell,
    Shell(Vec<u8>),
    ResizeShell { cols: u16, rows: u16 },
    List(String),
    Download { remote: String, local: PathBuf },
    Upload { remote: String, local: PathBuf },
    Text(String),
    SaveScreenshot(PathBuf),
    ClipboardGet,
    ClipboardSet(String),
}

#[derive(Clone)]
pub struct Client {
    commands: mpsc::Sender<Command>,
    state: watch::Sender<State>,
    input: Bridge,
}

impl Client {
    pub fn start() -> Result<(Self, watch::Receiver<State>)> {
        let (commands, receiver) = mpsc::channel(16);
        let (state, updates) = watch::channel(State {
            status: "Disconnected".into(),
            ..State::default()
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("Start network runtime")?;
        let output = state.clone();
        let input = Bridge::new();
        let network_input = input.clone();
        std::thread::Builder::new()
            .name("roc-remote-client".into())
            .spawn(move || {
                runtime.block_on(run(receiver, output, network_input));
            })
            .context("Start network worker")?;
        Ok((
            Self {
                commands,
                state,
                input,
            },
            updates,
        ))
    }

    pub fn send(&self, command: Command) {
        if self.commands.try_send(command).is_err() {
            error(
                &self.state,
                "The operation queue is full or the network worker stopped. Try again.",
            );
        }
    }

    #[cfg(not(target_os = "android"))]
    pub fn arm_input(&self) {
        self.input.arm();
    }

    pub fn release_input(&self) {
        self.input.release();
    }

    #[cfg(not(target_os = "android"))]
    pub fn forward_input(&self, display: u32, events: Vec<Value>) -> bool {
        if !self.state.borrow().connected || !self.input.is_armed() {
            return false;
        }
        let ok = self.input.send(display, events);
        if !ok {
            self.report(
                "Remote input paused: queue full or stream closed. Click the desktop to retry.",
            );
        }
        ok
    }

    pub fn report(&self, message: &str) {
        error(&self.state, message);
    }

    pub fn dismiss_error(&self) {
        self.state.send_modify(|state| state.error.clear());
    }
}

struct PendingOperation {
    state: watch::Sender<State>,
    operation: Operation,
}

impl PendingOperation {
    fn begin(state: &watch::Sender<State>, operation: Operation) -> Option<Self> {
        state
            .send_if_modified(|state| state.pending.insert(operation))
            .then(|| Self {
                state: state.clone(),
                operation,
            })
    }
}

impl Drop for PendingOperation {
    fn drop(&mut self) {
        self.state.send_modify(|state| {
            state.pending.remove(&self.operation);
        });
    }
}

fn spawn_operation(
    workers: &mut JoinSet<Result<()>>,
    state: &watch::Sender<State>,
    operation: Operation,
    future: impl Future<Output = Result<()>> + Send + 'static,
) {
    let Some(pending) = PendingOperation::begin(state, operation) else {
        return;
    };
    let state = state.clone();
    state.send_modify(|state| match operation {
        Operation::Transfer => state.transfer_status = "Transfer in progress…".into(),
        Operation::Clipboard => state.clipboard_status = "Clipboard request in progress…".into(),
        Operation::Listing => {}
    });
    workers.spawn(async move {
        let result = future.await;
        if result.is_err() {
            state.send_modify(|state| match operation {
                Operation::Transfer => {
                    state.transfer_status =
                        "Transfer failed. Check the destination before trying again.".into()
                }
                Operation::Clipboard => {
                    state.clipboard_status =
                        "Clipboard request failed. Nothing is retried automatically.".into()
                }
                Operation::Listing => {}
            });
        }
        drop(pending);
        result
    });
}

fn error(state: &watch::Sender<State>, message: impl Into<String>) {
    state.send_modify(|s| s.error = message.into());
}

fn append_terminal(state: &watch::Sender<State>, bytes: &[u8]) {
    state.send_modify(|s| {
        let text = String::from_utf8_lossy(bytes);
        s.terminal_total += text.len() as u64;
        s.terminal.push_str(&text);
        if s.terminal.len() > MAX_TEXT {
            let mut cut = s.terminal.len() - MAX_TEXT;
            while !s.terminal.is_char_boundary(cut) {
                cut += 1;
            }
            s.terminal.drain(..cut);
        }
    });
}

struct Request {
    method: &'static str,
    params: Value,
    reply: oneshot::Sender<Result<Value>>,
}
#[derive(Clone)]
struct Control(mpsc::Sender<Request>);
impl Control {
    async fn call(&self, method: &'static str, params: Value) -> Result<Value> {
        tokio::time::timeout(TIMEOUT, async {
            let (reply, response) = oneshot::channel();
            self.0
                .send(Request {
                    method,
                    params,
                    reply,
                })
                .await
                .context("Control channel closed")?;
            response.await.context("Control response closed")?
        })
        .await
        .context("Control request timed out")?
    }
}

async fn control_loop<T: AsyncRead + AsyncWrite + Unpin>(
    io: T,
    mut requests: mpsc::Receiver<Request>,
    state: watch::Sender<State>,
) -> Result<()> {
    let (mut reader, mut writer) = tokio::io::split(io);
    let mut pending = BTreeMap::new();
    let mut id = 0_u64;
    loop {
        // Keep this read alive while handling outgoing requests: cancelling a partial
        // length-prefixed read would lose framing when packets are fragmented.
        let read = records::read(&mut reader, records::MAX_CONTROL);
        tokio::pin!(read);
        let frame = loop {
            tokio::select! {
                frame = &mut read => break frame?.context("Control stream closed")?,
                request = requests.recv() => {
                    let Some(request) = request else { return Ok(()); };
                    pending.retain(|_, reply: &mut oneshot::Sender<Result<Value>>| !reply.is_closed());
                    ensure!(pending.len() < 32, "Too many pending control requests");
                    id = id.checked_add(1).context("Control request counter exhausted")?;
                    records::json(&mut writer, &json!({"id":id,"method":request.method,"params":request.params})).await?;
                    pending.insert(id, request.reply);
                }
            }
        };
        let message: Value = serde_json::from_slice(&frame).context("Invalid control JSON")?;
        if let Some(id) = message.get("id").and_then(Value::as_u64) {
            if let Some(reply) = pending.remove(&id) {
                let _ = reply.send(control_result(&message));
            }
        } else if message.get("event").is_some() {
            state.send_modify(|s| s.status = format!("Host event: {}", message["event"]));
        }
    }
}

fn control_result(message: &Value) -> Result<Value> {
    if let Some(failure) = message.get("error") {
        bail!(
            "{}: {}",
            failure["code"].as_str().unwrap_or("remote_error"),
            failure["message"]
                .as_str()
                .unwrap_or("Host rejected the operation")
        );
    }
    message
        .get("result")
        .cloned()
        .context("Control response omitted result")
}

async fn run(mut commands: mpsc::Receiver<Command>, state: watch::Sender<State>, input: Bridge) {
    while let Some(command) = commands.recv().await {
        if let Command::SaveScreenshot(path) = command {
            if let Err(failure) = save_screenshot(state.clone(), path).await {
                error(&state, format!("{failure:#}"));
            }
            continue;
        }
        let Command::Connect(text) = command else {
            if !matches!(command, Command::Disconnect) {
                error(&state, "Connect to a host first.");
            }
            continue;
        };
        let invite = match Invite::parse(&text) {
            Ok(invite) => invite,
            Err(_) => {
                error(
                    &state,
                    "Invalid invitation. Paste the complete invitation URL or fragment.",
                );
                continue;
            }
        };
        drop(text);
        state.send_modify(|s| {
            s.error.clear();
            s.connecting = true;
            s.status = "Connecting and authenticating…".into();
        });
        let connection = {
            let connect = tokio::time::timeout(
                TIMEOUT,
                link::connect(&invite, Role::Client, &invite.client_token),
            );
            tokio::pin!(connect);
            loop {
                tokio::select! {
                    result = &mut connect => break Some(result),
                    command = commands.recv() => match command {
                        None => return,
                        Some(Command::Disconnect) => break None,
                        _ => error(&state, "Connection in progress; disconnect to cancel."),
                    }
                }
            }
        };
        let Some(connection) = connection else {
            state.send_modify(|s| {
                s.connecting = false;
                s.status = "Disconnected".into();
            });
            continue;
        };
        let mut connection = match connection {
            Ok(Ok(connection)) => connection,
            Ok(Err(_)) => {
                error(
                    &state,
                    "Connection or authentication failed. Check relay availability and create a fresh invitation.",
                );
                state.send_modify(|s| {
                    s.connecting = false;
                    s.status = "Disconnected".into();
                });
                continue;
            }
            Err(_) => {
                error(&state, "Connection timed out.");
                state.send_modify(|s| {
                    s.connecting = false;
                    s.status = "Disconnected".into();
                });
                continue;
            }
        };
        drop(invite);
        state.send_modify(|s| {
            s.connected = true;
            s.connecting = false;
            s.error.clear();
            s.status = "Authenticated".into();
        });
        let (requests, request_rx) = mpsc::channel(16);
        let control = Control(requests);
        let handle = connection.handle.clone();
        let mut workers = JoinSet::new();
        let output = state.clone();
        workers.spawn(async move {
            let stream = handle.open("control", json!({})).await?;
            control_loop(stream, request_rx, output).await
        });
        spawn_capabilities(&mut workers, control.clone(), state.clone());
        let input_rx = input.attach();
        let input_bridge = input.clone();
        let input_handle = connection.handle.clone();
        let input_output = state.clone();
        let mut input_task = tokio::spawn(async move {
            let result = input_bridge
                .run(input_handle, input_rx, |message| {
                    error(&input_output, message)
                })
                .await;
            if let Err(failure) = &result {
                error(&input_output, format!("Remote input: {failure:#}"));
            }
            result
        });
        let mut desktop = None;
        let mut shell = None;
        let mut shell_input: Option<mpsc::Sender<Vec<u8>>> = None;
        loop {
            tokio::select! {
                result = &mut connection.task => {
                    error(&state, match result { Ok(Ok(())) => "The host closed the session.", _ => "Connection lost. Create a fresh invitation to reconnect." });
                    break;
                }
                incoming = connection.incoming.recv() => {
                    if incoming.is_none() { error(&state, "The session ended."); break; }
                    // No unsolicited host-initiated application route is accepted by this controller.
                    drop(incoming);
                }
                result = workers.join_next(), if !workers.is_empty() => {
                    if let Some(result) = result {
                        match result {
                            Ok(Err(failure)) => error(&state, format!("{failure:#}")),
                            Err(failure) if !failure.is_cancelled() => error(&state, "A controller operation stopped unexpectedly."),
                            _ => {}
                        }
                    }
                }
                command = commands.recv() => {
                    let Some(command) = command else { break; };
                    if workers.len() >= 24 && !matches!(command, Command::Disconnect | Command::StopDesktop | Command::CloseShell) {
                        error(&state, "Too many operations in progress. Wait for an operation to finish.");
                        continue;
                    }
                    state.send_modify(|s| s.error.clear());
                    match command {
                        Command::Disconnect => break,
                        Command::Connect(_) => error(&state, "Disconnect before opening another invitation."),
                        Command::Capabilities => spawn_capabilities(&mut workers, control.clone(), state.clone()),
                        Command::Displays => {
                            let control = control.clone(); let state = state.clone();
                            workers.spawn(async move {
                                let value = control.call("desktop.displays", json!({})).await?;
                                state.send_modify(|s| s.displays = pretty(&value)); Ok(())
                            });
                        }
                        Command::Desktop { display, single } => {
                            input.release();
                            if let Some(task) = desktop.take() { tokio::task::AbortHandle::abort(&task); }
                            let handle = connection.handle.clone(); let state = state.clone();
                            let control = control.clone();
                            state.send_modify(|s| { s.desktop_running = true; s.display = None; s.desktop = Arc::from(""); });
                            desktop = Some(workers.spawn(async move {
                                let result = desktop_loop(handle, control, display, single, state.clone()).await;
                                state.send_modify(|s| s.desktop_running = false);
                                result
                            }));
                        }
                        Command::StopDesktop => {
                            input.release();
                            if let Some(task) = desktop.take() { task.abort(); }
                            state.send_modify(|s| s.desktop_running = false);
                        }
                        Command::OpenShell => {
                            if state.borrow().shell_running { error(&state, "A shell is already open."); continue; }
                            if let Some(task) = shell.take() { tokio::task::AbortHandle::abort(&task); }
                            let (input, rx) = mpsc::channel(16); shell_input = Some(input);
                            let handle = connection.handle.clone(); let state = state.clone();
                            state.send_modify(|s| { s.shell_running = true; s.terminal.clear(); s.terminal_total = 0; s.terminal_epoch += 1; s.shell_id = None; });
                            shell = Some(workers.spawn(async move {
                                let result = shell_loop(handle, rx, state.clone()).await;
                                state.send_modify(|s| { s.shell_running = false; s.shell_id = None; });
                                result
                            }));
                        }
                        Command::CloseShell => {
                            shell_input = None;
                            if let Some(task) = shell.take() { task.abort(); }
                            state.send_modify(|s| { s.shell_running = false; s.shell_id = None; });
                        }
                        Command::ResizeShell { cols, rows } => {
                            let id = state.borrow().shell_id;
                            if let Some(stream_id) = id {
                                let control = control.clone();
                                workers.spawn(async move { control.call("shell.resize", json!({"stream_id":stream_id,"cols":cols,"rows":rows})).await?; Ok(()) });
                            }
                        }
                        Command::Shell(bytes) => {
                            if bytes.len() > 16384 || shell_input.as_ref().is_none_or(|tx| tx.try_send(bytes).is_err()) {
                                error(&state, "Open a shell first, or wait for its input queue to drain.");
                            }
                        }
                        Command::ClipboardGet => {
                            let control = control.clone(); let output = state.clone();
                            spawn_operation(&mut workers, &state, Operation::Clipboard, async move { read_remote_clipboard(control, output).await });
                        }
                        Command::ClipboardSet(text) => {
                            let control = control.clone(); let output = state.clone();
                            spawn_operation(&mut workers, &state, Operation::Clipboard, async move { write_remote_clipboard(control, output, text).await });
                        }
                        Command::List(path) => {
                            let control = control.clone(); let output = state.clone();
                            spawn_operation(&mut workers, &state, Operation::Listing, async move {
                                let value = control.call("fs.list", json!({"path":path})).await?;
                                output.send_modify(|s| { s.listing = pretty(&value); s.listing_path = path; }); Ok(())
                            });
                        }
                        Command::Download { remote, local } => {
                            let handle = connection.handle.clone(); let output = state.clone();
                            spawn_operation(&mut workers, &state, Operation::Transfer, async move {
                                download(handle, remote, local).await?;
                                output.send_modify(|s| { s.status = "Download complete".into(); s.transfer_status = "Download complete · saved without overwriting".into(); }); Ok(())
                            });
                        }
                        Command::Upload { remote, local } => {
                            let handle = connection.handle.clone(); let output = state.clone();
                            spawn_operation(&mut workers, &state, Operation::Transfer, async move {
                                upload(handle, remote, local).await?;
                                output.send_modify(|s| { s.status = "Upload complete".into(); s.transfer_status = "Upload complete · SHA-256 verified".into(); }); Ok(())
                            });
                        }
                        Command::SaveScreenshot(path) => {
                            let state = state.clone();
                            workers.spawn(async move { save_screenshot(state, path).await });
                        }
                        Command::Text(text) => {
                            ensure_text(&state, &text);
                            if !text.is_empty() && text.len() <= 16384 {
                                let selected = state.borrow().display;
                                if let Some(display) = selected {
                                    input.arm();
                                    if !input.send(display.id, vec![json!({"type":"text","text":text})]) { error(&state, "Remote input is unavailable; reconnect to retry."); }
                                } else { error(&state, "Start a desktop or capture a screenshot before typing."); }
                            }
                        }
                    }
                }
            }
        }
        input.detach();
        if tokio::time::timeout(Duration::from_secs(3), &mut input_task)
            .await
            .is_err()
        {
            input_task.abort();
        }
        workers.abort_all();
        while workers.join_next().await.is_some() {}
        drop(connection);
        state.send_modify(|s| {
            s.connected = false;
            s.connecting = false;
            s.desktop_running = false;
            s.shell_running = false;
            s.desktop = Arc::from("");
            s.display = None;
            s.capabilities.clear();
            s.displays.clear();
            s.listing.clear();
            s.listing_path.clear();
            s.pending.clear();
            s.transfer_status.clear();
            s.clipboard_status.clear();
            s.clipboard.clear();
            s.clipboard_revision += 1;
            s.terminal.clear();
            s.terminal_total = 0;
            s.shell_id = None;
            s.terminal_epoch += 1;
            s.status = "Disconnected".into();
        });
    }
}

/// Match the host's text limit and reject writes that would exceed the control
/// frame after JSON escaping, before they can break the shared control stream.
pub fn validate_clipboard_text(text: &str) -> Result<()> {
    ensure!(
        text.len() <= MAX_CLIPBOARD_BYTES,
        "Clipboard text exceeds 256 KiB"
    );
    ensure!(!text.contains('\0'), "Clipboard text cannot contain NUL");
    let request = json!({"id":u64::MAX,"method":"clipboard.set","params":{"text":text}});
    ensure!(
        serde_json::to_vec(&request)?.len() <= records::MAX_CONTROL,
        "Clipboard text exceeds the control limit after JSON encoding"
    );
    Ok(())
}

fn clipboard_text(response: &Value) -> Result<String> {
    let text = response["text"]
        .as_str()
        .context("Host clipboard response omitted text")?;
    validate_clipboard_text(text)?;
    Ok(text.to_owned())
}

async fn read_remote_clipboard(control: Control, state: watch::Sender<State>) -> Result<()> {
    let response = control.call("clipboard.get", json!({})).await?;
    let text = clipboard_text(&response)?;
    state.send_modify(|s| {
        s.clipboard = text;
        s.clipboard_revision += 1;
        s.status = "Remote clipboard loaded into editor".into();
        s.clipboard_status = "Remote clipboard loaded into editor".into();
    });
    Ok(())
}

async fn write_remote_clipboard(
    control: Control,
    state: watch::Sender<State>,
    text: String,
) -> Result<()> {
    validate_clipboard_text(&text)?;
    let response = control.call("clipboard.set", json!({"text":text})).await?;
    ensure!(
        response.is_object(),
        "Invalid host clipboard write response"
    );
    state.send_modify(|s| {
        s.status = "Remote clipboard written".into();
        s.clipboard_status = "Remote clipboard written".into();
    });
    Ok(())
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}
fn spawn_capabilities(
    workers: &mut JoinSet<Result<()>>,
    control: Control,
    state: watch::Sender<State>,
) {
    workers.spawn(async move {
        let value = control.call("capabilities", json!({})).await?;
        state.send_modify(|s| s.capabilities = pretty(&value));
        Ok(())
    });
}

async fn desktop_loop(
    handle: LinkHandle,
    control: Control,
    display: u32,
    single: bool,
    state: watch::Sender<State>,
) -> Result<()> {
    let geometry =
        Display::from_response(&control.call("desktop.displays", json!({})).await?, display)?;
    state.send_modify(|s| s.display = Some(geometry));
    let mut stream = handle
        .open(
            "desktop",
            json!({"display":display,"fps":10,"quality":70,"single":single}),
        )
        .await?;
    stream.shutdown().await?;
    while let Some(frame) = records::read(&mut stream, records::MAX_RECORD).await? {
        crate::desktop::admit_jpeg(&frame)?;
        let bytes: Arc<[u8]> = frame.into();
        let image: Arc<str> = format!("data:image/jpeg;base64,{}", STANDARD.encode(&bytes)).into();
        // watch replaces the previous frame; an idle renderer never builds up a frame queue.
        if single {
            ensure!(
                records::read(&mut stream, records::MAX_RECORD)
                    .await?
                    .is_none(),
                "Screenshot sent more than one frame"
            );
            stream.completed().await?;
            state.send_modify(|s| {
                s.desktop = image;
                s.screenshot = bytes;
            });
            return Ok(());
        }
        state.send_modify(|s| {
            s.desktop = image;
            s.screenshot = bytes;
        });
    }
    stream.completed().await?;
    Ok(())
}

fn ensure_text(state: &watch::Sender<State>, text: &str) {
    if text.is_empty() || text.len() > 16384 {
        error(state, "Input text must be 1–16384 bytes");
    }
}

async fn save_screenshot(state: watch::Sender<State>, path: PathBuf) -> Result<()> {
    let bytes = state.borrow().screenshot.clone();
    ensure!(!bytes.is_empty(), "No completed desktop frame to save");
    ensure!(
        !path.as_os_str().is_empty(),
        "Enter a screenshot destination ending in .jpg"
    );
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let temporary =
        tempfile::NamedTempFile::new_in(parent).context("Create screenshot temporary file")?;
    let mut file = tokio::fs::File::from_std(temporary.reopen()?);
    file.write_all(&bytes).await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    temporary
        .persist_noclobber(path)
        .context("Save screenshot (destination must not exist)")?;
    state.send_modify(|s| {
        s.error.clear();
        s.status = "Screenshot saved".into();
    });
    Ok(())
}

async fn shell_loop(
    handle: LinkHandle,
    mut input: mpsc::Receiver<Vec<u8>>,
    state: watch::Sender<State>,
) -> Result<()> {
    let mut stream = handle.open("shell", json!({"cols":100,"rows":28})).await?;
    state.send_modify(|s| s.shell_id = Some(stream.id));
    let mut pending = Vec::new();
    {
        let (mut reader, mut writer) = tokio::io::split(&mut stream);
        let mut buffer = [0; 8192];
        loop {
            tokio::select! {
                read = reader.read(&mut buffer) => {
                    let n = read?;
                    let text = decode_terminal(&mut pending, &buffer[..n], n == 0);
                    append_terminal(&state, text.as_bytes());
                    if n == 0 { writer.shutdown().await?; break; }
                }
                bytes = input.recv() => {
                    let Some(bytes) = bytes else { writer.shutdown().await?; break; };
                    writer.write_all(&bytes).await?;
                }
            }
        }
    }
    stream
        .completed()
        .await
        .context("Shell did not finish successfully")?;
    Ok(())
}

fn decode_terminal(pending: &mut Vec<u8>, bytes: &[u8], eof: bool) -> String {
    pending.extend_from_slice(bytes);
    let mut decoded = String::new();
    let mut consumed = 0;
    while consumed < pending.len() {
        match std::str::from_utf8(&pending[consumed..]) {
            Ok(text) => {
                decoded.push_str(text);
                consumed = pending.len();
            }
            Err(error) => {
                let end = consumed + error.valid_up_to();
                decoded.push_str(
                    std::str::from_utf8(&pending[consumed..end]).expect("validated UTF-8 prefix"),
                );
                consumed = end;
                match error.error_len() {
                    Some(length) => {
                        decoded.push('�');
                        consumed += length;
                    }
                    None if eof => {
                        decoded.push('�');
                        consumed = pending.len();
                    }
                    None => break,
                }
            }
        }
    }
    pending.drain(..consumed);
    decoded
}

async fn download(handle: LinkHandle, remote: String, local: PathBuf) -> Result<()> {
    ensure!(
        !local.as_os_str().is_empty(),
        "Enter a local destination path"
    );
    let parent = local
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    // A same-directory temporary file cleans itself up even when disconnection
    // aborts this future. Publish only after the transport confirms a successful FIN.
    let temporary =
        tempfile::NamedTempFile::new_in(parent).context("Create temporary download file")?;
    let mut file = tokio::fs::File::from_std(temporary.reopen()?);
    let mut stream = handle.open("file.get", json!({"path":remote})).await?;
    stream.shutdown().await?;
    let bytes = tokio::io::copy(&mut (&mut stream).take(MAX_FILE + 1), &mut file).await?;
    ensure!(bytes <= MAX_FILE, "Download exceeds 512 MiB limit");
    stream
        .completed()
        .await
        .context("Download did not finish successfully")?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    temporary
        .persist_noclobber(&local)
        .context("Publish download (destination must not exist)")?;
    Ok(())
}

async fn upload(handle: LinkHandle, remote: String, local: PathBuf) -> Result<Value> {
    let mut file = tokio::fs::File::open(local)
        .await
        .context("Open local upload file")?;
    let metadata = file.metadata().await?;
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_FILE,
        "Upload requires a regular file of at most 512 MiB"
    );
    let mut stream = handle
        .open("file.put", json!({"path":remote,"size":metadata.len()}))
        .await?;
    let mut copied = 0_u64;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 32768];
    while copied < metadata.len() {
        let count = (metadata.len() - copied).min(buffer.len() as u64) as usize;
        let n = file.read(&mut buffer[..count]).await?;
        ensure!(n > 0, "Local file changed during upload");
        hash.update(&buffer[..n]);
        stream.write_all(&buffer[..n]).await?;
        copied += n as u64;
    }
    ensure!(
        file.read(&mut buffer[..1]).await? == 0,
        "Local file grew during upload"
    );
    let checksum = format!("{:x}", hash.finalize());
    stream.shutdown().await?;
    let receipt = records::read(&mut stream, records::MAX_CONTROL)
        .await?
        .context("Upload receipt missing")?;
    let receipt: Value = serde_json::from_slice(&receipt)?;
    validate_upload_receipt(&receipt, copied, &checksum)?;
    ensure!(
        records::read(&mut stream, records::MAX_CONTROL)
            .await?
            .is_none(),
        "Unexpected additional upload receipt"
    );
    stream.completed().await?;
    Ok(receipt)
}

fn validate_upload_receipt(receipt: &Value, size: u64, checksum: &str) -> Result<()> {
    if let Some(error) = receipt.get("error").filter(|error| !error.is_null()) {
        bail!(
            "Host rejected upload: {}: {}",
            error["code"].as_str().unwrap_or("remote_error"),
            error["message"]
                .as_str()
                .unwrap_or_else(|| error.as_str().unwrap_or("Host rejected upload"))
        );
    }
    ensure!(
        receipt["size"].as_u64() == Some(size),
        "Host upload receipt size mismatch"
    );
    ensure!(
        receipt["sha256"].as_str() == Some(checksum),
        "Host upload receipt checksum mismatch"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn pending_operations_are_single_flight_and_release_after_success_or_error() {
        for succeeds in [true, false] {
            let (state, _) = watch::channel(State::default());
            let mut workers = JoinSet::new();
            let (complete, ready) = oneshot::channel::<()>();
            spawn_operation(&mut workers, &state, Operation::Transfer, async move {
                ready.await?;
                ensure!(succeeds, "test failure");
                Ok(())
            });
            assert!(state.borrow().pending.contains(&Operation::Transfer));
            spawn_operation(&mut workers, &state, Operation::Transfer, async {
                panic!("Duplicate side effect must never be polled");
            });
            assert_eq!(workers.len(), 1);
            complete.send(()).unwrap();
            assert_eq!(
                workers.join_next().await.unwrap().unwrap().is_ok(),
                succeeds
            );
            assert!(state.borrow().pending.is_empty());
            if !succeeds {
                assert!(
                    state
                        .borrow()
                        .transfer_status
                        .contains("Check the destination")
                );
            }
            assert!(PendingOperation::begin(&state, Operation::Transfer).is_some());
        }
    }

    #[tokio::test]
    async fn aborting_operations_releases_every_group_without_replay() {
        let (state, _) = watch::channel(State::default());
        let mut workers = JoinSet::new();
        for operation in [
            Operation::Listing,
            Operation::Transfer,
            Operation::Clipboard,
        ] {
            spawn_operation(&mut workers, &state, operation, std::future::pending());
        }
        assert_eq!(state.borrow().pending.len(), 3);
        workers.abort_all();
        while workers.join_next().await.is_some() {}
        assert!(state.borrow().pending.is_empty());
    }
    #[test]
    fn clipboard_bounds_cover_utf8_nul_and_json_expansion() {
        assert!(validate_clipboard_text("").is_ok());
        assert!(validate_clipboard_text(&"x".repeat(MAX_CLIPBOARD_BYTES)).is_ok());
        assert!(validate_clipboard_text(&"你".repeat(MAX_CLIPBOARD_BYTES / 3 + 1)).is_err());
        assert!(validate_clipboard_text("a\0b").is_err());
        assert!(validate_clipboard_text(&"\x01".repeat(MAX_CLIPBOARD_BYTES)).is_err());
        assert!(clipboard_text(&json!({"text":3})).is_err());
        assert!(clipboard_text(&json!({})).is_err());
        assert_eq!(clipboard_text(&json!({"text":""})).unwrap(), "");
    }

    #[tokio::test]
    async fn explicit_clipboard_requests_preserve_permission_errors_and_editor() {
        let (local, mut peer) = tokio::io::duplex(4096);
        let (tx, rx) = mpsc::channel(4);
        let control = Control(tx);
        let (state, _) = watch::channel(State::default());
        let runtime = tokio::spawn(control_loop(local, rx, state.clone()));
        let service = tokio::spawn(async move {
            for (method, params, result) in [
                (
                    "clipboard.get",
                    json!({}),
                    json!({"result":{"text":"远程\nclip"}}),
                ),
                (
                    "clipboard.set",
                    json!({"text":"edited"}),
                    json!({"result":{}}),
                ),
                (
                    "clipboard.get",
                    json!({}),
                    json!({"error":{"code":"permission_denied","message":"Clipboard disabled"}}),
                ),
            ] {
                let request = records::read(&mut peer, records::MAX_CONTROL)
                    .await
                    .unwrap()
                    .unwrap();
                let request: Value = serde_json::from_slice(&request).unwrap();
                assert_eq!(request["method"], method);
                assert_eq!(request["params"], params);
                let mut response = result;
                response["id"] = request["id"].clone();
                records::json(&mut peer, &response).await.unwrap();
            }
        });
        read_remote_clipboard(control.clone(), state.clone())
            .await
            .unwrap();
        assert_eq!(state.borrow().clipboard, "远程\nclip");
        assert_eq!(state.borrow().clipboard_revision, 1);
        write_remote_clipboard(control.clone(), state.clone(), "edited".into())
            .await
            .unwrap();
        assert_eq!(state.borrow().status, "Remote clipboard written");
        let failure = read_remote_clipboard(control, state.clone())
            .await
            .unwrap_err()
            .to_string();
        assert!(failure.contains("permission_denied: Clipboard disabled"));
        assert_eq!(state.borrow().clipboard, "远程\nclip");
        assert_eq!(state.borrow().clipboard_revision, 1);
        service.await.unwrap();
        runtime.abort();
    }

    #[tokio::test]
    async fn screenshot_export_is_exact_and_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("capture.jpg");
        let (state, _) = watch::channel(State {
            screenshot: Arc::from(&b"\xff\xd8\xff\xd9"[..]),
            ..State::default()
        });
        save_screenshot(state.clone(), path.clone()).await.unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"\xff\xd8\xff\xd9");
        assert!(save_screenshot(state, path.clone()).await.is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        let (empty, _) = watch::channel(State::default());
        assert!(
            save_screenshot(empty, directory.path().join("empty.jpg"))
                .await
                .is_err()
        );
    }
    #[test]
    fn upload_receipts_require_success_size_and_checksum() {
        let failure = validate_upload_receipt(
            &json!({"error":{"code":"denied","message":"Write disabled"}}),
            3,
            "abc",
        )
        .unwrap_err();
        assert!(failure.to_string().contains("Write disabled"));
        assert!(validate_upload_receipt(&json!({"size":3,"sha256":"abc"}), 3, "abc").is_ok());
        assert!(validate_upload_receipt(&json!({"size":2,"sha256":"abc"}), 3, "abc").is_err());
        assert!(validate_upload_receipt(&json!({"size":3,"sha256":"def"}), 3, "abc").is_err());
        assert!(validate_upload_receipt(&json!({"size":3}), 3, "abc").is_err());
    }
    #[test]
    fn remote_error_is_visible_and_missing_result_is_rejected() {
        let message = json!({"error":{"code":"permission_denied","message":"Shell disabled"}});
        assert!(
            control_result(&message)
                .unwrap_err()
                .to_string()
                .contains("Shell disabled")
        );
        assert!(control_result(&json!({"id":1})).is_err());
        assert_eq!(
            control_result(&json!({"result":{"ok":true}})).unwrap(),
            json!({"ok":true})
        );
    }
    #[test]
    fn terminal_retention_respects_utf8_boundary() {
        let (state, _) = watch::channel(State::default());
        append_terminal(&state, "你".repeat(MAX_TEXT).as_bytes());
        assert!(state.borrow().terminal.len() <= MAX_TEXT);
        assert!(state.borrow().terminal.chars().all(|c| c == '你'));
    }
    #[test]
    fn terminal_preserves_utf8_across_network_reads() {
        let mut pending = Vec::new();
        assert!(decode_terminal(&mut pending, &[0xe4, 0xbd], false).is_empty());
        assert_eq!(decode_terminal(&mut pending, &[0xa0, b'!'], false), "你!");
        assert_eq!(decode_terminal(&mut pending, &[0xff, 0xe4], false), "�");
        assert_eq!(decode_terminal(&mut pending, &[], true), "�");
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn control_keeps_fragmented_read_alive_while_sending_requests() {
        let (local, mut peer) = tokio::io::duplex(4096);
        let (tx, rx) = mpsc::channel(4);
        let control = Control(tx);
        let (state, _) = watch::channel(State::default());
        let task = tokio::spawn(control_loop(local, rx, state));
        // Send half of an unsolicited frame, then issue a request before completing it.
        let event = serde_json::to_vec(&json!({"event":"ready"})).unwrap();
        let prefix = (event.len() as u32).to_be_bytes();
        peer.write_all(&prefix[..2]).await.unwrap();
        let request = tokio::spawn(async move { control.call("capabilities", json!({})).await });
        let outbound = records::read(&mut peer, records::MAX_CONTROL)
            .await
            .unwrap()
            .unwrap();
        let outbound: Value = serde_json::from_slice(&outbound).unwrap();
        peer.write_all(&prefix[2..]).await.unwrap();
        peer.write_all(&event).await.unwrap();
        records::json(
            &mut peer,
            &json!({"id":outbound["id"],"result":{"desktop":false}}),
        )
        .await
        .unwrap();
        assert_eq!(request.await.unwrap().unwrap(), json!({"desktop":false}));
        task.abort();
    }
}
