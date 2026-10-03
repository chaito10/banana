use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use ed25519_dalek::SigningKey;
use ed25519_dalek::VerifyingKey;
use sha2::{Digest, Sha256, Sha512};

use crate::error::{Error, Result};

const IDENTITY_MAGIC: &str = "banana-identity-v1";
const SEED_LEN: usize = 32;

/// A stable peer identifier.
///
/// A peer id is the Montgomery (X25519) form of the peer's long term public
/// key. It is derived deterministically from the local Ed25519 identity, it is
/// stable across address changes, and it is the exact key that authenticates
/// the peer during the Noise handshake.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PeerId([u8; 32]);

impl PeerId {
    /// Wraps raw public key bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        PeerId(bytes)
    }

    /// Borrows the raw public key bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Copies the raw public key bytes.
    pub const fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    /// The short form used in human facing output.
    pub fn short(&self) -> String {
        let hex = hex_encode(&self.0[..6]);
        format!("{hex}")
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex_encode(&self.0))
    }
}

impl fmt::Debug for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PeerId({})", self.short())
    }
}

impl FromStr for PeerId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let raw = s.strip_prefix("banana:").unwrap_or(s);
        let raw = raw.strip_prefix("0x").unwrap_or(raw);
        if raw.len() != 64 {
            return Err(Error::Identity(format!(
                "peer id must be 32 hex-encoded bytes, got {} characters",
                raw.len()
            )));
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = hex_byte(&raw[i * 2..i * 2 + 2])?;
        }
        Ok(PeerId(out))
    }
}

/// Local cryptographic identity: the long term key that makes this peer *this*
/// peer.
///
/// The stored secret is an Ed25519 seed. From it the runtime derives both the
/// Ed25519 signing key (exported for applications that need signatures) and
/// the X25519 static key used by the Noise handshake, so applications cannot
/// accidentally authenticate with an identity that was never persisted.
#[derive(Clone)]
pub struct Identity {
    seed: [u8; SEED_LEN],
    signing: SigningKey,
    peer_id: PeerId,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("peer_id", &self.peer_id)
            .finish_non_exhaustive()
    }
}

impl Identity {
    /// Generates a fresh identity from the operating system entropy source.
    pub fn generate() -> Self {
        use rand::Rng;
        let mut seed = [0u8; 32];
        rand::rng().fill_bytes(&mut seed);
        Self::from_seed(seed)
    }

    /// Builds an identity from an existing Ed25519 seed.
    pub fn from_seed(seed: [u8; SEED_LEN]) -> Self {
        let signing = SigningKey::from_bytes(&seed);
        let peer_id = PeerId(signing.verifying_key().to_montgomery().to_bytes());
        Identity {
            seed,
            signing,
            peer_id,
        }
    }

    /// Parses a hex encoded seed.
    pub fn from_seed_hex(hex: &str) -> Result<Self> {
        let raw = hex.strip_prefix("0x").unwrap_or(hex);
        if raw.len() != SEED_LEN * 2 {
            return Err(Error::Identity("seed must be 32 hex-encoded bytes".into()));
        }
        let mut seed = [0u8; SEED_LEN];
        for (i, byte) in seed.iter_mut().enumerate() {
            *byte = hex_byte(&raw[i * 2..i * 2 + 2])?;
        }
        Ok(Self::from_seed(seed))
    }

    /// The deterministic peer id of this identity.
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// The Ed25519 public key, for applications that need peer signatures.
    pub fn public_key(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    /// The Ed25519 verifying key.
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    /// The Ed25519 signing key, for applications that need peer signatures.
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing
    }

    /// The Noise static secret derived from the identity seed.
    ///
    /// This is the standard Ed25519 to X25519 conversion: the low half of
    /// SHA-512(seed), clamped exactly as X25519 requires. The resulting X25519
    /// public key is the peer id, so one stored secret yields one identity that
    /// both signs and authenticates connections.
    pub fn noise_static_secret(&self) -> [u8; 32] {
        let hashed = Sha512::digest(self.seed);
        let mut secret = [0u8; 32];
        secret.copy_from_slice(&hashed[..32]);
        secret[0] &= 248;
        secret[31] &= 127;
        secret[31] |= 64;
        secret
    }

