//! Resident hosting with fresh session identity and deterministic shutdown.
#[path = "invite_file.rs"]
mod invite_file;

use self::invite_file::InviteFile;
use crate::{approval::Console, update};
use anyhow::{Context, Result, ensure};
use lightremote_core::{HostTicket, Invite, Room, crypto::Role, link};
use lightremote_services::{
    HostConfig, HostServices,
    permissions::{ApprovalMode, Scope},
};
use lightremote_updater::{Product, SessionGate};
use serde_json::json;
use std::{io::IsTerminal, path::PathBuf, time::Duration};
use tokio::{
    sync::watch,
    task::JoinSet,
    time::{Instant, sleep},
};

const ROOM_REPLY_LIMIT: usize = 16 * 1024;
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Host policy and lifecycle shared by CLI and service daemon.
#[non_exhaustive]
pub struct Options {
    pub relay: String,
    pub operator_token: String,
    pub web_url: String,
    pub config: HostConfig,
    pub unattended: bool,
    pub stay: bool,
    pub invite_file: Option<PathBuf>,
    pub auto_update: bool,
    pub update_product: Product,
    /// Stable lock shared by all sessions and updates of a service installation.
    /// Defaults to the current user's per-executable gate for interactive clients.
    pub session_gate_path: Option<PathBuf>,
    pub network: link::LinkOptions,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            relay: "http://127.0.0.1:8787".into(),
            operator_token: String::new(),
            web_url: "http://127.0.0.1:5173/".into(),
            config: HostConfig::default(),
            unattended: false,
            stay: false,
            invite_file: None,
            auto_update: false,
            session_gate_path: None,
            update_product: if cfg!(feature = "desktop") {
                Product::DesktopCli
            } else {
                Product::HeadlessCli
            },
            network: link::LinkOptions::default(),
        }
    }
}
impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostOptions")
            .field("operator_token", &"[redacted]")
            .field("unattended", &self.unattended)
            .field("stay", &self.stay)
            .field("auto_update", &self.auto_update)
            .field("update_product", &self.update_product)
            .finish_non_exhaustive()
    }
}

/// Run with OS termination signals and joined session workers.
pub async fn run(options: Options) -> Result<()> {
    run_with_shutdown(options, shutdown_signal()).await
}

/// Run with externally owned shutdown, including Windows SCM service controls.
/// Await completion after signalling shutdown so cleanup and worker joins finish.
pub async fn run_with_shutdown(
    options: Options,
    shutdown: impl std::future::Future<Output = Result<()>> + Send,
) -> Result<()> {
    ensure!(
        options.unattended || std::io::stdin().is_terminal(),
        "attended approval requires a terminal; explicitly use --unattended for a background host"
    );
    ensure!(
        !options.auto_update || options.stay,
        "--auto-update requires --stay"
    );
    ensure!(
        !options.stay || std::io::stdin().is_terminal() || options.invite_file.is_some(),
        "a noninteractive resident host requires --invite-file in a private directory"
    );
    let probe = Invite {
        relay: options.relay.clone(),
        room_id: "probe".into(),
        client_token: "probe".into(),
        secret: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
    };
    probe.validate()?;
    let mut invite_file = match options.invite_file.clone() {
        Some(path) => Some(tokio::task::spawn_blocking(move || InviteFile::acquire(&path)).await??),
        None => None,
    };
    let result = run_sessions(&options, shutdown, &mut invite_file).await;
    let cleared = edit_invite(&mut invite_file, InviteFile::clear).await;
    // Keep authority through explicit cleanup and all session/console joins.
    drop(invite_file);
    result.and(cleared)
}

async fn edit_invite(
    invite_file: &mut Option<InviteFile>,
    edit: impl FnOnce(&mut InviteFile) -> Result<()> + Send + 'static,
) -> Result<()> {
    let Some(mut file) = invite_file.take() else {
        return Ok(());
    };
    let (file, result) = tokio::task::spawn_blocking(move || {
        let result = edit(&mut file);
        (file, result)
    })
    .await?;
    *invite_file = Some(file);
    result
}

