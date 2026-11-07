//! Session-local permission requests. Only local host code can grant or revoke.
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fmt,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

/// Independently granted application capabilities; never TMP protocol fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Desktop,
    Input,
    Shell,
    FileRead,
    FileWrite,
    ClipboardRead,
    ClipboardWrite,
}
impl Scope {
    pub const ALL: [Self; 7] = [
        Self::Desktop,
        Self::Input,
        Self::Shell,
        Self::FileRead,
        Self::FileWrite,
        Self::ClipboardRead,
        Self::ClipboardWrite,
    ];
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Input => "input",
            Self::Shell => "shell",
            Self::FileRead => "file_read",
            Self::FileWrite => "file_write",
            Self::ClipboardRead => "clipboard_read",
            Self::ClipboardWrite => "clipboard_write",
        }
    }
    const fn index(self) -> usize {
        self as usize
    }
}
impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
impl std::str::FromStr for Scope {
    type Err = PermissionError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|scope| scope.name() == value)
            .ok_or(PermissionError::InvalidScope)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Attended,
    Unattended,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PendingRequest {
    pub id: u32,
    pub scopes: Vec<Scope>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approved,
    Denied,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PermissionDecision {
    pub id: u32,
    pub outcome: Decision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Snapshot {
    pub available: Vec<Scope>,
    pub granted: Vec<Scope>,
    pub pending: Option<PendingRequest>,
    pub revision: u32,
    pub mode: ApprovalMode,
    pub last_decision: Option<PermissionDecision>,
    #[serde(skip)]
    epochs: [u64; 7],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PermissionError {
    InvalidScope,
    OutsidePolicy,
    Busy,
    StaleRequest,
    Denied,
    Exhausted,
}
impl fmt::Display for PermissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidScope => "invalid permission scope",
            Self::OutsidePolicy => "permission is outside host policy",
            Self::Busy => "another permission request is pending",
            Self::StaleRequest => "permission request is no longer pending",
            Self::Denied => "permission has not been granted",
            Self::Exhausted => "permission revision limit reached",
        })
    }
}
impl std::error::Error for PermissionError {}

struct State {
    snapshot: Snapshot,
    next_request: u32,
}
struct Inner {
    state: Mutex<State>,
    changes: watch::Sender<Snapshot>,
}