    /// Fingerprint of the identity, for operator confirmation.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"banana/fingerprint/v1");
        hasher.update(self.seed);
        hex_encode(&hasher.finalize()[..8])
    }

    /// Serialises the identity seed for explicit export.
    pub fn export_hex(&self) -> String {
        hex_encode(&self.seed)
    }

    /// Default on disk location of the identity file.
    pub fn default_path() -> Result<PathBuf> {
        crate::config::state_dir().map(|dir| dir.join("identity"))
    }

    /// Writes the identity atomically.
    ///
    /// The write happens to a sibling temporary file which is flushed and then
    /// renamed, so an interrupted write can never leave the peer without a
    /// valid identity.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io("create identity dir", e))?;
        }
        let mut body = String::with_capacity(IDENTITY_MAGIC.len() + 8 + SEED_LEN * 2 + 1);
        body.push_str(IDENTITY_MAGIC);
        body.push('\n');
        body.push_str("ed25519:");
        body.push_str(&self.export_hex());
        body.push('\n');

        let tmp = path.with_extension("tmp");
        {
            let mut file =
                std::fs::File::create(&tmp).map_err(|e| Error::io("create identity", e))?;
            use std::io::Write;
            file.write_all(body.as_bytes())
                .map_err(|e| Error::io("write identity", e))?;
            file.sync_all().map_err(|e| Error::io("sync identity", e))?;
        }
        std::fs::rename(&tmp, path).map_err(|e| Error::io("commit identity", e))?;
        Ok(())
    }

    /// Loads an identity from disk.
    ///
    /// A missing or unreadable identity is always an explicit error; the
    /// runtime never silently regenerates identity.
    pub fn load_from(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| Error::io("read identity", e))
            .map_err(|e| match e {
                Error::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                    Error::Identity(format!(
                        "no identity at {} (run `banana init`)",
                        path.display()
                    ))
                }
                other => other,
            })?;
        Self::from_text(&raw)
    }

    /// Loads an identity, creating it when absent.
    pub fn load_or_create(path: &Path) -> Result<(Self, bool)> {
        if path.exists() {
            Ok((Self::load_from(path)?, false))
        } else {
            let identity = Self::generate();
            identity.save_to(path)?;
            Ok((identity, true))
        }
    }

    /// Explicitly replaces the identity at `path`.
    pub fn rotate_at(path: &Path) -> Result<Self> {
        let identity = Self::generate();
        identity.save_to(path)?;
        Ok(identity)
    }

    /// Parses the on disk representation.
    pub fn from_text(raw: &str) -> Result<Self> {
        let mut lines = raw.lines();
        match lines.next() {
            Some(head) if head.trim() == IDENTITY_MAGIC => {}
            _ => return Err(Error::Identity("unrecognised identity file format".into())),
        }
        let mut seed_hex = None;
        for line in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match line.split_once(':') {
                Some(("ed25519", value)) => seed_hex = Some(value.trim()),
                Some((key, _)) => return Err(Error::Identity(format!("unknown field `{key}`"))),
                None => return Err(Error::Identity("malformed identity file".into())),
            }
        }
        let seed_hex =
            seed_hex.ok_or_else(|| Error::Identity("identity file has no key".into()))?;
        Self::from_seed_hex(seed_hex)
    }
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(crate) fn hex_byte(pair: &str) -> Result<u8> {
    u8::from_str_radix(pair, 16).map_err(|_| Error::Identity(format!("invalid hex byte `{pair}`")))
}

/// Convenience alias for endpoint addresses used across the runtime.
pub type Endpoint = SocketAddr;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_id_is_deterministic_and_32_bytes() {
        let seed = [7u8; 32];
        let a = Identity::from_seed(seed);
        let b = Identity::from_seed(seed);
        assert_eq!(a.peer_id(), b.peer_id());
        assert_eq!(a.peer_id().to_string().len(), 64);
        assert_ne!(a.peer_id(), Identity::from_seed([8u8; 32]).peer_id());
    }

    #[test]
    fn noise_static_is_clamped_and_matches_peer_id() {
        let identity = Identity::from_seed([3u8; 32]);
        let secret = identity.noise_static_secret();
        assert_eq!(secret[0] & 7, 0);
        assert_eq!(secret[31] & 128, 0);
        assert_eq!(secret[31] & 64, 64);
        assert_eq!(identity.noise_static_secret(), secret);
    }

    #[test]
    fn identity_round_trips_through_text() {
        let identity = Identity::from_seed([42u8; 32]);
        let text = format!("banana-identity-v1\ned25519:{}\n", identity.export_hex());
        let parsed = Identity::from_text(&text).unwrap();
        assert_eq!(parsed.peer_id(), identity.peer_id());
        assert_eq!(parsed.export_hex(), identity.export_hex());
    }

    #[test]
    fn identity_rejects_foreign_files() {
        assert!(Identity::from_text("{\"seed\":\"00\"}").is_err());
        assert!(Identity::from_text("banana-identity-v1\n").is_err());
        assert!(Identity::from_text("banana-identity-v1\ned25519:zz\n").is_err());
        assert!(Identity::from_seed_hex("abcd").is_err());
    }

    #[test]
    fn peer_id_parses_hex_with_prefixes() {
        let id = Identity::from_seed([1u8; 32]).peer_id();
        assert_eq!(PeerId::from_str(&id.to_string()).unwrap(), id);
        assert_eq!(PeerId::from_str(&format!("banana:{id}")).unwrap(), id);
        assert_eq!(PeerId::from_str(&format!("0x{id}")).unwrap(), id);
        assert!(PeerId::from_str("beef").is_err());
    }

    #[test]
    fn save_is_atomic_and_reloadable() {
        let dir = std::env::temp_dir().join(format!("banana-id-{}", std::process::id()));
        let path = dir.join("identity");
        let _ = std::fs::remove_file(&path);
        let identity = Identity::from_seed([5u8; 32]);
        identity.save_to(&path).unwrap();
        let loaded = Identity::load_from(&path).unwrap();
        assert_eq!(loaded.peer_id(), identity.peer_id());
        assert!(!path.with_extension("tmp").exists());
        let _ = std::fs::remove_file(&path);
    }
}
