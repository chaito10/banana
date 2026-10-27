# Contributing

Banana is a small project with a strict design. Contributions are welcome, especially where
they make the implementation match the [RFC](https://github.com/chaito10/banana/blob/main/RFC.md)
more precisely or make a failure mode easier to understand.

## Before you start

- Read [Architecture](architecture.md) and the relevant RFC section.
- Check the [Roadmap](roadmap.md) so you are not duplicating work.
- For a large change, open an issue first. A design that adds a new abstraction, crate,
  protocol message or configuration language is likely to be rejected.
- For a vulnerability, **do not open a public issue**. Follow
  [SECURITY.md](https://github.com/chaito10/banana/blob/main/SECURITY.md).

## Development setup

```bash
git clone https://github.com/chaito10/banana
cd banana
cargo build
cargo test
```

Requirements:

- Rust 1.85 or newer (edition 2021)
- No C toolchain, Node.js, database or external service

The crate forbids `unsafe` (`#![forbid(unsafe_code)]`) and denies missing documentation
(`#![deny(missing_docs)]`). Both are intentional.

## Checks

Run these before opening a pull request:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo doc --no-deps
```

If you change the documentation site:

```bash
pip install "mkdocs-material>=9.6,<10"
mkdocs build --strict
mkdocs serve   # live preview at http://127.0.0.1:8000/
```

## Design rules

1. **One source of truth.** Mutable state has one owner. Actors own their state; handles send
   commands. Do not add a second cache of the same data.
2. **No hidden infrastructure.** No telemetry, no background service, no mandatory relay, no
   remote configuration.
3. **Direct first.** Path preference is direct, then punched, then relayed. A relay must never
   silently replace a direct path.
4. **Security is part of the connection.** No plaintext application channel, no second framing
   path, no ad-hoc cryptography. Use the existing primitives in `crypto` and `session`.
5. **Failure is a state.** Model it. Do not `unwrap` untrusted input, do not grow memory on
   failure, do not let one bad datagram stop an actor.
6. **Internal complexity stays internal.** Promote a type only when an application needs it.
7. **Configuration selects behaviour.** It does not become a second programming language.
8. **No feature without a reason.** New dependencies, messages and public types need a
   concrete problem and a test.

## Code expectations

- Keep the existing module boundaries; do not introduce a new crate for a single concern.
- Every public item needs a doc comment that says what it does and, where relevant, what it
  does not do.
- Prefer explicit bounds over unbounded collections on any path fed by the network.
- Use `Error` and `Result` from the crate. Do not add a subsystem-specific error type.
- Do not log secrets, seeds or key material. Peer ids, addresses and counters are fine.
- Preserve the canonical encoder. A new wire field means updating
  [Protocol](protocol.md), the decoder bounds and a round-trip test.
- A behaviour change needs a test that fails before and passes after.

## Wire and format changes

The wire version is `1`. Any incompatible layout change requires:

1. a version bump in the protocol and the Noise prologue,
2. an update to [Protocol](protocol.md),
3. decoder tests for truncation, trailing bytes and oversize input,
4. a note in the changelog or release notes.

Do not change a field meaning silently. Do not accept unknown trailing bytes.

## Documentation changes

Documentation must match the implementation. When you change status, update:

- `README.md`'s status table,
- [Roadmap](roadmap.md),
- any page that names the feature as planned or implemented.

Avoid documenting a planned API in the present tense. The docs are built with MkDocs Material
and deployed by `.github/workflows/pages.yml`; `mkdocs build --strict` must pass.

## Commits and pull requests

- Keep commits focused; a refactor and a behaviour change belong in separate commits.
- Use an imperative subject line, for example `session: reject oversized handshake messages`.
- Explain *why* in the body, especially for protocol or security decisions.
- Reference the RFC section or issue.
- A pull request should state what changed, how it was tested, and what remains unfinished.

## License

By contributing, you agree that your contribution is licensed under the
[Mozilla Public License 2.0](https://github.com/chaito10/banana/blob/main/LICENSE), matching
the project's inbound = outbound policy.
