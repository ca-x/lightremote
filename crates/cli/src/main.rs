use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use roc_remote_core::{HostTicket, Invite, Room, crypto::Role, link, records};
use roc_remote_services::{HostConfig, HostServices};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::{io::AsyncWriteExt, task::JoinSet};

#[derive(Parser)]
#[command(
    version,
    about = "roc-remote: encrypted desktop, shell and files over TMP"
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
        #[arg(long, env = "ROC_REMOTE_RELAY_TOKEN", hide_env_values = true)]
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
        shell: Option<PathBuf>,
        #[arg(long, default_value_t = 1073741824)]
        max_file_size: u64,
    },
    /// Connect using a host's invitation. Use ROC_REMOTE_INVITE to avoid shell history.
    Connect {
        #[arg(long, env = "ROC_REMOTE_INVITE", hide_env_values = true)]
        invite: String,
        #[command(subcommand)]
        action: Action,
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
    handle: &roc_remote_core::LinkHandle,
    method: &str,
    params: Value,
) -> Result<Value> {
    let mut stream = handle.open("control", json!({})).await?;
    records::json(
        &mut stream.io,
        &json!({"id":1,"method":method,"params":params}),
    )
    .await?;
    stream.io.shutdown().await?;
    while let Some(data) = records::read(&mut stream.io, records::MAX_CONTROL).await? {
        let response: Value = serde_json::from_slice(&data)?;
        if response["id"] == 1 {
            ensure!(
                response.get("error").is_none(),
                "host rejected request: {}",
                response["error"]
            );
            return Ok(response["result"].clone());
        }
    }
    anyhow::bail!("host closed control stream without response")
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
            shell,
            max_file_size,
        } => {
            // Validate URL before transmitting the operator credential.
            let probe = Invite {
                relay: relay.clone(),
                room_id: "probe".into(),
                client_token: "probe".into(),
                secret: base64_key(),
            };
            probe.validate()?;
            let services = HostServices::new(HostConfig {
                share_dir,
                allow_shell,
                allow_write,
                allow_desktop,
                allow_input,
                allow_clipboard,
                shell,
                max_file_size,
                ..Default::default()
            })?;
            let room: Room = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?
                .post(format!("{}/v1/rooms", relay.trim_end_matches('/')))
                .bearer_auth(operator_token)
                .json(&json!({}))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let ticket = HostTicket::new(&relay, room)?;
            println!("INVITE {}", ticket.invite.url(&web_url)?);
            eprintln!(
                "Waiting for one controller. Invitation grants the explicitly enabled permissions."
            );
            let mut connection =
                link::connect(&ticket.invite, Role::Host, &ticket.host_token).await?;
            eprintln!("Encrypted session connected.");
            let mut workers = JoinSet::new();
            loop {
                tokio::select! {
                    incoming=connection.incoming.recv()=>{
                        let Some(incoming)=incoming else {break};
                        let services=services.clone();
                        workers.spawn(async move {
                            let route=incoming.route.clone();
                            let result=roc_remote::serve_stream(services,incoming).await;
                            if result.is_err(){eprintln!("Service stream failed or was cancelled: {route}");}
                        });
                    }
                    result=&mut connection.task=>{result??;break;}
                    Some(_)=workers.join_next(), if !workers.is_empty()=>{},
                    _=tokio::signal::ctrl_c()=>break,
                }
            }
            workers.abort_all();
        }
        Command::Connect { invite, action } => {
            let invite = Invite::parse(&invite)?;
            let connection = link::connect(&invite, Role::Client, &invite.client_token).await?;
            match action {
                Action::Capabilities => println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &control(&connection.handle, "capabilities", json!({})).await?
                    )?
                ),
                Action::Ls { path } => println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &control(&connection.handle, "fs.list", json!({"path":path})).await?
                    )?
                ),
                Action::Get { remote, local } => {
                    let mut stream = connection
                        .handle
                        .open("file.get", json!({"path":remote}))
                        .await?;
                    stream.io.shutdown().await?;
                    let mut file = tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&local)
                        .await?;
                    match tokio::io::copy(&mut stream.io, &mut file).await {
                        Ok(size) => {
                            if let Err(error) = stream.completed().await {
                                drop(file);
                                let _ = tokio::fs::remove_file(local).await;
                                return Err(error);
                            }
                            file.sync_all().await?;
                            eprintln!("Downloaded {size} bytes.");
                        }
                        Err(error) => {
                            drop(file);
                            let _ = tokio::fs::remove_file(local).await;
                            return Err(error.into());
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
                    let sent = tokio::io::copy(&mut file, &mut stream.io).await?;
                    ensure!(sent == size, "local file changed during upload");
                    stream.io.shutdown().await?;
                    let receipt = records::read(&mut stream.io, records::MAX_CONTROL)
                        .await?
                        .context("upload failed before commit receipt")?;
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
                    stream.io.shutdown().await?;
                    let image = records::read(&mut stream.io, records::MAX_RECORD)
                        .await?
                        .context("desktop unavailable or screenshot failed")?;
                    let mut file = tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(output)
                        .await?;
                    file.write_all(&image).await?;
                }
                Action::Shell => {
                    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                    let stream = connection
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
                    let (mut reader, mut writer) = tokio::io::split(stream.io);
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
            }
        }
    }
    Ok(())
}
fn base64_key() -> String {
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into()
}
fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(run());
    runtime.shutdown_timeout(std::time::Duration::from_millis(100));
    result
}
