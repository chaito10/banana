# Roadmap

Banana is built from the smallest useful system outward, following the phases in
[RFC 38](https://github.com/chaito10/banana/blob/main/RFC.md). This page is the current,
honest scoreboard. If a feature is listed as planned, it is not in the crate yet.

## Status

| Phase | Scope | State |
|---|---|---|
| 0 | Repository, RFC, formatting, invariants | **Partial** — repository, RFC and lint policy exist; a test CI workflow is not committed yet |
| 1 | Identity, `PeerId`, persistence | **Done** |
| 2 | UDP/TCP transports, bounded queues | **Done** — a deterministic virtual transport is not shipped; tests use real loopback sockets |
| 3 | Noise handshake, secure sessions, streams | **Done** at the session/endpoint layer; the production dialer is missing |
| 4 | DHT bootstrap, routing, lookup, announce, expiry | **Done** |
| 5 | Topic swarms, selection, deduplication | **Done**; requires an application-supplied `Connector` to dial |
| 6 | NAT candidate exchange, hole punching, path selection | **Done**; relay fallback is not |
| 7 | Relay fallback and relay status | **Not started** |
| 8 | CLI and diagnostics | **Not started** — placeholder binary only |
| 9 | Holepunch interoperability | **Not started** — config flags reserved |
| 10 | Hardening: fuzzing, long-running tests, benchmarks | **Partial** — property tests and unit/integration tests exist; fuzzing and soak tests do not |

## What works today

- Ed25519 identity with deterministic X25519 peer ids and atomic persistence.
- One bounds-checked wire codec for datagrams, DHT messages, NAT messages and records.
- UDP and TCP transports with bounded queues and counters.
- `Noise_XX_25519_ChaChaPoly_SHA256` handshakes with expected-peer checking.
- Multiplexed streams with credit-based flow control, keepalive and reset semantics.
- An endpoint task that turns a `SecureChannel` plus a transport into a `Connection`.
- A Kademlia-style DHT with signed, expiring announcements and rate limiting.
- NAT reachability classification, candidate exchange, hole punching and deterministic path
  selection.
- Topic swarms with ranked candidates, bounded fanout and deduplicated dials.
- Layered configuration with strict validation and local-only observability.

## The blockers for a usable node

1. **Runtime facade.** There is no `Banana::builder()` and no single handle that owns the
   actors. Applications assemble `Dht`, `Nat` and `Swarm` themselves.
2. **Connection manager / dialer.** The glue that carries Noise handshake messages over UDP,
   consults NAT candidates, retries with backoff and calls `endpoint::spawn` does not exist.
   Until it does, end-to-end dialing is application work.
3. **Relay fallback.** A failed punch returns `Error::Unreachable`. The `relay_*` config keys
   are placeholders.
4. **CLI.** Operators cannot initialise, inspect or run a node without writing Rust.
5. **Interoperability.** The reserved `interop.*` flags do not enable a Holepunch adapter.

## Near-term order

1. Land the runtime facade over the existing actors.
2. Add the production connector: address selection, handshake transport, retry and dedup.
3. Add an integration test that dials between two processes, not two in-process harnesses.
4. Implement the CLI commands in [CLI](cli.md).
5. Add relay fallback with explicit, authenticated relay peers.
6. Add fuzzing for the decoder and soak tests for the actors.
7. Evaluate Holepunch wire interoperability only where tests can prove it.

## Explicit non-goals

These are not "coming later"; they are out of scope by design:

- consensus, blockchain or global naming
- a hosted discovery or signaling service
- a mandatory relay network
- application storage or replication (Hypercore-like)
- a browser/WASM transport as a core requirement
- a plugin system or per-module crate explosion
- telemetry or a remote control plane

## Versioning intent

`0.1.x` is a library preview. Expect additive changes and occasional breaking changes as the
facade and dialer settle. A `1.0` should mean: stable public surface, working CLI, relay
fallback, and documented interoperability limits — not merely "the tests pass".
