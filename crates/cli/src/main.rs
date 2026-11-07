use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use lightremote::{host, update};
use lightremote_core::{Invite, crypto::Role, link, records};
use lightremote_services::{HostConfig, permissions::Scope};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::io::AsyncWriteExt;
mod network;

#[derive(Parser)]
#[command(
    version,
    about = "LightRemote: encrypted desktop, shell and files over TMP"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Create a single-use invitation and serve one session. All permissions are opt-in.
    Host {
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        relay: String,
        #[arg(long, env = "LIGHTREMOTE_RELAY_TOKEN", hide_env_values = true)]
        operator_token: String,
        #[arg(long, default_value = "http://127.0.0.1:5173/")]
        web_url: String,
        #[arg(long)]
        share_dir: Option<PathBuf>,
        #[arg(long)]
        allow_shell: bool,
        #[arg(long)]
        allow_write: bool,
        #[arg(long)]
        allow_desktop: bool,
        #[arg(long)]
        allow_input: bool,
        #[arg(long)]
        allow_clipboard: bool,
        #[arg(long)]
        allow_clipboard_read: bool,
        #[arg(long)]
        allow_clipboard_write: bool,
        /// Explicitly grant configured capabilities without local approval prompts.
        #[arg(long)]
        unattended: bool,
        /// Stay available after each session; each attempt creates a fresh invitation.
        #[arg(long)]
        stay: bool,
        /// Atomically publish the current invitation in a private directory.
        #[arg(long)]
        invite_file: Option<PathBuf>,
        /// Check GitHub and install stable updates between sessions; requires --stay.
        #[arg(long, requires = "stay")]
        auto_update: bool,
        /// Increase bounded connection/write budgets on high-latency links.
        #[arg(long, value_enum, default_value = "standard")]
        network: network::Profile,
        #[arg(long)]
        shell: Option<PathBuf>,
        #[arg(long, default_value_t = 1073741824)]
        max_file_size: u64,
    },
    /// Connect using a host's invitation. Use LIGHTREMOTE_INVITE to avoid shell history.
    Connect {
        #[arg(long, env = "LIGHTREMOTE_INVITE", hide_env_values = true)]
        invite: String,
        #[arg(long, value_enum, default_value = "standard")]
        network: network::Profile,
        #[command(subcommand)]
        action: Action,
    },
    /// Check the official GitHub stable release for this product and architecture.
    Update {
        /// Verify and install the update. Active sessions prevent replacement.
        #[arg(long)]
        install: bool,
    },
}
#[derive(Subcommand)]
enum Action {
    Capabilities,
    Ls {
        #[arg(default_value = "")]
        path: String,
    },
    Get {
        remote: String,
        local: PathBuf,
    },
    Put {
        local: PathBuf,
        remote: String,
    },
    Shell,
    Screenshot {
        output: PathBuf,
        #[arg(long, default_value_t = 0)]
        display: u32,
    },
}
async fn control(
    handle: &lightremote_core::LinkHandle,
    method: &str,
    params: Value,
    budget: Duration,
) -> Result<Value> {
    tokio::time::timeout(budget, control_exchange(handle, method, params))
        .await
        .context("control request timed out")?
}
async fn control_exchange(
    handle: &lightremote_core::LinkHandle,
    method: &str,
    params: Value,
) -> Result<Value> {
    let mut stream = handle.open("control", json!({})).await?;
    records::json(
        &mut stream,
        &json!({"id":1,"method":method,"params":params}),
    )
    .await?;
    stream.shutdown().await?;
    let mut result = None;
    while let Some(data) = records::read(&mut stream, records::MAX_CONTROL).await? {
        let response: Value = serde_json::from_slice(&data)?;
        if response["id"] == 1 {
            ensure!(
                response.get("error").is_none(),
                "host rejected request: {}",
                response["error"]
            );
            ensure!(result.is_none(), "host sent duplicate control responses");
            result = Some(response["result"].clone());
        }
    }
    stream.completed().await?;
    result.context("host closed control stream without response")
}

