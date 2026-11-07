//! Bounded, cancellable controller operations. NSH never polls network I/O on its UI thread.
use crate::{
    desktop::Display,
    input::Bridge,
    permissions::{Decision, EffectiveCapabilities, Permissions, Scope},
    updates::{self, UpdateState},
};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use lightremote_core::{
    crypto::Role,
    invite::Invite,
    link::{self, LinkHandle, LinkHealth, LinkPhase},
    records,
};
use lightremote_updater::SessionGate;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
};

const TIMEOUT: Duration = Duration::from_secs(25);
const MAX_TEXT: usize = 128 * 1024;
const MAX_FILE: u64 = 512 * 1024 * 1024;
pub const MAX_CLIPBOARD_BYTES: usize = 256 * 1024;

#[derive(Clone, Default, PartialEq)]
pub struct State {
    pub connected: bool,
    pub connecting: bool,
    pub update: UpdateState,
    pub status: String,
    pub error: String,
    pub capabilities: String,
    pub effective: EffectiveCapabilities,
    pub permissions: Option<Permissions>,
    pub permission_epochs: [u64; 7],
    pub permission_notice: String,
    pub network: String,
    pub listing: String,
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
    pub fn allows(&self, scope: Scope) -> bool {
        self.connected
            && self.permissions.as_ref().is_some_and(|p| p.allows(scope))
            && self.effective.allows(scope)
    }

    pub fn begin_connect(&mut self) -> bool {
        if self.connected
            || self.connecting
            || self.update.installing
            || self.update.restart_required
        {
            return false;
        }
        self.connecting = true;
        true
    }

    pub fn begin_install(&mut self) -> bool {
        if self.connected
            || self.connecting
            || self.update.installing
            || self.update.restart_required
        {
            self.update.message = "Installation requires an idle session. Disconnect first, or finish the pending update.".into();
            return false;
        }
        self.update.installing = true;
        self.update.message = "Downloading and verifying update… Keep the application open.".into();
        true
    }

