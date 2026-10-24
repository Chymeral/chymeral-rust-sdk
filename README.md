<p align="center">
  <a href="https://github.com/Chymeral/chymeral"><img src="https://raw.githubusercontent.com/Chymeral/chymeral/main/branding/chymeral_logo.svg" alt="Chymeral logo" width="160"></a>
</p>

# Chymeral Rust SDK

<p align="center">
  <a href="https://github.com/Chymeral/chymeral-rust-sdk/actions/workflows/ci.yml"><img src="https://github.com/Chymeral/chymeral-rust-sdk/actions/workflows/ci.yml/badge.svg" alt="SDK quality checks"></a>
  <a href="https://crates.io/crates/chymeral-rust-sdk"><img src="https://img.shields.io/crates/v/chymeral-rust-sdk.svg" alt="crates.io version"></a>
  <a href="https://docs.rs/chymeral-rust-sdk"><img src="https://docs.rs/chymeral-rust-sdk/badge.svg" alt="docs.rs documentation"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/Chymeral/chymeral-rust-sdk" alt="MIT License"></a>
</p>

> The reference Rust client library for the Chymeral Link Protocol (CLP).

`chymeral-rust-sdk` lets a Rust program use the local graphics runtime exposed
by a running Chymeral terminal. It is a standalone client crate: it does not
depend on Chymeral source code, create a window, load plugins, or make Rust a
privileged language in the Chymeral ecosystem.

