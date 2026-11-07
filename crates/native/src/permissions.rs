//! Fail-closed view of the host's session permissions.
use anyhow::{Context, Result, ensure};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    pub fn name(self) -> &'static str {
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

    pub fn label(self) -> &'static str {
        match self {
            Self::Desktop => "View desktop",
            Self::Input => "Control keyboard and mouse",
            Self::Shell => "Remote shell",
            Self::FileRead => "Read shared files",
            Self::FileWrite => "Write shared files",
            Self::ClipboardRead => "Read clipboard",
            Self::ClipboardWrite => "Write clipboard",
        }
    }
}

impl Scope {
    fn route(self) -> Option<&'static str> {
        match self {
            Self::Desktop => Some("desktop"),
            Self::Input => Some("input"),
            Self::Shell => Some("shell"),
            Self::FileRead => Some("file.get"),
            Self::FileWrite => Some("file.put"),
            Self::ClipboardRead | Self::ClipboardWrite => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EffectiveCapabilities([bool; 7]);

impl EffectiveCapabilities {
    pub fn parse(value: &Value) -> Result<Self> {
        let routes = value["routes"]
            .as_array()
            .context("Host omitted effective routes")?;
        ensure!(
            routes.iter().all(Value::is_string),
            "Invalid effective host routes"
        );
        Ok(Self(Scope::ALL.map(|scope| {
            value[scope.name()] == true
                && scope
                    .route()
                    .is_none_or(|route| routes.iter().any(|value| value == route))
        })))
    }

    pub fn allows(&self, scope: Scope) -> bool {
        self.0[scope as usize]
    }
    pub fn revoke(&mut self, scope: Scope) {
        self.0[scope as usize] = false;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub id: u64,
    pub scopes: Vec<Scope>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permissions {
    pub available: Vec<Scope>,
    pub granted: Vec<Scope>,
    pub pending: Option<Pending>,
    pub revision: u64,
    pub attended: bool,
    pub last_decision: Option<Decision>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Approved,
    Denied,
    Cancelled,
}

fn scopes(value: &Value) -> Result<Vec<Scope>> {
    value
        .as_array()
        .context("Permission scopes must be an array")?
        .iter()
        .map(|value| {
            Scope::ALL
                .into_iter()
                .find(|scope| value.as_str() == Some(scope.name()))
                .context("Unknown permission scope")
        })
        .collect()
}

impl Permissions {
    pub fn parse(value: &Value) -> Result<Self> {
        let available = scopes(&value["available"])?;
        let granted = scopes(&value["granted"])?;
        ensure!(
            granted.iter().all(|scope| available.contains(scope)),
            "Granted scope unavailable on host"
        );
        let pending = if value["pending"].is_null() && value.get("pending").is_some() {
            None
        } else {
            Some(Pending {
                id: value["pending"]["id"]
                    .as_u64()
                    .context("Invalid permission request ID")?,
                scopes: scopes(&value["pending"]["scopes"])?,
            })
        };
        let attended = match value["mode"].as_str() {
            Some("attended") => true,
            Some("unattended") => false,
            _ => anyhow::bail!("Invalid host permission mode"),
        };
        let last_decision = if value["last_decision"].is_null() {
            None
        } else {
            Some(match value["last_decision"]["outcome"].as_str() {
                Some("approved") => Decision::Approved,
                Some("denied") => Decision::Denied,
                Some("cancelled") => Decision::Cancelled,
                _ => anyhow::bail!("Invalid permission decision"),
            })
        };
        Ok(Self {
            available,
            granted,
            pending,
            revision: value["revision"]
                .as_u64()
                .context("Invalid permission revision")?,
            attended,
            last_decision,
        })
    }

    pub fn allows(&self, scope: Scope) -> bool {
        self.available.contains(&scope) && self.granted.contains(&scope)
    }

    pub fn status(&self, scope: Scope) -> &'static str {
        if self.allows(scope) {
            "Granted"
        } else if !self.available.contains(&scope) {
            "Unavailable on this host"
        } else if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.scopes.contains(&scope))
        {
            "Waiting for host approval"
        } else if self.attended {
            "Needs host approval"
        } else {
            "Disabled by host policy"
        }
    }

    pub fn can_request(&self, scope: Scope) -> bool {
        self.attended
            && self.pending.is_none()
            && self.available.contains(&scope)
            && !self.allows(scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn permission_states_explain_policy_and_pending_approval() {
        let mut permissions = Permissions::parse(&json!({"available":["desktop","input"],"granted":["desktop"],"pending":{"id":12,"scopes":["input"]},"revision":4,"mode":"attended"})).unwrap();
        assert!(permissions.allows(Scope::Desktop));
        assert_eq!(
            permissions.status(Scope::Input),
            "Waiting for host approval"
        );
        assert!(!permissions.can_request(Scope::Input));
        assert_eq!(permissions.status(Scope::Shell), "Unavailable on this host");
        permissions.pending = None;
        assert!(permissions.can_request(Scope::Input));
        permissions.attended = false;
        assert!(!permissions.can_request(Scope::Input));
        assert_eq!(permissions.status(Scope::Input), "Disabled by host policy");
    }

    #[test]
    fn malformed_snapshots_never_grant_access() {
        assert!(Permissions::parse(&json!({})).is_err());
        assert!(Permissions::parse(&json!({"available":[],"granted":["input"],"pending":null,"revision":1,"mode":"attended"})).is_err());
        assert!(Permissions::parse(&json!({"available":["future_scope"],"granted":[],"pending":null,"revision":1,"mode":"attended"})).is_err());
    }
}
