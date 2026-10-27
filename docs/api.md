# API guide

Banana is one crate with a deliberately small promoted surface. The high-level
`Banana::builder()` facade described in the RFC is **not implemented yet**; today you compose
the implemented actors yourself.

```rust
use banana::{Config, Dht, DhtOptions, Identity, Result};
```

Everything in this guide is available from the crate root unless a module path is shown.
`#![deny(missing_docs)]` is enforced, so `cargo doc --open` is a reliable companion to this
page.

## Status of the public surface

| Module | Status | Stable? |
|---|---|---|
| `identity` | Implemented and tested | Yes within `0.1.x` |
| `config` | Implemented and tested | Yes within `0.1.x` |
| `wire` | Implemented and tested | Internal format; see [Protocol](protocol.md) |
| `transport` | Implemented and tested | Yes within `0.1.x` |
| `crypto` | Implemented and tested | Yes within `0.1.x` |
| `session` | Implemented and tested | Yes within `0.1.x` |
| `mux` | Implemented and tested | Yes within `0.1.x` |
| `endpoint` | Implemented and tested | Yes within `0.1.x` |
| `dht` | Implemented and tested | Yes within `0.1.x` |
| `nat` | Implemented; relay fallback planned | Mostly |
| `swarm` | Implemented; needs an application `Connector` | Mostly |
| `obs` | Implemented and tested | Yes within `0.1.x` |
| `error` | Implemented and tested | Yes within `0.1.x` |
| Runtime facade | **Planned** | — |

Within `0.1.x`, additive changes are expected and breaking changes are possible but should be
rare. Nothing here is a stability guarantee for a production network.

## Identity

```rust
use banana::{Identity, PeerId};

let (identity, created) = Identity::load_or_create(&Identity::default_path()?)?;
println!("{} {}", identity.peer_id(), created);

let identity = Identity::from_seed_hex("<64 hex characters>")?;
println!("{}", identity.fingerprint());
```

Key methods:

| Method | Purpose |
|---|---|
| `Identity::generate()` | Fresh key from OS entropy |
| `Identity::from_seed([u8; 32])` | Deterministic identity from a seed |
| `Identity::from_seed_hex(&str)` | Parse a 64-character hex seed |
| `Identity::load_or_create(&Path)` | Load, or create once, returning `(Identity, created)` |
| `Identity::load_from(&Path)` | Load only; missing files are errors |
| `Identity::save_to(&Path)` | Atomic write with `fsync` and rename |
| `Identity::rotate_at(&Path)` | Explicitly replace the stored identity |
| `Identity::export_hex()` | Export the seed for explicit operator use |
| `Identity::fingerprint()` | Human-comparable SHA-256 fingerprint |
| `PeerId::short()` | Short display form |
| `PeerId::{from_bytes, to_bytes, as_bytes}` | Raw 32-byte conversion |

`Identity` is `Clone`. It intentionally does not implement `Debug` for the seed and is not
serialisable through Serde. Do not log it.

## Configuration

```rust
use banana::Config;

let config = Config::load()?;                       // defaults if the file is absent
let config = Config::from_file(Path::new("node.conf"))?;
let config = Config::parse("log_level = \"info\"\n")?;
config.validate()?;
let text = config.render();
config.save(Path::new("node.conf"))?;
```

See [Configuration](configuration.md) for every key and default. `Config` is a plain value
type: clone it, pass it to actors, and keep it as the single source of truth for settings.

## Transports

```rust
use std::sync::Arc;
use banana::transport::{DatagramTransport, TcpTransport, UdpTransport};

let udp = Arc::new(UdpTransport::bind("0.0.0.0:49737".parse()?, 1024).await?);
let tcp = TcpTransport::bind("0.0.0.0:49737".parse()?, 128).await?;

let local = udp.local_addr()?;
udp.send_to(peer_addr, b"payload").await?;
let datagram = udp.recv_from().await?;
println!("{:?} {:?}", udp.stats(), tcp.stats());
```