async fn run_sessions(
    options: &Options,
    shutdown: impl std::future::Future<Output = Result<()>> + Send,
    invite_file: &mut Option<InviteFile>,
) -> Result<()> {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()?;
    let executable = std::env::current_exe()?;
    let gate = match &options.session_gate_path {
        Some(path) => SessionGate::new(path)?,
        None => SessionGate::for_executable(&executable)?,
    };
    let mode = if options.unattended {
        ApprovalMode::Unattended
    } else {
        ApprovalMode::Attended
    };
    tokio::pin!(shutdown);
    let mut backoff = Duration::from_secs(1);
    let mut last_update: Option<Instant> = None;
    loop {
        // No session lease exists here. Installation rechecks the exclusive gate
        // at commit, so another process cannot begin a session in the gap.
        if options.auto_update && last_update.is_none_or(|last| last.elapsed() >= UPDATE_INTERVAL) {
            last_update = Some(Instant::now());
            let result = tokio::select! {
                signal = &mut shutdown => return signal,
                result = update::install_if_available(&executable, &gate, options.update_product) => result,
            };
            match result {
                Ok(true) => return Err(update::RestartRequired.into()),
                Ok(false) => {}
                Err(_) => eprintln!(
                    "Automatic update check or installation failed; retaining the current executable. Next check is scheduled."
                ),
            }
        }
        let services = HostServices::with_approval(options.config.clone(), mode)?;
        let admission = gate
            .enter()
            .context("session admission is blocked by an update")?;
        let ticket = tokio::select! {
            signal = &mut shutdown => return signal,
            result = create_ticket(&http, options) => result,
        };
        let ticket = match ticket {
            Ok(ticket) => ticket,
            Err(error) if options.stay && retryable(&error) => {
                drop(admission);
                eprintln!(
                    "Relay unavailable; retrying with bounded backoff. No remote operation is replayed."
                );
                tokio::select! { signal = &mut shutdown => return signal, _ = sleep(backoff) => {} }
                backoff = backoff.saturating_mul(2).min(MAX_BACKOFF);
                continue;
            }
            Err(error) => return Err(error),
        };
        let invitation = ticket.invite.url(&options.web_url)?;
        if invite_file.is_some() {
            let value = invitation.clone();
            edit_invite(invite_file, move |file| file.publish(&value)).await?;
            eprintln!("A fresh single-use invitation is available in the private invitation file.");
        } else {
            // Explicit user-facing delivery, never a status log. Background hosts
            // must use the private file instead of journaling invitation secrets.
            println!("INVITE {invitation}");
        }
        eprintln!(
            "Waiting for one controller. {}",
            if options.unattended {
                "Unattended policy grants only the explicitly configured capabilities."
            } else {
                "The controller must request local approval for each capability."
            }
        );
        let connected = tokio::select! {
            signal = &mut shutdown => return signal,
            result = link::connect_with_options(&ticket.invite, Role::Host, &ticket.host_token, options.network) => result,
        };
        let cleared = edit_invite(invite_file, InviteFile::clear).await;
        let mut connection = match connected {
            Ok(connection) => connection,
            Err(error) if !options.stay => return Err(error),
            Err(_) => {
                cleared?;
                drop(admission);
                eprintln!(
                    "Invitation expired or connection failed; the next attempt uses a new invitation."
                );
                tokio::select! { signal = &mut shutdown => return signal, _ = sleep(backoff) => {} }
                backoff = backoff.saturating_mul(2).min(MAX_BACKOFF);
                continue;
            }
        };
        if let Err(error) = cleared {
            connection.task.abort();
            let _ = (&mut connection.task).await;
            services.shutdown().await?;
            return Err(error);
        }
        backoff = Duration::from_secs(1);
        eprintln!("Encrypted session connected.");
        let console = if options.unattended {
            None
        } else {
            match Console::start(services.permissions()) {
                Ok(console) => Some(console),
                Err(error) => {
                    connection.task.abort();
                    let _ = (&mut connection.task).await;
                    services.shutdown().await?;
                    return Err(error);
                }
            }
        };
        let (_no_quit, mut quit) = watch::channel(false);
        if let Some(console) = &console {
            quit = console.quit.clone();
        }
        let mut workers = JoinSet::new();
        let mut task_consumed = false;
        let mut stopping = false;
        let result = loop {
            tokio::select! {
                signal = &mut shutdown => { stopping = true; break signal; },
                changed = quit.changed() => {
                    if changed.is_err() || *quit.borrow_and_update() { stopping = true; break Ok(()); }
                },
                incoming = connection.incoming.recv() => {
                    let Some(incoming) = incoming else { break Ok(()); };
                    let services = services.clone();
                    workers.spawn(async move {
                        if crate::serve_stream(services, incoming).await.is_err() { eprintln!("Service stream failed, lost permission or was cancelled."); }
                    });
                },
                result = &mut connection.task => {
                    task_consumed = true;
                    break result.context("session runtime task failed").and_then(|result| result);
                },
                Some(result) = workers.join_next(), if !workers.is_empty() => {
                    if result.is_err() { eprintln!("Service worker stopped unexpectedly."); }
                },
            }
        };
        let revoked = services.permissions().revoke(&Scope::ALL);
        workers.abort_all();
        while workers.join_next().await.is_some() {}
        let result = if task_consumed {
            result
        } else if stopping {
            connection.task.abort();
            let _ = (&mut connection.task).await;
            result
        } else {
            match tokio::time::timeout(Duration::from_secs(5), &mut connection.task).await {
                Ok(outcome) => outcome
                    .context("session runtime task failed")
                    .and_then(|outcome| outcome),
                Err(error) => {
                    connection.task.abort();
                    let _ = (&mut connection.task).await;
                    Err(error.into())
                }
            }
        };
        let console_closed = match console {
            Some(console) => console.shutdown().await,
            None => Ok(()),
        };
        let services_closed = services.shutdown().await;
        drop(connection);
        drop(services);
        drop(admission);
        console_closed?;
        services_closed?;
        revoked?;
        if stopping || !options.stay {
            return result;
        }
        eprintln!(
            "Session ended; permissions and identity were discarded. Creating a fresh invitation."
        );
        tokio::select! { signal = &mut shutdown => return signal, _ = sleep(backoff) => {} }
    }
}

