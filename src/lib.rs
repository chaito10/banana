//! Banana: a minimal, single-binary, decentralized peer-to-peer runtime.
//!
//! The public surface is deliberately small:
//!
//! ```text
//! Banana::builder().build().await?   # start the runtime
//! banana.listen().await?              # announce ourselves to the DHT
//! banana.join(topic).await?           # discover peers for a topic
//! banana.connect(peer).await?         # reach one peer directly if possible
//! connection.open_stream().await?     # exchange opaque application bytes
//! ```
//!
//! Everything below that surface is an implementation detail and may change.
//! See `docs/architecture.md` for the component map, `docs/protocol.md` for the
//! wire formats and `docs/security.md` for the trust model.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod config;
/// Audited cryptographic primitives used by the runtime.
pub mod crypto;
/// Session endpoints: authenticated sessions plus their streams.
pub mod endpoint;
/// Error type shared by the whole runtime.
pub mod error;
/// Local key material and peer identity.
pub mod identity;
/// Stream multiplexing, flow control and the session control protocol.
pub mod mux;
pub mod obs;
/// Noise handshakes and encrypted channels.
pub mod session;
/// Socket level transports.
pub mod transport;
pub mod util;
pub mod wire;

pub use endpoint::Connection;
pub use error::{Error, Result};
pub use identity::{Identity, PeerId};
pub use mux::{Stream, StreamReader, StreamWriter};
pub use obs::{Event, EventKind, Events, Level, PathKind};