    pub fn permission_reason(&self, scope: Scope) -> &'static str {
        if !self.connected {
            "Connect to a host first"
        } else if self.permissions.as_ref().is_some_and(|p| p.allows(scope))
            && !self.effective.allows(scope)
        {
            "Granted · unavailable"
        } else {
            self.permissions
                .as_ref()
                .map_or("Checking host permissions…", |p| p.status(scope))
        }
    }

    fn set_effective(&mut self, effective: EffectiveCapabilities) {
        for scope in Scope::ALL {
            if self.allows(scope) && !effective.allows(scope) {
                self.permission_epochs[scope as usize] += 1;
                self.clear_scope(scope);
            }
        }
        self.effective = effective;
    }

    fn clear_scope(&mut self, scope: Scope) {
        match scope {
            Scope::Desktop => {
                self.desktop_running = false;
                self.display = None;
                self.desktop = Arc::from("");
                self.screenshot = Arc::from([]);
            }
            Scope::Shell => {
                self.shell_running = false;
                self.shell_id = None;
                self.terminal.clear();
                self.terminal_total = 0;
                self.terminal_epoch += 1;
            }
            Scope::ClipboardRead => {
                self.clipboard.clear();
                self.clipboard_revision += 1;
            }
            Scope::FileRead => self.listing.clear(),
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VideoPreset {
    #[default]
    Standard,
    LowBandwidth,
}

impl VideoPreset {
    fn metadata(self, display: u32, single: bool) -> Value {
        let (fps, quality) = if single {
            (1, 85)
        } else {
            match self {
                Self::Standard => (10, 70),
                Self::LowBandwidth => (3, 40),
            }
        };
        json!({"display":display,"fps":fps,"quality":quality,"single":single})
    }
}

pub enum Command {
    Connect(String),
    CheckUpdate,
    InstallUpdate,
    ApplyUpdateOnExit,
    AutomaticUpdates(bool),
    Disconnect,
    Capabilities,
    RequestPermission(Scope),
    CancelPermission,
    Displays,
    Desktop {
        display: u32,
        preset: VideoPreset,
        single: bool,
    },
    StopDesktop,
    OpenShell,
    CloseShell,
    Shell(Vec<u8>),
    ResizeShell {
        cols: u16,
        rows: u16,
    },
    List(String),
    Download {
        remote: String,
        local: PathBuf,
    },
    Upload {
        remote: String,
        local: PathBuf,
    },
    Text {
        display: u32,
        text: String,
    },
    SaveScreenshot(PathBuf),
    ClipboardGet,
    ClipboardSet(String),
}

#[derive(Clone)]
pub struct Client {
    commands: mpsc::Sender<Command>,
    update_commands: mpsc::Sender<updates::Command>,
    state: watch::Sender<State>,
    input: Bridge,
}

impl Client {
    pub fn start() -> Result<(Self, watch::Receiver<State>)> {
        let (commands, receiver) = mpsc::channel(16);
        let (update_commands, update_receiver) = mpsc::channel(4);
        let executable = std::env::current_exe().context("Locate running executable")?;
        let gate = SessionGate::for_executable(&executable)
            .context("Open session/update admission gate")?;
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
            .name("lightremote-client".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let update_task = tokio::spawn(updates::run(
                        update_receiver,
                        output.clone(),
                        gate.clone(),
                        executable,
                    ));
                    run(receiver, output, network_input, gate).await;
                    update_task.abort();
                    let _ = update_task.await;
                });
            })
            .context("Start network worker")?;
        Ok((
            Self {
                commands,
                update_commands,
                state,
                input,
            },
            updates,
        ))
    }

    pub fn send(&self, command: Command) {
        let update = match command {
            Command::CheckUpdate => Some(updates::Command::Check),
            Command::InstallUpdate => Some(updates::Command::Install),
            Command::ApplyUpdateOnExit => Some(updates::Command::ApplyDeferred),
            Command::AutomaticUpdates(enabled) => Some(updates::Command::Automatic(enabled)),
            _ => None,
        };
        if let Some(update) = update {
            if self.update_commands.try_send(update).is_err() {
                self.report("Update queue is busy or stopped. Try again.");
            }
            return;
        }
        if self.commands.try_send(command).is_err() {
            error(
                &self.state,
                "The operation queue is full or the network worker stopped. Try again.",
            );
        }
    }

    pub fn arm_input(&self) {
        let allowed = {
            let state = self.state.borrow();
            state.allows(Scope::Input)
        };
        if allowed {
            self.input.arm();
        }
    }

    pub fn release_input(&self) {
        self.input.release();
    }

    pub fn forward_input(&self, display: u32, events: Vec<Value>) -> bool {
        let allowed = {
            let state = self.state.borrow();
            state.allows(Scope::Input)
        };
        if !allowed || !self.input.is_armed() {
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
}

fn error(state: &watch::Sender<State>, message: impl Into<String>) {
    state.send_modify(|s| s.error = message.into());
}

fn append_terminal(state: &mut State, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    state.terminal_total += text.len() as u64;
    state.terminal.push_str(&text);
    if state.terminal.len() > MAX_TEXT {
        let mut cut = state.terminal.len() - MAX_TEXT;
        while !state.terminal.is_char_boundary(cut) {
            cut += 1;
        }
        state.terminal.drain(..cut);
    }
}

fn publish(
    state: &watch::Sender<State>,
    scope: Scope,
    epoch: u64,
    write: impl FnOnce(&mut State),
) -> Result<()> {
    let mut accepted = false;
    state.send_modify(|s| {
        if s.allows(scope) && s.permission_epochs[scope as usize] == epoch {
            write(s);
            accepted = true;
        }
    });
    ensure!(
        accepted,
        "{} permission or capability changed; stale result discarded",
        scope.label()
    );
    Ok(())
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
                    if request.reply.is_closed() { continue; }
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
        } else if message["event"] == "permissions.changed" {
            apply_permissions(&state, &message["data"])?;
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

async fn run(
    mut commands: mpsc::Receiver<Command>,
    state: watch::Sender<State>,
    input: Bridge,
    gate: SessionGate,
) {
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
        let mut admitted = false;
        state.send_modify(|s| admitted = s.begin_connect());
        if !admitted {
            error(
                &state,
                "Finish the update and restart LightRemote before connecting.",
            );
            continue;
        }
        let _session = match gate.enter() {
            Ok(session) => session,
            Err(failure) => {
                state.send_modify(|s| s.connecting = false);
                error(&state, format!("Session admission failed: {failure}"));
                continue;
            }
        };
        state.send_modify(|s| {
            s.error.clear();
            s.status = "Connecting and authenticating…".into();
        });
        let connection = {
            let connect = tokio::time::timeout(
                Duration::from_secs(95),
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
            s.connecting = false;
            s.connected = true;
            s.error.clear();
            s.status = "Authenticated".into();
        });
        let (requests, request_rx) = mpsc::channel(16);
        let control = Control(requests);
        let handle = connection.handle.clone();
        let mut workers = JoinSet::new();
        let output = state.clone();
        let (control_closed, mut control_failure) = oneshot::channel();
        workers.spawn(async move {
            let result = async {
                let stream = handle.open("control", json!({})).await?;
                control_loop(stream, request_rx, output).await
            }
            .await;
            let _ = control_closed.send(());
            result
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
        let mut permission_updates = state.subscribe();
        let mut health = connection.handle.health();
        state.send_modify(|s| s.network = health_summary(*health.borrow()));
        let mut input_epoch = {
            let current = state.borrow();
            (
                current.permission_epochs[Scope::Input as usize],
                current.permission_epochs[Scope::Desktop as usize],
            )
        };
        let mut permission_revision = None;
        let mut desktop: Option<tokio::task::AbortHandle> = None;
        let mut shell: Option<tokio::task::AbortHandle> = None;
        let mut shell_input: Option<mpsc::Sender<Vec<u8>>> = None;
        let mut connection_finished = false;
        loop {
            tokio::select! {
                _ = &mut control_failure => {
                    error(&state, "Control channel ended. Create a fresh invitation to reconnect; operations were not replayed.");
                    break;
                }
                changed = health.changed() => {
                    if changed.is_err() { break; }
                    let current = *health.borrow_and_update();
                    state.send_modify(|s| s.network = health_summary(current));
                    if current.phase == LinkPhase::Closed {
                        error(&state, "Connection lost. Create a fresh invitation to reconnect; pending operations were not replayed.");
                        break;
                    }
                }
                changed = permission_updates.changed() => {
                    if changed.is_err() { break; }
                    let current = permission_updates.borrow_and_update();
                    let next_epoch = (current.permission_epochs[Scope::Input as usize], current.permission_epochs[Scope::Desktop as usize]);
                    if !current.allows(Scope::Input) || next_epoch != input_epoch { input.release(); }
                    input_epoch = next_epoch;
                    if !current.allows(Scope::Desktop) && let Some(task) = desktop.take() { task.abort(); }
                    if !current.allows(Scope::Shell) {
                        shell_input = None;
                        if let Some(task) = shell.take() { tokio::task::AbortHandle::abort(&task); }
                    }
                    let revision = current.permissions.as_ref().map(|p| p.revision);
                    drop(current);
                    if revision.is_some() && revision != permission_revision {
                        permission_revision = revision;
                        spawn_capabilities(&mut workers, control.clone(), state.clone());
                    }
                }
                result = &mut connection.task => {
                    connection_finished = true;
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
                    let denied = command_scope(&command).filter(|scope| !state.borrow().allows(*scope));
                    if let Some(scope) = denied {
                        let reason = state.borrow().permission_reason(scope);
                        error(&state, format!("{}: {reason}. Use Permissions to request access.", scope.label()));
                        continue;
                    }
                    state.send_modify(|s| s.error.clear());
                    match command {
                        Command::Disconnect => break,
                        Command::CheckUpdate | Command::InstallUpdate | Command::ApplyUpdateOnExit | Command::AutomaticUpdates(_) => {},
                        Command::Connect(_) => error(&state, "Disconnect before opening another invitation."),
                        Command::Capabilities => spawn_capabilities(&mut workers, control.clone(), state.clone()),
                        Command::RequestPermission(scope) => {
                            if !state.borrow().permissions.as_ref().is_some_and(|p| p.can_request(scope)) {
                                error(&state, "This permission cannot be requested now. Wait for host approval or ask the host to change its policy.");
                                continue;
                            }
                            let control = control.clone(); let state = state.clone();
                            workers.spawn(async move {
                                let snapshot = control.call("permissions.request", json!({"scopes":[scope.name()]})).await?;
                                apply_permissions(&state, &snapshot)
                            });
                        }
                        Command::CancelPermission => {
                            let id = state.borrow().permissions.as_ref().and_then(|p| p.pending.as_ref().map(|p| p.id));
                            if let Some(id) = id {
                                let control = control.clone(); let state = state.clone();
                                workers.spawn(async move {
                                    let snapshot = control.call("permissions.cancel", json!({"id":id})).await?;
                                    apply_permissions(&state, &snapshot)
                                });
                            }
                        }
                        Command::Displays => {
                            let control = control.clone(); let state = state.clone();
                            workers.spawn(async move {
                                let epoch = state.borrow().permission_epochs[Scope::Desktop as usize];
                                let value = scoped(state.clone(), Scope::Desktop, control.call("desktop.displays", json!({}))).await?;
                                publish(&state, Scope::Desktop, epoch, |s| s.capabilities = display_summary(&value))
                            });
                        }
                        Command::Desktop { display, preset, single } => {
                            input.release();
                            if let Some(task) = desktop.take() { tokio::task::AbortHandle::abort(&task); }
                            let handle = connection.handle.clone(); let state = state.clone();
                            let control = control.clone();
                            state.send_modify(|s| { s.desktop_running = true; s.display = None; s.desktop = Arc::from(""); });
                            desktop = Some(workers.spawn(async move {
                                let result = scoped(state.clone(), Scope::Desktop, desktop_loop(handle, control, display, preset, single, state.clone())).await;
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
                                let result = scoped(state.clone(), Scope::Shell, shell_loop(handle, rx, state.clone())).await;
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
                                let state = state.clone();
                                workers.spawn(async move { scoped(state, Scope::Shell, control.call("shell.resize", json!({"stream_id":stream_id,"cols":cols,"rows":rows}))).await?; Ok(()) });
                            }
                        }
                        Command::Shell(bytes) => {
                            if bytes.len() > 16384 || shell_input.as_ref().is_none_or(|tx| tx.try_send(bytes).is_err()) {
                                error(&state, "Open a shell first, or wait for its input queue to drain.");
                            }
                        }
                        Command::ClipboardGet => {
                            let control = control.clone(); let state = state.clone();
                            workers.spawn(async move { scoped(state.clone(), Scope::ClipboardRead, read_remote_clipboard(control, state)).await });
                        }
                        Command::ClipboardSet(text) => {
                            let control = control.clone(); let state = state.clone();
                            workers.spawn(async move { scoped(state.clone(), Scope::ClipboardWrite, write_remote_clipboard(control, state, text)).await });
                        }
                        Command::List(path) => {
                            let control = control.clone(); let state = state.clone();
                            workers.spawn(async move {
                                let epoch = state.borrow().permission_epochs[Scope::FileRead as usize];
                                let value = scoped(state.clone(), Scope::FileRead, control.call("fs.list", json!({"path":path}))).await?;
                                publish(&state, Scope::FileRead, epoch, |s| s.listing = pretty(&value))
                            });
                        }
                        Command::Download { remote, local } => {
                            let handle = connection.handle.clone(); let state = state.clone();
                            workers.spawn(async move {
                                scoped(state.clone(), Scope::FileRead, download(handle, remote, local)).await?;
                                state.send_modify(|s| s.status = "Download complete".into()); Ok(())
                            });
                        }
                        Command::Upload { remote, local } => {
                            let handle = connection.handle.clone(); let state = state.clone();
                            workers.spawn(async move {
                                let receipt = scoped(state.clone(), Scope::FileWrite, upload(handle, remote, local)).await?;
                                state.send_modify(|s| s.status = format!("Upload complete: {receipt}")); Ok(())
                            });
                        }
                        Command::SaveScreenshot(path) => {
                            let state = state.clone();
                            workers.spawn(async move { save_screenshot(state, path).await });
                        }
                        Command::Text { display, text } => {
                            ensure_text(&state, &text);
                            if !text.is_empty() && text.len() <= 16384 {
                                input.arm();
                                if !input.send(display, vec![json!({"type":"text","text":text})]) { error(&state, "Remote input is unavailable; reconnect to retry."); }
                            }
                        }
                    }
                }
            }
        }
        state.send_modify(|s| {
            s.connected = false;
        });
        input.detach();
        if tokio::time::timeout(Duration::from_secs(3), &mut input_task)
            .await
            .is_err()
        {
            input_task.abort();
            let _ = input_task.await;
        }
        workers.abort_all();
        while workers.join_next().await.is_some() {}
        if !connection_finished {
            connection.task.abort();
            let _ = (&mut connection.task).await;
        }
        drop(connection);
        state.send_modify(|s| {
            s.connected = false;
            s.connecting = false;
            s.desktop_running = false;
            s.shell_running = false;
            s.desktop = Arc::from("");
            s.display = None;
            s.capabilities.clear();
            s.effective = EffectiveCapabilities::default();
            s.permissions = None;
            s.permission_notice.clear();
            s.network.clear();
            s.screenshot = Arc::from([]);
            s.listing.clear();
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

fn command_scope(command: &Command) -> Option<Scope> {
    match command {
        Command::Displays | Command::Desktop { .. } => Some(Scope::Desktop),
        Command::Text { .. } => Some(Scope::Input),
        Command::OpenShell | Command::Shell(_) | Command::ResizeShell { .. } => Some(Scope::Shell),
        Command::List(_) | Command::Download { .. } => Some(Scope::FileRead),
        Command::Upload { .. } => Some(Scope::FileWrite),
        Command::ClipboardGet => Some(Scope::ClipboardRead),
        Command::ClipboardSet(_) => Some(Scope::ClipboardWrite),
        _ => None,
    }
}

fn apply_permissions(state: &watch::Sender<State>, value: &Value) -> Result<()> {
    let permissions = Permissions::parse(value)?;
    state.send_modify(|s| {
        // A reply can arrive after a newer notification. Never resurrect revoked access.
        if s.permissions
            .as_ref()
            .is_some_and(|p| p.revision >= permissions.revision)
        {
            return;
        }
        if let Some(previous) = &s.permissions {
            let revoked = Scope::ALL
                .into_iter()
                .filter(|scope| previous.allows(*scope) && !permissions.allows(*scope))
                .map(Scope::label)
                .collect::<Vec<_>>();
            if !revoked.is_empty() {
                s.permission_notice = format!(
                    "Host revoked: {}. Related operations stopped.",
                    revoked.join(", ")
                );
            } else if previous.pending.is_some() && permissions.pending.is_none() {
                s.permission_notice = match permissions.last_decision {
                    Some(Decision::Approved) => {
                        "Host approved permissions. Choose an operation to start it."
                    }
                    Some(Decision::Denied) => {
                        "Host denied the permission request. You may ask again when appropriate."
                    }
                    Some(Decision::Cancelled) => "Permission request cancelled.",
                    None => "Permission request closed. Current grants are shown above.",
                }
                .into();
            } else if permissions.pending.is_some() {
                s.permission_notice =
                    "Waiting for the host to approve or deny your request…".into();
            }
        }
        for scope in Scope::ALL {
            if s.permissions.as_ref().is_some_and(|p| p.allows(scope)) && !permissions.allows(scope)
            {
                s.permission_epochs[scope as usize] += 1;
            }
        }
        for scope in Scope::ALL {
            if !permissions.allows(scope) {
                s.effective.revoke(scope);
                s.clear_scope(scope);
            }
        }
        s.permissions = Some(permissions);
    });
    Ok(())
}

fn apply_capabilities(state: &watch::Sender<State>, value: &Value) -> Result<()> {
    let permissions = Permissions::parse(&value["permissions"])?;
    if state
        .borrow()
        .permissions
        .as_ref()
        .is_some_and(|current| current.revision > permissions.revision)
    {
        return Ok(());
    }
    let effective = match EffectiveCapabilities::parse(value) {
        Ok(effective) => effective,
        Err(error) => {
            state.send_modify(|s| s.set_effective(EffectiveCapabilities::default()));
            return Err(error);
        }
    };
    apply_permissions(state, &value["permissions"])?;
    state.send_modify(|s| {
        // A newer notification may have arrived while the capability probe was pending.
        if s.permissions
            .as_ref()
            .is_none_or(|current| current.revision != permissions.revision)
        {
            return;
        }
        s.set_effective(effective);
        s.capabilities = capability_summary(value);
    });
    Ok(())
}

async fn scoped<T>(
    state: watch::Sender<State>,
    scope: Scope,
    operation: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    let mut updates = state.subscribe();
    ensure!(
        updates.borrow().allows(scope),
        "{} permission is no longer granted",
        scope.label()
    );
    let epoch = updates.borrow().permission_epochs[scope as usize];
    tokio::pin!(operation);
    loop {
        tokio::select! {
            biased;
            changed = updates.changed() => {
                changed.context("Session state closed")?;
                let current = updates.borrow_and_update();
                ensure!(current.allows(scope) && current.permission_epochs[scope as usize] == epoch, "{} permission revoked or capability unavailable; operation stopped", scope.label());
            }
            result = &mut operation => return result,
        }
    }
}

fn health_summary(health: LinkHealth) -> String {
    let phase = match health.phase {
        LinkPhase::Healthy => "Network healthy",
        LinkPhase::Degraded => "Network slow · operations may take longer",
        LinkPhase::Closed => "Connection closed · use a fresh invitation",
        _ => "Network status unavailable",
    };
    match health.rtt_ms {
        Some(rtt) => format!("{phase} · RTT {rtt} ms"),
        None => format!("{phase} · measuring RTT…"),
    }
}

fn display_summary(value: &Value) -> String {
    let Some(displays) = value["displays"].as_array() else {
        return "Host returned no display information".into();
    };
    displays
        .iter()
        .map(|display| {
            format!(
                "Display {} · {} × {}",
                display["id"], display["width"], display["height"]
            )
        })
        .collect::<Vec<_>>()
        .join("  |  ")
}

fn capability_summary(value: &Value) -> String {
    let enabled = [
        "desktop",
        "input",
        "shell",
        "file_read",
        "file_write",
        "clipboard_read",
        "clipboard_write",
    ]
    .into_iter()
    .filter(|name| value[*name] == true)
    .collect::<Vec<_>>();
    if enabled.is_empty() {
        "Host connected · permissions are listed below".into()
    } else {
        format!("Host capabilities: {}", enabled.join(", "))
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
    let epoch = state.borrow().permission_epochs[Scope::ClipboardRead as usize];
    let response = control.call("clipboard.get", json!({})).await?;
    let text = clipboard_text(&response)?;
    publish(&state, Scope::ClipboardRead, epoch, |s| {
        s.clipboard = text;
        s.clipboard_revision += 1;
        s.status = "Remote clipboard loaded into editor".into();
    })
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
    state.send_modify(|s| s.status = "Remote clipboard written".into());
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
        apply_capabilities(&state, &value)
    });
}

async fn desktop_loop(
    handle: LinkHandle,
    control: Control,
    display: u32,
    preset: VideoPreset,
    single: bool,
    state: watch::Sender<State>,
) -> Result<()> {
    let epoch = state.borrow().permission_epochs[Scope::Desktop as usize];
    let geometry =
        Display::from_response(&control.call("desktop.displays", json!({})).await?, display)?;
    publish(&state, Scope::Desktop, epoch, |s| {
        s.display = Some(geometry)
    })?;
    let mut stream = handle
        .open("desktop", preset.metadata(display, single))
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
            publish(&state, Scope::Desktop, epoch, |s| {
                s.desktop = image;
                s.screenshot = bytes;
            })?;
            return Ok(());
        }
        publish(&state, Scope::Desktop, epoch, |s| {
            s.desktop = image;
            s.screenshot = bytes;
        })?;
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
    let epoch = state.borrow().permission_epochs[Scope::Shell as usize];
    let mut stream = handle.open("shell", json!({"cols":100,"rows":28})).await?;
    publish(&state, Scope::Shell, epoch, |s| {
        s.shell_id = Some(stream.id)
    })?;
    let mut pending = Vec::new();
    {
        let (mut reader, mut writer) = tokio::io::split(&mut stream);
        let mut buffer = [0; 8192];
        loop {
            tokio::select! {
                read = reader.read(&mut buffer) => {
                    let n = read?;
                    let text = decode_terminal(&mut pending, &buffer[..n], n == 0);
                    publish(&state, Scope::Shell, epoch, |s| append_terminal(s, text.as_bytes()))?;
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

    fn snapshot(revision: u64, granted: &[&str]) -> Value {
        json!({"available":["desktop","input","shell","file_read","file_write","clipboard_read","clipboard_write"],"granted":granted,"pending":null,"revision":revision,"mode":"attended"})
    }

    fn capabilities(revision: u64, granted: &[&str]) -> Value {
        let mut value = json!({"permissions":snapshot(revision, granted),"routes":["control","desktop","input","shell","file.get","file.put"]});
        for scope in Scope::ALL {
            value[scope.name()] = json!(granted.contains(&scope.name()));
        }
        value
    }

    fn connected_state() -> State {
        State {
            connected: true,
            effective: EffectiveCapabilities::parse(&capabilities(0, &Scope::ALL.map(Scope::name)))
                .unwrap(),
            ..State::default()
        }
    }

    #[test]
    fn grants_need_effective_bools_and_routes_and_stale_caps_cannot_restore_revoked_access() {
        let (state, _) = watch::channel(State {
            connected: true,
            ..State::default()
        });
        let all = Scope::ALL.map(Scope::name);
        let mut unavailable = capabilities(1, &all);
        for scope in Scope::ALL {
            unavailable[scope.name()] = json!(false);
        }
        apply_capabilities(&state, &unavailable).unwrap();
        for scope in Scope::ALL {
            assert!(
                !state.borrow().allows(scope),
                "{} should be unavailable",
                scope.name()
            );
            assert_eq!(
                state.borrow().permission_reason(scope),
                "Granted · unavailable"
            );
        }
        let mut working = capabilities(1, &all);
        working["routes"] = json!(["control"]);
        apply_capabilities(&state, &working).unwrap();
        assert!(!state.borrow().allows(Scope::Desktop));
        assert!(!state.borrow().allows(Scope::Input));
        assert!(!state.borrow().allows(Scope::Shell));
        assert!(!state.borrow().allows(Scope::FileRead));
        assert!(!state.borrow().allows(Scope::FileWrite));
        assert!(state.borrow().allows(Scope::ClipboardWrite));
        apply_capabilities(&state, &capabilities(1, &all)).unwrap();
        for scope in Scope::ALL {
            assert!(state.borrow().allows(scope));
        }
        assert!(!state.borrow().desktop_running && !state.borrow().shell_running);
        apply_permissions(&state, &snapshot(2, &[])).unwrap();
        apply_capabilities(&state, &capabilities(1, &all)).unwrap();
        for scope in Scope::ALL {
            assert!(!state.borrow().allows(scope));
        }
        apply_permissions(&state, &snapshot(3, &all)).unwrap();
        for scope in Scope::ALL {
            assert!(
                !state.borrow().allows(scope),
                "Regrant must await fresh effective capabilities"
            );
        }
        apply_capabilities(&state, &capabilities(3, &all)).unwrap();
        for scope in Scope::ALL {
            assert!(state.borrow().allows(scope));
        }
    }

    #[test]
    fn input_grant_uses_its_backend_independently_of_desktop_view_permission() {
        let (state, _) = watch::channel(State {
            connected: true,
            ..State::default()
        });
        apply_capabilities(&state, &capabilities(1, &["desktop", "input"])).unwrap();
        apply_permissions(&state, &snapshot(2, &["input"])).unwrap();
        assert!(state.borrow().allows(Scope::Input));
        assert!(!state.borrow().allows(Scope::Desktop));
        let (commands, _) = mpsc::channel(1);
        let (update_commands, _) = mpsc::channel(1);
        let bridge = Bridge::new();
        let mut input = bridge.attach();
        let client = Client {
            commands,
            update_commands,
            state: state.clone(),
            input: bridge.clone(),
        };
        client.arm_input();
        assert!(bridge.is_armed());
        assert!(client.forward_input(0, vec![json!({"type":"text","text":"test"})]));
        assert!(input.try_recv().is_ok());
        let mut no_screen = capabilities(2, &["input"]);
        no_screen["input"] = json!(false);
        apply_capabilities(&state, &no_screen).unwrap();
        assert!(!state.borrow().allows(Scope::Input));
        assert!(!client.forward_input(0, Vec::new()));
        assert_eq!(
            command_scope(&Command::Text {
                display: 0,
                text: "test".into()
            }),
            Some(Scope::Input)
        );
        bridge.detach();
    }

    #[tokio::test]
    async fn capability_refresh_can_recover_the_backend_without_starting_a_stream() {
        let (state, _) = watch::channel(State {
            connected: true,
            ..State::default()
        });
        let (local, mut peer) = tokio::io::duplex(4096);
        let (requests, rx) = mpsc::channel(4);
        let control = Control(requests);
        let runtime = tokio::spawn(control_loop(local, rx, state.clone()));
        let (done, finished) = oneshot::channel();
        let service = tokio::spawn(async move {
            for enabled in [false, true] {
                let request = records::read(&mut peer, records::MAX_CONTROL)
                    .await
                    .unwrap()
                    .unwrap();
                let request: Value = serde_json::from_slice(&request).unwrap();
                assert_eq!(request["method"], "capabilities");
                let mut result = capabilities(1, &["desktop", "input"]);
                result["desktop"] = json!(enabled);
                result["input"] = json!(enabled);
                records::json(&mut peer, &json!({"id":request["id"],"result":result}))
                    .await
                    .unwrap();
            }
            let _ = finished.await;
        });
        let mut workers = JoinSet::new();
        spawn_capabilities(&mut workers, control.clone(), state.clone());
        workers.join_next().await.unwrap().unwrap().unwrap();
        assert_eq!(
            state.borrow().permission_reason(Scope::Input),
            "Granted · unavailable"
        );
        spawn_capabilities(&mut workers, control, state.clone());
        workers.join_next().await.unwrap().unwrap().unwrap();
        assert!(state.borrow().allows(Scope::Desktop) && state.borrow().allows(Scope::Input));
        assert!(!state.borrow().desktop_running);
        let _ = done.send(());
        service.await.unwrap();
        runtime.abort();
        let _ = runtime.await;
    }

    #[tokio::test]
    async fn lost_effective_capability_invalidates_running_work_even_if_probe_recovers() {
        let (state, _) = watch::channel(State {
            connected: true,
            ..State::default()
        });
        apply_capabilities(&state, &capabilities(1, &["shell"])).unwrap();
        let output = state.clone();
        let (started, ready) = oneshot::channel();
        let task = tokio::spawn(async move {
            scoped(output, Scope::Shell, async move {
                started.send(()).unwrap();
                std::future::pending::<Result<()>>().await
            })
            .await
        });
        ready.await.unwrap();
        let mut unavailable = capabilities(1, &["shell"]);
        unavailable["shell"] = json!(false);
        apply_capabilities(&state, &unavailable).unwrap();
        apply_capabilities(&state, &capabilities(1, &["shell"])).unwrap();
        assert!(state.borrow().allows(Scope::Shell));
        assert!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("revoked")
        );
    }

    #[test]
    fn video_presets_only_map_to_existing_desktop_metadata() {
        assert_eq!(
            VideoPreset::Standard.metadata(7, false),
            json!({"display":7,"fps":10,"quality":70,"single":false})
        );
        assert_eq!(
            VideoPreset::LowBandwidth.metadata(7, false),
            json!({"display":7,"fps":3,"quality":40,"single":false})
        );
        for preset in [VideoPreset::Standard, VideoPreset::LowBandwidth] {
            assert_eq!(
                preset.metadata(2, true),
                json!({"display":2,"fps":1,"quality":85,"single":true})
            );
            assert_eq!(
                command_scope(&Command::Desktop {
                    display: 2,
                    preset,
                    single: false
                }),
                Some(Scope::Desktop)
            );
        }
    }

    #[test]
    fn connection_and_install_admission_exclude_each_other_before_any_network_work() {
        let mut state = State::default();
        assert!(state.begin_connect());
        assert!(!state.begin_install());
        state.connecting = false;
        state.connected = true;
        assert!(!state.begin_install());
        state.connected = false;
        assert!(state.begin_install());
        assert!(!state.begin_connect());
        state.update.installing = false;
        state.update.restart_required = true;
        assert!(!state.begin_connect());
        assert!(!state.begin_install());
    }

    #[tokio::test]
    async fn idle_disconnect_keeps_the_network_worker_resident() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let gate = SessionGate::new(&directory.path().join("sessions.lock")).unwrap();
        let (commands, receiver) = mpsc::channel(16);
        let (state, mut changes) = watch::channel(State::default());
        let worker = tokio::spawn(run(receiver, state.clone(), Bridge::new(), gate));
        commands.send(Command::Disconnect).await.unwrap();
        commands.send(Command::Capabilities).await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            changes.wait_for(|s| s.error.contains("Connect to a host")),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!worker.is_finished());
        commands.send(Command::Disconnect).await.unwrap();
        commands
            .send(Command::Connect("invalid".into()))
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            changes.wait_for(|s| s.error.contains("Invalid invitation")),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!worker.is_finished());
        drop(commands);
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn revoked_snapshots_clear_sensitive_views_and_ignore_stale_replies() {
        let (state, _) = watch::channel(connected_state());
        apply_permissions(
            &state,
            &snapshot(
                1,
                &["desktop", "input", "shell", "file_read", "clipboard_read"],
            ),
        )
        .unwrap();
        state.send_modify(|s| {
            s.desktop = Arc::from("frame");
            s.screenshot = Arc::from(&b"jpeg"[..]);
            s.desktop_running = true;
            s.shell_running = true;
            s.shell_id = Some(7);
            s.clipboard = "private".into();
            s.terminal = "private".into();
            s.listing = "private".into();
        });
        apply_permissions(&state, &snapshot(3, &[])).unwrap();
        let revoked = state.borrow();
        assert!(!revoked.desktop_running && !revoked.shell_running);
        assert!(revoked.desktop.is_empty() && revoked.screenshot.is_empty());
        assert!(
            revoked.clipboard.is_empty()
                && revoked.terminal.is_empty()
                && revoked.listing.is_empty()
        );
        assert_eq!(revoked.shell_id, None);
        assert!(revoked.permission_notice.contains("Host revoked"));
        drop(revoked);
        apply_permissions(&state, &snapshot(2, &["desktop", "input"])).unwrap();
        assert!(!state.borrow().allows(Scope::Input));
        assert_eq!(state.borrow().permissions.as_ref().unwrap().revision, 3);
    }

    #[tokio::test]
    async fn revoke_regrant_never_resumes_a_previously_accepted_operation() {
        let (state, _) = watch::channel(connected_state());
        apply_permissions(&state, &snapshot(1, &["file_write"])).unwrap();
        let output = state.clone();
        let (started, ready) = oneshot::channel();
        let task = tokio::spawn(async move {
            scoped(output, Scope::FileWrite, async move {
                started.send(()).unwrap();
                std::future::pending::<Result<()>>().await
            })
            .await
        });
        ready.await.unwrap();
        // watch can coalesce these two snapshots; the epoch must still invalidate work.
        apply_permissions(&state, &snapshot(2, &[])).unwrap();
        apply_capabilities(&state, &capabilities(3, &["file_write"])).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(result.unwrap_err().to_string().contains("revoked"));
        assert!(state.borrow().allows(Scope::FileWrite));
    }

    #[test]
    fn completed_results_cannot_repopulate_a_revoked_view_after_regrant() {
        let (state, _) = watch::channel(connected_state());
        apply_permissions(&state, &snapshot(1, &["clipboard_read"])).unwrap();
        let epoch = state.borrow().permission_epochs[Scope::ClipboardRead as usize];
        apply_permissions(&state, &snapshot(2, &[])).unwrap();
        apply_permissions(&state, &snapshot(3, &["clipboard_read"])).unwrap();
        assert!(
            publish(&state, Scope::ClipboardRead, epoch, |s| s.clipboard =
                "stale result".into())
            .is_err()
        );
        assert!(state.borrow().clipboard.is_empty());
    }

    #[tokio::test]
    async fn denied_operations_never_poll_their_side_effect() {
        let (state, _) = watch::channel(connected_state());
        let ran = std::cell::Cell::new(false);
        let result = scoped(state, Scope::Shell, async {
            ran.set(true);
            Ok(())
        })
        .await;
        assert!(result.is_err());
        assert!(!ran.get());
    }

    #[tokio::test]
    async fn permission_notifications_are_consumed_without_an_outgoing_request() {
        let (local, mut peer) = tokio::io::duplex(4096);
        let (_tx, rx) = mpsc::channel(4);
        let (state, mut updates) = watch::channel(connected_state());
        let task = tokio::spawn(control_loop(local, rx, state.clone()));
        records::json(
            &mut peer,
            &json!({"event":"permissions.changed","data":snapshot(1, &["input"])}),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), updates.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(updates.borrow_and_update().allows(Scope::Input));
        records::json(
            &mut peer,
            &json!({"event":"permissions.changed","data":snapshot(2, &[])}),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), updates.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(!state.borrow().allows(Scope::Input));
        task.abort();
    }

    #[tokio::test]
    async fn cancelled_queued_control_requests_never_reach_the_peer() {
        let (local, mut peer) = tokio::io::duplex(4096);
        let (tx, rx) = mpsc::channel(4);
        let (reply, response) = oneshot::channel();
        drop(response);
        tx.send(Request {
            method: "clipboard.set",
            params: json!({"text":"must not send"}),
            reply,
        })
        .await
        .unwrap();
        drop(tx);
        let (state, _) = watch::channel(State::default());
        control_loop(local, rx, state).await.unwrap();
        assert!(
            records::read(&mut peer, records::MAX_CONTROL)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn permission_denial_and_cancellation_have_explicit_ui_notices() {
        let (state, _) = watch::channel(connected_state());
        let mut pending = snapshot(1, &[]);
        pending["pending"] = json!({"id":3,"scopes":["shell"]});
        apply_permissions(&state, &pending).unwrap();
        let mut denied = snapshot(2, &[]);
        denied["last_decision"] = json!({"id":3,"outcome":"denied"});
        apply_permissions(&state, &denied).unwrap();
        assert!(state.borrow().permission_notice.contains("Host denied"));
        assert!(!state.borrow().allows(Scope::Shell));
        pending["revision"] = json!(3);
        apply_permissions(&state, &pending).unwrap();
        denied["revision"] = json!(4);
        denied["last_decision"]["outcome"] = json!("cancelled");
        apply_permissions(&state, &denied).unwrap();
        assert!(state.borrow().permission_notice.contains("cancelled"));
    }

    #[test]
    fn command_scopes_keep_read_write_and_clipboard_directions_distinct() {
        assert_eq!(
            command_scope(&Command::ClipboardGet),
            Some(Scope::ClipboardRead)
        );
        assert_eq!(
            command_scope(&Command::ClipboardSet("a".into())),
            Some(Scope::ClipboardWrite)
        );
        assert_eq!(
            command_scope(&Command::Download {
                remote: "a".into(),
                local: "b".into()
            }),
            Some(Scope::FileRead)
        );
        assert_eq!(
            command_scope(&Command::Upload {
                remote: "a".into(),
                local: "b".into()
            }),
            Some(Scope::FileWrite)
        );
        assert_eq!(command_scope(&Command::CancelPermission), None);
        assert_eq!(command_scope(&Command::CloseShell), None);
        assert_eq!(command_scope(&Command::StopDesktop), None);
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
        let (state, _) = watch::channel(connected_state());
        apply_permissions(&state, &snapshot(1, &["clipboard_read", "clipboard_write"])).unwrap();
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
        state.send_modify(|s| append_terminal(s, "你".repeat(MAX_TEXT).as_bytes()));
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
