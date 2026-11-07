//! Cancellable local console. Remote peers cannot execute these commands.
use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use lightremote_services::permissions::{Permissions, Scope, Snapshot};
use std::{
    io::IsTerminal,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::sync::watch;

pub(crate) struct Console {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub(crate) quit: watch::Receiver<bool>,
}
impl Console {
    pub(crate) fn start(permissions: Permissions) -> Result<Self> {
        if !std::io::stdin().is_terminal() {
            bail!(
                "attended approval requires a terminal; use --unattended only to grant the configured permissions without local prompts"
            );
        }
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (quit, receiver) = watch::channel(false);
        let thread = std::thread::Builder::new().name("lightremote-approval".into()).spawn(move || {
            print_policy(&permissions.snapshot());
            let mut line = String::new();
            let mut pending = None;
            let mut pending_since = Instant::now();
            while !worker_stop.load(Ordering::Acquire) {
                let snapshot = permissions.snapshot();
                let id = snapshot.pending.as_ref().map(|request| request.id);
                if id != pending {
                    pending = id;
                    pending_since = Instant::now();
                    if let Some(request) = snapshot.pending {
                        eprintln!("\nPermission request {}: {}", request.id, scope_list(&request.scopes));
                        eprintln!("approve {} [scope ...] | deny {} | revoke <scope ...|all> | status | quit", request.id, request.id);
                    }
                }
                if pending.is_some() && pending_since.elapsed() >= Duration::from_secs(120) {
                    if let Some(id) = pending { let _ = permissions.deny(id); }
                    eprintln!("Permission request expired without approval.");
                }
                match event::poll(Duration::from_millis(100)) {
                    Ok(false) => continue,
                    Err(_) => break,
                    Ok(true) => {}
                }
                match event::read() {
                    Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                        KeyCode::Char(c) if c.is_ascii() && !c.is_ascii_control() && line.len() < 256 => line.push(c),
                        KeyCode::Backspace => { line.pop(); }
                        KeyCode::Enter => {
                            let command = std::mem::take(&mut line);
                            match apply(&permissions, &command) {
                                Ok(Control::Continue) => {},
                                Ok(Control::Quit) => break,
                                Err(_) => eprintln!("Invalid local command or stale request. Use approve <id> [scopes], deny <id>, revoke <scopes|all>, status or quit."),
                            }
                        }
                        _ => {}
                    },
                    Err(_) => break,
                    _ => {}
                }
            }
            let _ = permissions.revoke(&Scope::ALL);
            let _ = quit.send(true);
        }).context("start local approval console")?;
        Ok(Self {
            stop,
            thread: Some(thread),
            quit: receiver,
        })
    }
    pub(crate) async fn shutdown(mut self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            tokio::task::spawn_blocking(move || thread.join())
                .await?
                .map_err(|_| anyhow::anyhow!("approval console panicked"))?;
        }
        Ok(())
    }
}
impl Drop for Console {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub(crate) enum Control {
    Continue,
    Quit,
}
fn scopes(words: &[&str]) -> Result<Vec<Scope>> {
    words
        .iter()
        .map(|word| word.parse().map_err(Into::into))
        .collect()
}
pub(crate) fn apply(permissions: &Permissions, command: &str) -> Result<Control> {
    let words: Vec<_> = command.split_whitespace().collect();
    match words.as_slice() {
        [] => {}
        ["quit"] => return Ok(Control::Quit),
        ["status"] => print_policy(&permissions.snapshot()),
        ["approve", id, rest @ ..] => {
            let selected = (!rest.is_empty()).then(|| scopes(rest)).transpose()?;
            permissions.approve(id.parse()?, selected.as_deref())?;
            eprintln!(
                "Permission request approved. Granted: {}",
                scope_list(&permissions.snapshot().granted)
            );
        }
        ["deny", id] => {
            permissions.deny(id.parse()?)?;
            eprintln!("Permission request denied.");
        }
        ["revoke", "all"] => {
            permissions.revoke(&Scope::ALL)?;
            eprintln!("All permissions revoked; affected operations are stopping.");
        }
        ["revoke", rest @ ..] if !rest.is_empty() => {
            permissions.revoke(&scopes(rest)?)?;
            eprintln!("Selected permissions revoked; affected operations are stopping.");
        }
        _ => bail!("unknown local approval command"),
    }
    Ok(Control::Continue)
}
fn scope_list(scopes: &[Scope]) -> String {
    if scopes.is_empty() {
        "none".into()
    } else {
        scopes
            .iter()
            .map(|scope| scope.name())
            .collect::<Vec<_>>()
            .join(", ")
    }
}
fn print_policy(snapshot: &Snapshot) {
    eprintln!("Local policy ceiling: {}", scope_list(&snapshot.available));
    eprintln!("Currently granted: {}", scope_list(&snapshot.granted));
    eprintln!(
        "Shell executes as this OS user. File access stays inside the configured share. Input controls the host. Clipboard read/write are independent."
    );
    eprintln!("Only approve requests you expect. Type status, revoke all or quit at any time.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightremote_services::permissions::ApprovalMode;
    #[test]
    fn local_console_has_no_implicit_grant_and_validates_subset() {
        let permissions = Permissions::new([Scope::Shell, Scope::FileRead], ApprovalMode::Attended);
        assert!(apply(&permissions, "approve 1").is_err());
        let id = permissions
            .request(&[Scope::Shell, Scope::FileRead])
            .unwrap()
            .pending
            .unwrap()
            .id;
        apply(&permissions, &format!("approve {id} file_read")).unwrap();
        assert!(permissions.allows(Scope::FileRead));
        assert!(!permissions.allows(Scope::Shell));
        apply(&permissions, "revoke all").unwrap();
        assert!(permissions.snapshot().granted.is_empty());
        assert!(apply(&permissions, "grant shell").is_err());
    }
}
