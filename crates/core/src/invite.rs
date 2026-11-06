//! Invitation parsing. Fragments and tickets must never be logged.
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use url::Url;
use zeroize::Zeroizing;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invite {
    pub relay: String,
    pub room_id: String,
    pub client_token: String,
    pub secret: String,
}
#[derive(Deserialize, Serialize)]
pub struct Room {
    pub room_id: String,
    pub host_token: String,
    pub client_token: String,
    pub expires_at: u64,
}
pub struct HostTicket {
    pub invite: Invite,
    pub host_token: String,
}
impl HostTicket {
    pub fn new(relay: &str, room: Room) -> Result<Self> {
        let mut secret = Zeroizing::new([0; 32]);
        rand::thread_rng().fill_bytes(secret.as_mut());
        let invite = Invite {
            relay: relay.into(),
            room_id: room.room_id,
            client_token: room.client_token,
            secret: URL_SAFE_NO_PAD.encode(secret.as_ref()),
        };
        invite.validate()?;
        ensure!(valid_token(&room.host_token), "invalid host ticket");
        Ok(Self {
            invite,
            host_token: room.host_token,
        })
    }
}
fn valid_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
impl Invite {
    pub fn parse(text: &str) -> Result<Self> {
        let fragment = text.rsplit_once('#').map(|(_, f)| f).unwrap_or(text).trim();
        ensure!(fragment.len() <= 4096, "invitation length");
        let invite: Self = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(fragment)?)?;
        invite.validate()?;
        Ok(invite)
    }
    pub fn validate(&self) -> Result<()> {
        let url = Url::parse(&self.relay)?;
        let localhost = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        ensure!(
            matches!(url.scheme(), "https" | "wss")
                || (localhost && matches!(url.scheme(), "http" | "ws")),
            "relay requires TLS outside localhost"
        );
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "relay URL credentials or query"
        );
        ensure!(
            valid_token(&self.room_id) && valid_token(&self.client_token),
            "invalid room or ticket"
        );
        self.key()?;
        Ok(())
    }
    pub fn key(&self) -> Result<Zeroizing<[u8; 32]>> {
        let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(&self.secret)?);
        let key: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("invitation key length"))?;
        Ok(Zeroizing::new(key))
    }
    pub fn url(&self, web: &str) -> Result<String> {
        self.validate()?;
        let mut url = Url::parse(web)?;
        let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        ensure!(
            url.host_str().is_some()
                && (url.scheme() == "https" || (local && url.scheme() == "http")),
            "client requires HTTPS outside localhost"
        );
        ensure!(
            url.username().is_empty() && url.password().is_none(),
            "client URL credentials"
        );
        url.set_fragment(Some(&URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?)));
        Ok(url.into())
    }
    pub fn websocket_url(&self) -> Result<Url> {
        self.validate()?;
        let mut url = Url::parse(&self.relay)?;
        let scheme = if matches!(url.scheme(), "https" | "wss") {
            "wss"
        } else {
            "ws"
        };
        url.set_scheme(scheme)
            .map_err(|_| anyhow::anyhow!("websocket URL"))?;
        url.set_path(&format!(
            "{}/v1/rooms/{}/ws",
            url.path().trim_end_matches('/'),
            self.room_id
        ));
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invitation_requires_authentic_client_delivery() {
        let invite = Invite {
            relay: "https://relay.example".into(),
            room_id: "room".into(),
            client_token: "ticket".into(),
            secret: URL_SAFE_NO_PAD.encode([7; 32]),
        };
        assert!(invite.url("http://client.example").is_err());
        assert!(invite.url("https://user:password@client.example").is_err());
        let link = invite.url("https://client.example").unwrap();
        assert_eq!(Invite::parse(&link).unwrap().room_id, "room");
        assert!(invite.url("http://127.0.0.1:5173").is_ok());
        assert!(invite.url("http://[::1]:5173").is_ok());
    }
}
