//! Host services for LightRemote, independent of the TMP actor.
//!
//! [`HostServices::serve`] accepts transport-independent [`ServiceIo`]. The
//! adapter preserves verified FIN as EOF and RESET/carrier failure as I/O errors.
//! Dropping the service future releases held input and terminates its shell tree.

#[cfg(feature = "desktop")]
mod desktop;
mod files;
mod records;
mod shell;

use anyhow::{Context, Result, bail, ensure};
use cap_std::{ambient_authority, fs::Dir};
use files::Files;
use records::{CONTROL_LIMIT, read_record, write_record};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::{Semaphore, broadcast},
    time::timeout,
};

/// Application byte I/O. Adapters must expose verified remote FIN as EOF and
/// RESET/carrier failure as an I/O error, including zero-byte/exact-size uploads.
/// The adapter must independently cancel the service future on RESET/carrier
/// failure even after input FIN, when a silent output drain performs no I/O.
pub trait ServiceIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + ?Sized> ServiceIo for T {}

/// Owned, transport-independent I/O used internally by service workers.
pub type BoxServiceIo = Box<dyn ServiceIo>;

/// Explicit local permissions and resource limits. All permissions default off.
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// The only directory exposed to file services. `None` disables them.
    pub share_dir: Option<PathBuf>,
    pub allow_write: bool,
    pub allow_shell: bool,
    pub allow_desktop: bool,
    pub allow_input: bool,
    pub allow_clipboard: bool,
    /// Local shell executable override. Remote metadata cannot set commands.
    pub shell: Option<PathBuf>,
    /// Maximum upload or download size, default 1 GiB.
    pub max_file_size: u64,
    /// Maximum active service streams, default 16 (hard ceiling 64).
    pub max_concurrent_streams: usize,
    /// Deadline for each pending read, write, or backpressure wait.
    pub io_timeout: Duration,
}
impl Default for HostConfig {
    fn default() -> Self {
        Self {
            share_dir: None,
            allow_write: false,
            allow_shell: false,
            allow_desktop: false,
            allow_input: false,
            allow_clipboard: false,
            shell: None,
            max_file_size: 1024 * 1024 * 1024,
            max_concurrent_streams: 16,
            io_timeout: Duration::from_secs(60),
        }
    }
}

struct Inner {
    config: HostConfig,
    files: Option<Arc<Files>>,
    shells: Arc<shell::Shells>,
    streams: Arc<Semaphore>,
    input: Arc<Semaphore>,
    notifications: broadcast::Sender<Value>,
    #[cfg(feature = "desktop")]
    clipboard: Arc<std::sync::Mutex<desktop::Clipboard>>,
}

/// Cloneable, permission-gated host service dispatcher.
#[derive(Clone)]
pub struct HostServices(Arc<Inner>);
impl HostServices {
    /// Open the explicit share root once, retaining a capability handle.
    pub fn new(config: HostConfig) -> Result<Self> {
        ensure!(config.max_file_size > 0, "max_file_size must be positive");
        ensure!(
            (1..=64).contains(&config.max_concurrent_streams),
            "stream limit must be 1..=64"
        );
        ensure!(!config.io_timeout.is_zero(), "io_timeout must be positive");
        let files = config
            .share_dir
            .as_ref()
            .map(|path| {
                let root = Dir::open_ambient_dir(path, ambient_authority())
                    .context("cannot open share directory")?;
                Ok::<_, anyhow::Error>(Arc::new(Files::new(root, config.max_file_size)))
            })
            .transpose()?;
        let (notifications, _) = broadcast::channel(64);
        Ok(Self(Arc::new(Inner {
            streams: Arc::new(Semaphore::new(config.max_concurrent_streams)),
            config,
            files,
            shells: Arc::new(shell::Shells::default()),
            input: Arc::new(Semaphore::new(1)),
            notifications,
            #[cfg(feature = "desktop")]
            clipboard: Arc::new(std::sync::Mutex::new(desktop::Clipboard::new())),
        })))
    }

