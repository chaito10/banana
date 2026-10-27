# Observability

Banana observes itself locally. There is no telemetry, analytics, crash upload, remote logging
or hidden metrics endpoint. Everything goes to stderr or to a subscriber the application
explicitly created.

## Logs

`obs::log(level, event, fields)` writes one line to stderr:

```text
INFO 1761571200123 peer.connected peer_id=ab12… path=punched
```

```text
LEVEL timestamp_ms event key=value key="value with spaces"
```

| Field | Meaning |
|---|---|
| `LEVEL` | `ERROR`, `WARN`, `INFO`, `DEBUG` or `TRACE` |
| `timestamp_ms` | Unix milliseconds |
| `event` | Stable dotted name, for example `peer.discovered` |
| `key=value` | Structured fields; values containing spaces are quoted |

Rules the implementation follows:

- One sink, one line per record, no multi-line noise.
- Unstructured `Debug` output is never the log format.
- Only peer ids, addresses, reasons and counters are logged. Key material and seed bytes are
  never passed to `log`.
- Writing is best effort: a closed stderr does not panic or stop the runtime.

### Levels

`Level` is ordered `Error < Warn < Info < Debug < Trace`. A record is written when its level
is at or below the active level.

```rust
use banana::obs::{self, Level};

obs::set_level(Level::Info);
assert_eq!(obs::level(), Level::Info);
```

The default is `Warn`, so a library embed does not spam an application's stderr. The config
file's `log_level` is applied by the code that loads it; `obs` itself does not read the
environment.

- `error` — something the operator must know about
- `warn` — recoverable but suspicious
- `info` — lifecycle (start, stop, peer connected)
- `debug` — diagnostic detail; every event is also logged at this level
- `trace` — fine-grained tracing

## Events

Events are the structured, in-process counterpart to logs:

```rust
use banana::obs::{Event, EventBus, EventKind};

let events = EventBus::new(1024);
let mut subscription = events.subscribe();

tokio::spawn(async move {
    while let Some(Event { at, kind }) = subscription.recv().await {
        match kind {
            EventKind::PeerConnected { peer, path } => {
                println!("{at} connected {peer} via {path}");
            }
            EventKind::NatTraversalFailed { peer, reason } => {
                eprintln!("{at} punch failed for {peer}: {reason}");
            }
            _ => {}
        }
    }
});
```

`EventBus::new(capacity)` keeps at most `capacity` events buffered **per subscriber**. When a
slow subscriber falls behind, older events are dropped and `Events::recv` skips the gap; the
emitter never blocks. `capacity` is clamped to at least 1.

`Events::recv().await` waits for the next event and returns `None` when the bus is gone.
`Events::try_recv()` returns `Option<Event>` without waiting.

### Event catalogue

| Event | Fields | When |
|---|---|---|
| `RuntimeStarted` | `peer_id` | Runtime finished starting |
| `RuntimeStopping` | — | Runtime began shutting down |
| `PeerDiscovered` | `peer`, `topic?` | A candidate was found |
| `PeerConnecting` | `peer`, `purpose` | A connection attempt started |
| `PeerConnected` | `peer`, `path` | A session became usable |
| `PeerDisconnected` | `peer`, `reason` | A session ended |
| `StreamOpened` | `peer`, `stream` | A stream was opened |
| `StreamClosed` | `peer`, `stream`, `reason` | A stream closed |
| `NatTraversalStarted` | `peer` | NAT traversal began |
| `NatTraversalSucceeded` | `peer` | A direct path was found |
| `NatTraversalFailed` | `peer`, `reason` | Traversal did not succeed |
| `RelaySelected` | `peer`, `relay` | A relay was chosen (phase 7) |
| `ProtocolError` | `peer?`, `reason` | A peer sent something unacceptable |
| `LimitReached` | `what`, `limit` | A configured bound stopped work |

`PathKind` is the path label used by `PeerConnected`:

| Value | Meaning |
|---|---|
| `Direct` | A known-reachable address |
| `Punched` | A path created by hole punching |
| `Relayed` | Forwarded by an explicit relay (phase 7) |

Both `PathKind` and `Level` implement `Display`. `PeerId` displays as hex; `short()` gives a
compact form for human output.

## Counters

Each layer exposes a cheap snapshot instead of a metrics endpoint:

```rust
let dht_stats = dht.stats().await?;      // DhtStats
let nat_stats = nat.stats().await?;      // NatStats
let swarm_stats = swarm.stats().await?;  // SwarmStats
let session = connection.stats();        // SessionStats
let udp = udp_transport.stats();         // TransportStats
let tcp = tcp_transport.stats();         // TransportStats
```

| Snapshot | Fields |
|---|---|
| `DhtStats` | `known_nodes`, `stored_records`, `pending_datagrams`, `active_queries`, `bootstrapped` |
| `NatStats` | `active_punches`, `outstanding_requests`, `tracked_peers`, `probes_sent`, `paths_found`, `rejected_datagrams` |
| `SwarmStats` | `topics`, `candidates`, `connections`, `pending_dials` |
| `SessionStats` | `records_in`, `records_out`, `streams` |
| `TransportStats` | `sent`, `received`, `dropped`, `rejected` |

Counters are monotonic unless documented otherwise. `dropped` means the receive queue was
full; `rejected` means the datagram was oversized or otherwise refused. They are diagnostics,
not billing units.

## Recommended practice

- Keep `Warn` in production libraries; switch to `Info` for an operator-run node.
- Surface `PeerConnected.path` in your UI or logs. It answers "why is this peer reachable?"
  without inspecting internals.
- Treat `ProtocolError` and `LimitReached` as actionable signals. A sudden rise means either
  an attack or a limit set too low for the workload.
- Do not scrape stderr with brittle regexes. Subscribe to events; the log format may gain
  fields.
- Do not log peer addresses unless the operator needs them. They are personal data in many
  jurisdictions.

## What Banana will never add

- analytics or usage reporting
- crash upload
- a hosted dashboard
- a metrics or admin port
- a remote control API

If an integration needs long-term storage or aggregation, it belongs in your application,
fed by `Events` and the counter snapshots.
