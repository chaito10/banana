use std::fmt;

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// The single public error type.
///
/// The runtime deliberately keeps one error surface so applications do not
/// have to match on dozens of subsystem specific variants. Errors answer the
/// questions from the RFC: what failed, where it failed, whether a retry is
/// meaningful and whether the connection is still usable.
#[derive(Debug)]
pub enum Error {
    /// An operating system operation failed.
    Io {
        /// What the runtime was doing.
        context: &'static str,
        /// The underlying OS error.
        source: std::io::Error,
    },
    /// Local identity handling failed.
    Identity(String),
    /// A cryptographic handshake or record operation failed.
    Crypto(String),
    /// A peer sent something the runtime refuses to accept.
    Protocol(String),
    /// A remote peer reported a failure.
    Remote {
        /// Short, sanitised reason supplied by the peer.
        reason: String,
    },
    /// An operation exceeded its deadline.
    Timeout {
        /// What was being waited on.
        what: &'static str,
    },
    /// The resource was closed locally or by the peer.
    Closed {
        /// What was closed.
        what: &'static str,
    },
    /// A configured resource limit stopped the operation.
    Limit {
        /// The exhausted resource.
        what: &'static str,
        /// The configured limit.
        limit: u64,
    },
    /// The requested peer is not known to the runtime.
    UnknownPeer(String),
    /// The peer answered, but no direct or relayed path could be established.
    Unreachable {
        /// The peer that could not be reached.
        peer: String,
    },
    /// Configuration was rejected.
    Config(String),
    /// The runtime is not in a state that allows the operation.
    IllegalState(String),
    /// The feature exists in the protocol but is not enabled or implemented.
    Unsupported(String),
}

impl Error {
    /// Whether retrying the same operation later can succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Error::Io { .. }
                | Error::Timeout { .. }
                | Error::Unreachable { .. }
                | Error::UnknownPeer(_)
                | Error::Remote { .. }
        )
    }

    /// Whether the operation should never be retried as-is.
    pub fn is_permanent(&self) -> bool {
        matches!(
            self,
            Error::Identity(_)
                | Error::Config(_)
                | Error::IllegalState(_)
                | Error::Limit { .. }
                | Error::Unsupported(_)
        )
    }

    /// Whether the connection object involved is still usable.
    pub fn keeps_connection_alive(&self) -> bool {
        matches!(
            self,
            Error::Timeout { .. } | Error::Limit { .. } | Error::Remote { .. }
        )
    }

    pub(crate) fn io(context: &'static str, source: std::io::Error) -> Self {
        Error::Io { context, source }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { context, source } => write!(f, "io: {context}: {source}"),
            Error::Identity(m) => write!(f, "identity: {m}"),
            Error::Crypto(m) => write!(f, "crypto: {m}"),
            Error::Protocol(m) => write!(f, "protocol: {m}"),
            Error::Remote { reason } => write!(f, "remote: {reason}"),
            Error::Timeout { what } => write!(f, "timeout: {what}"),
            Error::Closed { what } => write!(f, "closed: {what}"),
            Error::Limit { what, limit } => write!(f, "limit: {what} exceeds {limit}"),
            Error::UnknownPeer(p) => write!(f, "unknown peer: {p}"),
            Error::Unreachable { peer } => write!(f, "peer unreachable: {peer}"),
            Error::Config(m) => write!(f, "config: {m}"),
            Error::IllegalState(m) => write!(f, "illegal state: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<Error> for std::io::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::Io { source, .. } => source,
            other => std::io::Error::new(std::io::ErrorKind::Other, other.to_string()),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Error::Io {
            context: "unspecified",
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryability_is_classified() {
        assert!(Error::Timeout { what: "dial" }.is_retryable());
        assert!(!Error::Config("bad".into()).is_retryable());
        assert!(Error::Limit {
            what: "peers",
            limit: 4
        }
        .is_permanent());
        assert!(Error::Remote {
            reason: "no".into()
        }
        .keeps_connection_alive());
        assert!(!Error::Crypto("tag".into()).keeps_connection_alive());
    }

    #[test]
    fn io_errors_round_trip() {
        let io = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "gone");
        let err: Error = Error::io("write", io);
        assert!(err.to_string().starts_with("io: write:"));
        let back: std::io::Error = err.into();
        assert_eq!(back.kind(), std::io::ErrorKind::BrokenPipe);
    }
}
