---
title: Overview
description: A minimal, single-binary, decentralized P2P runtime written in Rust.
---

# Banana

**A minimal, single-binary, decentralized peer-to-peer runtime written in Rust.**

Banana packages the part every P2P application rebuilds — identity, discovery, direct
connectivity, authenticated sessions and multiplexed streams — into one coherent runtime
that sits beneath your application.

```text
identity → discovery → connection → authenticated session → streams
```

[Get started](getting-started.md){ .md-button .md-button--primary }
[Read the RFC](https://github.com/chaito10/banana/blob/main/RFC.md){ .md-button }
[View on GitHub](https://github.com/chaito10/banana){ .md-button }

!!! warning "Early development"
    The building blocks are implemented and tested, but the high-level `Banana` runtime
    facade, production dialer and CLI are still landing. Treat `0.1.x` as a library
    preview, not a stable network. The [roadmap](roadmap.md) tracks the remaining work.

## What Banana gives you

<div class="grid cards" markdown>

-   **:material-fingerprint: Local identity**

    ---

    One 32-byte seed yields an Ed25519 signing key and a deterministic X25519 `PeerId`.
    Identity persists atomically and is never silently regenerated.

-   **:material-magnify: Decentralized discovery**

    ---

    A Kademlia-style DHT with bounded k-buckets, iterative lookup and signed, expiring
    announcements. Bootstrap nodes are entry points, not authorities.

-   **:material-lan-connect: Direct connectivity**

    ---

    Endpoint discovery, candidate exchange and coordinated hole punching — with an
    explicit preference for direct paths over relayed ones.

-   **:material-lock: Authenticated sessions**

    ---

    `Noise_XX_25519_ChaChaPoly_SHA256` authenticates both peers before any application
    bytes are exposed. Forward secrecy comes from ephemeral keys.

-   **:material-call-split: Multiplexed streams**

    ---

    Many logical streams over one session, with real credit-based flow control,
    keepalives and explicit stream lifecycle states.

-   **:material-eye-off: No hidden infrastructure**

    ---

    No telemetry, analytics, crash upload or metrics endpoint. Logs go to stderr;
    events are opt-in. Nothing starts unless you start it.

</div>

## The shape of the stack

```text
┌───────────────────────────────────────────────┐
│                 Your application              │
├───────────────────────────────────────────────┤
│  Swarm · topics · selection · deduplication   │
├───────────────────────────────────────────────┤
│  DHT · routing · announce · lookup            │
│  NAT · reachability · candidates · punching   │
├───────────────────────────────────────────────┤
│  Endpoint · Connection · Stream               │
│  Mux · framing · flow control                 │
├───────────────────────────────────────────────┤
│  Noise XX · authentication · encryption       │
├───────────────────────────────────────────────┤
│  Transport · UDP · TCP                        │
├───────────────────────────────────────────────┤
│  Wire · one canonical codec, strict bounds    │
└───────────────────────────────────────────────┘
```

## Design in one paragraph

Banana follows eight architectural laws: one source of truth per mutable state, no hidden
infrastructure, direct connectivity first, security as part of the connection, internal
complexity kept internal, failure modelled as a state, configuration that selects behaviour
instead of replacing the API, and no feature without a reason. The result is deliberately
small: one crate, one error type, one framing implementation and one public surface that
reads like intent — `join`, `connect`, `open_stream` — rather than implementation detail.

Read [Architecture](architecture.md) for the component map and [Protocol](protocol.md) for
the bytes on the wire.

## Status at a glance

| Capability | State |
|---|---|
| Identity, wire codec, UDP/TCP, Noise, streams | Implemented |
| DHT, NAT traversal, topic swarms | Implemented |
| Relay fallback | Planned |
| Runtime facade and production dialer | Planned |
| CLI | Placeholder |

See [Roadmap](roadmap.md) for the phase-by-phase view.

## License

Banana is licensed under the [Mozilla Public License 2.0](https://github.com/chaito10/banana/blob/main/LICENSE).