Both transports start a receive/accept task and expose a bounded queue. `queue_depth` is the
bound: when it is full, the task stops draining the socket rather than growing memory.

Traits exist so higher layers can be tested without sockets:

- `DatagramTransport` — `send_to`, `recv_from`, `local_addr`, `close`
- `StreamTransport` — `connect`, `accept`, `local_addr`, `close`
- `AsyncStream` — the byte stream handed to a session

## Sessions and streams

```rust
use banana::session::{datagram_record_limit, Handshake};

let mut handshake = Handshake::initiate(&identity, Some(expected_peer))?;
let first = handshake.next_message().expect("initiator speaks first");
// send `first` to the peer, then feed its replies back:
// handshake.read_message(&reply)?;
let channel = handshake.finish(datagram_record_limit())?;

let peer = channel.remote_peer();
let session = channel.session_id();
```

`Handshake` also supports `Handshake::accept(&identity, None)` for the responder side. Both
roles share one implementation, so the message sequence cannot drift.

A completed `SecureChannel` is handed to `endpoint::spawn`:

```rust
use banana::endpoint::{self, SessionConfig, Wire};
use banana::obs::PathKind;

let connection = endpoint::spawn(
    channel,
    Wire::Stream(tcp_stream),
    SessionConfig::default(),
    events.clone(),
    PathKind::Direct,
);
```

`Connection` offers:

| Method | Purpose |
|---|---|
| `peer()` | Authenticated remote peer id |
| `path()` | `Direct`, `Punched` or `Relayed` |
| `open_stream().await` | Open a bidirectional stream |
| `accept().await` / `try_accept()` | Accept a peer-initiated stream |
| `stats()` | Record and stream counters |
| `is_closed()` / `close_reason()` | Observe session end |
| `close()` | End the session |

`Stream` implements `AsyncRead + AsyncWrite` and also offers `id()`, `peer()`, `close()`,
`reset()`, `is_read_closed()` and `is_write_closed()`. `StreamReader` and `StreamWriter` split
it for independent tasks.

!!! warning "No production dialer yet"
    The code that carries `Handshake` messages over a real UDP socket, applies NAT candidates
    and calls `endpoint::spawn` is the planned connection manager. Until it lands you must
    provide that glue and a `swarm::Connector` implementation yourself.

## DHT

```rust
use std::sync::Arc;
use banana::obs::EventBus;
use banana::transport::UdpTransport;
use banana::{Config, Dht, DhtOptions, Identity};

let config = Config::load()?;
let identity = Identity::load_from(&Identity::default_path()?)?;
let transport = Arc::new(UdpTransport::bind(config.listen[0], 1024).await?);

let dht = Dht::spawn(DhtOptions {
    identity,
    transport,
    config: config.clone(),
    events: EventBus::new(1024),
    advertise: Vec::new(),
})?;

dht.bootstrap(&config.bootstrap).await?;

let topic = Dht::topic("example/topic");
let stored = dht.announce(topic).await?;
let nodes = dht.find(topic).await?;

println!("{stored} records stored, {} peers found", nodes.len());
dht.close().await;
```

| Method | Purpose |
|---|---|
| `Dht::spawn(DhtOptions)` | Start the actor; requires a Tokio runtime |
| `Dht::topic(&str)` | Domain-separated topic hash |
| `bootstrap(&[SocketAddr])` | Seed the routing table |
| `ping(SocketAddr)` | Liveness probe returning `NodeInfo` |
| `lookup(PeerId)` | Iterative node lookup |
| `announce(topic)` | Sign and store an announcement; returns the store count |
| `find(topic)` | Iterative lookup filtered to fresh, verified records |
| `stats()` | Snapshot of routing and record state |
| `close()` | Stop the actor; idempotent |

