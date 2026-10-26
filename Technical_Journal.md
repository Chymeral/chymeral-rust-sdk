# Chymeral Rust SDK Technical Journal

This journal records the decisions and delivery scope of the first public
Chymeral client SDK. It complements the product-facing [README](README.md) and
the Chymeral host's main manual. The current public baseline is SDK `1.7.0`
with CLP v2.4; numbered milestone and release references below are historical
records, not current guidance.

## M14 scope

M14 made CLP practical for an external application without making Rust part of
the host. The result is a separate repository and crate named
`chymeral-rust-sdk`, imported in Rust as `chymeral_rust_sdk`.

The initial crate implemented the negotiated CLP v1.3 lifecycle, typed
scene/resource commands, subscriptions, notifications, close semantics, and
the rectangle-overlap surface. M19 then replaced that provisional wire contract
with CLP v2.0 and its canonical node/image operations. It also includes two
historical executable clients:

- `clp_conformance`, a deterministic protocol and capability check.
- `clp_probe`, a visual drag-and-overlap exercise for a live Chymeral window.

The visual probe was retired after M19 validation, along with the later text
probe, so the distributable SDK repository retains only `clp_conformance`.

## Architectural decisions

### Separate client library

The SDK lives outside the Chymeral host repository so it has an independent
release, dependency, and compatibility lifecycle. The host never compiles it
into itself and never dynamically loads it. This preserves the central model:
applications are ordinary programs communicating with the runtime over CLP.

### Rust is a reference, not a privilege

CLP remains neutral to language and transport-independent at its contract
boundary. The Rust SDK is a typed convenience layer and a conformance reference;
it does not define a Rust-only API. Other language libraries may expose the
same protocol semantics in idiomatic forms.

### Synchronous first

The initial SDK is synchronous and has no async runtime dependency. That keeps
the dependency surface small, fits terminal programs naturally, and provides a
stable behavioral reference before asynchronous adapters are considered.

### Opaque ownership

`SessionId`, `NodeHandle`, and `TextureHandle` are opaque SDK values. The host
assigns them, validates their session ownership, and cleans their resources when
a session ends. The SDK does not expose host scene IDs, GPU types, socket handles,
or internal threads.

### Delivery pressure is observable

The client exposes `Notification::DeliveryGap` rather than pretending every
continuous event is lossless. When timing, raw pointer motion, drag motion, or
terminal output state was coalesced, clients can rebuild derived state. Explicit
closure remains the outcome if the host cannot preserve discrete input or resize.

### Clean process behavior

The SDK connects only through `CHYMERAL_CLP_ENDPOINT` and
`CHYMERAL_CLP_TOKEN`, inherited by applications launched in a Chymeral terminal.
It does not alter standard streams, create a window, run a global background
service, or close the terminal when `Client::close` ends its own session.

## M19–M20 stabilization

M19 aligned the SDK with the breaking CLP v2.0 contract: one implicit scene per
session, canonical `create_node`, `set_node`, `remove_node`, `upload_image`,
`remove_image`, `is_overlapping`, and `get_overlaps` operations, plus unified
RGBA8/GIF `ImageHandle` ownership. This deliberately removed v1.x aliases
before the first public binding release.

M20 implements interaction fidelity without raising the protocol version. The
public `ImageContactShape` lets an image retain normal rectangular contact or
use uploaded RGBA8 alpha/current GIF-frame alpha with a threshold from `1` to
`255`. The default is `Bounds`; `ImageContactShape::alpha()` selects the
default alpha threshold of one. The host applies the same shape to pointer,
drag/drop, external-file-drop, direct-overlap, and overlap-transition routing.

`input_passthrough` is a separate generic node property. It skips only the
node itself during pointer-derived and external-drop targeting; it does not
hide the node, suppress children, or alter overlap state. This enables visual
layers above a control without recreating the control's interaction logic.