/// The grant authority. Keep this handle on the host, never expose it to a peer.
#[derive(Clone)]
pub struct Permissions(Arc<Inner>);
impl Permissions {
    #[must_use]
    pub fn new(available: impl IntoIterator<Item = Scope>, mode: ApprovalMode) -> Self {
        let available: Vec<_> = available
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let granted = match mode {
            ApprovalMode::Attended => Vec::new(),
            ApprovalMode::Unattended => available.clone(),
        };
        let snapshot = Snapshot {
            available,
            granted,
            pending: None,
            revision: 0,
            mode,
            last_decision: None,
            epochs: [0; 7],
        };
        let (changes, _) = watch::channel(snapshot.clone());
        Self(Arc::new(Inner {
            state: Mutex::new(State {
                snapshot,
                next_request: 0,
            }),
            changes,
        }))
    }
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        self.0.changes.borrow().clone()
    }
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.0.changes.subscribe()
    }
    #[must_use]
    pub fn allows(&self, scope: Scope) -> bool {
        self.0.changes.borrow().granted.contains(&scope)
    }

    fn change(
        &self,
        run: impl FnOnce(&mut State) -> Result<(), PermissionError>,
    ) -> Result<Snapshot, PermissionError> {
        let mut state = self.0.state.lock().expect("permission state lock poisoned");
        if state.snapshot.revision == u32::MAX {
            return Err(PermissionError::Exhausted);
        }
        run(&mut state)?;
        state.snapshot.revision += 1;
        let snapshot = state.snapshot.clone();
        // Serialize publication with mutation so concurrent decisions cannot
        // publish an older grant after a newer revocation.
        self.0.changes.send_replace(snapshot.clone());
        Ok(snapshot)
    }

    /// Queue a request without granting it. Repeating the same request is idempotent.
    /// # Errors
    /// Rejects unknown/duplicate/out-of-policy scopes and a second distinct request.
    pub fn request(&self, scopes: &[Scope]) -> Result<Snapshot, PermissionError> {
        let selected: BTreeSet<_> = scopes.iter().copied().collect();
        if selected.is_empty() || selected.len() != scopes.len() {
            return Err(PermissionError::InvalidScope);
        }
        self.change(|state| {
            let snapshot = &mut state.snapshot;
            if selected
                .iter()
                .any(|scope| !snapshot.available.contains(scope))
            {
                return Err(PermissionError::OutsidePolicy);
            }
            let needed: Vec<_> = selected
                .into_iter()
                .filter(|scope| !snapshot.granted.contains(scope))
                .collect();
            if needed.is_empty() {
                return Ok(());
            }
            if let Some(pending) = &snapshot.pending {
                return if pending.scopes == needed {
                    Ok(())
                } else {
                    Err(PermissionError::Busy)
                };
            }
            state.next_request = state
                .next_request
                .checked_add(1)
                .ok_or(PermissionError::Exhausted)?;
            snapshot.pending = Some(PendingRequest {
                id: state.next_request,
                scopes: needed,
            });
            snapshot.last_decision = None;
            Ok(())
        })
    }

    /// Approve all requested scopes, or an explicit subset, from local host code.
    /// # Errors
    /// Fails for stale IDs or any scope not contained in the pending request.
    pub fn approve(
        &self,
        id: u32,
        selected: Option<&[Scope]>,
    ) -> Result<Snapshot, PermissionError> {
        self.change(|state| {
            let snapshot = &mut state.snapshot;
            let pending = snapshot
                .pending
                .as_ref()
                .filter(|p| p.id == id)
                .ok_or(PermissionError::StaleRequest)?;
            let scopes = selected.unwrap_or(&pending.scopes);
            if scopes.is_empty() || scopes.iter().any(|scope| !pending.scopes.contains(scope)) {
                return Err(PermissionError::OutsidePolicy);
            }
            let mut grants: BTreeSet<_> = snapshot.granted.iter().copied().collect();
            grants.extend(scopes.iter().copied());
            snapshot.granted = grants.into_iter().collect();
            snapshot.pending = None;
            snapshot.last_decision = Some(PermissionDecision {
                id,
                outcome: Decision::Approved,
            });
            Ok(())
        })
    }
    /// Deny a request using a local host decision.
    pub fn deny(&self, id: u32) -> Result<Snapshot, PermissionError> {
        self.resolve(id, Decision::Denied)
    }
    /// A remote requester may cancel its pending request, never approve it.
    pub fn cancel(&self, id: u32) -> Result<Snapshot, PermissionError> {
        self.resolve(id, Decision::Cancelled)
    }
    fn resolve(&self, id: u32, outcome: Decision) -> Result<Snapshot, PermissionError> {
        self.change(|state| {
            if state
                .snapshot
                .pending
                .as_ref()
                .is_none_or(|pending| pending.id != id)
            {
                return Err(PermissionError::StaleRequest);
            }
            state.snapshot.pending = None;
            state.snapshot.last_decision = Some(PermissionDecision { id, outcome });
            Ok(())
        })
    }

    /// Revoke grants and invalidate every previously accepted lease, including
    /// when these scopes are granted again before a worker next gets scheduled.
    pub fn revoke(&self, scopes: &[Scope]) -> Result<Snapshot, PermissionError> {
        self.change(|state| {
            for scope in scopes {
                let epoch = &mut state.snapshot.epochs[scope.index()];
                *epoch = epoch
                    .checked_add(1)
                    .expect("permission lease epoch exhausted");
            }
            state
                .snapshot
                .granted
                .retain(|scope| !scopes.contains(scope));
            // A stale pending prompt must not restore a just-revoked grant.
            state.snapshot.pending = None;
            Ok(())
        })
    }

    pub(crate) fn lease(&self, scopes: &[Scope]) -> Result<Lease, PermissionError> {
        let changes = self.subscribe();
        let snapshot = changes.borrow();
        if scopes.iter().any(|scope| !snapshot.granted.contains(scope)) {
            return Err(PermissionError::Denied);
        }
        let epochs = scopes
            .iter()
            .map(|scope| (*scope, snapshot.epochs[scope.index()]))
            .collect();
        drop(snapshot);
        Ok(Lease { changes, epochs })
    }
}

