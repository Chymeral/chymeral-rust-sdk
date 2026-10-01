# chymeral-rust-sdk

`chymeral-rust-sdk` is the standalone Rust client SDK for CLP v1.3. It talks
only to the endpoint and token injected into a child process by Chymeral; it
does not depend on Chymeral source code or create a window of its own.

The crate is deliberately synchronous and has no async-runtime dependency.
`Client::connect_from_env` completes the CLP lifecycle handshake, retains the
`init` context in `Client::session()`, and then exposes typed scene commands and
host notifications.

## Continuous integration

The included GitHub Actions workflow validates Rust 1.85 and stable Rust on
Linux and Windows. It runs formatting, all targets, Clippy with warnings denied,
and release builds of both examples.

## Local validation

Build and run the conformance client *inside an already running Chymeral
terminal*:

```text
cargo run --example clp_conformance
```

It validates the lifecycle, sustained timing delivery, subscriptions, RGBA8
upload and destruction, scene mutation, rectangular overlap query, and clean
session closure. It must return to the terminal without closing that terminal.

For the manual visual check:

```text
cargo run --example clp_probe
```

Drag the orange node onto the blue node. While they overlap, press `q`. The
probe exits cleanly; the Chymeral terminal stays open.

The detailed public manual and any publication metadata are intentionally
deferred to a later documentation and release phase.
