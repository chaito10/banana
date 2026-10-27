# Getting started

This guide builds Banana from source, creates a local identity, and runs two nodes that
discover each other through the DHT.

## Prerequisites

- Rust **1.85 or newer** (`rustup update stable`)
- A C toolchain is **not** required
- No database, daemon or external service

```bash
git clone https://github.com/chaito10/banana
cd banana
cargo build --release
cargo test
```

## 1. Create a scratch project

The fastest way to try the library is a small binary that depends on the repository:

```toml
# Cargo.toml
[package]
name = "banana-node"
version = "0.1.0"
edition = "2021"

[dependencies]
banana = { path = "../banana" }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
```

```rust
// src/main.rs
use std::sync::Arc;
use std::time::Duration;

use banana::obs::EventBus;
use banana::transport::UdpTransport;
use banana::{Config, Dht, DhtOptions, Identity};

#[tokio::main]
async fn main() -> banana::Result<()> {
    // Loads the identity at <data_dir>/identity, creating it exactly once.
    let (identity, _created) = Identity::load_or_create(&Identity::default_path()?)?;
    println!("peer id: {}", identity.peer_id());

    let config = Config::load()?;
    let transport = Arc::new(UdpTransport::bind(config.listen[0], 1024).await?);
    let dht = Dht::spawn(DhtOptions {
        identity,
        transport,
        config: config.clone(),
        events: EventBus::new(1024),
        advertise: Vec::new(),
    })?;

    if !config.bootstrap.is_empty() {
        dht.bootstrap(&config.bootstrap).await?;
    }

    let topic = Dht::topic("banana/getting-started");
    dht.announce(topic).await?;

    loop {
        for node in dht.find(topic).await? {
            println!("peer {} at {:?}", node.peer, node.addrs);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
```

## 2. Create two state directories

Banana stores the identity and configuration in one directory. Choose it with
`BANANA_HOME` (or rely on the platform default).

=== "Linux / macOS"

    ```bash
    mkdir -p run/a run/b

    cat > run/a/config <<'EOF'
    log_level = "info"
    listen = ["127.0.0.1:49737"]
    bootstrap = []
    EOF

    cat > run/b/config <<'EOF'
    log_level = "info"
    listen = ["127.0.0.1:49738"]
    bootstrap = ["127.0.0.1:49737"]
    EOF
    ```

=== "Windows PowerShell"

    ```powershell
    New-Item -ItemType Directory -Force run\a, run\b | Out-Null

    @'
    log_level = "info"
    listen = ["127.0.0.1:49737"]
    bootstrap = []
    '@ | Set-Content run\a\config

    @'
    log_level = "info"
    listen = ["127.0.0.1:49738"]
    bootstrap = ["127.0.0.1:49737"]
    '@ | Set-Content run\b\config
    ```

The first node listens on `49737` and has no bootstrap peers. The second node listens on
`49738` and uses the first node as its DHT entry point.

!!! note "Bootstrap is not authority"
    Node A is only an entry point. Once B has learned peers from it, A can disappear and B
    keeps operating from its own routing table.

## 3. Run both nodes

=== "Linux / macOS"

    ```bash
    # terminal 1
    BANANA_HOME=run/a cargo run

    # terminal 2
    BANANA_HOME=run/b cargo run
    ```

=== "Windows PowerShell"

    ```powershell
    # terminal 1
    $env:BANANA_HOME = "run\a"; cargo run

    # terminal 2
    $env:BANANA_HOME = "run\b"; cargo run
    ```

Each node announces the same topic and then polls for peers every five seconds. You should
see the peer id and address of the other node in both terminals.

## 4. Inspect the state directory

```text
run/a/
├── config
└── identity      # banana-identity-v1 + ed25519 seed, written atomically
```

- The identity file is created on first launch and reused afterwards.
- A missing or unreadable identity is an **error**; Banana never silently regenerates it.
- Delete the directory if you intentionally want a new identity.
- Private key material is never logged.

## How it works

1. `Identity::load_or_create` loads or creates the Ed25519 seed and derives the peer id.
2. `UdpTransport::bind` opens the datagram socket used by the DHT and NAT layer.
3. `Dht::spawn` starts the actor that owns the routing table and record store.
4. `bootstrap` pings the configured entry point and fills the routing table.
5. `announce` signs a record binding the topic key to this peer and stores it at the peers
   closest to the key.
6. `find` performs an iterative lookup and returns verified, non-expired records.

The DHT stores **hints** and **signed announcements**, never application data. See
[Protocol](protocol.md) and [Security](security.md) for the details.

## Connecting peers and opening streams

The session layer is available today:

```rust
use banana::session::{Handshake, datagram_record_limit};

let mut handshake = Handshake::initiate(&identity, Some(peer_id))?;
let message = handshake.next_message().expect("initiator speaks first");
// send `message` to the peer over your transport of choice…
// read the peer's replies with handshake.read_message(&reply)?…
let channel = handshake.finish(datagram_record_limit())?;
```

A finished `SecureChannel` is handed to `banana::endpoint::spawn`, which returns a
`Connection`; `connection.open_stream()` and `connection.accept()` yield `Stream` values that
implement `AsyncRead + AsyncWrite`.

!!! warning "Missing glue"
    The production connection manager that carries handshake messages over UDP, applies NAT
    candidates, and calls `endpoint::spawn` is **not implemented yet**. Today the session and
    stream layers are verified by in-process tests; applications that need end-to-end dialing
    should wait for the runtime facade or provide their own transport glue. Track this in the
    [roadmap](roadmap.md).

## Next steps

- [Configuration](configuration.md) — every setting and its default
- [Architecture](architecture.md) — how the actors fit together
- [Protocol](protocol.md) — exact wire formats
- [API guide](api.md) — module-by-module reference
- [Observability](observability.md) — logs, events and counters
