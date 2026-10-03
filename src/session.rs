//! Secure sessions: `Noise_XX_25519_ChaChaPoly_SHA256`.
//!
//! Law 4 of the RFC forbids an insecure "temporary" peer connection, so there
//! is no API here that hands plaintext application bytes to a caller before the
//! handshake has completed and the remote static key has been checked against
//! the expected peer id.
//!
//! The pattern was chosen because it authenticates both sides, hides the
//! initiator's identity until the third message, and provides forward secrecy
//! through ephemeral X25519 keys. See `docs/protocol.md`.

use snowstorm::snow::{HandshakeState, TransportState};
use snowstorm::Builder;

use crate::crypto::{punch_key, session_id};
use crate::error::{Error, Result};
use crate::identity::{Identity, PeerId};
use crate::wire::MAX_DATAGRAM;

/// The one and only Noise pattern Banana speaks.
pub const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_SHA256";

/// Prologue binding the handshake to this protocol and version.
///
/// Without it, a completed handshake would be replayable as some other
/// protocol's handshake.
pub const PROLOGUE: &[u8] = b"banana/noise/v1";

/// Noise message sizes for `XX`: 32 byte key plus 16 byte tag, per token.
pub const MAX_HANDSHAKE_MESSAGE: usize = 128;

/// Tag length appended by the AEAD to every record.
pub const TAG_LEN: usize = 16;

/// Which side of the handshake we are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// We dialed.
    Initiator,
    /// We accepted.
    Responder,
}

/// An authenticated, encrypted byte channel.
///
/// Records are numbered and replay protected. Because the underlying transport
/// may lose or reorder datagrams, a failed record advances the expected inbound
/// nonce by one and retries, within a bounded budget; the nonce never moves
/// backwards, so replays remain impossible.
pub struct SecureChannel {
    transport: TransportState,
    local: PeerId,
    remote: PeerId,
    handshake_hash: [u8; 32],
    max_record: usize,
    skips_left: u32,
}

impl std::fmt::Debug for SecureChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureChannel")
            .field("local", &self.local)
            .field("remote", &self.remote)
            .field("send_nonce", &self.transport.sending_nonce())
            .field("recv_nonce", &self.transport.receiving_nonce())
            .finish_non_exhaustive()
    }
}

impl SecureChannel {
    /// Largest plaintext this channel accepts in one record.
    pub fn max_record(&self) -> usize {
        self.max_record
    }

    /// The peer we are talking to.
    pub fn remote_peer(&self) -> PeerId {
        self.remote
    }

    /// Our own peer id.
    pub fn local_peer(&self) -> PeerId {
        self.local
    }

    /// Which side initiated.
    pub fn role(&self) -> Role {
        if self.transport.is_initiator() {
            Role::Initiator
        } else {
            Role::Responder
        }
    }

    /// The Noise handshake hash, shared by both peers.
    pub fn handshake_hash(&self) -> [u8; 32] {
        self.handshake_hash
    }

    /// Key used to seal NAT traversal messages with this peer.
    pub fn punch_key(&self) -> [u8; 32] {
        punch_key(&self.handshake_hash)
    }

    /// Session id derived from the handshake, used for datagram demultiplexing.
    pub fn session_id(&self) -> u32 {
        session_id(&self.handshake_hash)
    }

    /// Encrypts one record.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        if plaintext.len() > self.max_record {
            return Err(Error::Limit {
                what: "record",
                limit: self.max_record as u64,
            });
        }
        let mut buffer = vec![0u8; plaintext.len() + TAG_LEN];
        let len = self
            .transport
            .write_message(plaintext, &mut buffer)
            .map_err(|e| Error::Crypto(format!("seal: {e}")))?;
        buffer.truncate(len);
        Ok(buffer)
    }

    /// Decrypts one record, tolerating a bounded amount of record loss.
    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        if ciphertext.len() > self.max_record + TAG_LEN {
            return Err(Error::Limit {
                what: "record",
                limit: (self.max_record + TAG_LEN) as u64,
            });
        }
        let mut buffer = vec![0u8; ciphertext.len()];
        let mut nonce = self.transport.receiving_nonce();
        loop {
            match self.transport.read_message(ciphertext, &mut buffer) {
                Ok(len) => {
                    buffer.truncate(len);
                    return Ok(buffer);
                }
                Err(e) => {
                    if self.skips_left == 0
                        || nonce >= self.transport.receiving_nonce().max(u64::MAX - 2)
                    {
                        return Err(Error::Crypto(format!("open: {e}")));
                    }
                    self.skips_left -= 1;
                    nonce += 1;
                    self.transport.set_receiving_nonce(nonce);
                }
            }
        }
    }

    /// Decrypts a record into an existing buffer, avoiding one allocation.
    pub fn open_in_place(&mut self, ciphertext: &[u8], buffer: &mut Vec<u8>) -> Result<usize> {
        if ciphertext.len() > self.max_record + TAG_LEN {
            return Err(Error::Limit {
                what: "record",
                limit: (self.max_record + TAG_LEN) as u64,
            });
        }
        buffer.clear();
        buffer.resize(ciphertext.len(), 0);
        let mut nonce = self.transport.receiving_nonce();
        loop {
            match self.transport.read_message(ciphertext, buffer) {
                Ok(len) => return Ok(len),
                Err(e) => {
                    if self.skips_left == 0 {
                        return Err(Error::Crypto(format!("open: {e}")));
                    }
                    self.skips_left -= 1;
                    nonce += 1;
                    self.transport.set_receiving_nonce(nonce);
                }
            }
        }
    }
}

