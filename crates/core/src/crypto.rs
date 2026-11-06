//! PSK-authenticated record transport. Fresh per-connection nonces derive unique keys.
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};
pub const MAX_WIRE: usize = 150_000;
/// Conservative per-direction/per-key limits; reconnect with a fresh invitation.
pub const MAX_RECORDS: u64 = 1 << 32;
pub const MAX_KEY_BYTES: u64 = 64 * 1024 * 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Host,
    Client,
}
impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Client => "client",
        }
    }
    pub fn other(self) -> Self {
        match self {
            Self::Host => Self::Client,
            Self::Client => Self::Host,
        }
    }
    fn tag(self) -> u32 {
        match self {
            Self::Client => 1,
            Self::Host => 2,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    #[serde(rename = "type")]
    kind: String,
    version: u32,
    pub role: Role,
    pub nonce: String,
}
impl Hello {
    pub fn new(role: Role) -> Self {
        let mut random = [0; 32];
        rand::thread_rng().fill_bytes(&mut random);
        Self {
            kind: "lightremote.hello".into(),
            version: 1,
            role,
            nonce: URL_SAFE_NO_PAD.encode(random),
        }
    }
    pub fn parse(data: &[u8], expected: Role) -> Result<Self> {
        ensure!(data.len() <= 1024, "handshake length");
        let hello: Self = serde_json::from_slice(data)?;
        ensure!(
            hello.kind == "lightremote.hello" && hello.version == 1 && hello.role == expected,
            "handshake version or role"
        );
        ensure!(
            URL_SAFE_NO_PAD.decode(&hello.nonce)?.len() == 32,
            "handshake nonce"
        );
        Ok(hello)
    }
}
pub struct Cipher {
    cipher: Aes256Gcm,
    aad: Vec<u8>,
    role: Role,
    next: u64,
    bytes: u64,
}
impl Cipher {
    pub fn derive(
        secret: &[u8; 32],
        room: &str,
        client: &Hello,
        host: &Hello,
        role: Role,
    ) -> Result<Self> {
        ensure!(
            client.role == Role::Client && host.role == Role::Host,
            "handshake roles"
        );
        let mut salt = URL_SAFE_NO_PAD.decode(&client.nonce)?;
        salt.extend_from_slice(&URL_SAFE_NO_PAD.decode(&host.nonce)?);
        ensure!(salt.len() == 64, "handshake nonce length");
        let aad = format!("lightremote/v1/{room}/{}", role.name()).into_bytes();
        let mut key = Zeroizing::new([0u8; 32]);
        Hkdf::<Sha256>::new(Some(&salt), secret)
            .expand(&aad, key.as_mut())
            .map_err(|_| anyhow::anyhow!("key derivation"))?;
        salt.zeroize();
        Ok(Self {
            cipher: Aes256Gcm::new_from_slice(key.as_ref())
                .map_err(|_| anyhow::anyhow!("cipher key"))?,
            aad,
            role,
            next: 0,
            bytes: 0,
        })
    }
    fn nonce(&self) -> [u8; 12] {
        let mut nonce = [0; 12];
        nonce[..4].copy_from_slice(&self.role.tag().to_be_bytes());
        nonce[4..].copy_from_slice(&self.next.to_be_bytes());
        nonce
    }
    pub fn seal(&mut self, plain: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            plain.len() <= MAX_WIRE - 24
                && self.next < MAX_RECORDS
                && self.bytes + plain.len() as u64 <= MAX_KEY_BYTES,
            "record limit"
        );
        let mut record = self.next.to_be_bytes().to_vec();
        record.extend(
            self.cipher
                .encrypt(
                    Nonce::from_slice(&self.nonce()),
                    Payload {
                        msg: plain,
                        aad: &self.aad,
                    },
                )
                .map_err(|_| anyhow::anyhow!("record encryption"))?,
        );
        self.next += 1;
        self.bytes += plain.len() as u64;
        Ok(record)
    }
    pub fn open(&mut self, record: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            record.len() >= 24
                && record.len() <= MAX_WIRE
                && self.next < MAX_RECORDS
                && self.bytes + record.len().saturating_sub(24) as u64 <= MAX_KEY_BYTES,
            "record length"
        );
        ensure!(
            record[..8] == self.next.to_be_bytes(),
            "record counter mismatch"
        );
        let plain = self
            .cipher
            .decrypt(
                Nonce::from_slice(&self.nonce()),
                Payload {
                    msg: &record[8..],
                    aad: &self.aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("record authentication failed"))?;
        self.next += 1;
        self.bytes += plain.len() as u64;
        Ok(plain)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fails_closed_at_key_usage_limits() {
        let c = Hello::new(Role::Client);
        let h = Hello::new(Role::Host);
        let mut cipher = Cipher::derive(&[9; 32], "room", &c, &h, Role::Client).unwrap();
        cipher.next = MAX_RECORDS - 1;
        assert!(cipher.seal(b"last").is_ok());
        assert!(cipher.seal(b"over").is_err());
        let mut cipher = Cipher::derive(&[9; 32], "room", &c, &h, Role::Client).unwrap();
        cipher.bytes = MAX_KEY_BYTES;
        assert!(cipher.seal(b"over").is_err());
    }
    #[test]
    fn matches_webcrypto_shared_vector() {
        let vector: serde_json::Value =
            serde_json::from_str(include_str!("../../../testdata/secure-link-v1.json")).unwrap();
        let text = |field: &str| vector[field].as_str().unwrap();
        let key: [u8; 32] = URL_SAFE_NO_PAD
            .decode(text("secret_b64url"))
            .unwrap()
            .try_into()
            .unwrap();
        let make = |role: Role, nonce: &str| {
            Hello::parse(&serde_json::to_vec(&serde_json::json!({"type":"lightremote.hello","version":1,"role":role,"nonce":nonce})).unwrap(),role).unwrap()
        };
        let client = make(Role::Client, text("client_nonce_b64url"));
        let host = make(Role::Host, text("host_nonce_b64url"));
        let mut cipher =
            Cipher::derive(&key, text("room_id"), &client, &host, Role::Client).unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(cipher.seal(text("plaintext_utf8").as_bytes()).unwrap()),
            text("record_b64url")
        );
    }
    #[test]
    fn authenticates_direction_room_and_order() {
        let c = Hello::new(Role::Client);
        let h = Hello::new(Role::Host);
        let key = [42; 32];
        let mut tx = Cipher::derive(&key, "room", &c, &h, Role::Client).unwrap();
        let mut rx = Cipher::derive(&key, "room", &c, &h, Role::Client).unwrap();
        let frame = tx.seal(b"secret").unwrap();
        let mut tampered = frame.clone();
        tampered[9] ^= 1;
        assert!(rx.open(&tampered).is_err());
        assert_eq!(rx.open(&frame).unwrap(), b"secret");
        assert!(rx.open(&frame).is_err());
        assert!(
            Cipher::derive(&key, "other", &c, &h, Role::Client)
                .unwrap()
                .open(&frame)
                .is_err()
        );
        assert!(
            Cipher::derive(&key, "room", &c, &h, Role::Host)
                .unwrap()
                .open(&frame)
                .is_err()
        );
        assert!(
            Cipher::derive(&[43; 32], "room", &c, &h, Role::Client)
                .unwrap()
                .open(&frame)
                .is_err()
        );
    }
}
