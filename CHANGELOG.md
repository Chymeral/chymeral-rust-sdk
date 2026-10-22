# Changelog

All notable user-visible changes to the Chymeral Rust SDK are recorded here.
The crate follows [Semantic Versioning](https://semver.org/); the Chymeral
application and CLP protocol have independent version lines.

## [Unreleased]

## [1.5.0] - 2026-10-22

### Added

- CLP v2.1 support for `Capability::Presentation`,
  `PresentationId`, `PresentationUpdate`, `PresentationPropertiesPatch`,
  `PresentationStatus`, `Client::present_scene`, and
  `Client::present_scene_with_id`.
- Byte-checked CLP v2.1 fixture replay for an accepted packet and a rejected
  intrinsic-text bounds update.
- CLP v2.1 support for `Capability::WindowMetadata`,
  `WindowMetadataPatch`, `MetadataField`, `WindowMetadataStatus`, and
  `Client::set_window_metadata`.
- Byte-checked v2.1 replay for title/icon metadata, field preservation, and
  explicit clearing.

### Changed

- Connection now retries once with the frozen CLP v2.0 capability set when a
  v2.0-only host rejects the initial v2.1 lifecycle request. Presentation and
  window metadata are not requested on that retry; presentation never falls
  back to individual `set_node` commands.

### Compatibility

- Version `1.5.0` implements CLP v2.1 and retains one-time v2.0 fallback.
  The earlier `1.0.x` line implements CLP v2.0. The future breaking `2.0.0`
  line is reserved for the separately validated M26 platform expansion.

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