    /// Bounded event subscription, useful to the mux/session adapter. A slow
    /// subscriber receives `Lagged`; consumers must surface that loss.
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.0.notifications.subscribe()
    }

    /// Report configured services and probe real desktop backends when allowed.
    /// Probes never read clipboard content or capture a screen.
    pub async fn capabilities(&self) -> Value {
        let config = &self.0.config;
        #[cfg(feature = "desktop")]
        let (desktop, input, clipboard) = {
            let allow_desktop = config.allow_desktop;
            let allow_input = config.allow_input;
            let allow_clipboard = config.allow_clipboard;
            tokio::task::spawn_blocking(move || {
                let screen = (allow_desktop || allow_input)
                    && desktop::displays().is_ok_and(|v| {
                        v["displays"]
                            .as_array()
                            .is_some_and(|items| !items.is_empty())
                    });
                (
                    allow_desktop && screen,
                    allow_input && screen && desktop::probe_input(),
                    allow_clipboard && desktop::probe_clipboard(),
                )
            })
            .await
            .unwrap_or((false, false, false))
        };
        #[cfg(not(feature = "desktop"))]
        let (desktop, input, clipboard) = (false, false, false);
        let files = self.0.files.is_some();
        let write = files && config.allow_write;
        let mut routes = vec!["control"];
        if config.allow_shell {
            routes.push("shell");
        }
        if files {
            routes.push("file.get");
        }
        if write {
            routes.push("file.put");
        }
        if desktop {
            routes.push("desktop");
        }
        if input {
            routes.push("input");
        }
        json!({"desktop_compiled":cfg!(feature="desktop"),"desktop":desktop,"input":input,
            "clipboard":clipboard,"shell":config.allow_shell,"files":files,"file_read":files,"file_write":write,
            "routes":routes,"limits":{"file_size":config.max_file_size,"record":records::FRAME_LIMIT,
                "control_record":CONTROL_LIMIT,"input_record":records::INPUT_LIMIT,
                "streams":config.max_concurrent_streams,"shell_processes":shell::PROCESS_LIMIT}})
    }

    /// Dispatch one application stream. Shell metadata additionally accepts a
    /// required `stream_id` chosen by the mux adapter for `shell.resize`.
    pub async fn serve(
        &self,
        route: &str,
        metadata: Value,
        io: impl ServiceIo + 'static,
    ) -> Result<()> {
        let io: BoxServiceIo = Box::new(io);
        ensure!(
            serde_json::to_vec(&metadata)?.len() <= 4096,
            "metadata exceeds 4096 bytes"
        );
        let slot = Arc::new(
            self.0
                .streams
                .clone()
                .try_acquire_owned()
                .context("service stream limit reached")?,
        );
        let idle = self.0.config.io_timeout;
        match route {
            "control" => {
                empty_metadata(metadata)?;
                self.control_stream(io).await
            }
            "file.get" => {
                let options: PathOptions = parse(metadata)?;
                self.files()?.get(&options.path, io, idle).await
            }
            "file.put" => {
                ensure!(self.0.config.allow_write, "file writes are disabled");
                let options: PutOptions = parse(metadata)?;
                self.files()?
                    .put(&options.path, options.size, io, idle)
                    .await
            }
            "shell" => {
                ensure!(self.0.config.allow_shell, "shell is disabled");
                let options = parse(metadata)?;
                self.0
                    .shells
                    .serve(
                        options,
                        self.0.config.shell.clone(),
                        io,
                        idle,
                        self.0.notifications.clone(),
                        slot.clone(),
                    )
                    .await
            }
            "desktop" => {
                ensure!(self.0.config.allow_desktop, "desktop is disabled");
                #[cfg(feature = "desktop")]
                {
                    desktop::serve(parse(metadata)?, io, idle, slot.clone()).await
                }
                #[cfg(not(feature = "desktop"))]
                {
                    bail!("desktop support was not compiled")
                }
            }
            "input" => {
                ensure!(self.0.config.allow_input, "input is disabled");
                let input = self
                    .0
                    .input
                    .clone()
                    .try_acquire_owned()
                    .context("an input stream is already active")?;
                #[cfg(feature = "desktop")]
                {
                    desktop::input(parse(metadata)?, io, idle, slot.clone(), input).await
                }
                #[cfg(not(feature = "desktop"))]
                {
                    drop(input);
                    bail!("desktop support was not compiled")
                }
            }
            _ => bail!("unknown service route"),
        }
    }

    fn files(&self) -> Result<&Arc<Files>> {
        self.0.files.as_ref().context("file sharing is disabled")
    }

    /// Handle a full `{id,method,params}` request and return the corresponding
    /// result/error envelope. Remote errors are sanitized, never host paths.
    pub async fn control(&self, request: Value) -> Result<Value> {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let id_ok = id.is_null() || id.is_number() || id.as_str().is_some_and(|id| id.len() <= 128);
        if !id_ok || serde_json::to_vec(&request)?.len() > CONTROL_LIMIT {
            return Ok(error(
                Value::Null,
                "invalid_request",
                "invalid control request",
            ));
        }
        let request: ControlRequest = match serde_json::from_value(request) {
            Ok(request) => request,
            Err(_) => return Ok(error(id, "invalid_request", "invalid control request")),
        };
        let id = request.id;
        let method = request.method.as_str();
        let allowed = match method {
            "capabilities" => true,
            "fs.list" => self.0.files.is_some(),
            "fs.mkdir" | "fs.remove" => self.0.files.is_some() && self.0.config.allow_write,
            "shell.resize" => self.0.config.allow_shell,
            "desktop.displays" => {
                cfg!(feature = "desktop")
                    && (self.0.config.allow_desktop || self.0.config.allow_input)
            }
            "clipboard.get" | "clipboard.set" => {
                cfg!(feature = "desktop") && self.0.config.allow_clipboard
            }
            _ => return Ok(error(id, "method_not_found", "unknown control method")),
        };
        if !allowed {
            return Ok(error(
                id,
                "permission_denied",
                "service is disabled by host policy",
            ));
        }
        let result = self.control_method(method, request.params).await;
        Ok(match result {
            Ok(value) => json!({"id":id,"result":value}),
            Err(err) if err.is::<InvalidParams>() => {
                error(id, "invalid_params", "invalid method parameters")
            }
            Err(_) => error(
                id,
                "service_error",
                "host service could not complete the request",
            ),
        })
    }

    async fn control_method(&self, method: &str, params: Value) -> Result<Value> {
        match method {
            "capabilities" => {
                empty_metadata(params)?;
                Ok(self.capabilities().await)
            }
            "fs.list" | "fs.mkdir" | "fs.remove" => {
                let options: PathOptions = parse(params)?;
                let files = self.files()?.clone();
                let method = method.to_string();
                tokio::task::spawn_blocking(move || match method.as_str() {
                    "fs.list" => files.list(&options.path),
                    "fs.mkdir" => files.mkdir(&options.path),
                    _ => files.remove(&options.path),
                })
                .await?
            }
            "shell.resize" => {
                let options: ResizeOptions = parse(params)?;
                self.0
                    .shells
                    .resize(options.stream_id, options.cols, options.rows)
            }
            "desktop.displays" => {
                empty_metadata(params)?;
                #[cfg(feature = "desktop")]
                {
                    tokio::task::spawn_blocking(desktop::displays).await?
                }
                #[cfg(not(feature = "desktop"))]
                {
                    bail!("desktop support was not compiled")
                }
            }
            "clipboard.get" | "clipboard.set" => {
                #[cfg(feature = "desktop")]
                {
                    let text = if method == "clipboard.set" {
                        Some(parse::<ClipboardOptions>(params)?.text)
                    } else {
                        empty_metadata(params)?;
                        None
                    };
                    let clipboard = self.0.clipboard.clone();
                    tokio::task::spawn_blocking(move || {
                        let mut clipboard = clipboard
                            .lock()
                            .map_err(|_| anyhow::anyhow!("clipboard unavailable"))?;
                        match text {
                            Some(text) => clipboard.set(text),
                            None => clipboard.get(),
                        }
                    })
                    .await?
                }
                #[cfg(not(feature = "desktop"))]
                {
                    bail!("desktop support was not compiled")
                }
            }
            _ => bail!("unknown control method"),
        }
    }

    async fn control_stream(&self, io: BoxServiceIo) -> Result<()> {
        let mut events = self.subscribe();
        let idle = self.0.config.io_timeout;
        let (mut input, mut output) = tokio::io::split(io);
        // All responses and notifications have one writer; a bounded channel
        // lets the request reader progress independently from output credit.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Value>(8);
        let requests = async move {
            while let Some(record) = read_record(&mut input, CONTROL_LIMIT, idle).await? {
                let response = match serde_json::from_slice(&record) {
                    Ok(request) => self.control(request).await?,
                    Err(_) => error(
                        Value::Null,
                        "invalid_request",
                        "invalid JSON control record",
                    ),
                };
                timeout(idle, tx.send(response))
                    .await?
                    .context("control output closed")?;
            }
            drop(tx);
            Ok::<_, anyhow::Error>(())
        };
        let writer = async {
            loop {
                let message = tokio::select! {
                    message=rx.recv()=>match message {Some(message)=>message,None=>break},
                    event=events.recv()=>match event {
                        Ok(event)=>event,
                        Err(broadcast::error::RecvError::Lagged(n))=>json!({"event":"events.lagged","data":{"count":n}}),
                        Err(broadcast::error::RecvError::Closed)=>break,
                    }
                };
                let body = serde_json::to_vec(&message)?;
                write_record(&mut output, &body, CONTROL_LIMIT, idle).await?;
            }
            timeout(idle, output.shutdown()).await??;
            Ok::<_, anyhow::Error>(())
        };
        // Requests FIN closes the response queue only after accepted responses
        // have drained. RESET/drop cancels both halves through this future.
        tokio::try_join!(requests, writer)?;
        Ok(())
    }
}

#[derive(Debug)]
struct InvalidParams;
impl std::fmt::Display for InvalidParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid parameters")
    }
}
impl std::error::Error for InvalidParams {}
fn parse<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| InvalidParams.into())
}
fn empty_metadata(value: Value) -> Result<()> {
    ensure!(
        value.is_null() || value.as_object().is_some_and(|object| object.is_empty()),
        InvalidParams
    );
    Ok(())
}
fn error(id: Value, code: &str, message: &str) -> Value {
    json!({"id":id,"error":{"code":code,"message":message}})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathOptions {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutOptions {
    path: String,
    size: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResizeOptions {
    stream_id: u64,
    cols: u16,
    rows: u16,
}
#[cfg(feature = "desktop")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipboardOptions {
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlRequest {
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}
