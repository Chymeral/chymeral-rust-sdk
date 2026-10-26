# Contributing to the Chymeral Rust SDK

This repository is the standalone reference Rust client for the public
Chymeral Link Protocol (CLP). It must remain independent from Chymeral host
implementation code: it connects over the local protocol and never loads into
the terminal process.

## Development setup

The repository follows the current Rust `stable` channel through
`rust-toolchain.toml`. Install it with Rustup if necessary:

```text
rustup toolchain install stable --component rustfmt --component clippy --component rust-analyzer --component rust-src
```

The minimum supported Rust version remains `1.85` and is verified separately
in CI alongside the current stable compiler.

Run all required checks from the repository root:

```text
cargo fmt --check
cargo test --locked --all-targets -- --test-threads=1
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release --examples
```

Also build the documentation with warnings denied. In a Unix shell, run
`RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps`; in PowerShell, run
`$env:RUSTDOCFLAGS = "-D warnings"; cargo doc --locked --no-deps`.

The conformance executable needs a Chymeral terminal session for live
validation. Launch Chymeral, open its terminal, then run:

```text
cargo run --locked --release --example clp_conformance
```

Use `-- --manual` for the visual scene check; the example reports the CLP
version negotiated with the host. Do not commit `target/`, personal
configuration, generated binaries, or screenshots unless an issue explicitly
requires a small reproducible fixture.

## Publishing a release

Publish only from a clean `main` revision that has passed the checks above and
is tagged with the crate version. First run:

```text
cargo package
cargo publish --dry-run
```

Then publish the verified package with `cargo publish`. Do not use
`--allow-dirty`; the source revision, tag, changelog, and uploaded crate must
describe the same release.

## Public API and protocol rules

Published `1.7.x` is a typed binding for CLP v2.4 with capability negotiation
and a one-time compatibility retry for an earlier compatible host.
Preserve the synchronous, no-async runtime design unless a proposed major-
version change says otherwise.
Changes to public types, methods, serialization, error codes, or lifecycle
behavior need documentation, focused tests, and a corresponding conformance
update. A v2.4 client must retain its bounded compatibility retry, must never
expand presentation packets or scene batches into ordered node commands, and
must omit unavailable advanced capabilities from that retry.

For a CLP v2.4 change, update the reviewed
[v2.4 fixture corpus](https://github.com/Chymeral/chymeral/tree/main/protocol/fixtures/v2.4)
and its vendored `fixtures/clp/v2.4` snapshot together. Check both hashes, keep
the complete compatibility regression replay, and add a live conformance case
when the feature affects `SceneBatch` behavior. Do not add a network download
or a dependency on the Chymeral host to this crate's ordinary CI.

CLP is language-neutral. Do not add Rust-only protocol behavior, host internals
as a dependency, silent compatibility aliases, or APIs that cannot be modeled
by another conforming client. Coordinate any wire-contract change with the
Chymeral core repository before implementation.

Keep pull requests narrow and explain their API compatibility and Linux/Windows
validation. Security reports belong in [SECURITY.md](SECURITY.md), not in a
public issue.
