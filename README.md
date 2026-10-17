# Chymeral Rust SDK

> The reference Rust client library for the Chymeral Link Protocol (CLP).

`chymeral-rust-sdk` lets a Rust program use the local graphics runtime exposed
by a running Chymeral terminal. It is a standalone client crate: it does not
depend on Chymeral source code, create a window, load plugins, or make Rust a
privileged language in the Chymeral ecosystem.

The implementation supports the public **CLP v2.0** contract. Its v2
vocabulary intentionally does not preserve CLP v1.x command aliases. The SDK
is synchronous by design and has no async-runtime dependency. Version `1.0.0`
is the first stable Rust binding; it remains source-local for now
(`publish = false`), so use a local path dependency while developing against
this repository.

## What it provides

- `Client::connect_from_env`, which uses the endpoint and token inherited from
  a Chymeral terminal and completes the CLP lifecycle handshake.
- Typed capabilities, subscriptions, session data, close reasons, errors, and
  host notifications.
- Typed scene commands for node creation, atomic mutation, removal, image
  ownership, and direct overlap queries.
- Intrinsic scene-text nodes with terminal-cell metrics, text/style updates,
  and default terminal typography.
- Unified session-owned RGBA8 and GIF image upload and removal.
- Alpha-derived image contact and generic node input passthrough for layered
  scenes without duplicate interaction logic.
- Incoming external-file-drop events for opt-in scene nodes.
- Opaque `NodeHandle` and `ImageHandle` values that cannot be confused with
  host implementation identifiers.
- `clp_conformance`, an executable protocol-conformance check.

## Requirements

- Rust **1.85** or newer.
- A Chymeral window already running on Linux or Windows.
- The application must be launched from the shell inside that Chymeral window
  so it inherits `CHYMERAL_CLP_ENDPOINT` and `CHYMERAL_CLP_TOKEN`.
- Incoming external file drops require a Windows or X11/XWayland Chymeral
  backend; native Wayland does not currently grant `Capability::ExternalDrop`.

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
| Nodes | `create_node`, `set_node`, `remove_node` |
| Images | `upload_image`, `remove_image`, `ImageUpload`, `ImageHandle` |
| Text | `NodeInput::Text`, `NodeChanges::text`, `NodeChanges::text_style` |
| Interaction | `ImageContactShape`, `NodeProperties::input_passthrough`, overlap queries |
| External drop | `Capability::ExternalDrop`, `external_drop_enabled`, `SceneEvent::ExternalDrop` |
| Delivery | `next_notification`, `try_next_notification` |
| Termination | `report_fault`, `close` |

The central value types are `Capabilities`, `NodeProperties`,
`NodePropertiesPatch`, `NodeInput`, `NodeChanges`, `NodeVisual`, `SceneLayer`,
`Notification`, `Event`, `SceneEvent`, and `CloseReason`. Read their Rust documentation in `src/lib.rs`
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

Use `NodeInput::Empty`, `NodeInput::Rectangle`, `NodeInput::Image`, or
`NodeInput::Text` with `create_node`. Pass an optional parent to create a child
and an optional layer only for roots. `set_node` applies a complete validated
patch for properties, parenting, root layer, visual content, or text. Images
reference one session-owned `ImageHandle`, uploaded as RGBA8 pixels or encoded
GIF bytes through `upload_image`. The host validates dimensions, exact RGBA8
byte length, ownership, resource limits, and capability grants.

Overlap is opt-in. Request both `Capability::Scene` and
`Capability::Overlap`, then set `overlap_enabled: true` in node properties or a
property patch. `is_overlapping` returns direct contact state and
`get_overlaps` returns current same-session contacts, while
`SceneEvent::OverlapEnter` and `SceneEvent::OverlapLeave` describe transitions.

Images use `ImageContactShape::Bounds` by default, preserving ordinary
rectangular contact. Set `ImageContactShape::alpha()` or
`ImageContactShape::Alpha { threshold }` to use source alpha instead: alpha
zero passes through and thresholds range from `1` to `255`. Static RGBA8 images
use their uploaded alpha; GIF images use their current decoded frame. The same
shape is used for pointer routing, drag/drop targets, incoming desktop-file
drops, direct overlap queries, and overlap transitions. Visibility, clipping,
and paint order still do not alter overlap participation.

Every node family also has `input_passthrough`. When true, that node is skipped
for pointer-derived and external-drop target selection, while its children stay
independently interactive. It does not hide the node, alter its stacking, or
disable its overlap participation. This is useful for a decorative image or
text layer painted above an interactive node.

## Intrinsic scene text

Request `Capability::Scene` and `Capability::Text`, then call `create_node`
with `NodeInput::Text`. The node origin is the
top-left corner of its first cell. Chymeral computes its rectangle from the
widest explicit line and number of rows, using the terminal cell width and line
height; the creation and update calls return those metrics.

Text accepts printable Unicode, `\n`, and tabs at local eight-cell stops. It
does not wrap or ellipsize. Empty text has a `0 × 0` intrinsic rectangle; a
trailing newline creates an additional blank row. Omitting all `TextStyle`
fields uses the terminal font, size, line height, and foreground color. A
custom `font` is an explicit `scene_text.fonts` alias from `chymeral.toml`, not
a host-system family lookup.

## Validation

From a terminal inside Chymeral, run:

```text
cargo run --example clp_conformance
```

`clp_conformance` verifies lifecycle, subscriptions, sustained timing, RGBA8
and GIF image upload/removal, alpha-contact visual mutation, text, overlap
queries, and a clean close. It must return to the same terminal without closing
it.

For an interactive M20 check in a graphical Chymeral session, run:

```text
cargo run --example clp_conformance -- --manual
```

The same executable keeps a blue target, an orange alpha-frame image, a pink
GIF tile that alternates between opaque and transparent, and a visible
passthrough band on screen. Its terminal output identifies directed scene
events and animated overlap transitions. Click the orange frame versus its
transparent center, click the pink tile in both states, click the white band,
optionally drop a desktop file on each region where the backend supports it,
then click a scene node and press `q`. Only the CLP client closes; the terminal
remains open.

The repository CI validates Rust 1.85 and stable Rust on Linux and Windows. It
runs formatting, all targets, Clippy with warnings denied, and release builds
of the conformance example.

For local checks:

```text
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --release --example clp_conformance
```

## Relationship to Chymeral

CLP is language-neutral. This crate is the first reference SDK, not a protocol
fork and not a requirement for applications written in other languages. A
future SDK or direct CLP client has the same status when it preserves the
negotiated lifecycle, capability, ownership, and delivery contracts.

See the [Technical Journal](Technical_Journal.md) for the M14–M20 record and
design decisions. The host repository's `Main_Manual.md` is the authoritative
guide to Chymeral configuration and the complete runtime behavior.

## License

This repository is distributed under the [MIT License](LICENSE).
