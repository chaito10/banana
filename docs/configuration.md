# Configuration

Banana is configured by a small local file plus, when an application embeds the library,
ordinary Rust values. There is no remote configuration service.

```text
built-in defaults  <  user file  <  CLI arguments  <  application API
```

Configuration selects behaviour. It never replaces the runtime API, and unknown keys are
**rejected** so a typo cannot silently change behaviour.

## Location

The state directory is chosen in this order:

1. `BANANA_HOME`, when set and not blank
2. `$XDG_DATA_HOME/banana`
3. `%APPDATA%\banana` on Windows
4. `$HOME/.banana`
5. `<system temp>/banana` as a last resort

The configuration file is `<data_dir>/config`. The identity lives beside it at
`<data_dir>/identity`.

```bash
BANANA_HOME=/var/lib/banana ./my-node
```

```powershell
$env:BANANA_HOME = "C:\ProgramData\banana"; .\my-node.exe
```

`Config::load()` reads the default path and returns defaults when the file is absent.
`Config::from_file(path)` reads an explicit path. Neither creates or modifies the file.

## Syntax

The format is intentionally tiny:

- `key = value` lines
- `[section]` headers
- `#` starts a comment
- strings and paths must be quoted (`"info"`)
- booleans are exactly `true` or `false`
- address lists are arrays of quoted `ip:port` strings
- integer suffixes such as `ms` are not parsed; values are plain integers

```ini
# Banana configuration
log_level = "info"
listen = ["0.0.0.0:49737"]
bootstrap = ["203.0.113.10:49737"]

[limits]
max_peers = 64
max_frame_bytes = 65536

[dht]
k = 20
alpha = 3

[net]
keepalive_ms = 15000
```

A malformed line, an unknown key, a value of the wrong type, or a failed validation aborts
loading with `Error::Config` and the line number. Banana never starts with a partially applied
configuration.

## Top-level keys

| Key | Type | Default | Meaning |
|---|---|---|---|
| `data_dir` | quoted path | platform default | Identity and cache directory |
| `listen` | address list | `["0.0.0.0:49737"]` | UDP/TCP bind addresses |
| `bootstrap` | address list | `[]` | DHT entry points; not authorities |
| `log_level` | quoted string | `"WARN"` | `error`, `warn`, `info`, `debug`, `trace` |
| `accept_relays` | bool | `false` | Reserved for phase 7 relaying |
| `use_relays` | bool | `true` | Reserved for phase 7 relaying |
| `relay_listen` | quoted address | unset | Reserved for phase 7 relaying |
| `advertised_relay` | quoted address | unset | Reserved for phase 7 relaying |
| `hyperswarm_bootstrap` | bool | `false` | Reserved interoperability switch |
| `holepunch_wire` | bool | `false` | Reserved interoperability switch |

`relay_*` keys parse today but no relay path exists yet. Treat them as forward-compatible
placeholders, not working features.

## `[limits]`

| Key | Default | Validation |
|---|---:|---|
| `max_peers` | `64` | must be `> 0` |
| `max_streams_per_peer` | `256` | must be `> 0` |
| `max_frame_bytes` | `65536` | `1..=16777216` |
| `max_buffered_bytes_per_stream` | `4194304` | must be `>= max_frame_bytes` |
| `max_datagrams_per_second` | `2000` | parsed as `u32`; no extra range check |
| `max_pending_connections` | `32` | parsed as `u32`; no extra range check |
| `max_punch_attempts` | `8` | parsed as `u32`; no extra range check |
| `max_topic_records` | `2048` | parsed as `usize`; no extra range check |
| `max_cached_addresses` | `512` | parsed as `usize`; no extra range check |

## `[dht]`

| Key | Default | Validation |
|---|---:|---|
| `k` | `20` | `1..=20` (the wire node limit) |
| `alpha` | `3` | `1..=k` |
| `announce_interval_ms` | `30000` | must be `> 0` |
| `maintenance_interval_ms` | `60000` | must be `> 0` |
| `peer_ttl_ms` | `900000` | parsed as `u64`; no extra range check |
| `record_ttl_ms` | `120000` | parsed as `u64`; no extra range check |

A larger `k` increases both resilience and message size; 20 is the maximum that fits the
DHT response budget. `alpha` above `k` would ask for more parallelism than a lookup can use.

## `[swarm]`

| Key | Default | Validation |
|---|---:|---|
| `default_fanout` | `8` | `1..=max_fanout` |
| `max_fanout` | `64` | parsed as `u32`; no extra range check |
| `membership_interval_ms` | `5000` | parsed as `u64`; no extra range check |

## `[net]`