async fn request_permission(
    handle: &lightremote_core::LinkHandle,
    scope: Scope,
    budget: Duration,
) -> Result<()> {
    let capabilities = control(handle, "capabilities", json!({}), budget).await?;
    let Some(permissions) = capabilities.get("permissions") else {
        return Ok(());
    };
    let contains = |value: &Value, key: &str| {
        value[key].as_array().is_some_and(|scopes| {
            scopes
                .iter()
                .any(|value| value.as_str() == Some(scope.name()))
        })
    };
    if contains(permissions, "granted") {
        return Ok(());
    }
    ensure!(
        contains(permissions, "available"),
        "the host does not allow this operation in its local policy"
    );
    let requested = control(
        handle,
        "permissions.request",
        json!({"scopes":[scope.name()]}),
        budget,
    )
    .await?;
    if contains(&requested, "granted") {
        return Ok(());
    }
    let id = requested["pending"]["id"]
        .as_u64()
        .context("host did not accept the permission request")?;
    eprintln!("Waiting for local host approval of {scope}. No operation has started.");
    let wait = async {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let snapshot = control(handle, "permissions.get", json!({}), budget).await?;
            if contains(&snapshot, "granted") {
                return Ok::<_, anyhow::Error>(());
            }
            ensure!(
                snapshot["pending"]["id"].as_u64() == Some(id),
                "the permission request was denied, cancelled or revoked by the host"
            );
        }
    };
    let result = tokio::select! {
        result = tokio::time::timeout(std::time::Duration::from_secs(125), wait) => result.context("host permission approval timed out").and_then(|result| result),
        signal = tokio::signal::ctrl_c() => { signal?; Err(anyhow::anyhow!("permission request cancelled")) },
    };
    if result.is_err() {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            control(handle, "permissions.cancel", json!({"id":id}), budget),
        )
        .await;
    }
    result
}
async fn run() -> Result<()> {
    match Args::parse().command {
        Command::Host {
            relay,
            operator_token,
            web_url,
            share_dir,
            allow_shell,
            allow_write,
            allow_desktop,
            allow_input,
            allow_clipboard,
            allow_clipboard_read,
            allow_clipboard_write,
            unattended,
            stay,
            invite_file,
            auto_update,
            network,
            shell,
            max_file_size,
        } => {
            let mut options = host::Options::default();
            options.relay = relay;
            options.operator_token = operator_token;
            options.web_url = web_url;
            options.unattended = unattended;
            options.stay = stay;
            options.invite_file = invite_file;
            options.auto_update = auto_update;
            options.network = network.options();
            options.config = HostConfig {
                share_dir,
                allow_shell,
                allow_write,
                allow_desktop,
                allow_input,
                allow_clipboard,
                allow_clipboard_read,
                allow_clipboard_write,
                shell,
                max_file_size,
                ..Default::default()
            };
            options.config.io_timeout =
                options.config.io_timeout.max(options.network.write_timeout);
            host::run(options).await?
        }

        Command::Update { install } => {
            update::run(if install {
                update::Mode::Install
            } else {
                update::Mode::Check
            })
            .await?
        }
        Command::Connect {
            invite,
            network,
            action,
        } => {
            let invite = Invite::parse(&invite)?;
            let gate = lightremote_updater::SessionGate::for_executable(&std::env::current_exe()?)?;
            let _admission = gate.enter()?;
            let rpc_budget = network.rpc_budget();
            let connection = link::connect_with_options(
                &invite,
                Role::Client,
                &invite.client_token,
                network.options(),
            )
            .await?;
            let scopes = match &action {
                Action::Capabilities => None,
                Action::Ls { .. } | Action::Get { .. } => Some(Scope::FileRead),
                Action::Put { .. } => Some(Scope::FileWrite),
                Action::Shell => Some(Scope::Shell),
                Action::Screenshot { .. } => Some(Scope::Desktop),
            };
            if let Some(scope) = scopes {
                request_permission(&connection.handle, scope, rpc_budget).await?;
            }
            match action {
                Action::Capabilities => println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &control(&connection.handle, "capabilities", json!({}), rpc_budget).await?
                    )?
                ),
                Action::Ls { path } => println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &control(
                            &connection.handle,
                            "fs.list",
                            json!({"path":path}),
                            rpc_budget
                        )
                        .await?
                    )?
                ),
                Action::Get { remote, local } => {
                    let mut stream = connection
                        .handle
                        .open("file.get", json!({"path":remote}))
                        .await?;
                    stream.shutdown().await?;
                    let mut file = tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&local)
                        .await?;
                    let transfer = async {
                        let size = tokio::io::copy(&mut stream, &mut file).await?;
                        stream.completed().await?;
                        file.sync_all().await?;
                        Ok::<_, anyhow::Error>(size)
                    }
                    .await;
                    drop(file);
                    match transfer {
                        Ok(size) => {
                            eprintln!("Downloaded {size} bytes.");
                        }
                        Err(error) => {
                            let _ = tokio::fs::remove_file(local).await;
                            return Err(error);
                        }
                    }
                }
                Action::Put { local, remote } => {
                    let mut file = tokio::fs::File::open(local).await?;
                    let size = file.metadata().await?.len();
                    let mut stream = connection
                        .handle
                        .open("file.put", json!({"path":remote,"size":size}))
                        .await?;
                    let sent = tokio::io::copy(&mut file, &mut stream).await?;
                    ensure!(sent == size, "local file changed during upload");
                    stream.shutdown().await?;
                    let receipt = records::read(&mut stream, records::MAX_CONTROL)
                        .await?
                        .context("upload failed before commit receipt")?;
                    ensure!(
                        records::read(&mut stream, records::MAX_CONTROL)
                            .await?
                            .is_none(),
                        "host sent multiple upload receipts"
                    );
                    stream.completed().await?;
                    println!("{}", String::from_utf8(receipt)?);
                }
                Action::Screenshot { output, display } => {
                    let mut stream = connection
                        .handle
                        .open(
                            "desktop",
                            json!({"display":display,"fps":1,"quality":85,"single":true}),
                        )
                        .await?;
                    stream.shutdown().await?;
                    let image = records::read(&mut stream, records::MAX_RECORD)
                        .await?
                        .context("desktop unavailable or screenshot failed")?;
                    ensure!(
                        records::read(&mut stream, records::MAX_RECORD)
                            .await?
                            .is_none(),
                        "host sent multiple screenshot frames"
                    );
                    stream.completed().await?;
                    let mut file = tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(output)
                        .await?;
                    file.write_all(&image).await?;
                }
                Action::Shell => {
                    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                    let mut stream = connection
                        .handle
                        .open("shell", json!({"cols":cols,"rows":rows}))
                        .await?;
                    let raw = crossterm::terminal::enable_raw_mode().is_ok();
                    struct Raw(bool);
                    impl Drop for Raw {
                        fn drop(&mut self) {
                            if self.0 {
                                let _ = crossterm::terminal::disable_raw_mode();
                            }
                        }
                    }
                    let _raw = Raw(raw);
                    {
                        let (mut reader, mut writer) = tokio::io::split(&mut stream);
                        let upload = async {
                            tokio::io::copy(&mut tokio::io::stdin(), &mut writer).await?;
                            writer.shutdown().await?;
                            Ok::<_, anyhow::Error>(())
                        };
                        let download = async {
                            let mut stdout = tokio::io::stdout();
                            tokio::io::copy(&mut reader, &mut stdout).await?;
                            stdout.flush().await?;
                            Ok::<_, anyhow::Error>(())
                        };
                        tokio::pin!(upload, download);
                        tokio::select! {result=&mut download=>result?,result=&mut upload=>{result?;download.await?;}}
                    }
                    stream.completed().await?;
                }
            }
        }
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Cannot start runtime: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(run());
    runtime.shutdown_timeout(std::time::Duration::from_millis(100));
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) if error.is::<update::RestartRequired>() => {
            eprintln!("{error}");
            std::process::ExitCode::from(75)
        }
        Err(error) => {
            eprintln!("{error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
