# Security

Banana's security model is deliberately narrow: **discovery does not imply trust, and
authentication does not imply authorization.** This page describes what the implementation
protects, what it does not, and which assumptions an application must not make.

The normative requirements are in [RFC.md](https://github.com/chaito10/banana/blob/main/RFC.md)
(sections 9, 18 and 29). This page reflects the current code.

## Trust model

| Question | Answered by | Meaning |
|---|---|---|
| "Is this endpoint associated with this identity?" | DHT / discovery | A claim, possibly stale or malicious |
| "Does this endpoint control that identity's private key?" | Noise XX handshake | Cryptographic authentication |
| "May this peer perform this application operation?" | **Your application** | Banana has no authorization model |

A DHT node can withhold, delay or reorder announcements. It cannot forge one: every
announcement is signed by the announcing peer and verified before use. Finding a peer is
therefore a hint, not a promise.

## Identity and key material

```text
32-byte seed
   ├── Ed25519 signing key        DHT records, application signatures
   └── SHA-512(seed)[..32], clamped
           └── X25519 static key → public key = PeerId
```

- The peer id is the Montgomery form of the Ed25519 public key. It is exactly the key that
  authenticates the Noise handshake, so a signed record and a session cannot disagree about
  identity without failing verification.
- Identity is stored in `<data_dir>/identity` with the header `banana-identity-v1`. Writes go
  to a sibling temporary file, are `fsync`ed and renamed, so a crash cannot leave a
  half-written identity.
- `Identity::load_or_create` only creates a key when the file is absent. An unreadable file
  is an error. There is no silent rotation; `rotate_at` is explicit.
- The file is created with the process umask. Use a restrictive umask, a protected data
  directory, or full-disk encryption; Banana does not change file permissions.
- `Identity::export_hex` exists for explicit operator export. Never log it, and never pass
  seed material to `obs::log`.

The fingerprint (`Identity::fingerprint`) is a SHA-256 over a domain-separated label and the
seed. It is safe to show to a human for out-of-band comparison; the seed and signing key are
not.

## What each layer protects

| Layer | Protection | Notes |
|---|---|---|
| Datagram framing | Integrity of structure | Bounds-checked before dispatch; not confidentiality |
| DHT records | Authenticity, integrity, freshness | Ed25519 signature covers key, peer, public key, expiry and addresses |
| NAT coordination | Confidentiality and integrity within a peer pair | Rendezvous key is derived from public ids, so it authenticates nobody |
| Noise session | Mutual authentication, confidentiality, forward secrecy | `Noise_XX_25519_ChaChaPoly_SHA256` |
| Streams | Confidentiality and integrity | Every frame travels inside a sealed Noise record |
| Relay (planned) | Not implemented | Do not assume a fallback path exists |

### DHT

An announcement is accepted only when:

- the key, peer id, public key, expiry and address list verify under one Ed25519 signature,
- the public key maps to the claimed peer id,
- the record has not expired,
- the address list is non-empty, at most 8 entries, and contains no port `0`.

A malicious DHT node can still observe who asks for which topic and which addresses are
advertised, and can mount Sybil or eclipse attacks. Banana does not claim resistance to
those; it claims that a record cannot be attributed to a peer that did not sign it.

### NAT coordination

NAT messages must be protected before a session exists, so they use:

- ChaCha20-Poly1305 with a key derived from the two peer ids (sorted, so both sides agree),
- a random 12-byte nonce and a per-sender monotonic counter,
- the 52-byte clear prefix (`sender || nonce || counter`) as additional authenticated data,
- a 64-message sliding replay window per sender.

**The rendezvous key is not a secret.** Peer ids are public, so anyone who knows both ids can
derive the key and inject or read coordination traffic. It scopes messages to a pair and
stops off-path cross-talk; it does not authenticate either side. Once a Noise handshake hash
exists, `Nat::register_channel(peer, punch_key)` upgrades the pair to a key derived from that
handshake. The path itself is authenticated by the Noise handshake that runs over it.

Unknown senders are bounded (1024 tracked peers), idle state expires after ten minutes, and
inbound traffic is rate-limited. None of this is a substitute for authentication.

### Noise sessions

- One pattern only: `Noise_XX_25519_ChaChaPoly_SHA256`, prologue `banana/noise/v1`.
- Both sides prove possession of their static key; when an expected peer id is supplied, a
  mismatch ends the handshake.
- Self-connections (`remote peer == local peer`) are refused.
- Handshake messages are capped at 128 bytes.
- Completed sessions expose only sealed records. A failed open may advance the inbound nonce
  by at most 64 positions to tolerate UDP loss; replays remain impossible.
- Forward secrecy comes from ephemeral X25519 keys. Compromise of the long-term seed does not
  decrypt past sessions.

### Streams

Frames are sealed inside records. A stream cannot bypass the session cipher, and the mux does
not expose a second, unauthenticated framing path. Flow control is enforced with a 64 KiB
receive window per direction, so a peer that never reads cannot make the writer allocate
without bound.

## Threat table

| Threat | Mitigation | Residual risk |
|---|---|---|
| Passive network observer | Noise encryption; NAT envelopes sealed | Traffic volume and timing are visible |
| Active man-in-the-middle | Noise XX authentication; expected peer id check | None known within the protocol |
| Forged DHT record | Ed25519 signature, expiry, peer-id binding | A peer can sign a record for itself with false addresses |
| Replayed NAT message | Per-sender counter and sliding window | Replay within the window and before state exists is bounded, not impossible |
| Spoofed source address | Source addresses are hints; sessions authenticate | UDP reflection is possible if an application amplifies |
| Malicious peer floods a node | Datagram size cap, per-source rate limit, bounded queues, fail-closed tables | A distributed flood is out of scope |
| Malicious peer exhausts streams | `max_streams_per_peer`, `max_peers`, bounded chunk queues | Application-level floods are the application's problem |
| Oversized or malformed message | Bounds-checked decoder; trailing bytes rejected | Bugs remain possible; fuzzing is planned |
| Peer impersonation after discovery | Noise authentication; peer id binds the static key | Application must check *which* peer it expects |
| Sybil / eclipse | None | Explicit non-goal |
| Traffic analysis / anonymity | None | Explicit non-goal |
| Relay coercion | Relays are opt-in and planned | Not implemented yet |

## Failure behaviour

Security-relevant failures fail closed:

- an unknown datagram kind or version is rejected, not guessed,
- an invalid record never reaches routing or lookup code,
- a full routing table or query budget refuses new work instead of growing,
- a failed handshake closes the session and emits a protocol event,
- a stream reset fails that stream only; the session stays usable,
- a closed actor fails pending operations with `Error::Closed`.

## Operational guidance

- Run with a dedicated data directory and a restrictive umask (`umask 077` on Unix).
- Keep `log_level` at `info` or lower in production; `debug`/`trace` add addresses and
  counters, never key material.
- Treat bootstrap addresses as untrusted entry points. They are not certificate authorities.
- Pin the peer id you expect before connecting; do not accept "whoever the DHT returned".
- Do not expose an application protocol that trusts a peer merely because the session
  authenticated. Authentication answers *who*, not *what may they do*.
- Back up the identity file if the peer id must survive a machine loss. Leaking it does not
  reveal past sessions, but it does allow impersonation from that point on.

## Residual risks

- **No anonymity.** IP addresses are visible to peers and to any relay.
- **No application authorization.** Banana provides identity and connectivity, not policy.
- **No relay yet.** A failed hole punch currently ends in `Error::Unreachable`.
- **No independent audit.** The cryptographic crates are well known; this composition is not
  audited.
- **No fuzzing or formal verification yet.** The decoder is bounds-checked and property
  tested, but that is not a proof.
- **Sybil and eclipse attacks are unmitigated.** Discovery is probabilistic.
- **No protection against a compromised host.** Malware with access to the data directory has
  the identity.

## Reporting a vulnerability

Please report privately through GitHub's private vulnerability reporting:

**<https://github.com/chaito10/banana/security/advisories/new>**

Do not open a public issue for a suspected vulnerability. See [SECURITY.md](https://github.com/chaito10/banana/blob/main/SECURITY.md)
for the response process and what to include.
