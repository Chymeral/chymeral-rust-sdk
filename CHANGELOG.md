# Changelog

All notable user-visible changes to the Chymeral Rust SDK are recorded here.
The crate follows [Semantic Versioning](https://semver.org/); the Chymeral
application and CLP protocol have independent version lines.

## [Unreleased]

No unreleased public API changes.

## [1.0.0] - 2026-10-17

### Added

- The first stable, synchronous Rust binding for the public CLP v2.0 lifecycle,
  capabilities, subscriptions, notifications, and close model.
- Typed scene operations for nodes, atomic mutation, hierarchy, RGBA8/GIF
  images, intrinsic text, overlap queries, alpha image contact, input
  passthrough, and supported external-file-drop events.
- A `clp_conformance` executable covering protocol lifecycle and supported
  client operations against a live Chymeral endpoint.

### Compatibility

- `1.0.x` supports CLP v2.0 only. It intentionally does not retain CLP v1.x or
  provisional v1.4 command aliases.
- Released SDK versions are distributed as source crates through `crates.io`;
  the repository remains the authoritative development source.
