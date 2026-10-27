# Protocol

This page describes the bytes Banana actually puts on the wire. The codec lives in one
place — [`src/wire.rs`](https://github.com/chaito10/banana/blob/main/src/wire.rs) — and no
other module is allowed to hand-roll encoding.

- **Wire version:** 1
- **Maximum datagram:** 1200 bytes (MTU safe)
- **Integer byte order:** little-endian unless stated otherwise
- **One framing implementation:** every sender and receiver uses the same encoder/decoder

!!! note "Implementation status"
    DHT, NAT, Noise records and stream framing are implemented. The connection manager that
    carries handshake messages over a real socket and calls `endpoint::spawn` is still
    planned. The layouts below are the ones the codec and tests enforce today.

## Datagram header

Every UDP datagram starts with an 8-byte control header:

| Offset | Size | Field | Notes |
|---:|---:|---|---|
| 0 | 2 | `magic` | `u16` = `0x424E`, encoded little-endian |
| 2 | 1 | `version` | must be `1` |
| 3 | 1 | `kind` | see table below |
| 4 | 4 | `session` | `u32`, Noise session id, or `0` for connectionless kinds |

| Kind | Value | Purpose |
|---|---:|---|
| `DHT` | 1 | Routing, lookup and announcements |
| `NAT` | 2 | Reachability probing and hole punch coordination |
| `NOISE` | 3 | Noise handshake messages and encrypted records |
| `RESYNC` | 4 | Reserved for an unknown Noise session id |

A decoder rejects:

- datagrams longer than 1200 bytes,
- datagrams shorter than 8 bytes,
- a bad magic or version,
- unknown kinds at dispatch,
- any trailing bytes in a structured message.

## Primitive encodings

### Peer id

Exactly **32 raw bytes**: the X25519 public key derived from the peer's Ed25519 identity.
See [Security](security.md#identity-and-key-material) for the derivation.

### Socket address

| Field | Size | Notes |
|---|---:|---|
| family | 1 | `4` or `6` |
| port | 2 | `u16` |
| address | 4 or 16 | raw IPv4 or IPv6 octets |

### Address list

| Field | Size | Notes |
|---|---:|---|
| count | 1 | at most 8 |
| addresses | variable | `count` socket addresses |

Addresses with port `0` are rejected. Duplicates are collapsed by constructors before
encoding.

### Length prefixes

| Prefix | Size | Limit | Used by |
|---|---:|---|---|
| `prefixed` | `u16` | caller supplied | generic byte strings |
| `counted` | `u8` | caller supplied | short byte strings |
| node list | `u8` | 20 nodes | DHT `Nodes` |
| address list | `u8` | 8 addresses | DHT/NAT |

## DHT protocol

The DHT payload follows the datagram header. It starts with a message tag and a transaction
id, then the sender's `NodeInfo`.

### NodeInfo

```text
peer_id      32 bytes
addr_count    1 byte
addresses    addr_count × socket address
```

### Message envelope

```text
kind          1 byte   (1..=8)
txid          4 bytes  little-endian
body          message-specific
```

Requests and responses share the same `txid`, so unrelated traffic cannot be mistaken for an
answer.

| Tag | Message | Body after `txid` |
|---:|---|---|
| 1 | `Ping` | `from: NodeInfo` |
| 2 | `Pong` | `from: NodeInfo` |
| 3 | `FindNode` | `from: NodeInfo`, `target: 32 bytes` |
| 4 | `Nodes` | `from: NodeInfo`, `count: u8`, `NodeInfo × count` |
| 5 | `Store` | `from: NodeInfo`, `SignedRecord` |
| 6 | `Stored` | `from: NodeInfo` |
| 7 | `Get` | `from: NodeInfo`, `key: 32 bytes` |
| 8 | `Value` | `from: NodeInfo`, `present: u8`, record when `1` |

`Nodes` keeps the list inside the payload budget: the encoder writes the count byte only
after deciding which nodes fit, so an oversized list can never be produced.

### Signed announcement

```text
key            32 bytes    topic or rendezvous key
peer           32 bytes    announcing peer id
public_key     32 bytes    Ed25519 public key of the announcer
expires_at      8 bytes    unix milliseconds
addr_count      1 byte
addresses       addr_count × socket address
signature      64 bytes    Ed25519 signature
```

The signature covers a domain-separated canonical body:

```text
"banana/dht/record/v1" || key || peer || public_key || expires_at || addresses
```

Verification rejects a record when:

- it carries no address, more than 8 addresses, or a port `0`,
- it has expired,
- the public key does not match the peer id,
- the Ed25519 signature is invalid.

A failed verification drops the record before it reaches routing or lookup code. Records are
signed by the announcing peer, so a malicious DHT node can withhold a record but cannot
attribute a topic to a peer that never claimed it.

### Routing

- 256 k-buckets, one per bit of the 256-bit key space.
- Bucket index = position of the most significant differing bit of the XOR distance.
- Bucket size `k` defaults to 20; a full bucket only evicts an entry that has been stale for
  the configured peer TTL.
- Entries age out after `dht.peer_ttl_ms` without a refresh.
- Iterative lookup drives up to `dht.alpha` queries at a time, with a 1 s per-query timeout
  and an 8 s overall deadline.
- Discovery is probabilistic by design: a lookup returns candidates, not a guarantee.

### Announcement lifetime

- `dht.record_ttl_ms` controls how long an announcement is valid (default 120 s).
- `dht.announce_interval_ms` controls renewal (default 30 s).
- Stored records are pruned when expired; lookups also re-check expiry and signature.

## NAT protocol

NAT messages are connectionless (`session = 0`) and run before any Noise session exists.
Every message is wrapped in a sealed envelope.

### Envelope

```text
sender      32 bytes   clear
nonce       12 bytes   clear, random per datagram
counter      8 bytes   clear, little-endian, per sender
body        variable   AEAD ciphertext of the NAT message
```

- **AEAD:** ChaCha20-Poly1305.
- **Additional data:** the 52-byte clear prefix, so `sender`, `nonce` and `counter` cannot be
  rewritten.
- **Key:** `rendezvous_key(a, b)` derived from the two public peer ids, so both sides can
  open the message before a session exists. Callers that already share a Noise handshake
  hash upgrade it with `Nat::register_channel(peer, punch_key)`.
- The rendezvous key **authenticates nothing by itself**; it scopes coordination traffic to a
  pair. The Noise handshake that runs on the established path provides authentication.
- Replay protection is a 64-message sliding window per sender. Unknown senders are bounded
  (1024 tracked peers) and idle state expires after 10 minutes.

### Messages

| Tag | Message | Body |
|---:|---|---|
| 1 | `Probe` | `token: u64` |
| 2 | `ProbeReply` | `token: u64`, `observed: address` |
| 3 | `Exchange` | `token: u64`, `count: u8`, addresses, `punch: u8` |
| 4 | `ExchangeReply` | `token: u64`, `count: u8`, addresses |
| 5 | `Punch` | `attempt: u32` |

`Exchange` with `punch = 1` asks the receiver to start punching back. `ExchangeReply` never
triggers a punch, so exchanges cannot ping-pong.

### Hole punch state machine

```text
Discover → Candidates → Probe → DirectPath
                    │
                    └── budget exhausted ──▶ Relay territory ──▶ Unreachable (phase 7)
```

- Both sides probe every exchanged candidate.
- Any valid `Punch` from the peer proves the direct path.
- `limits.max_punch_attempts` and `net.punch_timeout_ms` bound the effort.
- Probe spacing is at least 50 ms.
- A successful punch returns `PunchOutcome { state: DirectPath, addr, attempts }`.
- Failure returns `Error::Unreachable` until relays are implemented.

### Path selection

`select_path` prefers `Direct`, then `Punched`, then `Relayed`, breaking ties on the lowest
address so both ends choose the same path from the same options.

## Noise session

Banana speaks exactly one pattern:

```text
Noise_XX_25519_ChaChaPoly_SHA256
```

- **Prologue:** `banana/noise/v1`
- **Static key:** the X25519 secret derived from the identity seed
- **Authentication:** both sides prove possession of the static key; if an expected peer id
  is supplied, the remote static key must match it
- **Forward secrecy:** ephemeral X25519 keys from the `XX` pattern
- **Self-connection:** refused (`remote peer == local peer`)
- **Handshake message limit:** 128 bytes

`XX` exchanges three messages:

```text
initiator → responder   e
responder → initiator   e, ee, s, es
initiator → responder   s, se
```

The handshake hash is retained after completion. Banana derives two values from it with
HKDF-SHA256 (salt `banana/hkdf/v1`):

| Value | Info label | Use |
|---|---|---|
| `session_id` | `banana/session-id/v1` | first 4 bytes little-endian; datagram demultiplexing |
| `punch_key` | `banana/holepunch/v1` | sealing NAT coordination after a session exists |

### Records

Noise transport messages are ChaCha20-Poly1305 records with a 16-byte tag:

- `datagram_record_limit()` = `1200 - 64 = 1136` bytes.
- `stream_record_limit(configured)` = `min(configured, 64 KiB)`.
- The default endpoint session record is `1200 - 8 - 16 = 1176` bytes.
- Nonces are managed by the Noise transport state; they never move backwards.
- Because UDP may lose or reorder datagrams, a failed open may advance the expected inbound
  nonce by one, within a budget of 64 skips. Replays remain impossible.

### Framing over each wire

```text
Datagram:  header(8) || sealed record
Stream:    length: u32 little-endian || sealed record
```

The stream wire is used for reliable transports such as TCP and the planned relay. The
length prefix must not exceed the configured record limit.

## Multiplexing and streams

Every stream frame uses one 9-byte header:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 1 | frame kind |
| 1 | 4 | stream id (`u32`) |
| 5 | 4 | payload length (`u32`) |
| 9 | n | payload |

| Kind | Value | Payload |
|---|---:|---|
| `Open` | 1 | empty |
| `OpenAck` | 2 | receiver window (`u32`) |
| `Close` | 3 | optional reason |
| `Reset` | 4 | optional reason |
| `Data` | 5 | application bytes |
| `WindowUpdate` | 6 | credit (`u32`) |
| `Ping` | 7 | empty |
| `Pong` | 8 | empty |

- Stream `0` is reserved for control frames.
- Locally allocated stream ids start at `1` and advance monotonically; any peer-chosen id
  advances the allocator past it.
- Frames are encoded, sealed and written by one task, so there is no interleaving hazard.
- Frame payloads are capped by `SessionConfig::max_frame` (1 KiB in the default endpoint
  config) and by the record size. The `limits.max_frame_bytes` setting is validated but is not
  yet wired into the endpoint in `0.1.x`.

### Flow control

Each direction of a stream has credit:

```text
initial window = SessionConfig::window (64 KiB default)
sender writes only while credit >= frame size
reader consumes data and returns credit via WindowUpdate
```

A peer that stops reading therefore blocks the writer instead of causing unbounded memory
growth. If a reader stalls long enough, the stream's bounded chunk queue fills; the session
does not allocate more.

### Stream lifecycle

```text
opening → open → closing → closed
             │
             └── error / Reset ──▶ closed
```

- `close()` finishes the sending direction.
- `reset()` discards buffered data and fails pending operations.
- Reading after close returns end-of-stream; writing after close fails.
- Keepalive `Ping`/`Pong` detects silent sessions (`net.keepalive_ms`, default 15 s; timeout
  45 s in the default `SessionConfig`).

## Limits and versioning

| Limit | Default | Enforced by |
|---|---:|---|
| Datagram size | 1200 B | transport + wire |
| Addresses per message | 8 | wire constructors |
| Nodes per DHT response | 20 | DHT codec |
| Handshake message | 128 B | session |
| Record (datagram) | 1136 B | session |
| Record (stream) | 64 KiB | session |
| Frame payload | 1 KiB endpoint default | `SessionConfig` (config field reserved) |
| Open streams per peer | 256 | `SessionConfig::max_streams` |
| Peers | 64 | `limits.max_peers` (swarm) |

Versioning is explicit: a datagram with an unknown version fails with
`Error::Unsupported`. There is no compatibility matrix; a new incompatible layout must bump
the wire version and the Noise prologue.

## Interoperability

The RFC targets pragmatic Holepunch interoperability. `interop.hyperswarm_bootstrap` and
`interop.holepunch_wire` exist as reserved switches, but no compatibility adapter is
implemented yet. Compatibility will live at the protocol boundary, never inside the core
state machines. See [Roadmap](roadmap.md).
