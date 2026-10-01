# Chymeral Rust SDK

> The reference Rust client library for the Chymeral Link Protocol (CLP).

`chymeral-rust-sdk` lets a Rust program use the local graphics runtime exposed
by a running Chymeral terminal. It is a standalone client crate: it does not
depend on Chymeral source code, create a window, load plugins, or make Rust a
privileged language in the Chymeral ecosystem.

The current implementation supports **CLP v1.3** and is synchronous by design.
It has no async-runtime dependency. Its package is not published to crates.io
yet (`publish = false`); use a local path dependency while developing against
this repository.

## What it provides

- `Client::connect_from_env`, which uses the endpoint and token inherited from
  a Chymeral terminal and completes the CLP lifecycle handshake.
- Typed capabilities, subscriptions, session data, close reasons, errors, and
  host notifications.
- Typed scene commands for roots, children, property changes, content changes,
  reparenting, removal, and overlap queries.
- Session-owned RGBA8 texture upload and destruction.
- Opaque `NodeHandle` and `TextureHandle` values that cannot be confused with
  host implementation identifiers.
- `clp_conformance` and `clp_probe` examples for automated and visual checks.

## Requirements

- Rust **1.85** or newer.
- A Chymeral window already running on Linux or Windows.
- The application must be launched from the shell inside that Chymeral window
  so it inherits `CHYMERAL_CLP_ENDPOINT` and `CHYMERAL_CLP_TOKEN`.

Starting the program from an ordinary terminal is valid Rust execution, but
`connect_from_env` will correctly return `MissingEnvironment` because no CLP
session exists there.

## Add it to an application

For local development, reference the checked-out SDK:

```toml
[dependencies]
chymeral-rust-sdk = { path = "../chymeral-rust-sdk" }
```

The crate package name uses hyphens and the Rust import name uses underscores:

```rust
use chymeral_rust_sdk::Client;
```

## Minimal connection

Run this program from a Chymeral terminal:

```rust
use chymeral_rust_sdk::{
    Capabilities, Capability, Client, CloseReason, Notification, Subscription,
};

fn main() -> chymeral_rust_sdk::Result<()> {
    let mut client = Client::connect_from_env(Capabilities::from([
        Capability::Input,
        Capability::Timing,
    ]))?;

    client.subscribe([Subscription::Timing])?;

    if let Notification::Tick(dt) = client.next_notification()? {
        println!("received a tick after {dt:?}");
    }

    let reason = client.close()?;
    assert_eq!(reason, CloseReason::ClientRequested);
    Ok(())
}
```

`connect_from_env` completes this lifecycle before it returns:

```text
hello → init → init_complete → start → started
```

The returned `Client` retains negotiated information in `client.session()`:
protocol version, session identifier, granted capabilities, and launch context.

## API map

| Area | Public API |
| --- | --- |
| Connection | `Client::connect_from_env`, `Client::connect`, `session`, `has_capability` |
| Subscriptions | `subscribe` with `RawInput`, `Terminal`, and `Timing` |
| Nodes | `create_root`, `create_child`, `update_node`, `set_content`, `reparent`, `remove_node` |
| Textures | `upload_texture`, `destroy_texture` |
| Overlap | `query_overlaps`, `NodeProperties::overlap_enabled` |
| Delivery | `next_notification`, `try_next_notification` |
| Termination | `report_fault`, `close` |

The central value types are `Capabilities`, `NodeProperties`,
`NodePropertiesPatch`, `NodeContent`, `SceneLayer`, `Notification`, `Event`,
`SceneEvent`, and `CloseReason`. Read their Rust documentation in `src/lib.rs`
for field-level contracts.

`Notification` delivers events, `Tick`, `Frame`, `Resize`, `DeliveryGap`,
`Closing`, and `Closed`. A client must treat `DeliveryGap` as a request to
rebuild state derived from continuous data. Discrete input and resize are not
silently discarded by Chymeral; a session that cannot preserve them is isolated
with an explicit close reason instead.

## Scene and overlap

The SDK exposes a retained scene in logical pixels. Roots draw in an `Underlay`
behind the terminal grid or an `Overlay` in front of it. Children use local
coordinates. Sibling `z_index` and insertion order determine paint order.

Use `NodeContent::Empty`, `NodeContent::Solid`, or `NodeContent::Image`.
Images reference a session-owned RGBA8 texture uploaded through `upload_texture`.
The host validates dimensions, exact RGBA8 byte length, ownership, resource
limits, and capability grants.

Rectangular overlap is opt-in. Request both `Capability::Scene` and
`Capability::Overlap`, then set `overlap_enabled: true` in node properties or a
property patch. `query_overlaps` returns current same-session contacts, while
`SceneEvent::OverlapEnter` and `SceneEvent::OverlapLeave` describe transitions.
This is geometry, not a physics engine: visibility, clipping, paint order, and
image alpha do not affect the rectangle test.

## Validation

From a terminal inside Chymeral, run:

```text
cargo run --example clp_conformance
cargo run --example clp_probe
```

`clp_conformance` verifies lifecycle, subscriptions, sustained timing, RGBA8
resource upload and destruction, scene mutation, overlap queries, and a clean
close. It must return to the same terminal without closing it.

`clp_probe` creates orange and blue nodes. Drag orange onto blue, observe the
overlap transitions, then press `q` while they overlap. The probe session exits;
the Chymeral terminal stays alive.

The repository CI validates Rust 1.85 and stable Rust on Linux and Windows. It
runs formatting, all targets, Clippy with warnings denied, and release builds
of both examples.

For local checks:

```text
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --release --examples
```

## Relationship to Chymeral

CLP is language-neutral. This crate is the first reference SDK, not a protocol
fork and not a requirement for applications written in other languages. A
future SDK or direct CLP client has the same status when it preserves the
negotiated lifecycle, capability, ownership, and delivery contracts.

See the [Technical Journal](Technical_Journal.md) for the M14 record and design
decisions. The host repository's `Main_Manual.md` is the authoritative guide to
Chymeral configuration and the complete runtime behavior.

## License

This repository is distributed under the [MIT License](LICENSE).
