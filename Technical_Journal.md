# Chymeral Rust SDK Technical Journal

This journal records the decisions and delivery scope of the first public
Chymeral client SDK. It complements the product-facing [README](README.md) and
the Chymeral host's main manual.

## M14 scope

M14 made CLP practical for an external application without making Rust part of
the host. The result is a separate repository and crate named
`chymeral-rust-sdk`, imported in Rust as `chymeral_rust_sdk`.

The crate implements the negotiated CLP v1.3 lifecycle, typed scene/resource
commands, subscriptions, notifications, close semantics, and the full
rectangle-overlap surface. It also includes two executable clients:

- `clp_conformance`, a deterministic protocol and capability check.
- `clp_probe`, a visual drag-and-overlap exercise for a live Chymeral window.

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

## Validation delivery

The SDK is checked in a Linux/Windows GitHub Actions matrix for Rust 1.85 and
stable. Each job runs formatting, all-target tests, warnings-denied Clippy, and
release builds of both examples.

The conformance program establishes a session, requests and subscribes to
capabilities, confirms timing delivery, uploads and destroys RGBA8 data, mutates
the retained scene, queries an overlap, and validates `ClientRequested` closure.
The visual probe verifies practical input routing, drag updates, overlap-enter
and overlap-leave transitions, and a session-only close.

Manual validation on Linux and Windows confirmed that a completed or failed CLP
session returns to the same shell, while the surrounding Chymeral terminal
remains active. Windows named-pipe backpressure was specifically hardened in
the host before this SDK was accepted as the M14 reference client.

## Intentional boundaries

- The crate is source-local for now and is not published to crates.io.
- No async adapter, code generator, or language-neutral IDL is supplied yet.
- It does not add widgets, physics, 3D, gamepad handling, external drag-and-drop,
  or non-rectangular collision to the host protocol.
- It is not an alternative terminal, renderer, or application framework.

Those boundaries make the SDK small enough to be a reliable reference while
leaving CLP open to future language-specific clients.
