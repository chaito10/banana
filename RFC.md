# RFC: Banana — A Minimal, Single-Binary, Decentralized Rust P2P Runtime

**Status:** Draft  
**Author:** Banana Contributors  
**Date:** 2026-10-03  
**PR/Discussion:** [TODO]  
**Related:** Inspired by and intended to interoperate pragmatically with the [Holepunch](https://github.com/holepunchto) ecosystem

---

## Table of Contents

- [1. Abstract](#1-abstract)
- [2. Motivation](#2-motivation)
- [3. Problem Statement](#3-problem-statement)
- [4. Goals](#4-goals)
- [5. Non-Goals](#5-non-goals)
- [6. Design Philosophy](#6-design-philosophy)
- [7. Architectural Laws](#7-architectural-laws)
- [8. Terminology](#8-terminology)
- [9. System Model](#9-system-model)
- [10. High-Level Architecture](#10-high-level-architecture)
- [11. Core Components](#11-core-components)
- [12. Peer Identity and Trust](#12-peer-identity-and-trust)
- [13. Transport and Networking](#13-transport-and-networking)
- [14. DHT and Discovery](#14-dht-and-discovery)
- [15. NAT Traversal and Direct Connectivity](#15-nat-traversal-and-direct-connectivity)
- [16. Swarm and Peer Selection](#16-swarm-and-peer-selection)
- [17. Secure Sessions](#17-secure-sessions)
- [18. Multiplexing and Streams](#18-multiplexing-and-streams)
- [19. RPC and Wire Protocol](#19-rpc-and-wire-protocol)
- [20. Lifecycle and State Machines](#20-lifecycle-and-state-machines)
- [21. Storage and Persistence](#21-storage-and-persistence)
- [22. Relay and Fallback Model](#22-relay-and-fallback-model)
- [23. CLI and User Experience](#23-cli-and-user-experience)
- [24. Single Binary and Cross-Platform Strategy](#24-single-binary-and-cross-platform-strategy)
- [25. Repository and Code Organization](#25-repository-and-code-organization)
- [26. Dependency Philosophy](#26-dependency-philosophy)
- [27. API Design](#27-api-design)
- [28. Configuration](#28-configuration)
- [29. Security Model](#29-security-model)
- [30. Failure Model and Recovery](#30-failure-model-and-recovery)
- [31. Resource Management](#31-resource-management)
- [32. Observability](#32-observability)
- [33. Testing and Verification](#33-testing-and-verification)
- [34. Performance and Efficiency](#34-performance-and-efficiency)
- [35. Interoperability](#35-interoperability)
- [36. Compatibility Strategy](#36-compatibility-strategy)
- [37. Alternatives Considered](#37-alternatives-considered)
- [38. Implementation Phases](#38-implementation-phases)
- [39. Open Questions](#39-open-questions)
- [40. Acceptance Criteria](#40-acceptance-criteria)
- [41. Definition of Done](#41-definition-of-done)
- [42. References](#42-references)

---

# 1. Abstract

Banana is a from-scratch Rust implementation of a compact peer-to-peer runtime inspired by the networking model of the Holepunch ecosystem.

The purpose of Banana is not to reproduce an existing package ecosystem one crate at a time. The purpose is to **distill the smallest coherent runtime required to build genuinely peer-to-peer applications**.

Banana therefore treats the following as architectural constraints rather than preferences:

- one repository;
- one primary runtime;
- one distributable executable;
- no mandatory central server;
- no mandatory cloud account;
- no JavaScript runtime;
- no unnecessary micro-crates;
- no duplicated protocol implementations;
- no hidden background services;
- no telemetry or phone-home behavior by default;
- direct peer-to-peer connectivity whenever the network permits it;
- decentralized discovery rather than application-owned central registries;
- local ownership of identity and state;
- explicit failure and recovery;
- auditable cryptography and protocol boundaries.

The intended result is a small P2P foundation that can sit underneath tools such as decentralized Git/Fossil workflows, file synchronization, private collaboration, local-first applications, distributed development tools, research infrastructure, and future P2P services.

The central architectural idea is:

> **A peer should be able to own its identity, discover another peer, authenticate that peer, establish a direct connection, exchange data, and recover from failure without depending on a centralized application server.**

The runtime should make that possible without requiring every application developer to rebuild DHT discovery, NAT traversal, encryption, connection management, and peer lifecycle independently.

---

# 2. Motivation

Modern P2P systems frequently become difficult to understand because networking functionality is distributed across many packages, services, daemons, protocols, and deployment assumptions.

The Holepunch ecosystem demonstrates a useful model built around primitives such as DHT discovery, Hyperswarm-style topic discovery, encrypted peer connections, and append-oriented distributed data structures. The current RFC already identifies the main motivation for rethinking that model in Rust: runtime overhead, distribution complexity, fragmentation, safety, and portability. 

Banana takes the next step:

**Do not merely port the packages. Reconsider the boundaries.**

The goal is to answer:

> What is the minimum amount of machinery required for a useful, secure, decentralized P2P runtime?

This question changes the implementation strategy.

Instead of:

```text
existing package
    ↓
Rust port
    ↓
Rust crate
    ↓
another Rust crate
    ↓
wrapper crate
    ↓
CLI
```

Banana prefers:

```text
                 Banana
                    │
        ┌───────────┴───────────┐
        │                       │
     Library                 Binary
        │                       │
        └───────────┬───────────┘
                    │
          One coherent runtime
                    │
       ┌────────────┼────────────┐
       │            │            │
    Discovery   Connectivity  Security
       │            │            │
       └────────────┼────────────┘
                    │
              Peer connection
```

The runtime should be understandable as one system rather than as a collection of unrelated products.

---

# 3. Problem Statement

Banana addresses five related problems.

## 3.1 P2P infrastructure is repeatedly reinvented

Application developers who want a decentralized application often have to solve:

- identity;
- peer discovery;
- DHT routing;
- NAT traversal;
- connection establishment;
- encryption;
- multiplexing;
- peer lifecycle;
- retry;
- backpressure;
- resource limits;
- observability.

This causes duplicated implementations and inconsistent security assumptions.

Banana should provide these as one coherent runtime.

## 3.2 Distribution is unnecessarily complicated

A P2P tool should ideally be downloadable and runnable as:

```text
banana
```

rather than requiring:

```text
runtime
+ package manager
+ native dependencies
+ application packages
+ configuration service
+ daemon
+ external database
```

A single executable is not merely a packaging preference. It reduces operational state.

## 3.3 Centralization is often introduced accidentally

Many systems begin as P2P applications but gradually acquire:

```text
client
  ↓
central API
  ↓
central database
  ↓
central identity
```

Banana deliberately reverses this dependency:

```text
peer ↔ peer
 ↑     ↑
 └─ DHT ┘
```

The DHT is infrastructure for finding peers, not an application-owned authority.

## 3.4 Modularity can become fragmentation

Modularity is useful when boundaries correspond to independent responsibilities.

It becomes harmful when every function becomes:

- a package;
- a service;
- a repository;
- a configuration layer;
- a version boundary;
- a network protocol.

Banana therefore distinguishes **logical modularity** from **distribution modularity**.

The code may have modules internally without turning every module into an independently distributed crate.

## 3.5 P2P systems need failure-first design

The network is inherently unreliable.

Peers disappear.

NAT mappings change.

Packets are lost.

Addresses become stale.

Processes crash.

Storage can become unavailable.

The runtime must therefore model failure as normal operation rather than as an exceptional afterthought.

---

# 4. Goals

## 4.1 Primary goals

1. **Complete minimal P2P runtime in one repository.**
2. **Single executable distribution.**
3. **Rust-first implementation.**
4. **Local-first operation.**
5. **Decentralized discovery.**
6. **Direct peer-to-peer connectivity whenever possible.**
7. **Encrypted and authenticated peer sessions.**
8. **NAT traversal and hole punching.**
9. **Optional relay fallback.**
10. **Minimal public API.**
11. **Explicit state machines.**
12. **Deterministic testing.**
13. **Predictable resource consumption.**
14. **No mandatory cloud infrastructure.**
15. **No mandatory telemetry.**
16. **No unnecessary protocol duplication.**
17. **No application-specific assumptions in the runtime core.**
18. **Practical interoperability with Holepunch protocols where useful.**

## 4.2 Philosophical goals

Banana should encourage applications where:

```text
identity       → belongs to the peer
data           → belongs to the user/application
discovery      → decentralized
connection     → peer-to-peer
encryption     → end-to-end
availability   → emerges from peers
infrastructure → optional
server         → not required
```

This does not mean every application must be fully decentralized.

It means the runtime should not force centralization where it is unnecessary.

---

# 5. Non-Goals

Banana is not intended to:

- become a general-purpose distributed computing framework;
- reproduce every Holepunch package;
- provide a full blockchain;
- provide consensus;
- provide global naming authority;
- provide a mandatory relay network;
- provide a hosted SaaS control plane;
- reproduce Node.js semantics;
- expose every internal subsystem as a public API;
- support every historical protocol;
- become a generic replacement for all P2P libraries;
- embed an application database;
- impose a specific application-level replication model;
- solve application authorization for every use case.

Banana provides **connectivity primitives**, not an entire decentralized application stack.

---

# 6. Design Philosophy

Banana follows six principles.

## 6.1 Simple

A developer should be able to explain the runtime without drawing a hundred boxes.

Prefer:

```text
identity → discovery → connection → secure stream
```

over:

```text
identity service
→ registry service
→ discovery abstraction
→ resolver
→ transport abstraction
→ connection factory
→ session manager
→ stream provider
→ adapter
```

Every abstraction must justify its existence.

## 6.2 Minimal

Minimal does not mean incomplete.

Minimal means:

> Remove anything that is not necessary to preserve the system's intended capability.

A feature is not justified merely because another framework provides it.

## 6.3 No duplication

There must be one canonical implementation for each fundamental concern.

Examples:

- one peer identity model;
- one connection registry;
- one retry policy;
- one framing implementation;
- one serialization path;
- one lifecycle model;
- one source of truth for connection state.

"DRY" must not be interpreted as "create abstractions everywhere."

The correct rule is:

> **Deduplicate concepts, not necessarily every line of code.**

## 6.4 Local ownership

The peer owns:

- its identity;
- its private keys;
- its configuration;
- its local state;
- its connections;
- its application data.

The runtime should not require an account on a central service.

## 6.5 Decentralization by default

A central service may be used when an application explicitly needs one.

The runtime itself should not make it mandatory.

The preferred dependency graph is:

```text
                    ┌──────────────┐
                    │  Application │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │    Banana    │
                    └──────┬───────┘
                           │
             ┌─────────────┼─────────────┐
             │             │             │
          Peer A        Peer B        Peer C
             │             │             │
             └─────────────┼─────────────┘
                           │
                         DHT
```

The DHT helps peers find one another; it does not become the owner of application state.

## 6.6 Boring infrastructure

Banana should prefer mature, well-understood mechanisms over novel mechanisms.

The runtime should be innovative primarily in **composition and simplification**, not by inventing new cryptographic primitives or unnecessary protocols.

---

# 7. Architectural Laws

These laws are stronger than ordinary design guidelines.

## Law 1 — One source of truth

For every piece of mutable runtime state, exactly one subsystem owns authoritative state.

Examples:

```text
peer identity       → Identity
connection state    → Connection Manager
routing state       → DHT
topic membership    → Swarm
runtime lifecycle   → Runtime
```

Other components may hold references, caches, or derived state, but they must not create competing authorities.

## Law 2 — No hidden infrastructure

Starting Banana must not silently start:

- a cloud service;
- a telemetry client;
- a background server;
- a database server;
- a proxy;
- a relay service.

Everything external must be explicit.

## Law 3 — Direct connectivity first

The connection strategy is:

```text
local/direct
    ↓
NAT traversal / hole punch
    ↓
explicit relay
    ↓
failure
```

A relay is a fallback, not the default architecture.

## Law 4 — Security is part of the connection

There should not be an insecure "temporary" peer connection that applications are expected to secure later.

The connection lifecycle is:

```text
discover
   ↓
connect
   ↓
authenticate
   ↓
encrypt
   ↓
stream
```

## Law 5 — Internal complexity stays internal

A complex implementation detail does not automatically become a public abstraction.

The public API should expose intent:

```rust
join(topic)
connect(peer)
open_stream()
```

rather than implementation details:

```rust
bucket_table()
socket_registry()
noise_state()
mux_frame()
nat_probe()
```

## Law 6 — Failure is a state

A peer is not simply:

```text
connected / disconnected
```

It may be:

```text
unknown
discovering
candidate
connecting
authenticating
connected
degraded
reconnecting
closed
failed
```

The implementation should model these transitions explicitly.

## Law 7 — Configuration is not a second programming language

Configuration should select behavior.

It should not become a declarative replacement for the runtime API.

## Law 8 — No feature without a reason

Every major feature should answer:

1. What problem does it solve?
2. Why does the runtime need it?
3. Why can it not live at the application layer?
4. What complexity does it introduce?
5. What is the failure behavior?
6. What is the security implication?

If those questions cannot be answered, the feature does not belong in the core.

---

# 8. Terminology

| Term | Definition |
|---|---|
| **Peer** | A runtime instance identified by a public cryptographic identity. |
| **Peer ID** | Stable identifier derived from the peer's public identity key. |
| **Identity** | Local cryptographic key material and its public identifier. |
| **DHT** | Distributed routing/discovery system used to locate peers and rendezvous points. |
| **Topic** | Deterministic identifier representing a logical peer group. |
| **Swarm** | Runtime subsystem managing discovery and connection membership for a topic. |
| **Transport** | Network mechanism carrying packets or streams. |
| **Session** | Authenticated encrypted relationship between two peers. |
| **Stream** | Logical bidirectional byte channel inside a session. |
| **Relay** | A peer or service that forwards traffic when direct connectivity fails. |
| **Runtime** | The complete Banana execution engine. |
| **Application** | Software embedding or invoking Banana. |
| **Control plane** | Discovery, connection negotiation, lifecycle, and management messages. |
| **Data plane** | Application payload flowing through established streams. |

---

# 9. System Model

Banana assumes an asynchronous, partially connected network.

A peer may have:

- a stable identity;
- zero or more local addresses;
- zero or more public addresses;
- temporary NAT mappings;
- changing network interfaces;
- changing reachability;
- no globally routable address.

The runtime therefore separates:

```text
identity
   ≠
address
   ≠
reachability
   ≠
authorization
```

This distinction is fundamental.

A peer identity should remain stable even when:

- its IP changes;
- its port changes;
- it moves from Wi-Fi to Ethernet;
- it moves between networks;
- its NAT mapping expires.

## 9.1 Trust model

Discovery does not imply trust.

Finding a peer means:

> "This endpoint claims to be associated with this identity."

Authentication establishes:

> "The endpoint controls the private key corresponding to that identity."

Application authorization establishes:

> "This authenticated peer is allowed to perform this application operation."

These are separate layers.

---

# 10. High-Level Architecture

```text
┌───────────────────────────────────────────────────────────────┐
│                         BANANA BINARY                         │
│                         banana CLI                            │
├───────────────────────────────────────────────────────────────┤
│                         Public API                            │
│                Minimal embedding interface                   │
├───────────────────────────────────────────────────────────────┤
│                           Runtime                             │
│   lifecycle · events · resource limits · shutdown · tasks    │
├───────────────────────┬───────────────────────┬───────────────┤
│       Discovery       │     Connectivity      │   Security    │
│                       │                       │               │
│  DHT · topics ·       │  connections · NAT   │ Noise · keys  │
│  routing · announce   │  traversal · relay   │ authentication │
├───────────────────────┴───────────────────────┴───────────────┤
│                     Session / Stream                          │
│              framing · multiplexing · flow control            │
├───────────────────────────────────────────────────────────────┤
│                       Transport                              │
│                    UDP · TCP · sockets                         │
├───────────────────────────────────────────────────────────────┤
│                       Platform                               │
│                OS · time · filesystem · RNG                    │
└───────────────────────────────────────────────────────────────┘
```

The architecture is intentionally horizontal rather than package-oriented.

---

# 11. Core Components

Banana has the following conceptual components.

## 11.1 Identity

Responsible for:

- key generation;
- public identity;
- secure local persistence;
- identity loading;
- explicit rotation/import/export.

It does not perform authorization.

## 11.2 Transport

Responsible for:

- UDP sockets;
- TCP sockets;
- packet I/O;
- stream I/O;
- local address discovery;
- socket lifecycle.

It does not know application topics.

## 11.3 DHT

Responsible for:

- peer routing;
- lookup;
- announcement;
- rendezvous;
- bootstrap.

It does not own application data.

## 11.4 NAT traversal

Responsible for:

- endpoint discovery;
- reachability testing;
- hole-punch coordination;
- direct path establishment.

It does not become a permanent proxy.

## 11.5 Connection Manager

Responsible for:

- connection deduplication;
- connection state;
- reconnect;
- graceful close;
- peer connection limits.

It is the authoritative source of connection state.

## 11.6 Secure Session

Responsible for:

- handshake;
- peer authentication;
- encryption;
- session keys;
- replay/order protections as required by the selected protocol.

## 11.7 Multiplexer

Responsible for:

- logical streams;
- framing;
- flow control;
- stream lifecycle.

## 11.8 Swarm

Responsible for:

- topic membership;
- peer discovery;
- candidate selection;
- connection intent;
- membership lifecycle.

## 11.9 Runtime

Responsible for:

- startup;
- shutdown;
- task ownership;
- event delivery;
- global resource limits;
- cancellation.

---

# 12. Peer Identity and Trust

## 12.1 Identity model

The preferred model is:

```text
private key
    │
    └── public key
            │
            └── PeerId
```

Ed25519 may be used for long-term identity.

X25519 may be used for key agreement through the selected Noise pattern.

The exact cryptographic construction must be documented as a protocol decision before implementation is considered stable.

## 12.2 Identity persistence

On first launch:

```text
banana
  ↓
generate identity
  ↓
store locally
  ↓
use identity for future sessions
```

Identity must not silently regenerate after a storage error.

A missing or unreadable identity should result in an explicit error.

## 12.3 Identity rotation

Identity rotation should be explicit.

The runtime must never silently replace the peer identity.

If rotation is implemented, it should require an explicit command or API call.

## 12.4 Trust separation

Banana does not equate:

```text
known identity
```

with:

```text
authorized application user
```

Applications remain responsible for policy.

---

# 13. Transport and Networking

## 13.1 UDP

UDP is the preferred datagram transport for:

- DHT;
- discovery;
- endpoint probing;
- hole punching.

The implementation must account for:

- MTU;
- packet loss;
- duplication;
- reordering;
- fragmentation avoidance;
- rate limiting.

## 13.2 TCP

TCP provides a reliable byte stream where appropriate.

It is useful for:

- fallback connectivity;
- environments where UDP is unavailable;
- application streams where TCP semantics are sufficient.

## 13.3 Transport abstraction

The transport interface must remain narrow.

Conceptually:

```rust
trait DatagramTransport {
    async fn send(&self, endpoint: Endpoint, data: &[u8]) -> Result<()>;
    async fn recv(&self) -> Result<Datagram>;
}
```

and:

```rust
trait StreamTransport {
    async fn connect(&self, endpoint: Endpoint) -> Result<RawConnection>;
    async fn accept(&self) -> Result<RawConnection>;
}
```

The actual API may differ, but the principle remains:

> The higher layers should depend on capabilities, not operating-system socket details.

## 13.4 One runtime I/O model

Banana should avoid multiple competing async runtimes.

The default design should use one coherent I/O model.

The runtime should own the event loop and task lifecycle.

---

# 14. DHT and Discovery

## 14.1 Purpose

The DHT exists to answer:

> "Where can I find a peer associated with this identifier or topic?"

It does not answer:

> "What data does that peer own?"

Data ownership remains at the application layer.

## 14.2 Routing

The DHT should use a Kademlia-inspired routing model where appropriate.

The implementation should optimize for:

- predictable memory;
- bounded routing state;
- efficient lookup;
- resilience against stale peers;
- deterministic tests.

## 14.3 Bootstrap

Bootstrap nodes are entry points, not authorities.

A peer should be able to:

1. bootstrap;
2. discover peers;
3. learn additional peers;
4. continue operating without treating one bootstrap node as a central server.

Bootstrap configuration should be:

```text
built-in defaults
        +
user override
        +
application override
```

No mandatory remote bootstrap-list fetch.

## 14.4 Announce

A peer can announce availability for a topic or rendezvous identifier.

Announcements should be:

- ephemeral by default;
- refreshed periodically;
- invalidated when possible;
- treated as hints rather than permanent truth.

## 14.5 Discovery is probabilistic

A DHT cannot guarantee that a peer will always be found.

Therefore the API must expose discovery as an asynchronous process rather than as an absolute lookup guarantee.

---

# 15. NAT Traversal and Direct Connectivity

## 15.1 Principle

The preferred path is:

```text
Peer A
  │
  │ direct
  ▼
Peer B
```

not:

```text
Peer A
  │
  ▼
Central Server
  │
  ▼
Peer B
```

## 15.2 Candidate exchange

The runtime should exchange reachable endpoint candidates through the rendezvous mechanism.

Candidate types may include:

- local address;
- observed address;
- public address;
- previously known address.

## 15.3 Hole punching

The NAT traversal state machine should be explicit:

```text
DISCOVER
   ↓
CANDIDATES
   ↓
PROBE
   ↓
DIRECT_PATH
   │
   └──── failure ────┐
                     ↓
                  RELAY
```

## 15.4 NAT behavior

The implementation should not assume all NATs behave identically.

Testing must cover:

- endpoint-independent mapping;
- address-dependent behavior;
- port-dependent behavior;
- symmetric NAT scenarios;
- expired mappings;
- simultaneous outbound connections.

## 15.5 No permanent proxy assumption

A failed hole punch should not automatically imply that Banana becomes a general relay network.

Relay support must be explicit and bounded.

---

# 16. Swarm and Peer Selection

A swarm represents interest in a topic.

Conceptually:

```text
topic
  ↓
discover peers
  ↓
rank candidates
  ↓
connect
  ↓
maintain bounded membership
```

## 16.1 Topic identity

A topic should be represented by a deterministic cryptographic identifier.

Applications may derive it from:

```text
application namespace
+
project identifier
+
application-specific secret/public value
```

Banana should not impose an application naming system.

## 16.2 Connection deduplication

For each peer:

```text
(peer_id, connection purpose)
```

there should be one authoritative active connection unless the protocol explicitly requires multiple paths.

## 16.3 Fanout

Default fanout should be conservative.

Applications can request higher fanout when required.

This prevents a small command-line tool from accidentally opening hundreds or thousands of sockets.

## 16.4 Peer scoring

Peer selection should initially remain simple.

Potential inputs:

- reachability;
- latency;
- recent successful connection;
- connection health;
- topic relevance;
- resource availability.

Complex reputation systems should remain outside the initial runtime.

---

# 17. Secure Sessions

## 17.1 Security boundary

The secure session begins before application data is exposed.

```text
transport
   ↓
handshake
   ↓
peer authentication
   ↓
session keys
   ↓
encrypted stream
```

## 17.2 Noise

The implementation should use a documented Noise pattern compatible with the desired interoperability target.

A candidate is:

```text
Noise_XX_25519_ChaChaPoly_SHA256
```

The final choice must be made from actual protocol requirements rather than selected only for familiarity.

## 17.3 Authentication

A peer must be able to verify the identity of the remote peer.

Authentication failure must terminate the session.

## 17.4 Forward secrecy

Ephemeral session keys should provide forward secrecy according to the selected Noise pattern.

## 17.5 Cryptographic scope

Banana must not invent cryptographic primitives.

Use established algorithms and audited implementations.

---

# 18. Multiplexing and Streams

A secure peer session may carry multiple logical application streams.

```text
Secure connection
├── stream 1
├── stream 2
├── stream 3
└── stream N
```

## 18.1 Why multiplex?

Multiplexing avoids:

- connection-per-request overhead;
- repeated handshakes;
- duplicated transport state.

## 18.2 Framing

Each frame should contain only information required to:

- identify stream;
- identify frame type;
- determine payload length;
- process flow-control semantics.

Avoid nested serialization layers.

## 18.3 Backpressure

The runtime must not buffer unbounded application data.

Backpressure is a correctness property, not only an optimization.

## 18.4 Stream lifecycle

A stream should have explicit states:

```text
opening
  ↓
open
  ↓
closing
  ↓
closed
```

Errors should transition the stream to a terminal state.

---

# 19. RPC and Wire Protocol

Banana should distinguish:

```text
control protocol
```

from:

```text
application payload
```

## 19.1 Control protocol

Used for:

- handshake;
- stream creation;
- stream closure;
- ping/health;
- connection negotiation;
- protocol version.

## 19.2 Application payload

Banana should not prescribe an application serialization format unless necessary for the runtime.

Applications should be able to send opaque bytes over streams.

## 19.3 Versioning

Protocol versioning should be explicit.

The runtime should fail clearly when compatibility is impossible.

Avoid large negotiation matrices.

## 19.4 Canonical framing

There should be exactly one implementation of the canonical Banana framing rules.

No component should independently encode the same control message.

---

# 20. Lifecycle and State Machines

State machines are central to Banana.

## 20.1 Runtime lifecycle

```text
NEW
 ↓
INITIALIZING
 ↓
RUNNING
 ↓
DRAINING
 ↓
STOPPED
```

No component should create an independent global lifecycle.

## 20.2 Connection lifecycle

```text
NEW
 ↓
DISCOVERING
 ↓
CONNECTING
 ↓
AUTHENTICATING
 ↓
ESTABLISHED
 ↓
DEGRADED
 ↓
RECONNECTING
 ↓
CLOSED
```

Not every connection must traverse every state.

## 20.3 Shutdown

Shutdown should proceed approximately as:

```text
stop accepting new work
        ↓
stop discovery
        ↓
stop new connections
        ↓
drain streams
        ↓
close sessions
        ↓
close sockets
        ↓
persist required state
        ↓
exit
```

Shutdown should be bounded and observable.

---

# 21. Storage and Persistence

Banana should persist only what the runtime actually needs.

Potential persistent state:

```text
identity
configuration
optional peer cache
optional bootstrap cache
```

It should not automatically become a database.

## 21.1 Application data

Application data belongs outside the runtime.

For example:

```text
Fossil repository
Git repository
Hypercore
file store
database
```

may use Banana as a transport layer.

## 21.2 Cache semantics

Peer addresses and discovery state are caches.

A stale cache must never be treated as authoritative identity information.

## 21.3 Crash safety

Persistent identity must be written atomically.

A crash must not leave the peer with an ambiguous identity.

---

# 22. Relay and Fallback Model

Banana recognizes that direct P2P connectivity is not always possible.

The runtime therefore supports an optional fallback hierarchy:

```text
1. local/direct
2. NAT hole punch
3. known peer relay
4. explicit relay service
5. failure
```

The relay must not become invisible infrastructure.

The user/application should be able to determine whether a connection is:

```text
direct
punched
relayed
```

This is important for:

- privacy;
- performance;
- cost;
- debugging;
- trust.

Relay policy should be configurable.

---

# 23. CLI and User Experience

The CLI should expose the runtime without exposing its internal architecture.

A conceptual CLI:

```text
banana init
banana id
banana listen
banana connect <peer>
banana join <topic>
banana peers
banana status
banana doctor
```

## 23.1 `banana init`

Creates local runtime state and identity.

## 23.2 `banana id`

Displays:

- Peer ID;
- public identity information;
- local identity path.

Private key material must never be printed by default.

## 23.3 `banana listen`

Starts the runtime.

## 23.4 `banana connect`

Attempts to establish a connection to a peer.

## 23.5 `banana join`

Joins a topic and discovers peers.

## 23.6 `banana peers`

Displays current known/connected peers.

## 23.7 `banana status`

Displays:

- runtime state;
- addresses;
- DHT state;
- active peers;
- direct/relay status;
- resource usage.

## 23.8 `banana doctor`

Runs local diagnostics without requiring a cloud service.

---

# 24. Single Binary and Cross-Platform Strategy

The project should produce one executable per target platform.

The user experience should remain:

```text
download
↓
chmod / run
↓
banana
```

No external runtime is required.

## 24.1 Target platforms

| Platform | Target |
|---|---|
| Linux | x86_64 GNU |
| Linux | x86_64 musl |
| Linux | aarch64 GNU |
| Linux | aarch64 musl |
| macOS | x86_64 |
| macOS | aarch64 |
| Windows | x86_64 MSVC |
| Windows | aarch64 MSVC |

Additional targets may be added only when maintenance cost is justified.

## 24.2 Static linking

Linux musl should be the primary fully-static target.

Windows and macOS should aim for self-contained native executables without application-specific runtime dependencies.

## 24.3 Reproducibility

Release builds should record:

- Rust toolchain;
- target;
- source revision;
- dependency lockfile;
- build flags.

The release process should produce checksums.

## 24.4 Binary size

Binary size is an engineering metric, not a fetish.

The project should minimize:

- unused dependencies;
- duplicate libraries;
- unnecessary features;
- embedded assets.

Correctness and security take precedence over a marginal reduction in bytes.

---

# 25. Repository and Code Organization

Banana remains one repository.

Recommended structure:

```text
banana/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── RFC.md
├── LICENSE
├── docs/
│   ├── protocol.md
│   ├── security.md
│   └── architecture.md
├── src/
│   ├── identity/
│   ├── transport/
│   ├── dht/
│   ├── nat/
│   ├── connection/
│   ├── session/
│   ├── mux/
│   ├── swarm/
│   ├── runtime/
│   ├── config/
│   └── lib.rs
├── bin/
│   └── banana.rs
├── tests/
├── benches/
├── examples/
└── xtask/
```

A workspace with several crates is allowed only where the boundary is justified by compilation, API stability, or independent testing.

The default assumption is **one primary crate**, not many crates.

If crates are eventually required, the maximum should remain small:

```text
banana-core
banana-api
banana-cli
```

The earlier RFC intentionally limited the workspace to three crates to prevent package fragmentation; this revision makes the stronger rule explicit: even those crates are optional boundaries rather than mandatory package boundaries. fileciteturn0file0L235-L264

---

# 26. Dependency Philosophy

Every dependency creates:

- code;
- update pressure;
- security surface;
- licensing considerations;
- compile-time cost;
- binary size;
- transitive dependencies;
- possible semantic duplication.

Therefore:

> **Dependencies must earn their place.**

Before adding a dependency, answer:

1. Is the functionality fundamental?
2. Is the dependency maintained?
3. Is it smaller than implementing the required subset internally?
4. Does it introduce a second abstraction model?
5. Does it introduce unsafe/native/runtime requirements?
6. Does it create protocol duplication?
7. Can it be replaced later without architectural damage?

## 26.1 Avoid dependency ideology

"Use a crate" and "write everything ourselves" are both bad rules.

The correct rule is:

> Use the smallest trustworthy implementation that preserves the architecture.

Cryptography is a strong example where using established implementations is generally preferable to writing primitives from scratch.

---

# 27. API Design

The public API should be small enough to remember.

Conceptual surface:

```rust
Banana::builder()
    .identity(identity)
    .bootstrap(bootstrap)
    .build()
    .await?;
```

Then:

```rust
banana.listen().await?;
```

Discovery:

```rust
let swarm = banana.join(topic).await?;
```

Connection:

```rust
let connection = banana.connect(peer).await?;
```

Stream:

```rust
let stream = connection.open_stream().await?;
```

The exact API is subject to implementation experience.

## 27.1 Avoid builder explosion

Builders should not expose every internal knob.

Prefer:

```rust
Banana::builder()
    .bootstrap(...)
    .build()
```

over:

```rust
Banana::builder()
    .dht_bucket_size(...)
    .dht_refresh_interval(...)
    .nat_probe_timeout(...)
    .mux_window(...)
    .noise_pattern(...)
    .socket_buffer(...)
    .scheduler(...)
    ...
```

Advanced tuning can be introduced later when real workloads justify it.

## 27.2 Error model

Errors should communicate:

- what failed;
- where it failed;
- whether retry is meaningful;
- whether the connection is still usable.

Avoid dozens of public error types unless they encode genuinely different recovery behavior.

---

# 28. Configuration

Configuration should be layered:

```text
built-in defaults
      ↓
user configuration
      ↓
CLI arguments
      ↓
application API
```

Later layers override earlier layers.

## 28.1 Configuration principles

Configuration should be:

- explicit;
- inspectable;
- deterministic;
- local;
- optional.

## 28.2 No configuration server

Banana must not require a remote configuration service.

## 28.3 Environment variables

Environment variables may be supported for automation, but they should not become the primary configuration mechanism.

---

# 29. Security Model

## 29.1 Security goals

Banana should provide:

- authenticated peer identity;
- encrypted transport;
- forward secrecy;
- malformed-message resistance;
- bounded resources;
- explicit trust boundaries;
- safe key persistence.

## 29.2 Threats

The design should account for:

- passive network observers;
- active MITM attackers;
- malicious peers;
- malformed packets;
- connection floods;
- memory exhaustion;
- CPU exhaustion;
- replay attempts;
- stale peer records;
- malicious bootstrap nodes;
- malicious relay nodes.

## 29.3 DHT is untrusted

DHT responses are untrusted input.

A malicious DHT node may provide:

- false endpoints;
- stale endpoints;
- malicious endpoints;
- excessive responses.

The runtime must validate all data before use.

## 29.4 Relay is untrusted

A relay must not be able to read encrypted application data.

The relay should operate on encrypted traffic only.

## 29.5 Resource exhaustion

Every untrusted input path must have bounded resources.

Examples:

```text
maximum packet size
maximum frame size
maximum peers
maximum streams
maximum queued bytes
maximum handshake attempts
maximum retry rate
```

---

# 30. Failure Model and Recovery

Failure is normal.

## 30.1 Discovery failure

If DHT discovery fails:

```text
retry with backoff
↓
try alternate bootstrap
↓
use cached candidates
↓
report unavailable
```

The application should remain alive when possible.

## 30.2 Connection failure

A connection failure should not automatically destroy the swarm.

The swarm may rediscover and reconnect.

## 30.3 Peer disappearance

Peers may disappear without graceful shutdown.

Connection timeouts and keepalive mechanisms should detect this.

## 30.4 Network change

When the local network changes:

```text
old addresses
    ↓
invalidate
    ↓
discover new addresses
    ↓
refresh DHT state
    ↓
reconnect as needed
```

## 30.5 Backoff

Retries should use bounded exponential backoff with jitter.

No tight reconnect loops.

## 30.6 Partial failure

One broken peer must not destabilize the entire runtime.

One broken stream must not necessarily terminate the session.

One broken session must not terminate the runtime.

This is a core containment rule.

---

# 31. Resource Management

Banana is intended for:

- laptops;
- servers;
- developer tools;
- edge devices;
- containers;
- constrained environments.

Therefore resource use must be explicit.

## 31.1 Resource categories

Track:

- open sockets;
- active peers;
- active sessions;
- active streams;
- queued bytes;
- DHT entries;
- task count;
- memory buffers.

## 31.2 Defaults

Defaults should be conservative.

The runtime should prefer predictable degradation over uncontrolled growth.

## 31.3 Application control

Applications should be able to specify broad limits without controlling every internal variable.

Example:

```rust
limits.max_peers(64)
       .max_streams(256)
       .max_buffered_bytes(16 * 1024 * 1024);
```

---

# 32. Observability

Observability must be local and opt-in.

## 32.1 No telemetry

Banana should not phone home.

No:

- analytics;
- usage reporting;
- crash upload;
- hidden metrics endpoint.

## 32.2 Structured logs

Logging should support:

```text
error
warn
info
debug
trace
```

## 32.3 Events

Core events may include:

```text
RuntimeStarted
RuntimeStopping
PeerDiscovered
PeerConnecting
PeerConnected
PeerDisconnected
StreamOpened
StreamClosed
NatTraversalStarted
NatTraversalSucceeded
NatTraversalFailed
RelaySelected
ProtocolError
```

There should be one event model rather than multiple competing event buses.

---

# 33. Testing and Verification

P2P software cannot be validated by unit tests alone.

Testing should exist at four levels.

## 33.1 Unit tests

Test:

- identity;
- framing;
- routing;
- state machines;
- retry logic;
- resource limits.

## 33.2 Property tests

Test invariants such as:

```text
decode(encode(x)) == x
```

and:

```text
connection_count(peer) <= configured_limit
```

and:

```text
closed_stream cannot receive new application data
```

## 33.3 Deterministic simulation

Create a virtual network containing:

```text
Peer A
Peer B
Peer C
NAT A
NAT B
Packet loss
Delay
Reordering
Address changes
```

This allows difficult failures to become reproducible.

## 33.4 Integration tests

Run:

```text
Peer A
    ↕
DHT
    ↕
Peer B
```

and verify:

- discovery;
- authentication;
- direct connection;
- encrypted stream;
- disconnect;
- reconnect.

## 33.5 Real network tests

CI should not depend on the public Internet.

However, release qualification should include real-world networks.

Test at minimum:

- LAN;
- public IPv4;
- IPv6 where available;
- restrictive NAT;
- mobile/hotspot networks;
- changing interfaces.

## 33.6 Fuzzing

Fuzz:

- packet parsing;
- frame parsing;
- DHT messages;
- handshake inputs;
- configuration parsing.

---

# 34. Performance and Efficiency

Performance should be measured, not assumed.

Important metrics:

- startup time;
- binary size;
- memory at idle;
- memory with N peers;
- connection establishment time;
- handshake latency;
- DHT lookup latency;
- stream throughput;
- CPU utilization;
- reconnect latency.

## 34.1 Optimization priority

The project should optimize in this order:

1. correctness;
2. security;
3. predictable resource usage;
4. architectural simplicity;
5. measured performance;
6. micro-optimization.

A complicated optimization that saves little CPU but makes the runtime harder to understand should generally be rejected.

## 34.2 Zero-copy where justified

Zero-copy techniques may be introduced where profiling demonstrates value.

They must not be used merely to make the implementation look low-level.

---

# 35. Interoperability

Banana should interoperate pragmatically with the Holepunch ecosystem.

The current RFC identifies HyperDHT, Hyperswarm semantics, and Noise as the principal compatibility targets. fileciteturn0file0L295-L302

## 35.1 Compatibility layers

Compatibility belongs at protocol boundaries.

The core architecture should not be forced to reproduce foreign internal abstractions.

For example:

```text
Holepunch wire protocol
        ↓
compatibility adapter
        ↓
Banana internal model
```

not:

```text
Holepunch package hierarchy
        ↓
Rust package hierarchy
```

## 35.2 Compatibility priorities

Priority order:

1. discovery;
2. rendezvous;
3. authenticated connection;
4. topic/swarm interoperability;
5. stream interoperability;
6. higher-level data structures.

Hypercore-style storage interoperability should not be a prerequisite for the core runtime.

---

# 36. Compatibility Strategy

Compatibility must not become a permanent architectural tax.

For every compatibility feature, document:

```text
protocol
version
supported subset
unsupported subset
test vectors
known limitations
```

If a legacy behavior is required solely for interoperability, isolate it.

This makes future removal possible.

---

# 37. Alternatives Considered

## 37.1 Port every Holepunch package

Rejected because it preserves package fragmentation instead of solving it.

## 37.2 Embed Node.js

Rejected because it introduces a runtime dependency and undermines the single-binary Rust-first model.

## 37.3 Use libp2p wholesale

libp2p is a valuable ecosystem, but adopting it wholesale would introduce a larger abstraction and protocol surface than Banana currently requires.

Banana may borrow ideas from P2P ecosystems without inheriting their complete architecture.

## 37.4 Build everything as separate Rust crates

Rejected by default.

Separate crates are useful when they represent true API or compilation boundaries. They should not exist merely because the original ecosystem used separate packages.

## 37.5 Central signaling server

Rejected as a mandatory dependency.

Applications may use centralized signaling if desired, but Banana itself should provide decentralized discovery.

## 37.6 Mandatory relay infrastructure

Rejected.

Relays are useful but should be explicit fallback infrastructure.

## 37.7 Blockchain-based discovery

Rejected.

Banana needs peer discovery, not global consensus.

A blockchain would add:

- consensus;
- storage;
- synchronization;
- economic mechanisms;
- additional trust assumptions.

This is outside the runtime's purpose.

---

# 38. Implementation Phases

Implementation should proceed from the smallest useful system outward.

## Phase 0 — Repository and invariants

Deliver:

- Rust project;
- CI;
- formatting;
- linting;
- dependency policy;
- RFC;
- architecture tests.

No networking yet.

## Phase 1 — Identity

Deliver:

- Ed25519 identity;
- PeerId;
- secure local persistence;
- identity CLI;
- tests.

## Phase 2 — Transport

Deliver:

- UDP;
- TCP;
- local endpoint discovery;
- deterministic virtual transport.

## Phase 3 — Secure direct connection

Deliver:

- Noise handshake;
- peer authentication;
- encrypted session;
- stream abstraction.

At this point Banana should already provide:

```text
peer A ↔ peer B
```

without a DHT.

## Phase 4 — Minimal DHT

Deliver:

- bootstrap;
- routing;
- lookup;
- announce;
- expiration.

## Phase 5 — Swarm

Deliver:

- topic hashing;
- discovery;
- bounded peer selection;
- connection deduplication.

## Phase 6 — NAT traversal

Deliver:

- endpoint discovery;
- hole punching;
- reachability detection;
- direct-path selection.

## Phase 7 — Relay fallback

Deliver:

- explicit relay protocol;
- encrypted relay traffic;
- relay status reporting.

## Phase 8 — CLI and diagnostics

Deliver:

- init;
- id;
- listen;
- connect;
- join;
- peers;
- status;
- doctor.

## Phase 9 — Interoperability

Implement only the compatibility subset that is backed by tests.

## Phase 10 — Hardening

Deliver:

- fuzzing;
- resource limits;
- failure injection;
- long-running tests;
- release builds;
- cross-platform qualification.

---

# 39. Open Questions

## 39.1 Exact Holepunch compatibility target

How much HyperDHT interoperability is required for the first stable release?

Suggested answer:

> Bootstrap, discovery, rendezvous, and connection establishment first; higher-level replication later.

## 39.2 Transport scope

Should QUIC be supported?

Initial answer:

> Not unless a concrete interoperability or deployment requirement justifies it.

Adding another transport increases:

- code;
- testing;
- state;
- configuration;
- interoperability surface.

## 39.3 Relay discovery

Should relay peers be discovered through the same DHT?

Possible answer:

> Yes, if relay functionality becomes necessary, but relay capability should be explicit and authenticated.

## 39.4 Persistence

How much peer state should survive restart?

Initial recommendation:

> Identity must persist; peer and endpoint caches may persist as disposable hints.

## 39.5 Hypercore

Should Hypercore-like storage be integrated?

Initial recommendation:

> No. Keep Banana as a transport/discovery runtime. Storage systems can build on top.

This preserves the separation between:

```text
connectivity
```

and:

```text
data model
```

## 39.6 Mobile support

Mobile platforms may eventually require different lifecycle and networking behavior.

This should be addressed only when a concrete target exists.

## 39.7 WASM

WASM may be useful for browser-based applications, but it conflicts with the initial "native single binary" focus.

Browser transport constraints are fundamentally different from native socket access.

Do not make WASM a core requirement.

---

# 40. Acceptance Criteria

The RFC is considered architecturally satisfied when the implementation demonstrates all of the following.

## 40.1 Distribution

- [ ] A native `banana` executable exists.
- [ ] No JavaScript runtime is required.
- [ ] No application-specific daemon is required.
- [ ] Release artifacts are reproducible enough to audit.
- [ ] Checksums are produced.

## 40.2 Identity

- [ ] Peer identity persists across restarts.
- [ ] Private keys are never logged by default.
- [ ] Peer ID is deterministic from the public identity.

## 40.3 Connectivity

- [ ] Two peers can connect directly.
- [ ] Connections are authenticated.
- [ ] Traffic is encrypted.
- [ ] Multiple streams can share one session.
- [ ] Backpressure is enforced.

## 40.4 Discovery

- [ ] Peers can bootstrap into the DHT.
- [ ] Peers can announce.
- [ ] Peers can lookup.
- [ ] Topic discovery works.
- [ ] Discovery does not require a central application server.

## 40.5 NAT traversal

- [ ] Endpoint candidates can be exchanged.
- [ ] Hole punching is implemented where feasible.
- [ ] Direct connectivity is preferred.
- [ ] Relay is distinguishable from direct connectivity.

## 40.6 Failure handling

- [ ] Peer disappearance is detected.
- [ ] Reconnect uses bounded backoff.
- [ ] Network changes are recoverable.
- [ ] One failed stream does not kill unrelated streams.
- [ ] One failed peer does not kill the runtime.

## 40.7 Security

- [ ] Wire inputs are validated.
- [ ] Resource limits exist.
- [ ] Fuzzing covers parsers.
- [ ] Cryptographic algorithms are documented.
- [ ] No default telemetry exists.

## 40.8 Architecture

- [ ] One authoritative connection registry exists.
- [ ] One authoritative lifecycle exists.
- [ ] One framing implementation exists.
- [ ] One serialization path exists for each protocol.
- [ ] No duplicate DHT implementations exist.
- [ ] No duplicate retry mechanisms exist.
- [ ] Public APIs do not expose unnecessary internal state.

---

# 41. Definition of Done

Banana is not "done" merely because two machines can exchange bytes.

A stable Banana release requires:

```text
             ┌──────────────────┐
             │     Identity     │
             └────────┬─────────┘
                      │
             ┌────────▼─────────┐
             │    Discovery     │
             └────────┬─────────┘
                      │
             ┌────────▼─────────┐
             │   Connectivity   │
             └────────┬─────────┘
                      │
             ┌────────▼─────────┐
             │  Authentication  │
             └────────┬─────────┘
                      │
             ┌────────▼─────────┐
             │ Secure Sessions  │
             └────────┬─────────┘
                      │
             ┌────────▼─────────┐
             │ Streams / Data   │
             └────────┬─────────┘
                      │
             ┌────────▼─────────┐
             │ Failure Recovery │
             └────────┬─────────┘
                      │
             ┌────────▼─────────┐
             │  Single Binary   │
             └──────────────────┘
```

The implementation is considered complete only when these properties work together as one coherent runtime.

The ultimate test is not:

> "Does Banana implement every feature found in another P2P framework?"

The test is:

> **"Can an application become peer-to-peer without rebuilding the network stack?"**

If yes, Banana has fulfilled its purpose.

---

# 42. References

- [Holepunch](https://holepunch.to/) — Project homepage
- [Holepunch GitHub](https://github.com/holepunchto) — Source ecosystem
- [HyperDHT](https://github.com/holepunchto/hyperdht) — DHT design
- [Hyperswarm](https://github.com/holepunchto/hyperswarm) — Swarm semantics
- [Noise Protocol Framework](https://noiseprotocol.org/) — Secure session framework
- [Kademlia](https://en.wikipedia.org/wiki/Kademlia) — Distributed hash table routing model
- [RFC 768](https://datatracker.ietf.org/doc/html/rfc768) — UDP
- [RFC 9293](https://datatracker.ietf.org/doc/html/rfc9293) — TCP
- [RFC 8446](https://datatracker.ietf.org/doc/html/rfc8446) — TLS 1.3, for comparative security architecture
- Rust documentation and ecosystem standards

---

## Final Design Principle

Banana should remain small enough that a single engineer can understand the architecture.

The runtime should not attempt to become an ecosystem of services.

It should be a **piece of infrastructure** that disappears beneath the application:

```text
                    APPLICATION
                         │
                         ▼
                  ┌─────────────┐
                  │   Banana    │
                  └──────┬──────┘
                         │
             ┌───────────┼───────────┐
             │           │           │
          Identity   Discovery   Connectivity
             │           │           │
             └───────────┼───────────┘
                         │
                    PEER NETWORK
                         │
             ┌───────────┼───────────┐
             │           │           │
           Peer A      Peer B      Peer C
```

**Simple enough to understand.  
Small enough to audit.  
Local enough to own.  
Decentralized enough to survive.  
Modular enough to extend.  
Minimal enough to remain coherent.**

That is Banana.
