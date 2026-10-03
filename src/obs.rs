//! Local, opt in observability.
//!
//! One event model and one log sink. No telemetry, no analytics, no crash
//! upload, no hidden metrics endpoint. Everything goes to stderr or to a
//! subscriber the application explicitly asked for.

use std::fmt;
use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::broadcast;

use crate::identity::PeerId;

/// Severity of a log record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Something the user must know about.
    Error,
    /// Something recoverable but suspicious.
    Warn,
    /// Lifecycle information.
    Info,
    /// Diagnostic detail.
    Debug,
    /// Very fine grained tracing.
    Trace,
}

impl Level {
    const fn as_u8(self) -> u8 {
        match self {
            Level::Error => 0,
            Level::Warn => 1,
            Level::Info => 2,
            Level::Debug => 3,
            Level::Trace => 4,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }

    /// Parses a level name, case insensitive.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "error" => Some(Level::Error),
            "warn" | "warning" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" => Some(Level::Debug),
            "trace" => Some(Level::Trace),
            _ => None,
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Warn.as_u8());

/// Sets the global log level.
pub fn set_level(level: Level) {
    LEVEL.store(level.as_u8(), Ordering::Relaxed);
}

/// The active log level.
pub fn level() -> Level {
    match LEVEL.load(Ordering::Relaxed) {
        0 => Level::Error,
        1 => Level::Warn,
        2 => Level::Info,
        3 => Level::Debug,
        _ => Level::Trace,
    }
}

/// Milliseconds since the unix epoch.
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Emits one structured record to stderr.
///
/// Private key material must never be passed here; call sites only pass peer
/// ids, addresses and counters.
pub fn log(level: Level, event: &str, fields: &[(&str, String)]) {
    if level > self::level() {
        return;
    }
    let mut line = String::with_capacity(64 + event.len());
    line.push_str(level.label());
    line.push(' ');
    line.push_str(&now_millis().to_string());
    line.push(' ');
    line.push_str(event);
    for (key, value) in fields {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        if value.contains(' ') {
            line.push('"');
            line.push_str(value);
            line.push('"');
        } else {
            line.push_str(value);
        }
    }
    line.push('\n');
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(line.as_bytes());
}

/// How a session reaches its peer. Always observable, never hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// An address we already knew was reachable.
    Direct,
    /// A path created by NAT hole punching.
    Punched,
    /// Forwarded by an explicit relay peer.
    Relayed,
}

impl PathKind {
    /// Lowercase label for logs and CLI output.
    pub fn as_str(self) -> &'static str {
        match self {
            PathKind::Direct => "direct",
            PathKind::Punched => "punched",
            PathKind::Relayed => "relayed",
        }
    }
}

impl fmt::Display for PathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Every event the runtime emits.
#[derive(Debug, Clone)]
pub enum EventKind {
    /// The runtime finished starting.
    RuntimeStarted {
        /// Local peer id.
        peer_id: PeerId,
    },
    /// The runtime began shutting down.
    RuntimeStopping,
    /// A peer was found for a topic.
    PeerDiscovered {
        /// The candidate.
        peer: PeerId,
        /// Topic hash, when discovery was topic based.
        topic: Option<[u8; 32]>,
    },
    /// A connection attempt started.
    PeerConnecting {
        /// Remote peer.
        peer: PeerId,
        /// Why the connection is being made.
        purpose: &'static str,
    },
    /// A session became usable.
    PeerConnected {
        /// Remote peer.
        peer: PeerId,
        /// How the path was obtained.
        path: PathKind,
    },
    /// A session ended.
    PeerDisconnected {
        /// Remote peer.
        peer: PeerId,
        /// Why it ended.
        reason: String,
    },
    /// A stream was opened.
    StreamOpened {
        /// Remote peer.
        peer: PeerId,
        /// Stream id.
        stream: u32,
    },
    /// A stream was closed.
    StreamClosed {
        /// Remote peer.
        peer: PeerId,
        /// Stream id.
        stream: u32,
        /// Why it closed.
        reason: String,
    },
    /// NAT traversal started for a peer.
    NatTraversalStarted {
        /// Remote peer.
        peer: PeerId,
    },
    /// NAT traversal produced a direct path.
    NatTraversalSucceeded {
        /// Remote peer.
        peer: PeerId,
    },
    /// NAT traversal did not produce a direct path.
    NatTraversalFailed {
        /// Remote peer.
        peer: PeerId,
        /// Why it failed.
        reason: String,
    },
    /// A relay was used instead of a direct path.
    RelaySelected {
        /// Remote peer.
        peer: PeerId,
        /// Relay peer.
        relay: PeerId,
    },
    /// A peer sent something unacceptable.
    ProtocolError {
        /// Remote peer, when known.
        peer: Option<PeerId>,
        /// What was wrong.
        reason: String,
    },
    /// A resource limit stopped an operation.
    LimitReached {
        /// The exhausted resource.
        what: &'static str,
        /// The configured limit.
        limit: u64,
    },
}

/// An event with a timestamp.
#[derive(Debug, Clone)]
pub struct Event {
    /// When the event happened, in unix milliseconds.
    pub at: u64,
    /// What happened.
    pub kind: EventKind,
}

/// Bounded fan out of runtime events.
#[derive(Debug)]
pub struct Events {
    receiver: broadcast::Receiver<Event>,
}

impl Events {
    pub(crate) fn new(receiver: broadcast::Receiver<Event>) -> Self {
        Events { receiver }
    }