/// A handshake in progress.
///
/// Both roles share one implementation so the message sequence can never drift
/// between the two sides.
#[derive(Debug)]
pub struct Handshake {
    state: HandshakeState,
    role: Role,
    local: PeerId,
    expected: Option<PeerId>,
    step: u8,
    handshake_hash: [u8; 32],
}

impl Handshake {
    /// Starts a handshake as the initiator.
    pub fn initiate(identity: &Identity, expected: Option<PeerId>) -> Result<Self> {
        Self::new(identity, Role::Initiator, expected)
    }

    /// Starts a handshake as the responder.
    pub fn accept(identity: &Identity, expected: Option<PeerId>) -> Result<Self> {
        Self::new(identity, Role::Responder, expected)
    }

    fn new(identity: &Identity, role: Role, expected: Option<PeerId>) -> Result<Self> {
        let pattern = NOISE_PATTERN
            .parse()
            .map_err(|e| Error::Crypto(format!("pattern: {e}")))?;
        let static_secret = identity.noise_static_secret();
        let builder = Builder::new(pattern)
            .prologue(PROLOGUE)
            .local_private_key(&static_secret);
        let state = match role {
            Role::Initiator => builder.build_initiator(),
            Role::Responder => builder.build_responder(),
        }
        .map_err(|e| Error::Crypto(format!("build handshake: {e}")))?;
        Ok(Handshake {
            state,
            role,
            local: identity.peer_id(),
            expected,
            step: 0,
            handshake_hash: [0u8; 32],
        })
    }

    /// Which side this handshake is playing.
    pub fn role(&self) -> Role {
        self.role
    }

    /// Whether the handshake is finished and a channel can be built.
    pub fn is_complete(&self) -> bool {
        self.state.is_handshake_finished()
    }

    /// The next handshake message to put on the wire, if this is our turn.
    pub fn next_message(&mut self) -> Option<Vec<u8>> {
        if self.state.is_handshake_finished() {
            return None;
        }
        let mut buffer = [0u8; MAX_HANDSHAKE_MESSAGE];
        let len = self.state.write_message(&[], &mut buffer).ok()?;
        self.remember_hash();
        Some(buffer[..len].to_vec())
    }

    /// Consumes a handshake message from the peer.
    pub fn read_message(&mut self, message: &[u8]) -> Result<()> {
        if message.is_empty() || message.len() > MAX_HANDSHAKE_MESSAGE {
            return Err(Error::Protocol("bad handshake message length".into()));
        }
        if self.state.is_handshake_finished() {
            return Err(Error::Protocol("handshake already finished".into()));
        }
        let mut buffer = [0u8; MAX_HANDSHAKE_MESSAGE];
        self.state
            .read_message(message, &mut buffer)
            .map_err(|e| Error::Crypto(format!("handshake message rejected: {e}")))?;
        self.remember_hash();
        self.step = self.step.saturating_add(1);
        self.verify_remote_if_known()?;
        Ok(())
    }

    fn remember_hash(&mut self) {
        let hash = self.state.get_handshake_hash();
        if hash.len() == 32 {
            self.handshake_hash.copy_from_slice(hash);
        }
    }

