//! Audited cryptography used outside the Noise transport itself.
//!
//! The Noise handshake and session keys come from `snow`, which is the
//! implementation the Holepunch ecosystem uses. The only cryptography Banana
//! adds itself is a sealing box for hole punch coordination messages, which
//! must be protected *before* a session exists. It reuses the same Noise
//! handshake hash and established primitives rather than inventing anything.

use chacha20poly1305::aead::AeadInOut;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;

use crate::error::{Error, Result};

/// Key derivation context for hole punch sealing.
const PUNCH_INFO: &[u8] = b"banana/holepunch/v1";
/// Key derivation context for session identifiers.
const SESSION_INFO: &[u8] = b"banana/session-id/v1";

/// Derives a purpose specific key from the Noise handshake hash.
pub fn derive(handshake_hash: &[u8], info: &[u8], out: &mut [u8]) {
    let hk = Hkdf::<Sha256>::new(Some(b"banana/hkdf/v1"), handshake_hash);
    hk.expand(info, out)
        .expect("32 byte output fits an HKDF-SHA256 expand");
}

/// Derives the key used to seal NAT traversal messages between two peers.
pub fn punch_key(handshake_hash: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    derive(handshake_hash, PUNCH_INFO, &mut key);
    key
}

/// Derives a stable session id from the handshake hash.
pub fn session_id(handshake_hash: &[u8]) -> u32 {
    let mut key = [0u8; 32];
    derive(handshake_hash, SESSION_INFO, &mut key);
    u32::from_le_bytes([key[0], key[1], key[2], key[3]])
}

/// Authenticates and encrypts a message with a derived key.
///
/// Used for hole punch candidate exchange, which happens before any session
/// key exists but after both peers already share a Noise handshake hash.
#[derive(Clone)]
pub struct Sealer {
    cipher: ChaCha20Poly1305,
}

impl Sealer {
    /// Builds a sealer from a derived key.
    pub fn new(key: &[u8; 32]) -> Self {
        let cipher = ChaCha20Poly1305::new(key.into());
        Sealer { cipher }
    }

    /// Seals `plaintext` under an explicit nonce.
    pub fn seal(&self, nonce: [u8; 12], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut buffer = plaintext.to_vec();
        let nonce = Nonce::try_from(nonce.as_slice())
            .map_err(|_| Error::Crypto("nonce rejected".into()))?;
        self.cipher
            .encrypt_in_place(&nonce, aad, &mut buffer)
            .map_err(|_| Error::Crypto("seal failed".into()))?;
        Ok(buffer)
    }

    /// Opens a sealed message in place, leaving the plaintext in `buffer`.
    pub fn open_in_place(&self, nonce: [u8; 12], aad: &[u8], buffer: &mut Vec<u8>) -> Result<()> {
        let nonce = Nonce::try_from(nonce.as_slice())
            .map_err(|_| Error::Crypto("nonce rejected".into()))?;
        self.cipher
            .decrypt_in_place(&nonce, aad, buffer)
            .map_err(|_| Error::Crypto("authentication failed".into()))
    }
}

/// Random 96 bit nonce.
pub fn random_nonce() -> [u8; 12] {
    use rand::Rng;
    let mut nonce = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce);
    nonce
}

/// Sliding replay window over a 64 message nonce space.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReplayWindow {
    highest: u64,
    bitmap: u64,
    primed: bool,
}

impl ReplayWindow {
    /// Accepts a nonce, returning `false` when it is a replay or too old.
    pub fn accept(&mut self, nonce: u64) -> bool {
        if !self.primed {
            self.primed = true;
            self.highest = nonce;
            self.bitmap = 1;
            return true;
        }
        if nonce > self.highest {
            let shift = nonce - self.highest;
            self.bitmap = if shift >= 64 {
                1
            } else {
                (self.bitmap << shift) | 1
            };
            self.highest = nonce;
            return true;
        }
        let offset = self.highest - nonce;
        if offset >= 64 {
            return false;
        }
        let bit = 1u64 << offset;
        if self.bitmap & bit != 0 {
            return false;
        }
        self.bitmap |= bit;
        true
    }

    /// Whether the window has seen anything at all.
    pub fn is_primed(&self) -> bool {
        self.primed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealer_round_trips_and_detects_tampering() {
        let sealer = Sealer::new(&[7u8; 32]);
        let nonce = [3u8; 12];
        let sealed = sealer.seal(nonce, b"aad", b"payload").unwrap();
        let mut buffer = sealed.clone();
        sealer.open_in_place(nonce, b"aad", &mut buffer).unwrap();
        assert_eq!(buffer, b"payload");

        let mut tampered = sealed.clone();
        tampered[0] ^= 1;
        assert!(sealer.open_in_place(nonce, b"aad", &mut tampered).is_err());
        assert!(sealer
            .open_in_place([4u8; 12], b"aad", &mut sealed.clone())
            .is_err());
        let mut wrong_aad = sealed;
        assert!(sealer
            .open_in_place(nonce, b"other", &mut wrong_aad)
            .is_err());
    }

    #[test]
    fn derived_keys_are_deterministic_and_domain_separated() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        derive(b"handshake", b"context-a", &mut a);
        derive(b"handshake", b"context-b", &mut b);
        assert_ne!(a, b);
        let mut again = [0u8; 32];
        derive(b"handshake", b"context-a", &mut again);
        assert_eq!(a, again);
    }

    #[test]
    fn replay_window_accepts_once_per_nonce() {
        let mut window = ReplayWindow::default();
        assert!(!window.is_primed());
        assert!(window.accept(10));
        assert!(!window.accept(10));
        assert!(window.accept(11));
        assert!(window.accept(9));
        assert!(!window.accept(9));
        assert!(window.accept(1000));
        assert!(!window.accept(1000 - 65));
        assert!(window.accept(1000 - 63));
    }
}