async fn create_ticket(client: &reqwest::Client, options: &Options) -> Result<HostTicket> {
    let mut response = client
        .post(format!("{}/v1/rooms", options.relay.trim_end_matches('/')))
        .bearer_auth(&options.operator_token)
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?;
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= ROOM_REPLY_LIMIT as u64),
        "relay room response is too large"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= ROOM_REPLY_LIMIT,
            "relay room response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    let room: Room = serde_json::from_slice(&bytes).context("invalid relay room response")?;
    HostTicket::new(&options.relay, room)
}
fn retryable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<reqwest::Error>().is_some_and(|error| {
        error.status().is_none_or(|status| {
            status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        })
    })
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result?, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_clears_invitation_before_releasing_writer_authority() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let directory = tempfile::tempdir().unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                    .unwrap();
            }
            let path = directory.path().join("invite");
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let relay = format!("http://{}", listener.local_addr().unwrap());
            let router = lightremote_relay::router(
                lightremote_relay::Config::new("test-operator".into()).unwrap(),
            )
            .unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, router).await });
            for fail_shutdown in [false, true] {
                let options = Options {
                    relay: relay.clone(),
                    operator_token: "test-operator".into(),
                    unattended: true,
                    invite_file: Some(path.clone()),
                    ..Default::default()
                };
                let competing = Options {
                    relay: relay.clone(),
                    unattended: true,
                    invite_file: Some(path.clone()),
                    ..Default::default()
                };
                let stopping = async {
                    while !path.exists() {
                        sleep(Duration::from_millis(5)).await;
                    }
                    let error = run_with_shutdown(competing, std::future::pending())
                        .await
                        .unwrap_err();
                    assert!(error.to_string().contains("another host already owns"));
                    if fail_shutdown {
                        anyhow::bail!("test shutdown failure");
                    }
                    Ok(())
                };
                let result = run_with_shutdown(options, stopping).await;
                assert_eq!(result.is_err(), fail_shutdown);
                assert!(
                    !path.exists(),
                    "all exit results must clear their invitation"
                );
                let mut next = InviteFile::acquire(&path).unwrap();
                next.publish("next host").unwrap();
                next.clear().unwrap();
            }
            server.abort();
            let _ = server.await;
        })
        .await
        .expect("host shutdown test exceeded its deadline");
    }
}
