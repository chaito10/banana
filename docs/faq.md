# FAQ

### Is Banana production-ready?

No. `0.1.x` is a library preview. The building blocks are implemented and tested, but the
runtime facade, production dialer and CLI are still landing. Do not put it in front of users
who depend on connectivity. See the [Roadmap](roadmap.md).

### Does it work over the public internet today?

Only partially. UDP/TCP transports, Noise sessions, DHT discovery and hole punching exist, but
the connection manager that ties them into an end-to-end dial is not shipped. NAT traversal
also has no relay fallback, so a failed punch ends in `Error::Unreachable`.

### Why another P2P stack instead of libp2p?

libp2p is valuable but larger than Banana's target surface. Banana deliberately keeps one
implementation of identity, discovery, sessions, framing and configuration. The RFC rejects
adopting libp2p wholesale for that reason, while allowing ideas to be borrowed. See
[RFC 37](https://github.com/chaito10/banana/blob/main/RFC.md).

### What does the DHT actually store?

Routing hints and signed announcements that bind a topic key to a peer id, an expiry and a
list of addresses. It does not store application data, files or replication state. A record
is accepted only after signature and expiry checks.

### Is discovery trusted?

No. Finding a peer means "this endpoint claims to be associated with this identity".
Authentication happens in the Noise handshake. Authorization is your application's job. Read
[Security](security.md).

### How do I get my peer id?

```rust
let (identity, _) = banana::Identity::load_or_create(&banana::Identity::default_path()?)?;
println!("{}", identity.peer_id());
```

`banana id` is planned but not implemented. The peer id is stable as long as the identity file
in the data directory survives.

### What happens if I delete the data directory?

A new identity is created on the next `load_or_create`. The new peer id is unrelated to the
old one; peers that pinned the old id will reject it. Back up the identity file if the id must
survive.

### Can I use it without Tokio?

No. The actors, transports and timers are Tokio tasks. Spawning must happen inside a Tokio
runtime.

### Does Banana phone home?

No. There is no telemetry, analytics, crash upload, metrics endpoint or update check. Logs go
to stderr; events are opt-in. See [Observability](observability.md).

### Can I run it in a browser or on mobile?

Not today. WASM is deliberately not a core requirement because browser transports differ
fundamentally from native sockets. Mobile lifecycle and networking are deferred until a
concrete target exists.

### Does it work on Windows and macOS?

The code is platform-neutral Rust using Tokio sockets. The state directory uses `APPDATA` on
Windows and `$HOME/.banana` on Unix. CI coverage across all targets is still being built out.

### IPv6?

`SocketAddr` is used throughout and the wire codec encodes both families. Bind with
`listen = ["[::]:49737"]` and ensure your bootstrap entries are reachable over the same
family.

### Why 1200-byte datagrams?

1200 bytes is a conservative MTU-safe payload that avoids IP fragmentation on typical paths.
The datagram record limit is `1200 - 64 = 1136` bytes to leave room for headers and tags.

### Why does `cargo run` do nothing?

`bin/banana.rs` is a placeholder `fn main() {}`. The CLI is phase 8. Use the library, or wait
for [CLI](cli.md).

### Is it compatible with Hyperswarm/Holepunch?

Not yet. `interop.hyperswarm_bootstrap` and `interop.holepunch_wire` are reserved switches and
enable nothing. Compatibility, when it exists, will be tested at the protocol boundary.

### Why is the license MPL-2.0?

MPL-2.0 is file-level copyleft: modifications to Banana's files stay open, while larger works
that combine it under separate files can use other terms. See [LICENSE](https://github.com/chaito10/banana/blob/main/LICENSE).

### How many peers and streams are supported?

Defaults are 64 peers, 256 streams per peer, a 64 KiB receive window per stream, `k = 20` DHT
buckets and fanout 8 per topic. Every bound is configurable; see
[Configuration](configuration.md).

### Does it reconnect after a network change?

Reconnect backoff exists (`util::Backoff`) and the swarm retries membership maintenance, but
there is no production connection manager to drive full reconnect policy yet. Treat automatic
recovery as incomplete.

### Where do I report a bug or a vulnerability?

Bugs: <https://github.com/chaito10/banana/issues>. Vulnerabilities: privately through
<https://github.com/chaito10/banana/security/advisories/new>. See
[SECURITY.md](https://github.com/chaito10/banana/blob/main/SECURITY.md).
