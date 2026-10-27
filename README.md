<div align="center">

<img src="docs/assets/banana.svg" alt="Banana" width="96" height="96">

# Banana

**A minimal, single-binary, decentralized peer-to-peer runtime written in Rust.**

[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-yellow.svg)](LICENSE)
[![Rust: 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Documentation](https://img.shields.io/badge/docs-chaito10.github.io%2Fbanana-yellow)](https://chaito10.github.io/banana/)

[Documentation](https://chaito10.github.io/banana/) ·
[Getting started](docs/getting-started.md) ·
[Architecture](docs/architecture.md) ·
[Protocol](docs/protocol.md) ·
[Security](docs/security.md) ·
[RFC](RFC.md)

</div>

---

Banana is a connectivity runtime: identity, discovery, direct connectivity, authenticated
sessions and multiplexed streams in one coherent Rust crate. It is designed to sit **under**
an application, not beside it — no daemon, no database server, no cloud control plane, no
telemetry.

```text
identity → discovery → connection → authenticated session → streams
```

The design rationale, laws and acceptance criteria live in [RFC.md](RFC.md). The
implementation follows that RFC closely; where it does not yet, this README and the
[roadmap](docs/roadmap.md) say so explicitly.

## Status

> **Early development.** The transport, identity, Noise, multiplexing, DHT, NAT and swarm
> building blocks are implemented and tested. The high-level `Banana` runtime facade, the
> production connection manager/dialer and the CLI are still landing. Treat `0.1.x` as a
> library preview, not a stable network.

| Area | State | Where |
|---|---|---|
| Ed25519 identity, deterministic peer ids, atomic persistence | Implemented | [`src/identity.rs`](src/identity.rs) |
| Canonical wire codec, strict bounds checking | Implemented | [`src/wire.rs`](src/wire.rs) |
| UDP and TCP transports with bounded queues | Implemented | [`src/transport.rs`](src/transport.rs) |
| `Noise_XX_25519_ChaChaPoly_SHA256` sessions | Implemented | [`src/session.rs`](src/session.rs) |
| Multiplexed streams, flow control, keepalive | Implemented | [`src/mux.rs`](src/mux.rs), [`src/endpoint.rs`](src/endpoint.rs) |
| Kademlia DHT, routing, signed announcements | Implemented | `src/dht.rs` |
| NAT traversal, candidate exchange, hole punching | Implemented | `src/nat.rs` |
| Topic swarms, bounded peer selection, dedup | Implemented; dialer is pluggable | `src/swarm.rs` |
| Layered configuration, validation and limits | Implemented; some bounds reserved | [`src/config.rs`](src/config.rs) |
| Local-only logs, events and path reporting | Implemented | [`src/obs.rs`](src/obs.rs) |
| High-level `Banana::builder()` runtime | Planned | [Roadmap](docs/roadmap.md) |
| Production connection manager / dialer | Planned | [Roadmap](docs/roadmap.md) |
| Relay fallback (phase 7) | Planned | `src/nat.rs` (phase 7) |
| CLI (`banana init`, `id`, `listen`, …) | Placeholder binary | [`bin/banana.rs`](bin/banana.rs) |
| Holepunch wire interoperability | Config flags reserved | [`src/config.rs`](src/config.rs) |

## Why Banana

Most peer-to-peer applications end up rebuilding the same stack: identities, discovery,
routing, NAT traversal, handshakes, stream multiplexing, retries and resource limits. Banana
packages that stack once, with one implementation of each concern:

- **Identity is local.** A 32-byte seed becomes an Ed25519 signing key and an X25519
  `PeerId`. Addresses may change; identity does not.
- **Discovery is decentralized.** A Kademlia-style DHT finds peers; bootstrap nodes are
  entry points, never authorities.
- **Connections are direct first.** Local/direct beats punched, punched beats relayed, and
  a relay can never silently replace a direct path.
- **Security is part of the connection.** There is no plaintext application channel. Noise
  authenticates both sides before a single `Stream` exists.
- **Failure is a state.** Timeouts, bounded retries, backoff, replay windows and hard
  resource limits are part of the design, not an afterthought.
- **Nothing phones home.** Logs go to stderr; events are opt-in; there is no telemetry,
  analytics or hidden metrics endpoint.

## Quick start

Add Banana to a project:

```toml
[dependencies]
banana = { git = "https://github.com/chaito10/banana" }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

Create an identity and run a DHT node that announces and discovers a topic:

```rust
use std::sync::Arc;

use banana::obs::EventBus;
use banana::transport::UdpTransport;
use banana::{Config, Dht, DhtOptions, Identity};

#[tokio::main]
async fn main() -> banana::Result<()> {
    // Identity is created once and then reused; it is never regenerated silently.
    let (identity, _created) = Identity::load_or_create(&Identity::default_path()?)?;
    let config = Config::load()?;

    let transport = Arc::new(UdpTransport::bind(config.listen[0], 256).await?);
    let events = EventBus::new(256);

    let dht = Dht::spawn(DhtOptions {
        identity,
        transport,
        config: config.clone(),
        events,
        advertise: Vec::new(),
    })?;

    if !config.bootstrap.is_empty() {
        dht.bootstrap(&config.bootstrap).await?;
    }

    let topic = Dht::topic("banana/example");
    dht.announce(topic).await?;

    for peer in dht.find(topic).await? {
        println!("{} at {:?}", peer.peer, peer.addrs);
    }

    dht.close().await;
    Ok(())
}
```

Run it twice with different state directories and ports to see discovery locally. See
[Getting started](docs/getting-started.md) for a two-node walkthrough.

Streams and sessions are available as building blocks:

```rust
use banana::session::Handshake;

let mut handshake = Handshake::initiate(&identity, Some(peer))?;
let first = handshake.next_message().expect("initiator speaks first");
// …send `first`, receive the reply, read it, then finish the handshake…
```

The glue that carries handshake messages over UDP and hands the finished channel to
`endpoint::spawn` is part of the connection manager, which is not shipped yet. See the
[API guide](docs/api.md) for the current boundaries.

## Configuration

Banana looks for a small, local configuration file at `<data_dir>/config`:

```ini
# ~/.banana/config on Unix, %APPDATA%\banana\config on Windows
log_level = "info"
listen = ["0.0.0.0:49737"]
bootstrap = ["203.0.113.10:49737"]

[limits]
max_peers = 64
max_streams_per_peer = 256

[dht]
k = 20
alpha = 3

[swarm]
default_fanout = 8
```

Unknown keys are rejected, so a typo never silently changes behaviour. Every setting, its
default and its validation range is documented in [Configuration](docs/configuration.md).

Set `BANANA_HOME` to move the identity and configuration directory:

```bash
BANANA_HOME=/var/lib/banana ./my-node
```

## Building

```bash
git clone https://github.com/chaito10/banana
cd banana
cargo build --release
cargo test
```

Requirements:

- Rust **1.85+** (edition 2021)
- Tokio, `snowstorm`, `ed25519-dalek`, `chacha20poly1305`, `hkdf`, `sha2`, `rand`
- No C toolchain, JavaScript runtime, database or external service

Release profiles optimise for size (`opt-level = "z"`, fat LTO, stripped binary), while
dependencies are compiled with `opt-level = 3`.

## Documentation

The documentation site is built from [`docs/`](docs/) and published at
**<https://chaito10.github.io/banana/>**.

| Document | Contents |
|---|---|
| [Getting started](docs/getting-started.md) | Install, first node, two-node discovery, next steps |
| [Architecture](docs/architecture.md) | Layers, actors, state ownership, lifecycle |
| [Protocol](docs/protocol.md) | Datagram, DHT, NAT, Noise and stream formats |
| [Security](docs/security.md) | Trust model, cryptography, threats, limits |
| [Configuration](docs/configuration.md) | Every key, default and validation rule |
| [API guide](docs/api.md) | Public modules, types and usage patterns |
| [Observability](docs/observability.md) | Logs, events, counters and path reporting |
| [CLI](docs/cli.md) | Planned commands and current placeholder status |
| [Roadmap](docs/roadmap.md) | RFC implementation phases and current progress |
| [FAQ](docs/faq.md) | Common questions and sharp edges |
| [Contributing](docs/contributing.md) | Development workflow and review expectations |

The site is deployed by [`.github/workflows/pages.yml`](.github/workflows/pages.yml) using
MkDocs Material.

## Project layout

```text
banana/
├── Cargo.toml
├── RFC.md                  design specification
├── README.md
├── mkdocs.yml              documentation site configuration
├── bin/banana.rs           CLI entry point (placeholder)
├── docs/                   documentation source
└── src/
    ├── lib.rs              public surface and re-exports
    ├── identity.rs         Ed25519 identity and PeerId
    ├── crypto.rs           HKDF, sealing, replay windows
    ├── wire.rs             one canonical wire codec
    ├── transport.rs        UDP/TCP sockets
    ├── session.rs          Noise XX handshake and secure channel
    ├── mux.rs              framing, streams, flow control
    ├── endpoint.rs         session task and Connection/Stream API
    ├── dht.rs              bootstrap, routing, lookup, announce
    ├── dht/                DHT messages and k-bucket table
    ├── nat.rs              reachability, candidates, hole punching
    ├── swarm.rs            topics, selection, connection dedup
    ├── config.rs           layered configuration
    ├── obs.rs              logs and events
    ├── error.rs            one public error type
    └── util.rs             time, backoff, deterministic RNG
```

## Security

- All traffic that carries application bytes is Noise-encrypted and authenticated.
- DHT announcements are signed and expiry-checked; the DHT is treated as untrusted.
- NAT coordination messages are AEAD-sealed, replay-protected and rate-limited.
- Every untrusted input path is bounded by configuration.
- There is no telemetry, analytics or hidden network service.

The full trust model, threat table and residual risks are in [Security](docs/security.md).
Please report vulnerabilities privately as described in [SECURITY.md](SECURITY.md).

## Contributing

Issues and pull requests are welcome. Read [Contributing](docs/contributing.md) first.
In short:

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

Documentation changes build with:

```bash
pip install "mkdocs-material>=9.6,<10"
mkdocs serve
```

## License

Banana is licensed under the [Mozilla Public License 2.0](LICENSE).

---

<div align="center">

**Simple enough to understand. Small enough to audit. Local enough to own.**
**Decentralized enough to survive. Minimal enough to remain coherent.**

</div>
