# Security policy

Banana is an early-stage networking runtime. Security reports are taken seriously, but the
project does not currently offer a bug bounty or a formal service-level agreement.

## Supported versions

| Version | Supported |
|---|---|
| `0.1.x` | Best-effort security fixes |
| Older / forks | Not supported |

There has been no stable release yet. Fixes land on `main` first.

## Reporting a vulnerability

**Do not open a public issue.**

Use GitHub's private vulnerability reporting:

<https://github.com/chaito10/banana/security/advisories/new>

Please include:

- a description of the issue and its impact,
- the affected version or commit,
- reproduction steps or a proof of concept,
- any relevant configuration, platform and logs (redact seeds and private keys),
- whether the issue is already public or known to others.

If GitHub private reporting is unavailable, contact the maintainer listed on the repository
profile and ask for a private channel. Do not send exploit details in a public channel first.

## What to expect

- Acknowledgement, best effort, within **7 days**.
- An initial assessment and a decision on severity within **14 days** where possible.
- Credit in the advisory if you want it.
- Coordinated disclosure: we will agree on a timeline before publishing details. Please give
  the project a reasonable window to ship a fix.

This is a volunteer project; these are targets, not guarantees.

## In scope

- Cryptographic misuse in identity, Noise sessions, record sealing, HKDF labels or replay
  handling.
- Memory safety or denial of service reachable from untrusted network input.
- Authentication or authorization bypass in session establishment.
- Signature verification bypass in DHT records.
- Resource-limit bypass that lets a remote peer exhaust memory, tasks or sockets.
- Replay or cross-peer confusion in NAT coordination.
- Secret leakage through logs, error messages or serialisation.

## Out of scope

- The absence of features that the [Roadmap](docs/roadmap.md) lists as planned, including
  relay fallback, the CLI, the runtime facade and Holepunch interoperability.
- Attacks that require the attacker to already control the host, the data directory or the
  process.
- Traffic analysis, anonymity, Sybil attacks and eclipse attacks, which the design explicitly
  does not claim to resist. See [Security](docs/security.md).
- Spoofing of UDP source addresses or generic network flooding.
- Vulnerabilities in dependencies that are already publicly known and have an available
  upgrade, unless Banana's usage makes the impact materially worse.
- Social engineering, physical access and denial of service by sheer volume.

## Safe harbour

We will not pursue legal action against researchers who:

- act in good faith and only against their own deployments or test networks,
- avoid accessing, modifying or exfiltrating other people's data,
- report privately and give a reasonable time to fix,
- do not exploit the issue beyond what is needed to demonstrate it.

## Hardening guidance for operators

- Protect the data directory; it contains the identity seed.
- Use a restrictive umask and full-disk encryption.
- Treat bootstrap nodes as untrusted entry points.
- Pin expected peer ids in your application.
- Do not build authorization on top of "the session authenticated"; that answers *who*, not
  *what may they do*.