The crate manifest moves to `1.0.0` as the first stable Rust SDK surface. It
is published on [crates.io](https://crates.io/crates/chymeral-rust-sdk) as the
released Rust binding for CLP v2.0. Graphical Linux and Windows acceptance
passed for the M20 alpha, passthrough, and animated-image scenario; the host's
`NEXT_STEPS.md` records that evidence before the release tag is made.

## Validation delivery

The SDK is checked in a Linux/Windows GitHub Actions matrix for Rust 1.85 and
stable. Each job runs formatting, all-target tests, warnings-denied Clippy, and
the release build of the conformance example.

The conformance program establishes a session, requests and subscribes to
capabilities, confirms timing delivery, uploads and destroys RGBA8/GIF data,
mutates the retained scene with alpha-contact visuals, queries an overlap, and
validates `ClientRequested` closure.

M22 adds a versioned, language-neutral CLP v2.0 fixture corpus in the host
repository. This crate vendors the exact snapshot and SHA-256 manifest, then
replays every vector through its public synchronous API and an independent
fake host in the Linux/Windows Rust 1.85 and stable matrix. It does not depend
on host code or network retrieval during CI.
The retired visual probe verified practical input routing, drag updates,
overlap-enter and overlap-leave transitions, and a session-only close.

M23–M24 add the opt-in CLP v2.1 presentation and application-window-metadata
capabilities without weakening the synchronous control plane.
The crate models metadata's omitted/cleared/set wire distinction explicitly as
`MetadataField`, so a title-only update preserves a selected icon. Its one-time
v2.0 reconnect removes both v2.1-only capabilities. The v2.1 fixture replay
covers the resulting title/icon patch sequence independently from live host
tests.

M25 consolidates that API as the `1.5.0` stable Rust binding for CLP v2.1. It
retains the deliberate one-time v2.0 lifecycle retry without silently
translating presentation traffic into ordered node mutations. Locked formatting,
tests, warnings-denied Clippy and Rustdoc, package validation, and release
example builds passed on Linux and Windows. The vendored v2.1 fixture snapshot
and SHA-256 manifest match the core and Node SDK snapshots byte for byte. Live
Rust conformance and the Node visual probes passed against the matching
Chymeral `1.5.0` host on Windows, Linux/X11, and native Wayland, with native
Wayland correctly reporting dynamic application icons unsupported.

Manual validation on Linux and Windows confirmed that a completed or failed CLP
session returns to the same shell, while the surrounding Chymeral terminal
remains active. Windows named-pipe backpressure was specifically hardened in
the host before this SDK was accepted as the M14 reference client.

## M26 atomic scene batches

The released `1.7.0` line adds the typed CLP v2.4 `SceneBatch` API without
changing the SDK's synchronous transport model. A batch is an ordered list of
`create_node`, `set_node`, and `remove_node` operations. `BatchNodeRef` makes
the distinction between an existing opaque `NodeHandle` and a batch-local alias
explicit; a `create_node` alias is usable only by later operations. The helper
does not define a widget abstraction, virtual DOM, host plugin, or alternative
scene graph.

`Client::apply_scene_batch` first requires both `Scene` and `SceneBatch`, then
validates operation count, command size, aliases, parent/layer structure, and
the already-established image, text, overlap, and external-drop capability
requirements. Its result preserves operation order and exposes only aliases
whose nodes remain live at commit. This allows a batch to use temporary nodes
without leaking a false long-lived handle.

The feature does not hide a compatibility policy inside the client. A host
negotiated below v2.4 cannot grant `SceneBatch`, and the method returns an
explicit capability error rather than expanding a batch into serial scene
commands. The existing v2.0 reconnect remains limited to lifecycle fallback.
The v2.4 fixture corpus and live conformance path exercise the same public API
against an independent fake host and a real Chymeral window respectively.

## M26.1 internal source structure

M26.1 is a no-wire, no-API maintenance pass for the `1.7.0` line. `lib.rs`
remains the documented public facade; client orchestration, the
private wire schema, local framing, and private tests are separated into
focused source units. This preserves the synchronous transport model, every
public Rust and Rustdoc path, and the byte-identical vendored fixture corpus.
The private module paths are implementation detail, not additional supported
imports for consumers.

## Intentional boundaries

- The crate is published to crates.io; this repository remains the authoritative
  development source and conformance reference.
- No async adapter, code generator, or language-neutral IDL is supplied yet.
- It does not add widgets, physics, 3D, gamepad handling, outgoing external
  drag-and-drop, or arbitrary vector/polygon collision to the host protocol.
- It is not an alternative terminal, renderer, or application framework.

Those boundaries make the SDK small enough to be a reliable reference while
leaving CLP open to future language-specific clients.