The released `1.5.0` crate supports public **CLP v2.1**, including frame
presentation and application window metadata. The `current-dev` source line
prepares `1.7.0` for **CLP v2.4**, adding reliable atomic scene batches. It
retains a one-time v2.0 fallback for a v2.0-only host and no CLP v1.x command
aliases. The SDK is synchronous by design and has no async-runtime dependency.
Version `1.0.0` remains the first stable Rust binding, and the current release
is available from [crates.io](https://crates.io/crates/chymeral-rust-sdk) with
generated API documentation on [docs.rs](https://docs.rs/chymeral-rust-sdk).
The matching [GitHub release](https://github.com/Chymeral/chymeral-rust-sdk/releases/latest)
records its human-readable release notes. Use a local path dependency while
developing changes against this repository.

## What it provides

- `Client::connect_from_env`, which uses the endpoint and token inherited from
  a Chymeral terminal and completes the CLP lifecycle handshake.
- Typed capabilities, subscriptions, session data, close reasons, errors, and
  host notifications.
- Typed scene commands for node creation, atomic mutation, removal, image
  ownership, and direct overlap queries.
- A bounded `SceneBatch` builder for reliable atomic create/update/remove
  revisions, with local aliases and no implicit sequential fallback.
- Intrinsic scene-text nodes with terminal-cell metrics, text/style updates,
  and default terminal typography.
- Unified session-owned RGBA8 and GIF image upload and removal.
- Alpha-derived image contact and generic node input passthrough for layered
  scenes without duplicate interaction logic.
- Incoming external-file-drop events for opt-in scene nodes.
- An opt-in, coalesced CLP v2.1 frame-presentation lane for bounded transient
  geometry.
- Session-owned CLP v2.1 application window metadata with verbatim titles and
  bounded static RGBA8 icons.
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

For a released version, add the crate from `crates.io`:

```text
cargo add chymeral-rust-sdk
```

Or declare it explicitly:

```toml
[dependencies]
chymeral-rust-sdk = "1.5"
```

For local development, reference the checked-out SDK instead:

```toml
[dependencies]
chymeral-rust-sdk = { path = "../chymeral-rust-sdk" }
```

The published `1.5.x` crate implements CLP v2.1. Use the `current-dev` path
checkout to develop or validate the unreleased `1.7.0` CLP v2.4 scene-batch
API before its registry publication.

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
| Atomic batches | `SceneBatch`, `SceneBatchOperation`, `BatchNodeRef`, `apply_scene_batch` |
| Images | `upload_image`, `remove_image`, `ImageUpload`, `ImageHandle` |
| Text | `NodeInput::Text`, `NodeChanges::text`, `NodeChanges::text_style` |
| Interaction | `ImageContactShape`, `NodeProperties::input_passthrough`, overlap queries |
| External drop | `Capability::ExternalDrop`, `external_drop_enabled`, `SceneEvent::ExternalDrop` |
| Frame presentation | `Capability::Presentation`, `present_scene`, `present_scene_with_id` |
| Window metadata | `Capability::WindowMetadata`, `set_window_metadata`, `WindowMetadataPatch` |
| Delivery | `next_notification`, `try_next_notification` |
| Termination | `report_fault`, `close` |

The central value types are `Capabilities`, `NodeProperties`,
`NodePropertiesPatch`, `NodeInput`, `NodeChanges`, `NodeVisual`, `SceneLayer`,
`Notification`, `Event`, `SceneEvent`, and `CloseReason`. Read their Rust documentation in `src/lib.rs`
for field-level contracts.

`Notification` delivers events, `Tick`, `Frame`, `Resize`, `DeliveryGap`,
`PresentationStatus`, `Closing`, and `Closed`. A client must treat `DeliveryGap` as a request to
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

## Atomic scene batches (CLP v2.4)

Use `SceneBatch` when a complete initial tree or structural revision must reach
the retained scene as one state. The builder remains a thin typed command
description, not a widget toolkit or virtual DOM:

```rust
use chymeral_rust_sdk::{
    BatchNodeRef, NodeInput, NodeProperties, NodePropertiesPatch, SceneBatch,
    SceneBatchNodeChanges, SceneLayer,
};

let batch = SceneBatch::new()
    .create_node(
        "panel",
        None,
        Some(SceneLayer::Overlay),
        NodeInput::Rectangle {
            properties: NodeProperties::new(24.0, 24.0, 280.0, 160.0),
            color: [36, 88, 216, 255],
        },
    )
    .create_node(
        "label",
        Some(BatchNodeRef::alias("panel")),
        None,
        NodeInput::Text {
            properties: chymeral_rust_sdk::TextNodeProperties::new(16.0, 20.0),
            text: "Ready".to_owned(),
            style: Default::default(),
        },
    )
    .set_node(
        BatchNodeRef::alias("panel"),
        SceneBatchNodeChanges {
            properties: Some(NodePropertiesPatch {
                clip_children: Some(true),
                ..NodePropertiesPatch::default()
            }),
            ..SceneBatchNodeChanges::default()
        },
    );
let result = client.apply_scene_batch(batch)?;
let panel = result.nodes["panel"].node.clone();
```

Request `Capability::Scene` and `Capability::SceneBatch`. A batch has 1–512
ordered operations, a 512 KiB JSON command-envelope limit, and unique aliases
of 1–128 UTF-8 bytes without control characters. An alias may be referenced
only after its `create_node` operation, so parent-before-child construction is
explicit. `create_node`, `set_node`, and `remove_node` are supported; image
uploads/removals, metadata, subscriptions, and frame presentation remain
separate operations.

The host validates the revision atomically, requests one redraw on success,
and reports handles only for aliases that remain live. It does not promise
compositor acknowledgement or physical scanout. A rejected batch returns an
error without exposing a partial tree. If the host does not grant
`Capability::SceneBatch`, `apply_scene_batch` returns
`Error::CapabilityNotGranted`; it never replays the batch as visible sequential
`create_node` or `set_node` traffic.

## Frame-paced presentation (CLP v2.1)

`present_scene` is for the newest transient geometry, not for reliable scene
control. Request `Capability::Scene` and `Capability::Presentation`, then send
one atomic packet with `Client::present_scene` or an application-defined
`PresentationId` through `Client::present_scene_with_id`:

```rust
use chymeral_rust_sdk::{PresentationPropertiesPatch, PresentationUpdate};

let presentation_id = client.present_scene([
    PresentationUpdate {
        node: viewport_root.clone(),
        properties: PresentationPropertiesPatch {
            x: Some(128.0),
            y: Some(72.0),
            ..PresentationPropertiesPatch::default()
        },
    },
    PresentationUpdate {
        node: scrollbar_thumb.clone(),
        properties: PresentationPropertiesPatch {
            y: Some(96.0),
            ..PresentationPropertiesPatch::default()
        },
    },
])?;
```

Each packet has one through 256 distinct owned nodes and changes only `x`, `y`,
`width`, and `height`; intrinsic text permits only `x` and `y`. The SDK rejects
empty, duplicate, non-finite, oversized, and over-64-KiB packets before it
writes them. The host validates ownership and the complete packet atomically at
its renderer boundary.

The newest unpresented packet wins. `Notification::PresentationStatus` later
reports `Presented`, `Superseded`, or `Rejected`, but is advisory and coalesced:
do not wait for one status per high-rate input sample. `Presented` reaches the
host presentation call, not confirmed monitor scanout. Keep creates, removes,
style changes, hierarchy changes, and other ordered work on `set_node`.

`Client::connect` first requests CLP v2.4. If a v2.0-only host explicitly
rejects that lifecycle request, it reconnects once using v2.0 without
`Capability::Presentation`, `Capability::WindowMetadata`, or
`Capability::SceneBatch`. A v2.1 host can negotiate its lower minor version
and likewise does not grant `SceneBatch`. In either case the newer methods
return `CapabilityNotGranted`; the SDK never expands a presentation packet or
scene batch into individual `set_node` commands.

## Application window metadata (CLP v2.1)

Request `Capability::WindowMetadata` to let this session identify its ordinary
program in the native Chymeral window. `WindowMetadataPatch` uses explicit
three-state fields: `MetadataField::Preserve` omits a field, `Clear` writes
JSON `null`, and `Set` replaces it. At least one field must be non-preserved.

```rust
use chymeral_rust_sdk::{
    Capability, ImageUpload, MetadataField, WindowMetadataPatch,
};

if client.has_capability(Capability::WindowMetadata) {
    let icon = client.upload_image(ImageUpload::Rgba8 {
        width: 1,
        height: 1,
        pixels: &[37, 99, 235, 255],
    })?;

    let status = client.set_window_metadata(WindowMetadataPatch {
        title: MetadataField::Set("Example editor".to_owned()),
        icon: MetadataField::Set(icon),
    })?;
    println!("dynamic icon support: {}", status.icon_supported);

    // The icon remains selected because this patch omits it.
    client.set_window_metadata(WindowMetadataPatch {
        title: MetadataField::Set("Example editor — project".to_owned()),
        ..WindowMetadataPatch::default()
    })?;
}
```

Titles are verbatim, nonempty, control-free Unicode with a 256-byte UTF-8
limit. An icon is a session-owned static RGBA8 `ImageHandle`, at most 256 × 256
pixels and 256 KiB; GIFs, paths, URLs, and foreign handles are rejected. The
host refuses `remove_image` while that handle remains the selected icon.

Each session retains a claim and the newest live nonempty claim is visible.
Clearing the active claim or closing its session restores the newest remaining
claim, then Chymeral's terminal-derived title and bundled icon when none
remain. `client.session().context.window_metadata` is `Some` when negotiated
and reports `icon_supported`; native Wayland can apply a title but reports
`false` for portable dynamic icons. The claim stays valid in that case.

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
queries, atomic scene-batch creation/update/removal, a title-plus-icon metadata
update, title-only preservation, explicit metadata clearing when a v2.1 host
grants that capability, and a clean close. It must return to the same terminal
without closing it.

The host repository also maintains a versioned,
language-neutral [CLP v2.0 fixture corpus](https://github.com/Chymeral/chymeral/tree/main/protocol/fixtures/v2.0).
This crate vendors its exact snapshot in `fixtures/clp/v2.0`, including
`SHA256SUMS`. The `fixture_corpus` integration test verifies that checksum and
replays every vector through the public client and an independent fake host.
Update both files only from a reviewed core revision. The corpus complements,
rather than replaces, this crate's focused fake-host tests and live
conformance executable.

This crate also vendors the byte-checked CLP v2.1 fixture corpus in
`fixtures/clp/v2.1`. It verifies the public client against an accepted atomic
packet, a rejected intrinsic-text bounds packet, and application window
metadata patch preservation and clearing, while the v2.0 replay verifies
automatic one-time fallback.

The `fixtures/clp/v2.4` snapshot adds the byte-checked `scene_batch` lifecycle
and result vector. It verifies the public `SceneBatch` API, local aliases, and
the ordered atomic result. A client negotiated below v2.4 never receives
`Capability::SceneBatch`; applications decide explicitly whether to use their
own incremental alternative.

For interactive scene validation in a graphical Chymeral session, run:

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
runs formatting, all targets including the vendored corpus replay, Clippy and
Rustdoc with warnings denied, and release builds of the conformance example.

For local checks:

```text
cargo fmt --check
cargo test --locked --all-targets -- --test-threads=1
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release --example clp_conformance
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the warnings-denied Rustdoc command
and its Linux/Windows shell forms.

## Relationship to Chymeral

CLP is language-neutral. This crate is the first reference SDK, not a protocol
fork and not a requirement for applications written in other languages. A
direct CLP client, the separate
[Chymeral Node SDK](https://github.com/Chymeral/chymeral-node-sdk), and future
language SDKs have the same status when they preserve the negotiated lifecycle,
capability, ownership, and delivery contracts. The Node SDK is published on
[npm](https://www.npmjs.com/package/@chymeral/node-sdk).

See the [Technical Journal](Technical_Journal.md) for the M14–M25 record and
design decisions. The host repository's `Main_Manual.md` is the authoritative
guide to Chymeral configuration and the complete runtime behavior.

## Version and contribution policy

This crate's released `1.5.x` line is the stable Rust binding for CLP v2.1 and
retains one-time CLP v2.0 fallback. The `current-dev` `1.7.0` line prepares CLP
v2.4 scene batches while retaining the same fallback. The earlier `1.0.x` line
supports CLP v2.0. The future breaking `2.0.0` line is reserved for a
separately validated platform-capability expansion. Chymeral application
versions, CLP protocol versions, and SDK versions are independent:
compatibility follows the negotiated CLP contract rather than matching version
strings. The core Chymeral application is not a supported Rust library; this
crate is the supported Rust client surface.

Read [CONTRIBUTING.md](CONTRIBUTING.md) before changing the public API,
[CHANGELOG.md](CHANGELOG.md) for release history, and
[SECURITY.md](SECURITY.md) for private vulnerability reporting.

## License

This repository is distributed under the [MIT License](LICENSE).