`DhtOptions.advertise` adds addresses to the transport's own address; the list is deduplicated
and capped at 8.

## NAT traversal

```rust
use banana::{Nat, NatOptions};

let nat = Nat::spawn(NatOptions {
    identity,
    transport,
    config,
    events,
    advertise: Vec::new(),
})?;

let report = nat.reachability(peer, peer_addr).await?;
let observed = report.observed;
let candidates = nat.exchange_candidates(peer, peer_addr).await?;
let outcome = nat.punch(peer, candidates).await?;

println!("{:?} via {:?} after {} attempts", outcome.state, outcome.addr, outcome.attempts);
nat.close().await;
```

| Method | Purpose |
|---|---|
| `observe(peer, addr)` | Ask a peer what source address it sees |
| `reachability(peer, addr)` | Probe and classify local/observed addresses |
| `exchange_candidates(peer, addr)` | Exchange candidate lists without punching |
| `punch(peer, candidates)` | Coordinate a direct path |
| `register_channel(peer, key)` | Upgrade coordination to a session-derived key |
| `stats()` | In-flight punches, probes, rejections |
| `close()` | Stop the actor; idempotent |

`punch` returns `PunchOutcome { state: PunchState::DirectPath, addr, attempts }` on success and
`Error::Unreachable` when the budget is exhausted. Relay fallback is phase 7 and not present.

## Swarms

```rust
use std::sync::Arc;
use banana::{Purpose, Swarm, SwarmOptions, Topic};

let swarm = Swarm::spawn(SwarmOptions {
    discovery: Arc::new(dht.clone()),
    connector: Some(my_connector),
    config: config.clone(),
    events,
})?;

let peers = swarm.join(Topic::new("example/topic")).await?;
let connection = swarm.connect(peer, &addrs, Purpose::Swarm(topic)).await?;
swarm.leave(topic).await?;
swarm.close().await;
```

- `Discovery` is implemented for `Dht`; `discover` must not return the local peer.
- `Connector` is application-supplied. Returning a connection for a different peer is
  rejected by the swarm.
- `SwarmOptions.connector: None` is valid: the swarm discovers and ranks candidates but does
  not dial.
- Connections are keyed by `(peer, Purpose)` and deduplicated; concurrent connects share one
  dial.
- `join` records membership even when discovery fails, so a later maintenance tick retries.

## Events

```rust
use banana::obs::EventBus;

let events = EventBus::new(1024);
let mut stream = events.subscribe();

tokio::spawn(async move {
    while let Some(event) = stream.recv().await {
        println!("{} {:?}", event.at, event.kind);
    }
});
```

`EventBus` is cheap to clone; every actor accepts one in its options. Events are bounded per
subscriber and never block the emitter. See [Observability](observability.md).

## Errors

There is one error type:

```rust
use banana::{Error, Result};

match dht.find(topic).await {
    Ok(nodes) => println!("{}", nodes.len()),
    Err(error) if error.is_retryable() => eprintln!("retry later: {error}"),
    Err(error) => return Err(error),
}
```

| Method | True for |
|---|---|
| `is_retryable()` | `Io`, `Timeout`, `Unreachable`, `UnknownPeer`, `Remote` |
| `is_permanent()` | `Identity`, `Config`, `IllegalState`, `Limit`, `Unsupported` |
| `keeps_connection_alive()` | `Timeout`, `Limit`, `Remote` |

`Error` is `Clone` (OS errors are rebuilt from kind and message) and implements
`std::error::Error`. `Result<T>` is `std::result::Result<T, Error>`.

## What is not in the API

- `Banana::builder()` and a runtime handle
- A production connection manager/dialer
- Relay selection and relay transport
- A CLI beyond the placeholder binary
- Holepunch wire interoperability
- Any telemetry, metrics exporter or remote control surface

These are tracked in the [Roadmap](roadmap.md). If an example in the wild uses them, it is
ahead of the crate.
