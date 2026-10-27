# Architecture

Banana is one crate with horizontal layers. Each layer has one job, and mutable state has
exactly one owner. This page describes the components, the actor model, the data flow and
the boundaries that keep the runtime auditable.

The normative design is [RFC.md](https://github.com/chaito10/banana/blob/main/RFC.md);
this page maps that design onto the current implementation.

## Layers

```text
┌──────────────────────────────────────────────────────────────┐
│ Application                                                  │
│   uses Connection, Stream, Dht, Nat, Swarm                   │
├──────────────────────────────────────────────────────────────┤
│ Swarm        topics · candidate ranking · dedup              │
├──────────────────────────────────────────────────────────────┤
│ Discovery                 │ Connectivity                     │
│ Dht  routing/announce     │ Nat  reachability/hole punch     │
├───────────────────────────┴──────────────────────────────────┤
│ Session endpoints                                            │
│ Connection · Stream · mux framing · flow control             │
├──────────────────────────────────────────────────────────────┤
│ Secure session                                               │
│ Noise XX · authentication · record encryption                │
├──────────────────────────────────────────────────────────────┤
│ Transport                                                    │
│ UDP · TCP · bounded queues                                   │
├──────────────────────────────────────────────────────────────┤
│ Wire                                                         │
│ one canonical codec · bounds-checked decoding                │
└──────────────────────────────────────────────────────────────┘
```

| Layer | Module | Responsibility | Explicitly not responsible for |
|---|---|---|---|
| Identity | [`identity`](https://github.com/chaito10/banana/blob/main/src/identity.rs) | Key generation, peer id, atomic persistence | Authorization, naming |
| Wire | [`wire`](https://github.com/chaito10/banana/blob/main/src/wire.rs) | Canonical encoding/decoding, size limits | Policy, retries |
| Transport | [`transport`](https://github.com/chaito10/banana/blob/main/src/transport.rs) | Sockets, queues, transport counters | Topics, sessions |
| Crypto | [`crypto`](https://github.com/chaito10/banana/blob/main/src/crypto.rs) | HKDF labels, sealing, replay windows | Handshakes |
| Session | [`session`](https://github.com/chaito10/banana/blob/main/src/session.rs) | Noise XX handshake, record encryption | Stream semantics |
| Mux | [`mux`](https://github.com/chaito10/banana/blob/main/src/mux.rs) | Framing, streams, flow control | Session crypto |
| Endpoint | [`endpoint`](https://github.com/chaito10/banana/blob/main/src/endpoint.rs) | Session task, `Connection`, `Stream` | Discovery, dialing |
| DHT | `dht` | Routing, lookup, announcements | Application data |
| NAT | `nat` | Reachability, candidates, punching | Permanent relaying |
| Swarm | `swarm` | Topic membership, selection, dedup | Serialization |
| Config | [`config`](https://github.com/chaito10/banana/blob/main/src/config.rs) | Layered settings, limits, validation | Runtime API |
| Observability | [`obs`](https://github.com/chaito10/banana/blob/main/src/obs.rs) | Logs, events, path labels | Remote telemetry |
| Errors | [`error`](https://github.com/chaito10/banana/blob/main/src/error.rs) | One public error type | Subsystem-specific taxonomies |

## The public surface

`src/lib.rs` re-exports the types most applications need:

```rust
pub use config::Config;
pub use dht::{Dht, DhtOptions, DhtStats};
pub use endpoint::Connection;
pub use error::{Error, Result};
pub use identity::{Identity, PeerId};
pub use mux::{Stream, StreamReader, StreamWriter};
pub use nat::{Candidate, Nat, NatOptions, PathOption, Reachability, …};
pub use obs::{Event, EventKind, Events, Level, PathKind};
pub use swarm::{Connector, Discovery, Purpose, Swarm, SwarmOptions, Topic};
```

Everything else remains reachable through its module, but is not promoted to the facade
until it proves useful. The planned `Banana::builder()` runtime will compose these handles;
it is not implemented yet.

## Actor model

Three long-lived actors own mutable state. Applications hold cheap cloneable handles and
send commands over bounded `mpsc` channels. There is no shared lock on a datagram path.

```text
             ┌──────────────┐
  handle ───▶│  Dht actor   │  routing table · record store · queries · rate limiter
             └──────────────┘
             ┌──────────────┐
  handle ───▶│  Nat actor   │  replay windows · punch state · outstanding waits
             └──────────────┘
             ┌──────────────┐
  handle ───▶│ Swarm actor  │  topics · candidates · connections · dials in flight
             └──────────────┘
```

Each actor also owns the receive loop for its datagram kind. Transport receive tasks copy
datagrams into bounded queues, and the actor is the only code that touches its own state.

Additional task boundaries:

| Task | Owner | Why |
|---|---|---|
| UDP receive pump | `UdpTransport` | Keeps socket reads off actor threads |
| TCP accept pump | `TcpTransport` | Same, for reliable streams |
| Session writer/reader | `endpoint` | One writer per session; no interleaving |
| Discovery run | `swarm` (spawned) | The actor never blocks on the network |
| Dial attempt | `swarm` (spawned) | Same |

### One source of truth

| Mutable state | Authoritative owner | Others hold |
|---|---|---|
| Identity and seed | `Identity` (immutable) | Only public peer ids |
| Routing table | DHT actor | Snapshots via `DhtStats` |
| Announcement store | DHT actor | No cache |
| Replay windows | NAT actor | No cache |
| Connection state | `Connection` / session task | Cheap handles |
| Topic membership | Swarm actor | Ranked snapshots |
| Configuration | `Config` value | Copies passed at spawn |

The session layer is an exception in *mechanism* but not in law: the session task is the
single writer and the single owner of stream state. `Connection` clones share that one task.

## Identity model

```text
32-byte random seed
        │
        ├── Ed25519 signing key        (announcements, application signatures)
        │
        └── SHA-512(seed)[..32], clamped
                │
                └── X25519 static key  →  public key = PeerId
```

- `PeerId` is the Montgomery form of the Ed25519 public key, so it is exactly the key that
  authenticates the Noise handshake.
- The peer id is stable across address, interface and NAT changes.
- `Identity::save_to` writes a sibling temporary file, `fsync`s it and renames it, so a
  crash cannot leave an ambiguous identity.
- `load_or_create` only creates an identity when the file is absent. A read error is an
  error, never an implicit rotation.
- Identity rotation is explicit (`rotate_at`).

## Connection path

```text
PeerId ──▶ DHT lookup ──▶ endpoint hints
                              │
                              ├── direct dial (planned dialer)
                              ├── NAT candidate exchange + punch
                              └── relay (planned, phase 7)
                                        │
                                        ▼
                            Noise XX handshake
                                        │
                            authenticated SecureChannel
                                        │
                            endpoint::spawn → Connection
                                        │
                            open_stream() / accept()
                                        │
                                     Stream
```

Status: everything from `Noise XX` down is implemented and tested, including datagram and
stream wires, flow control and keepalive. The dialer/connection manager that selects
candidates, carries handshake messages and retries is still planned. Until it lands, the
swarm can discover and rank peers but requires an application-provided `Connector` to dial.

## Streams and flow control

- Frames share one codec (`mux::Frame`) on both the data and control paths.
- Stream `0` is reserved for control frames (`Open`, `OpenAck`, `Close`, `Reset`,
  `WindowUpdate`, `Ping`, `Pong`).
- Every stream has a receive window (`SessionConfig::window`, default 64 KiB). A sender may
  only write when it holds credit; readers return credit with `WindowUpdate`.
- A stalled reader therefore applies backpressure instead of causing unbounded buffering.
- Keepalive pings detect silent peers; `keepalive_timeout` ends the session.
- Closing is per direction; `reset` discards buffered data and fails pending reads.

## Ports for byte notation

The canonical encoding is little-endian for integers and uses:

| Value | Encoding |
|---|---|
| `PeerId` | 32 raw bytes |
| Address | `family:u8` (4 or 6), `port:u16`, `ip:4|16 bytes` |
| Address list | `count:u8` (max 8), then addresses |
| Datagram header | `magic:u16` (`0x424E`), `version:u8`, `kind:u8`, `session:u32` |

See [Protocol](protocol.md) for every message.

## Failure model

Failure is a state, not an exception. The implementation follows four containment rules:

1. **One broken datagram must not break the actor.** Decode errors are counted, rate-limited
   and reported as events; the receive loop continues.
2. **One broken stream must not kill the session.** A stream reset fails that stream only.
3. **One broken session must not kill the runtime.** `Connection` reports closure; handles
   remain valid for statistics.
4. **One broken peer must not kill the swarm.** Connections are deduplicated per
   `(peer, purpose)` and reaped independently.

Retries use bounded exponential backoff with jitter (`util::Backoff`). Timeouts are explicit
(`Error::Timeout`), and every untrusted input path has a configured bound.

## Startup and shutdown

Startup is explicit: create an identity, bind transports, spawn actors. Nothing starts
implicitly; no hidden daemon or service is launched.

Shutdown is handle-driven:

```text
drop or close handle
        │
        ▼
actor stops accepting commands
        │
        ▼
in-flight operations fail with Error::Closed
        │
        ▼
sessions close, streams end, events are emitted
```

`Dht::close`, `Nat::close` and `Swarm::close` are idempotent. Dropping the last handle also
stops the actor. The planned runtime facade will add ordered draining of streams and
sessions on top of these primitives.

## What is deliberately absent

- No consensus, blockchain or global naming.
- No application database or replication model.
- No mandatory relay or signaling service.
- No per-module crate explosion.
- No second serialization path.
- No telemetry.

Those are non-goals from the RFC, not unfinished work.