| Key | Default | Validation |
|---|---:|---|
| `punch_timeout_ms` | `1500` | must be `> 0` |
| `connect_timeout_ms` | `15000` | must be `> 0` |
| `keepalive_ms` | `15000` | must be `>= 1000` |
| `backoff_max_ms` | `60000` | parsed as `u64`; no extra range check |

`punch_timeout_ms` bounds one hole-punch round, not the whole reachability attempt.
`keepalive_ms` is the idle ping interval; the endpoint's keepalive timeout is separate and
currently fixed at 45 s.

## Validation summary

`Config::validate()` rejects:

- `limits.max_peers == 0`
- `limits.max_streams_per_peer == 0`
- `limits.max_frame_bytes` outside `1..=16 MiB`
- `limits.max_buffered_bytes_per_stream < limits.max_frame_bytes`
- `dht.k` outside `1..=20`
- `dht.alpha` outside `1..=dht.k`
- zero announce or maintenance intervals
- `swarm.default_fanout` outside `1..=swarm.max_fanout`
- zero punch or connect timeout
- `net.keepalive_ms < 1000`
- more than 16 listen addresses
- a listen address with port `0`

Everything else is limited only by its integer type. If you set an extreme value, you own
the consequences; the runtime treats these fields as trusted local input.

## Writing configuration

```rust
use banana::Config;

let mut config = Config::load()?;
config.log_level = banana::obs::Level::Info;
config.limits.max_peers = 128;
config.validate()?;
config.save(&config.data_dir.join("config"))?;
```

`Config::save` creates parent directories, writes a sibling temporary file and renames it
into place. `Config::render` produces the same syntax for inspection or diffing.

## Layering

`Config::apply(&patch)` overlays a higher-priority value. For fields where the default
doubles as "unset" — `listen`, `bootstrap`, `log_level`, relay flags, interoperability
flags and `data_dir` — only non-default values override. Numeric and structural sections
(`limits`, `dht`, `swarm`, connection timings) are replaced wholesale.

Because of that asymmetry, applications that want exact control should build a `Config`
value directly or start from `Config::load()` and assign fields, rather than stacking
several partial patches.

## Wired versus reserved

Not every parsed field is consumed by the current implementation. This table is the honest
status for `0.1.x`; a setting listed as reserved is still validated and round-trips through the
file, but it does not yet change runtime behaviour.

| Setting | Status |
|---|---|
| `data_dir`, `listen`, `bootstrap`, `log_level` | Wired |
| `limits.max_peers` | Wired: cap on swarm connections plus dials in flight |
| `limits.max_punch_attempts` | Wired: NAT probe budget |
| `limits.max_topic_records` | Wired: DHT record-store ceiling |
| `dht.k`, `dht.alpha` | Wired: routing and lookup parallelism |
| `dht.announce_interval_ms`, `dht.maintenance_interval_ms` | Wired |
| `dht.peer_ttl_ms`, `dht.record_ttl_ms` | Wired: expiry |
| `swarm.default_fanout`, `swarm.max_fanout` | Wired: selection bound |
| `swarm.membership_interval_ms` | Wired: maintenance tick |
| `net.punch_timeout_ms` | Wired: NAT round budget |
| `limits.max_streams_per_peer` | Reserved; the endpoint uses `SessionConfig::max_streams` (256) |
| `limits.max_frame_bytes` | Reserved; the endpoint uses `SessionConfig::max_frame` (1 KiB) |
| `limits.max_buffered_bytes_per_stream` | Reserved; the endpoint uses its chunk queue |
| `limits.max_datagrams_per_second` | Reserved; the DHT uses a fixed 128/s per-source limit |
| `limits.max_pending_connections` | Reserved |
| `limits.max_cached_addresses` | Reserved |
| `net.connect_timeout_ms` | Reserved until the connection manager lands |
| `net.keepalive_ms` | Reserved; the endpoint currently uses a fixed 15 s / 45 s pair |
| `net.backoff_max_ms` | Reserved until the connection manager lands |
| `relay_*` | Reserved for phase 7 |
| `interop.*` | Reserved for phase 9 |

If you depend on a reserved value, pin the commit you tested. It may start taking effect
without a format change once the corresponding layer is wired.

## Environment variables

| Variable | Effect |
|---|---|
| `BANANA_HOME` | Overrides the state directory |
| `XDG_DATA_HOME` | Used when `BANANA_HOME` is unset (Unix convention) |
| `APPDATA` | Used on Windows when the above are unset |
| `HOME` | Fallback for `$HOME/.banana` |

Log level is **not** read from the environment. Use `log_level` in the file or
`banana::obs::set_level` at runtime.
