# CLI

!!! warning "The CLI is not implemented yet"
    `bin/banana.rs` currently contains an empty `main`. Running the binary exits successfully
    without doing anything. This page documents the planned interface from
    [RFC 23 and phase 8](https://github.com/chaito10/banana/blob/main/RFC.md), so the shape is
    clear before the implementation lands.

## Current state

```bash
cargo run --release
# exits 0, prints nothing, starts no runtime
```

Nothing is registered on the network, no identity is created, and no files are written.
Do not package this binary as a working node.

## Planned commands

| Command | Purpose |
|---|---|
| `banana init` | Create the identity and a default config in the data directory |
| `banana id` | Print the peer id and fingerprint |
| `banana listen` | Start a node and keep it running |
| `banana connect <peer>` | Establish a direct session to a known peer |
| `banana join <topic>` | Join a topic and report discovered peers |
| `banana peers` | List known peers and their paths |
| `banana status` | Print runtime, DHT and swarm counters |
| `banana doctor` | Diagnose configuration, reachability and identity problems |

The RFC requires these to be diagnostics over the same runtime, not a second implementation.
A command must never invent behaviour the library does not have.

## Intended usage

```bash
# create state once
banana init

# print the stable identity
banana id

# run a node
banana listen --listen 0.0.0.0:49737 --bootstrap 203.0.113.10:49737

# inspect it from another terminal
banana status
banana peers
banana doctor
```

Until the CLI exists, the equivalent library calls are shown in
[Getting started](getting-started.md) and the [API guide](api.md). Any command sequence in
older notes or third-party posts should be treated as aspirational.

## Design constraints

When the CLI is implemented it must follow the same rules as the library:

- **Local only.** No command contacts a Banana-operated service.
- **Explicit state.** `init` is the only command that creates an identity; the rest never
  regenerate it silently.
- **No hidden daemon.** A running node is the process you started.
- **Machine-readable where useful.** `status` and `peers` should offer stable output for
  scripts, not only human prose.
- **Honest errors.** A missing identity says so and suggests `banana init`; it does not
  create one.

Track progress in the [Roadmap](roadmap.md).