    /// Waits for the next event.
    pub async fn recv(&mut self) -> Option<Event> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    }

    /// Returns already buffered events without waiting.
    pub fn try_recv(&mut self) -> Option<Event> {
        loop {
            match self.receiver.try_recv() {
                Ok(event) => return Some(event),
                Err(broadcast::error::TryRecvError::Empty) => return None,
                Err(broadcast::error::TryRecvError::Closed) => return None,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }
    }
}

/// Publishes events to every subscriber.
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
}

impl EventBus {
    /// Builds a bus that keeps `capacity` events buffered per subscriber.
    ///
    /// The runtime owns bus creation; tests build their own so they can assert
    /// on events without a running node.
    #[allow(dead_code)]
    pub(crate) fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        EventBus { sender }
    }

    /// Subscribes to future events.
    pub fn subscribe(&self) -> Events {
        Events::new(self.sender.subscribe())
    }

    /// Publishes an event and logs it at debug level.
    pub fn emit(&self, kind: EventKind) {
        let event = Event {
            at: now_millis(),
            kind,
        };
        if let Some(name) = event_name(&event.kind) {
            log(Level::Debug, name, &event_fields(&event.kind));
        }
        let _ = self.sender.send(event);
    }
}

fn event_name(kind: &EventKind) -> Option<&'static str> {
    Some(match kind {
        EventKind::RuntimeStarted { .. } => "runtime.started",
        EventKind::RuntimeStopping => "runtime.stopping",
        EventKind::PeerDiscovered { .. } => "peer.discovered",
        EventKind::PeerConnecting { .. } => "peer.connecting",
        EventKind::PeerConnected { .. } => "peer.connected",
        EventKind::PeerDisconnected { .. } => "peer.disconnected",
        EventKind::StreamOpened { .. } => "stream.opened",
        EventKind::StreamClosed { .. } => "stream.closed",
        EventKind::NatTraversalStarted { .. } => "nat.started",
        EventKind::NatTraversalSucceeded { .. } => "nat.succeeded",
        EventKind::NatTraversalFailed { .. } => "nat.failed",
        EventKind::RelaySelected { .. } => "relay.selected",
        EventKind::ProtocolError { .. } => "protocol.error",
        EventKind::LimitReached { .. } => "limit.reached",
    })
}

fn event_fields(kind: &EventKind) -> Vec<(&'static str, String)> {
    match kind {
        EventKind::RuntimeStarted { peer_id } => vec![("peer_id", peer_id.to_string())],
        EventKind::RuntimeStopping => vec![],
        EventKind::PeerDiscovered { peer, topic } => {
            let mut fields = vec![("peer_id", peer.to_string())];
            if let Some(topic) = topic {
                fields.push(("topic", crate::identity::hex_encode(topic)));
            }
            fields
        }
        EventKind::PeerConnecting { peer, purpose } => {
            vec![
                ("peer_id", peer.to_string()),
                ("purpose", purpose.to_string()),
            ]
        }
        EventKind::PeerConnected { peer, path } => {
            vec![
                ("peer_id", peer.to_string()),
                ("path", path.as_str().to_string()),
            ]
        }
        EventKind::PeerDisconnected { peer, reason } => {
            vec![("peer_id", peer.to_string()), ("reason", reason.clone())]
        }
        EventKind::StreamOpened { peer, stream } => {
            vec![
                ("peer_id", peer.to_string()),
                ("stream", stream.to_string()),
            ]
        }
        EventKind::StreamClosed {
            peer,
            stream,
            reason,
        } => vec![
            ("peer_id", peer.to_string()),
            ("stream", stream.to_string()),
            ("reason", reason.clone()),
        ],
        EventKind::NatTraversalStarted { peer } => vec![("peer_id", peer.to_string())],
        EventKind::NatTraversalSucceeded { peer } => vec![("peer_id", peer.to_string())],
        EventKind::NatTraversalFailed { peer, reason } => {
            vec![("peer_id", peer.to_string()), ("reason", reason.clone())]
        }
        EventKind::RelaySelected { peer, relay } => {
            vec![("peer_id", peer.to_string()), ("relay", relay.to_string())]
        }
        EventKind::ProtocolError { peer, reason } => {
            let mut fields = Vec::new();
            if let Some(peer) = peer {
                fields.push(("peer_id", peer.to_string()));
            }
            fields.push(("reason", reason.clone()));
            fields
        }
        EventKind::LimitReached { what, limit } => {
            vec![("what", what.to_string()), ("limit", limit.to_string())]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_parse_and_order() {
        assert_eq!(Level::parse("WARN"), Some(Level::Warn));
        assert_eq!(Level::parse("trace"), Some(Level::Trace));
        assert_eq!(Level::parse("loud"), None);
        assert!(Level::Error < Level::Trace);
    }

    #[test]
    fn events_are_bounded_and_delivered() {
        let bus = EventBus::new(8);
        let mut events = bus.subscribe();
        bus.emit(EventKind::RuntimeStarted {
            peer_id: PeerId::from_bytes([1u8; 32]),
        });
        let event = events.try_recv().unwrap();
        assert!(matches!(event.kind, EventKind::RuntimeStarted { .. }));
        assert!(event.at > 0);
    }

    #[test]
    fn path_kinds_render() {
        assert_eq!(PathKind::Relayed.to_string(), "relayed");
        assert_eq!(PathKind::Punched.as_str(), "punched");
    }
}