    fn verify_remote_if_known(&self) -> Result<()> {
        let Some(remote) = self.state.get_remote_static() else {
            return Ok(());
        };
        if remote.len() != 32 {
            return Err(Error::Crypto(
                "remote static key has the wrong length".into(),
            ));
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(remote);
        let peer = PeerId::from_bytes(bytes);
        if let Some(expected) = self.expected {
            if peer != expected {
                return Err(Error::Crypto(format!(
                    "peer identity mismatch: expected {}, got {}",
                    expected.short(),
                    peer.short()
                )));
            }
        }
        Ok(())
    }

    /// Finishes the handshake, verifying that the remote peer is the expected one.
    pub fn finish(self, max_record: usize) -> Result<SecureChannel> {
        if !self.state.is_handshake_finished() {
            return Err(Error::IllegalState("handshake is not finished".into()));
        }
        self.verify_remote_if_known()?;
        let remote_bytes = self
            .state
            .get_remote_static()
            .ok_or_else(|| Error::Crypto("remote static key missing".into()))?;
        if remote_bytes.len() != 32 {
            return Err(Error::Crypto(
                "remote static key has the wrong length".into(),
            ));
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(remote_bytes);
        let remote = PeerId::from_bytes(bytes);
        if remote == self.local {
            return Err(Error::Crypto("refusing to connect to self".into()));
        }
        let transport = self
            .state
            .into_transport_mode()
            .map_err(|e| Error::Crypto(format!("transport mode: {e}")))?;
        Ok(SecureChannel {
            transport,
            local: self.local,
            remote,
            handshake_hash: self.handshake_hash,
            max_record,
            skips_left: 64,
        })
    }
}

/// Builds a channel over a maximum plaintext record size.
pub fn datagram_record_limit() -> usize {
    MAX_DATAGRAM - 64
}

/// Builds a channel for stream transports, where records may be larger.
pub fn stream_record_limit(configured: usize) -> usize {
    configured.min(64 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(byte: u8) -> Identity {
        Identity::from_seed([byte; 32])
    }

    fn handshake_pair(expected_for_responder: Option<PeerId>) -> (Handshake, Handshake) {
        let initiator = Handshake::initiate(&identity(1), Some(identity(2).peer_id())).unwrap();
        let responder = Handshake::accept(&identity(2), expected_for_responder).unwrap();
        (initiator, responder)
    }

    #[test]
    fn noise_xx_completes_and_encrypts_both_directions() {
        let (mut initiator, mut responder) = handshake_pair(Some(identity(1).peer_id()));

        let first = initiator.next_message().expect("initiator speaks first");
        assert_eq!(first.len(), 32, "message 1: ephemeral key only");
        responder.read_message(&first).unwrap();
        let second = responder.next_message().unwrap();
        assert_eq!(second.len(), 96, "message 2: ephemeral key, encrypted");
        initiator.read_message(&second).unwrap();
        let third = initiator.next_message().unwrap();
        assert_eq!(third.len(), 64, "message 3: static key, encrypted");
        responder.read_message(&third).unwrap();
        assert!(initiator.is_complete() && responder.is_complete());
        assert!(initiator.next_message().is_none());

        let mut a = initiator.finish(1024).unwrap();
        let mut b = responder.finish(1024).unwrap();
        assert_eq!(a.remote_peer(), identity(2).peer_id());
        assert_eq!(b.remote_peer(), identity(1).peer_id());
        assert_eq!(a.session_id(), b.session_id());
        assert_eq!(a.punch_key(), b.punch_key());
        assert_eq!(a.role(), Role::Initiator);

        let sealed = a.seal(b"hello").unwrap();
        assert_ne!(sealed.as_slice(), b"hello");
        assert_eq!(b.open(&sealed).unwrap(), b"hello");
        let sealed = b.seal(b"world").unwrap();
        assert_eq!(a.open(&sealed).unwrap(), b"world");
    }

    #[test]
    fn handshake_rejects_unexpected_peer() {
        let initiator = Handshake::initiate(&identity(1), Some(identity(9).peer_id())).unwrap();
        let responder = Handshake::accept(&identity(2), None).unwrap();
        let mut initiator = initiator;
        let mut responder = responder;
        let first = initiator.next_message().unwrap();
        responder.read_message(&first).unwrap();
        let second = responder.next_message().unwrap();
        let err = initiator.read_message(&second).unwrap_err();
        assert!(err.to_string().contains("identity mismatch"));
    }

    #[test]
    fn responder_learns_the_initiator_identity_in_the_third_message() {
        let (mut initiator, mut responder) = handshake_pair(Some(identity(1).peer_id()));
        let first = initiator.next_message().unwrap();
        responder.read_message(&first).unwrap();
        let second = responder.next_message().unwrap();
        initiator.read_message(&second).unwrap();
        let third = initiator.next_message().unwrap();
        responder.read_message(&third).unwrap();
        assert!(responder.finish(1024).is_ok());
    }

    #[test]
    fn responder_can_pin_the_initiator() {
        let (mut initiator, mut responder) = handshake_pair(Some(identity(7).peer_id()));
        let first = initiator.next_message().unwrap();
        responder.read_message(&first).unwrap();
        let second = responder.next_message().unwrap();
        initiator.read_message(&second).unwrap();
        let third = initiator.next_message().unwrap();
        let err = responder.read_message(&third).unwrap_err();
        assert!(err.to_string().contains("identity mismatch"));
    }

    #[test]
    fn corrupted_handshake_is_rejected() {
        let (mut initiator, mut responder) = handshake_pair(None);
        let first = initiator.next_message().unwrap();
        responder.read_message(&first).unwrap();
        let mut second = responder.next_message().unwrap();
        let last = second.len() - 1;
        second[last] ^= 0xff;
        assert!(
            initiator.read_message(&second).is_err(),
            "tampered tag must fail"
        );

        let mut initiator = Handshake::initiate(&identity(1), None).unwrap();
        let mut responder = Handshake::accept(&identity(2), None).unwrap();
        let mut first = initiator.next_message().unwrap();
        first.resize(MAX_HANDSHAKE_MESSAGE + 1, 0);
        assert!(responder.read_message(&first).is_err());
        assert!(initiator.read_message(&[]).is_err());
    }

    #[test]
    fn records_are_replay_and_tamper_resistant() {
        let (mut initiator, mut responder) = handshake_pair(None);
        drive_handshake(&mut initiator, &mut responder);
        let mut a = initiator.finish(1024).unwrap();
        let mut b = responder.finish(1024).unwrap();

        let sealed = a.seal(b"once").unwrap();
        assert_eq!(b.open(&sealed).unwrap(), b"once");
        assert!(b.open(&sealed).is_err(), "replay must fail");

        let sealed = a.seal(b"tamper").unwrap();
        let mut corrupted = sealed.clone();
        corrupted[3] ^= 0x40;
        assert!(b.open(&corrupted).is_err());
    }

    #[test]
    fn record_loss_is_tolerated_within_budget() {
        let (mut initiator, mut responder) = handshake_pair(None);
        drive_handshake(&mut initiator, &mut responder);
        let mut a = initiator.finish(1024).unwrap();
        let mut b = responder.finish(1024).unwrap();
        for index in 0..3u8 {
            a.seal(&[index; 8]).ok();
        }
        let third = a.seal(b"three").unwrap();
        assert_eq!(b.open(&third).unwrap(), b"three");
        let fourth = a.seal(b"four").unwrap();
        assert_eq!(b.open(&fourth).unwrap(), b"four");
    }

    #[test]
    fn record_size_is_bounded() {
        let (mut initiator, mut responder) = handshake_pair(None);
        drive_handshake(&mut initiator, &mut responder);
        let mut a = initiator.finish(64).unwrap();
        assert!(a.seal(&[0u8; 64]).is_ok());
        assert!(matches!(a.seal(&[0u8; 65]), Err(Error::Limit { .. })));
    }

    #[test]
    fn in_place_open_avoids_extra_allocation() {
        let (mut initiator, mut responder) = handshake_pair(None);
        drive_handshake(&mut initiator, &mut responder);
        let mut a = initiator.finish(1024).unwrap();
        let mut b = responder.finish(1024).unwrap();
        let sealed = a.seal(b"in place").unwrap();
        let mut buffer = Vec::new();
        let len = b.open_in_place(&sealed, &mut buffer).unwrap();
        assert_eq!(&buffer[..len], b"in place");
    }

    #[test]
    fn prologue_binds_the_protocol() {
        let mut initiator = Handshake::initiate(&identity(1), None).unwrap();
        let mut other = Handshake::accept(&identity(2), None).unwrap();
        let first = initiator.next_message().unwrap();
        other.read_message(&first).unwrap();
        let second = other.next_message().unwrap();
        let mut foreign = Builder::new(NOISE_PATTERN.parse().unwrap())
            .prologue(b"some-other-prologue")
            .local_private_key(&identity(2).noise_static_secret())
            .build_responder()
            .unwrap();
        let mut buffer = [0u8; MAX_HANDSHAKE_MESSAGE];
        foreign.read_message(&first, &mut buffer).unwrap();
        let mut buffer = [0u8; MAX_HANDSHAKE_MESSAGE];
        assert!(
            foreign.read_message(&second, &mut buffer).is_err(),
            "a handshake bound to another prologue must not complete"
        );
    }

    fn drive_handshake(initiator: &mut Handshake, responder: &mut Handshake) {
        let first = initiator.next_message().unwrap();
        responder.read_message(&first).unwrap();
        let second = responder.next_message().unwrap();
        initiator.read_message(&second).unwrap();
        let third = initiator.next_message().unwrap();
        responder.read_message(&third).unwrap();
    }
}