#[derive(Clone)]
pub(crate) struct Lease {
    changes: watch::Receiver<Snapshot>,
    epochs: Vec<(Scope, u64)>,
}
impl Lease {
    pub(crate) fn valid(&self) -> bool {
        let current = self.changes.borrow();
        self.epochs.iter().all(|(scope, epoch)| {
            current.granted.contains(scope) && current.epochs[scope.index()] == *epoch
        })
    }
    pub(crate) async fn revoked(&mut self) {
        loop {
            let valid = {
                let current = self.changes.borrow_and_update();
                self.epochs.iter().all(|(scope, epoch)| {
                    current.granted.contains(scope) && current.epochs[scope.index()] == *epoch
                })
            };
            if !valid || self.changes.changed().await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_never_grant_and_cannot_escape_policy() {
        let p = Permissions::new([Scope::Shell, Scope::FileRead], ApprovalMode::Attended);
        assert!(p.snapshot().granted.is_empty());
        assert_eq!(
            p.request(&[Scope::Input]),
            Err(PermissionError::OutsidePolicy)
        );
        let pending = p.request(&[Scope::Shell]).unwrap().pending.unwrap();
        assert!(!p.allows(Scope::Shell));
        assert_eq!(p.request(&[Scope::FileRead]), Err(PermissionError::Busy));
        assert_eq!(
            p.approve(pending.id, Some(&[Scope::Input])),
            Err(PermissionError::OutsidePolicy)
        );
        p.deny(pending.id).unwrap();
        assert!(!p.allows(Scope::Shell));
        assert_eq!(
            p.approve(pending.id, None),
            Err(PermissionError::StaleRequest)
        );
    }
    #[tokio::test]
    async fn revoke_and_immediate_regrant_cannot_revive_old_work() {
        let p = Permissions::new([Scope::Shell], ApprovalMode::Unattended);
        let mut old = p.lease(&[Scope::Shell]).unwrap();
        p.revoke(&[Scope::Shell]).unwrap();
        let id = p.request(&[Scope::Shell]).unwrap().pending.unwrap().id;
        p.approve(id, None).unwrap();
        tokio::time::timeout(std::time::Duration::from_millis(100), old.revoked())
            .await
            .expect("old work survived revoke/regrant");
        assert!(p.lease(&[Scope::Shell]).is_ok());
    }
    #[test]
    fn partial_approval_cancel_and_directional_clipboard_are_independent() {
        let p = Permissions::new(
            [Scope::ClipboardRead, Scope::ClipboardWrite],
            ApprovalMode::Attended,
        );
        let id = p
            .request(&[Scope::ClipboardRead, Scope::ClipboardWrite])
            .unwrap()
            .pending
            .unwrap()
            .id;
        p.approve(id, Some(&[Scope::ClipboardWrite])).unwrap();
        assert!(!p.allows(Scope::ClipboardRead));
        assert!(p.allows(Scope::ClipboardWrite));
        let id = p
            .request(&[Scope::ClipboardRead])
            .unwrap()
            .pending
            .unwrap()
            .id;
        p.cancel(id).unwrap();
        assert_eq!(
            p.snapshot().last_decision.unwrap().outcome,
            Decision::Cancelled
        );
        assert!(p.snapshot().pending.is_none());
    }
}
