//! A synchronous, dependency-light reference client for CLP v2.1.
//!
//! The crate speaks only the public Chymeral Link Protocol. It neither imports
//! Chymeral internals nor creates a graphics window of its own.

use std::{
    collections::{BTreeSet, VecDeque},
    env,
    error::Error as StdError,
    fmt::{Display, Formatter},
    io::{self, Read, Write},
    thread,
    time::{Duration, Instant},
};

use interprocess::local_socket::{
    GenericNamespaced, ToNsName, prelude::LocalSocketStream, traits::Stream as _,
};
use serde::{Deserialize, Serialize};

/// The endpoint environment variable inherited from a Chymeral child process.
pub const ENDPOINT_ENV: &str = "CHYMERAL_CLP_ENDPOINT";
/// The session token environment variable inherited from a Chymeral child process.
pub const TOKEN_ENV: &str = "CHYMERAL_CLP_TOKEN";
/// The largest CLP frame, including its one-byte kind discriminator.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// The newest CLP version implemented by this SDK.
pub const CLP_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new(2, 1);
const CLP_PROTOCOL_V2_0: ProtocolVersion = ProtocolVersion::new(2, 0);
const IO_RETRY_INTERVAL: Duration = Duration::from_millis(5);
const IO_STALL_TIMEOUT: Duration = Duration::from_secs(5);
const BINARY_FRAGMENT_BYTES: usize = 64 * 1024;
/// Maximum geometry updates accepted in one frame-presentation packet.
pub const MAX_PRESENTATION_UPDATES: usize = 256;
/// Maximum UTF-8 byte length of a presentation identifier.
pub const MAX_PRESENTATION_ID_BYTES: usize = 128;
/// Maximum serialized JSON payload size for one presentation packet.
pub const MAX_PRESENTATION_PACKET_BYTES: usize = 64 * 1024;
/// Maximum UTF-8 byte length of an application window title.
pub const MAX_WINDOW_TITLE_BYTES: usize = 256;
/// Maximum width or height of an application window icon.
pub const MAX_WINDOW_ICON_DIMENSION: u32 = 256;
/// Maximum RGBA8 byte length of an application window icon.
pub const MAX_WINDOW_ICON_BYTES: usize = 256 * 1024;

/// A CLP protocol version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProtocolVersion {
    /// Major protocol version.
    pub major: u16,
    /// Minor protocol version.
    pub minor: u16,
}

impl ProtocolVersion {
    /// Creates a protocol version.
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }
}

impl Display for ProtocolVersion {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

/// A feature negotiated during the CLP handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Retained scene nodes and their contents.
    Scene,
    /// Session-owned RGBA8 and GIF image data.
    Images,
    /// Keyboard, pointer, focus, scene, and terminal input.
    Input,
    /// Tick and frame notifications.
    Timing,
    /// Rectangle overlap queries and transitions.
    Overlap,
    /// Incoming desktop-file drops targeted to opt-in scene nodes.
    ExternalDrop,
    /// Intrinsic scene text nodes with terminal-cell layout.
    Text,
    /// Coalesced, frame-paced geometry presentation packets.
    Presentation,
    /// Session-owned application window title and static icon metadata.
    WindowMetadata,
}

/// A deterministic set of negotiated or requested capabilities.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities(BTreeSet<Capability>);

impl Capabilities {
    /// No capabilities.
    pub fn none() -> Self {
        Self::default()
    }

    /// Every capability implemented by this SDK.
    pub fn all() -> Self {
        Self::from([
            Capability::Scene,
            Capability::Images,
            Capability::Input,
            Capability::Timing,
            Capability::Overlap,
            Capability::ExternalDrop,
            Capability::Text,
            Capability::Presentation,
            Capability::WindowMetadata,
        ])
    }

    /// Returns whether this set contains a capability.
    pub fn contains(&self, capability: Capability) -> bool {
        self.0.contains(&capability)
    }

    /// Iterates in wire-order-independent deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.0.iter().copied()
    }

    fn without(&self, capability: Capability) -> Self {
        let mut capabilities = self.clone();
        capabilities.0.remove(&capability);
        capabilities
    }
}

impl<const N: usize> From<[Capability; N]> for Capabilities {
    fn from(capabilities: [Capability; N]) -> Self {
        Self(capabilities.into_iter().collect())
    }
}

impl FromIterator<Capability> for Capabilities {
    fn from_iter<T: IntoIterator<Item = Capability>>(capabilities: T) -> Self {
        Self(capabilities.into_iter().collect())
    }
}

/// An opaque session identifier owned by Chymeral.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Returns the protocol representation, such as `session-1`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for SessionId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// An opaque scene node handle, valid only in its creating session.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeHandle(String);

impl NodeHandle {
    /// Returns the protocol representation, such as `node-1`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for NodeHandle {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// An opaque image handle, valid only in its creating session.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ImageHandle(String);

impl ImageHandle {
    /// Returns the protocol representation, such as `image-1`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ImageHandle {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Arguments and launch commands supplied by Chymeral during initialization.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct InitContext {
    /// Program arguments from the launcher.
    pub arguments: Vec<String>,
    /// Initial commands requested by the launcher.
    pub initial_commands: Vec<StartupCommand>,
    /// Backend-specific support for the negotiated window metadata capability.
    ///
    /// This is absent on hosts that did not grant
    /// [`Capability::WindowMetadata`]. A native Wayland host may grant the
    /// capability while reporting `icon_supported: false`, because it can set
    /// the title but not a portable taskbar icon.
    #[serde(default)]
    pub window_metadata: Option<WindowMetadataSupport>,
}

/// Host support details for [`Capability::WindowMetadata`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct WindowMetadataSupport {
    /// Whether a static application icon can be applied on this backend.
    pub icon_supported: bool,
}

/// One field in a [`WindowMetadataPatch`].
///
/// `Preserve` omits the wire field, `Clear` writes JSON `null`, and `Set`
/// writes the supplied title or session-owned image handle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MetadataField<T> {
    /// Leave the field from this session's current claim unchanged.
    #[default]
    Preserve,
    /// Remove this field from this session's current claim.
    Clear,
    /// Replace this field in this session's current claim.
    Set(T),
}

impl<T> MetadataField<T> {
    const fn is_preserve(&self) -> bool {
        matches!(self, Self::Preserve)
    }
}

impl<T> Serialize for MetadataField<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Preserve | Self::Clear => serializer.serialize_none(),
            Self::Set(value) => value.serialize(serializer),
        }
    }
}

/// A partial session-owned application window metadata update.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowMetadataPatch {
    /// Verbatim native window title. It must be nonempty, control-free, and no
    /// longer than [`MAX_WINDOW_TITLE_BYTES`] UTF-8 bytes when set.
    pub title: MetadataField<String>,
    /// A session-owned static RGBA8 [`ImageHandle`] when set. GIF resources,
    /// resources larger than the documented icon limits, and foreign handles
    /// are rejected by the host.
    pub icon: MetadataField<ImageHandle>,
}

impl WindowMetadataPatch {
    /// Returns whether both fields would be omitted from the command.
    pub const fn is_empty(&self) -> bool {
        self.title.is_preserve() && self.icon.is_preserve()
    }
}

/// Result returned by [`Client::set_window_metadata`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowMetadataStatus {
    /// Whether the active host backend applied or can apply the requested icon.
    ///
    /// The metadata claim itself remains valid when this is false, so the
    /// native application title still applies on native Wayland.
    pub icon_supported: bool,
}

/// A launch command supplied in [`InitContext`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct StartupCommand {
    /// Command name.
    pub name: String,
    /// Command arguments.
    pub arguments: Vec<String>,
}

/// Immutable data negotiated before the session enters the running state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    /// Version selected by the host.
    pub protocol_version: ProtocolVersion,
    /// The session identity.
    pub session_id: SessionId,
    /// Capabilities granted by the host.
    pub capabilities: Capabilities,
    /// Launch context from the host.
    pub context: InitContext,
}

/// A root-node drawing layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneLayer {
    /// Behind the terminal grid.
    Underlay,
    /// In front of the terminal grid.
    #[default]
    Overlay,
}

/// Complete properties used when creating a scene node.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct NodeProperties {
    /// Local x coordinate in logical pixels.
    pub x: f64,
    /// Local y coordinate in logical pixels.
    pub y: f64,
    /// Width in logical pixels.
    pub width: f64,
    /// Height in logical pixels.
    pub height: f64,
    /// Whether the node is drawn. Invisible nodes may still overlap.
    pub visible: bool,
    /// Whether children are clipped to this node's bounds.
    pub clip_children: bool,
    /// Sibling paint order.
    pub z_index: i32,
    /// Enables this node as a rectangular overlap target.
    pub overlap_enabled: bool,
    /// Allows this node to receive incoming desktop-file drops.
    pub external_drop_enabled: bool,
    /// Skips this node during pointer and external-drop targeting.
    ///
    /// Its children remain independently interactive, and its display,
    /// overlap participation, and program-owned state are unaffected.
    pub input_passthrough: bool,
}

/// Complete properties used when creating an intrinsic text node.
///
/// Text bounds are derived by Chymeral from the text, style, and terminal-cell
/// metrics. The origin is the top-left corner of the first text cell.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TextNodeProperties {
    /// Local x coordinate in logical pixels.
    pub x: f64,
    /// Local y coordinate in logical pixels.
    pub y: f64,
    /// Whether the node is drawn. Invisible nodes may still overlap.
    pub visible: bool,
    /// Whether children are clipped to the text node's intrinsic bounds.
    pub clip_children: bool,
    /// Sibling paint order.
    pub z_index: i32,
    /// Enables this node as a rectangular overlap target.
    pub overlap_enabled: bool,
    /// Allows this node to receive incoming desktop-file drops.
    pub external_drop_enabled: bool,
    /// Skips this node during pointer and external-drop targeting.
    pub input_passthrough: bool,
}

impl TextNodeProperties {
    /// Creates visible, unclipped text-node properties with default stacking order.
    pub const fn new(x: f64, y: f64) -> Self {
        Self {
            x,
            y,
            visible: true,
            clip_children: false,
            z_index: 0,
            overlap_enabled: false,
            external_drop_enabled: false,
            input_passthrough: false,
        }
    }
}

/// The interactive shape used by a static RGBA8 or animated GIF image node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageContactShape {
    /// Use the complete image-node rectangle.
    #[default]
    Bounds,
    /// Use the source alpha of the static image or current GIF frame.
    Alpha {
        /// The inclusive alpha threshold in the range `1..=255`.
        threshold: u8,
    },
}

impl ImageContactShape {
    /// Creates alpha-based contact with the default threshold of one.
    pub const fn alpha() -> Self {
        Self::Alpha { threshold: 1 }
    }
}

/// Horizontal alignment of each explicit text line within the intrinsic width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    /// Align lines to their start edge.
    #[default]
    Start,
    /// Center lines within the widest line's cell width.
    Center,
    /// Align lines to their end edge.
    End,
}

/// Font weight for an intrinsic text node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextWeight {
    /// The terminal's normal font weight.
    #[default]
    Normal,
    /// The terminal's bold font weight.
    Bold,
}

/// Optional presentation values for an intrinsic text node.
///
/// Omitted fields inherit the terminal font, size, line height, and foreground
/// color. `font` must name a `scene_text.fonts` alias configured by the host.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TextStyle {
    /// Optional explicit font alias from Chymeral configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Optional font size in logical pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    /// Optional line height in logical pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f32>,
    /// Optional RGBA8 text color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 4]>,
    /// Font weight.
    pub weight: TextWeight,
    /// Explicit-line alignment.
    pub align: TextAlign,
}

/// Host-computed terminal-cell metrics for an intrinsic text node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub struct TextMetrics {
    /// Width of the widest explicit line in terminal cells.
    pub columns: u32,
    /// Number of explicit lines, including a trailing empty line after `\n`.
    pub rows: u32,
    /// Intrinsic width in logical pixels.
    pub width: f64,
    /// Intrinsic height in logical pixels.
    pub height: f64,
}

/// A newly created node and optional metrics for intrinsic text.
#[derive(Clone, Debug, PartialEq)]
pub struct CreatedNode {
    /// Opaque node handle.
    pub node: NodeHandle,
    /// Initial intrinsic text metrics, present only for a text node.
    pub text_metrics: Option<TextMetrics>,
}

impl NodeProperties {
    /// Creates visible, unclipped node properties with default stacking order.
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
            visible: true,
            clip_children: false,
            z_index: 0,
            overlap_enabled: false,
            external_drop_enabled: false,
            input_passthrough: false,
        }
    }
}

/// A partial node-property update.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct NodePropertiesPatch {
    /// Optional local x coordinate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    /// Optional local y coordinate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    /// Optional width.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// Optional height.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Optional visibility flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// Optional child clipping flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip_children: Option<bool>,
    /// Optional sibling paint order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub z_index: Option<i32>,
    /// Optional overlap flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlap_enabled: Option<bool>,
    /// Optional external-file-drop target flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_drop_enabled: Option<bool>,
    /// Optional pointer and external-drop passthrough flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_passthrough: Option<bool>,
}

/// Geometry fields eligible for CLP's frame-paced presentation lane.
///
/// Presentation intentionally excludes hierarchy, content, visibility,
/// stacking, clipping, and interaction flags. Use [`Client::set_node`] for
/// those reliable retained-scene mutations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct PresentationPropertiesPatch {
    /// Optional local x coordinate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    /// Optional local y coordinate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    /// Optional width. Intrinsic text nodes cannot receive this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// Optional height. Intrinsic text nodes cannot receive this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
}

impl PresentationPropertiesPatch {
    /// Returns whether the patch contains no eligible visual change.
    pub const fn is_empty(self) -> bool {
        self.x.is_none() && self.y.is_none() && self.width.is_none() && self.height.is_none()
    }
}

/// One geometry update in an atomic frame-presentation packet.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PresentationUpdate {
    /// Session-owned node to update.
    pub node: NodeHandle,
    /// Geometry fields to replace when the packet reaches a rendered frame.
    pub properties: PresentationPropertiesPatch,
}

/// Client-generated identity for an asynchronous presentation packet.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PresentationId(String);

impl PresentationId {
    /// Creates an explicit identifier for [`Client::present_scene_with_id`].
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_PRESENTATION_ID_BYTES {
            return Err(Error::InvalidInput(format!(
                "a presentation_id must contain between 1 and {MAX_PRESENTATION_ID_BYTES} bytes"
            )));
        }
        Ok(Self(value))
    }

    /// Returns the protocol representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for PresentationId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Complete node data used by [`Client::create_node`].
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeInput {
    /// A node without visible content.
    Empty {
        /// Complete rectangle properties.
        properties: NodeProperties,
    },
    /// A solid RGBA8 rectangle.
    Rectangle {
        /// Complete rectangle properties.
        properties: NodeProperties,
        /// RGBA8 color components.
        color: [u8; 4],
    },
    /// A static RGBA8 or animated GIF image with independent opacity.
    Image {
        /// Complete rectangle properties.
        properties: NodeProperties,
        /// Session-owned image.
        image: ImageHandle,
        /// Image opacity in the inclusive range accepted by Chymeral.
        opacity: f32,
        /// Interactive image shape. `Bounds` preserves rectangular contact.
        contact_shape: ImageContactShape,
    },
    /// Text with host-computed terminal-cell bounds.
    Text {
        /// Complete text-node properties without width or height.
        properties: TextNodeProperties,
        /// UTF-8 text using terminal-cell layout.
        text: String,
        /// Optional text presentation values.
        style: TextStyle,
    },
}

/// A visual replacement accepted by [`NodeChanges`].
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeVisual {
    /// A node without visible content.
    Empty,
    /// A solid RGBA8 rectangle.
    Rectangle {
        /// RGBA8 color components.
        color: [u8; 4],
    },
    /// A static RGBA8 or animated GIF image with independent opacity.
    Image {
        /// Session-owned image.
        image: ImageHandle,
        /// Image opacity in the inclusive range accepted by Chymeral.
        opacity: f32,
        /// Interactive image shape. `Bounds` preserves rectangular contact.
        contact_shape: ImageContactShape,
    },
}

/// A structural parent change for [`NodeChanges`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeParent {
    /// Make the node a root.
    Root,
    /// Attach the node to another session-owned node.
    Node {
        /// Parent node.
        node: NodeHandle,
    },
}

/// An atomic partial node update.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NodeChanges {
    /// Partial geometry and interaction-property changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<NodePropertiesPatch>,
    /// Structural parent change; omit to preserve the current parent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<NodeParent>,
    /// Root drawing-layer change; omit to preserve the current layer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<SceneLayer>,
    /// Visual replacement. Text nodes may not receive visual changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visual: Option<NodeVisual>,
    /// Text replacement. Visual nodes may not receive text changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Text-style replacement. Visual nodes may not receive text changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_style: Option<TextStyle>,
}

/// Bytes submitted with [`Client::upload_image`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageUpload<'a> {
    /// Exact RGBA8 pixels in row-major order.
    Rgba8 {
        /// Image width in pixels.
        width: u32,
        /// Image height in pixels.
        height: u32,
        /// Exact `width * height * 4` RGBA8 payload.
        pixels: &'a [u8],
    },
    /// Encoded GIF bytes.
    Gif {
        /// Complete encoded GIF payload.
        bytes: &'a [u8],
    },
}

/// A requested host notification subscription.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subscription {
    /// Raw keyboard, pointer, scroll, focus, and custom events.
    RawInput,
    /// Terminal output and exit state.
    Terminal,
    /// Tick and frame notifications.
    Timing,
}

/// A category of continuous notifications that was coalesced or dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryCategory {
    /// Raw pointer input.
    RawInput,
    /// Scene pointer or drag movement.
    Scene,
    /// Terminal output state.
    Terminal,
    /// Tick or frame timing.
    Timing,
}

/// A program phase used when reporting a client fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramPhase {
    /// Initialization processing.
    Init,
    /// Startup processing.
    Start,
    /// Event processing.
    Event,
    /// Command processing.
    Command,
    /// Tick processing.
    Tick,
    /// Frame processing.
    Frame,
}

/// A terminal reason for a session closure.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CloseReason {
    /// The client requested closure.
    ClientRequested,
    /// The client reported an application fault.
    ProgramFault {
        /// Phase that failed.
        phase: ProgramPhase,
        /// Client-provided diagnostic message.
        message: String,
    },
    /// Chymeral is shutting down.
    HostShutdown,
    /// This session exceeded its bounded host-notification queue.
    HostOverloaded,
    /// The session violated CLP.
    ProtocolViolation,
    /// The transport disconnected before normal closure.
    TransportDisconnected,
}

/// A two-dimensional logical coordinate or delta.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Vec2 {
    /// Horizontal component.
    pub x: f64,
    /// Vertical component.
    pub y: f64,
}

/// Modifier keys accompanying an input event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Modifiers {
    /// Shift modifier.
    pub shift: bool,
    /// Control modifier.
    pub control: bool,
    /// Alt modifier.
    pub alt: bool,
    /// Platform super modifier.
    pub super_key: bool,
}

/// A logical key reported by Chymeral.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Key {
    /// A text character key.
    Character { value: String },
    /// A named non-text key.
    Named { value: String },
    /// A dead key, optionally with its character.
    Dead { value: Option<String> },
    /// A key Chymeral cannot identify.
    Unidentified,
}

/// Press or release state of a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    /// Pressed.
    Pressed,
    /// Released.
    Released,
}

/// A pointer button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PointerButton {
    /// Primary pointer button.
    Primary,
    /// Secondary pointer button.
    Secondary,
    /// Middle pointer button.
    Middle,
    /// Back pointer button.
    Back,
    /// Forward pointer button.
    Forward,
    /// A platform-specific button.
    Other { value: u16 },
}

/// Unit used by a pointer scroll delta.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollUnit {
    /// Logical text lines.
    Line,
    /// Logical pixels.
    Pixel,
}

/// Window focus state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusState {
    /// The Chymeral window gained focus.
    Gained,
    /// The Chymeral window lost focus.
    Lost,
}

/// Terminal state reported to a subscriber.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    /// Terminal output is available.
    OutputAvailable,
    /// The terminal child exited.
    Exited,
}

/// A directed scene event.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SceneEvent {
    /// Pointer entered a node.
    PointerEnter {
        target: NodeHandle,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// Pointer left a node.
    PointerLeave {
        target: NodeHandle,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// Pointer moved over a node.
    PointerMove {
        target: NodeHandle,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// Pointer pressed a node.
    PointerPress {
        target: NodeHandle,
        button: PointerButton,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// Pointer released over a node.
    PointerRelease {
        target: NodeHandle,
        button: PointerButton,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// A node gained keyboard focus.
    FocusGained { target: NodeHandle },
    /// A node lost keyboard focus.
    FocusLost { target: NodeHandle },
    /// A drag began from a node.
    DragStart {
        source: NodeHandle,
        origin: Vec2,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// A drag entered a target.
    DragEnter {
        source: Option<NodeHandle>,
        target: Option<NodeHandle>,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// A drag left a target.
    DragLeave {
        source: Option<NodeHandle>,
        target: Option<NodeHandle>,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// A drag moved.
    DragMove {
        source: Option<NodeHandle>,
        target: Option<NodeHandle>,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// A drag was dropped.
    Drop {
        source: Option<NodeHandle>,
        target: Option<NodeHandle>,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// A drag ended.
    DragEnd {
        source: Option<NodeHandle>,
        target: Option<NodeHandle>,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// A drag was cancelled.
    DragCancel {
        source: NodeHandle,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// Two enabled node rectangles began overlapping.
    OverlapEnter {
        first: NodeHandle,
        second: NodeHandle,
    },
    /// Two enabled node rectangles stopped overlapping.
    OverlapLeave {
        first: NodeHandle,
        second: NodeHandle,
    },
    /// An external file drag entered an opt-in node.
    ExternalDropEnter { target: NodeHandle, position: Vec2 },
    /// An external file drag moved over an opt-in node.
    ExternalDropOver { target: NodeHandle, position: Vec2 },
    /// An external file drag left an opt-in node.
    ExternalDropLeave { target: NodeHandle, position: Vec2 },
    /// Desktop files were dropped on an opt-in node.
    ExternalDrop {
        target: NodeHandle,
        position: Vec2,
        files: Vec<ExternalFile>,
    },
}

/// A desktop file delivered to an opt-in scene node.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ExternalFile {
    /// Canonical percent-encoded `file:` URI supplied by Chymeral.
    pub uri: String,
}

/// An input or scene event from Chymeral.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// Keyboard input for the focused scene owner.
    Key {
        key: Key,
        text: Option<String>,
        state: KeyState,
        modifiers: Modifiers,
        repeat: bool,
    },
    /// Raw pointer movement.
    PointerMove {
        position: Vec2,
        modifiers: Modifiers,
    },
    /// Raw pointer press.
    PointerPress {
        button: PointerButton,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// Raw pointer release.
    PointerRelease {
        button: PointerButton,
        position: Vec2,
        modifiers: Modifiers,
    },
    /// Raw pointer scroll.
    Scroll {
        position: Vec2,
        delta: Vec2,
        unit: ScrollUnit,
        modifiers: Modifiers,
    },
    /// Chymeral window focus state.
    Focus { state: FocusState },
    /// A scene event directed to this session.
    Scene { event: SceneEvent },
    /// A subscribed terminal state.
    Terminal { state: TerminalState },
    /// A host-defined custom event.
    Custom { name: String, payload: Vec<u8> },
}

/// Current dimensions of the Chymeral surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resize {
    /// Physical surface width in pixels.
    pub physical_width: u32,
    /// Physical surface height in pixels.
    pub physical_height: u32,
    /// Logical surface width.
    pub logical_width: f64,
    /// Logical surface height.
    pub logical_height: f64,
    /// Physical pixels per logical unit.
    pub scale_factor: f64,
}

/// Outcome reported by the host for the newest observable presentation packet.
///
/// These notifications are themselves coalesced under sustained traffic. They
/// are confirmation and diagnostics, not a per-packet acknowledgement queue;
/// retain the application's newest desired visual state instead of waiting for
/// every identifier to reappear.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationState {
    /// The packet's complete geometry state reached a presented host frame.
    Presented,
    /// Validation failed before the packet could mutate the scene.
    Rejected,
    /// A newer packet or a reliable scene mutation replaced this pending packet.
    Superseded,
}

/// A coalesced asynchronous status for a [`PresentationId`].
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PresentationStatus {
    /// Client-generated packet identity.
    pub presentation_id: PresentationId,
    /// Final or latest observable state.
    pub status: PresentationState,
    /// Host error code when `status` is [`PresentationState::Rejected`].
    #[serde(default)]
    pub code: Option<String>,
    /// Host diagnostic when `status` is [`PresentationState::Rejected`].
    #[serde(default)]
    pub message: Option<String>,
}

/// A message delivered after the client completes the CLP lifecycle handshake.
#[derive(Clone, Debug, PartialEq)]
pub enum Notification {
    /// Input, scene, terminal, or custom event.
    Event(Event),
    /// Scheduler tick duration.
    Tick(Duration),
    /// Render-frame duration.
    Frame(Duration),
    /// Current surface dimensions.
    Resize(Resize),
    /// The host coalesced continuous notifications.
    DeliveryGap(Vec<DeliveryCategory>),
    /// Coalesced state for an asynchronous frame-presentation packet.
    PresentationStatus(PresentationStatus),
    /// The host asks the client to close during shutdown.
    Closing(CloseReason),
    /// Terminal session closure.
    Closed(CloseReason),
}

/// Errors emitted by the SDK before, during, or after a CLP exchange.
#[derive(Debug)]
pub enum Error {
    /// An inherited environment variable is missing.
    MissingEnvironment(&'static str),
    /// Local-socket I/O failed.
    Io(io::Error),
    /// JSON encoding or decoding failed.
    Json(serde_json::Error),
    /// The host sent an invalid or unexpected wire message.
    Protocol(String),
    /// The host rejected a non-command operation.
    Host { code: String, message: String },
    /// The host rejected a scene command.
    Command {
        request_id: String,
        code: String,
        message: String,
    },
    /// A public SDK operation needs a capability not granted at handshake.
    CapabilityNotGranted(Capability),
    /// The session closed before an operation could complete.
    SessionClosed(CloseReason),
    /// An SDK method received invalid local input.
    InvalidInput(String),
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEnvironment(name) => {
                write!(formatter, "missing inherited environment variable {name}")
            }
            Self::Io(error) => write!(formatter, "CLP transport I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "CLP JSON codec failed: {error}"),
            Self::Protocol(message) => write!(formatter, "CLP protocol violation: {message}"),
            Self::Host { code, message } => write!(formatter, "CLP host error {code}: {message}"),
            Self::Command {
                request_id,
                code,
                message,
            } => write!(
                formatter,
                "CLP command {request_id} failed with {code}: {message}"
            ),
            Self::CapabilityNotGranted(capability) => {
                write!(formatter, "CLP capability {capability:?} was not granted")
            }
            Self::SessionClosed(reason) => write!(formatter, "CLP session is closed: {reason:?}"),
            Self::InvalidInput(message) => write!(formatter, "invalid CLP client input: {message}"),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Result type returned by public SDK operations.
pub type Result<T> = std::result::Result<T, Error>;

/// A synchronous CLP v2.1 client with one-time CLP v2.0 fallback.
///
/// `connect_from_env` completes the `hello → init → init_complete → start →
/// started` handshake. Its returned [`SessionInfo`] preserves the data from
/// `init`, while a successful return means the program has entered `started`.
pub struct Client {
    stream: LocalSocketStream,
    reader: FrameReader,
    session: SessionInfo,
    next_request_id: u64,
    next_presentation_id: u64,
    pending_notifications: VecDeque<Notification>,
    closed_reason: Option<CloseReason>,
}

impl Client {
    /// Connects using the endpoint and token inherited from Chymeral.
    pub fn connect_from_env(requested_capabilities: Capabilities) -> Result<Self> {
        let endpoint =
            env::var(ENDPOINT_ENV).map_err(|_| Error::MissingEnvironment(ENDPOINT_ENV))?;
        let token = env::var(TOKEN_ENV).map_err(|_| Error::MissingEnvironment(TOKEN_ENV))?;
        Self::connect(&endpoint, &token, requested_capabilities)
    }

    /// Connects to an explicit local endpoint. This is intended for launchers and tests.
    pub fn connect(
        endpoint: &str,
        token: &str,
        requested_capabilities: Capabilities,
    ) -> Result<Self> {
        if token.is_empty() {
            return Err(Error::InvalidInput(
                "the CLP token may not be empty".to_owned(),
            ));
        }
        match Self::connect_once(
            endpoint,
            token,
            requested_capabilities.clone(),
            CLP_PROTOCOL_VERSION,
        ) {
            Err(Error::Host { code, .. })
                if code == "unsupported_version" || code == "invalid_message" =>
            {
                // CLP v2.0 hosts predate the v2.1 presentation and window
                // metadata capabilities and may reject either enum value
                // before they reach version negotiation. Reconnect exactly
                // once with the frozen v2.0 capability set.
                Self::connect_once(
                    endpoint,
                    token,
                    requested_capabilities
                        .without(Capability::Presentation)
                        .without(Capability::WindowMetadata),
                    CLP_PROTOCOL_V2_0,
                )
            }
            result => result,
        }
    }

    fn connect_once(
        endpoint: &str,
        token: &str,
        requested_capabilities: Capabilities,
        requested_protocol: ProtocolVersion,
    ) -> Result<Self> {
        let name = endpoint
            .to_ns_name::<GenericNamespaced>()
            .map_err(|error| Error::InvalidInput(format!("invalid CLP endpoint: {error}")))?;
        let mut stream = LocalSocketStream::connect(name)?;
        if let Err(error) = stream.set_send_timeout(Some(IO_STALL_TIMEOUT)) {
            if error.kind() != io::ErrorKind::Unsupported {
                return Err(Error::Io(error));
            }
        }

        write_json(
            &mut stream,
            &WireClientMessage::Hello {
                protocol: requested_protocol,
                token: token.to_owned(),
                capabilities: requested_capabilities.iter().collect(),
            },
        )?;

        let mut reader = FrameReader::default();
        let (protocol, session_id, capabilities, context) =
            match read_json(&mut stream, &mut reader)? {
                WireHostMessage::Init {
                    protocol,
                    session_id,
                    capabilities,
                    context,
                } => (protocol, session_id, capabilities, context),
                WireHostMessage::Error { code, message } => {
                    return Err(Error::Host { code, message });
                }
                message => return Err(unexpected("init", message.name())),
            };
        if protocol.major != requested_protocol.major || protocol.minor > requested_protocol.minor {
            return Err(Error::Protocol(format!(
                "host negotiated unsupported version {protocol}; requested {requested_protocol}"
            )));
        }
        let session = SessionInfo {
            protocol_version: protocol,
            session_id,
            capabilities: capabilities.into_iter().collect(),
            context,
        };

        write_json(&mut stream, &WireClientMessage::InitComplete)?;
        let start = read_json(&mut stream, &mut reader)?;
        match start {
            WireHostMessage::Start { session_id } if session_id == session.session_id => {}
            WireHostMessage::Start { session_id } => {
                return Err(Error::Protocol(format!(
                    "start belongs to {session_id}, expected {}",
                    session.session_id
                )));
            }
            WireHostMessage::Error { code, message } => return Err(Error::Host { code, message }),
            other => return Err(unexpected("start", other.name())),
        }
        write_json(&mut stream, &WireClientMessage::Started)?;

        Ok(Self {
            stream,
            reader,
            session,
            next_request_id: 1,
            next_presentation_id: 1,
            pending_notifications: VecDeque::new(),
            closed_reason: None,
        })
    }

    /// Returns immutable negotiated session data, including initialization context.
    pub fn session(&self) -> &SessionInfo {
        &self.session
    }

    /// Returns whether the host granted a capability.
    pub fn has_capability(&self, capability: Capability) -> bool {
        self.session.capabilities.contains(capability)
    }

    /// Replaces notification subscriptions and returns the effective host set.
    pub fn subscribe(
        &mut self,
        subscriptions: impl IntoIterator<Item = Subscription>,
    ) -> Result<Vec<Subscription>> {
        let subscriptions: Vec<_> = subscriptions.into_iter().collect();
        for subscription in &subscriptions {
            match subscription {
                Subscription::RawInput | Subscription::Terminal => {
                    self.require(Capability::Input)?
                }
                Subscription::Timing => self.require(Capability::Timing)?,
            }
        }
        self.ensure_open()?;
        self.write_client_json(&WireClientMessage::Subscribe { subscriptions })?;
        loop {
            match self.read_running_message()? {
                WireHostMessage::Subscribed {
                    session_id,
                    subscriptions,
                } => {
                    self.expect_session(&session_id)?;
                    return Ok(subscriptions);
                }
                message => self.queue_notification(message)?,
            }
        }
    }

    /// Creates a root or child node in the session's implicit scene tree.
    ///
    /// `layer` is accepted only for roots; pass `None` for children. Text
    /// nodes return their host-computed terminal-cell metrics.
    pub fn create_node(
        &mut self,
        parent: Option<&NodeHandle>,
        layer: Option<SceneLayer>,
        node: NodeInput,
    ) -> Result<CreatedNode> {
        self.require_node_input(&node)?;
        match self.command(
            WireSceneCommand::CreateNode {
                parent: parent.cloned(),
                layer,
                node,
            },
            None,
        )? {
            WireCommandResult::Node { node, text_metrics } => {
                Ok(CreatedNode { node, text_metrics })
            }
            result => Err(unexpected_command_result("node", result.name())),
        }
    }

    /// Atomically changes selected node properties, hierarchy, visual data, or text.
    ///
    /// Returns fresh metrics when changing text or its style, otherwise `None`.
    pub fn set_node(
        &mut self,
        node: &NodeHandle,
        changes: NodeChanges,
    ) -> Result<Option<TextMetrics>> {
        self.require_node_changes(&changes)?;
        match self.command(
            WireSceneCommand::SetNode {
                node: node.clone(),
                changes,
            },
            None,
        )? {
            WireCommandResult::NodeUpdated { text_metrics } => Ok(text_metrics),
            result => Err(unexpected_command_result("node_updated", result.name())),
        }
    }

    /// Sends the newest transient geometry state without waiting for a command
    /// response. The host applies the complete packet atomically at a renderer
    /// frame and later emits a coalesced [`Notification::PresentationStatus`].
    ///
    /// This method never falls back to individual [`Client::set_node`] calls on
    /// a CLP v2.0 host. In that case it returns
    /// [`Error::CapabilityNotGranted`] for [`Capability::Presentation`].
    pub fn present_scene(
        &mut self,
        updates: impl IntoIterator<Item = PresentationUpdate>,
    ) -> Result<PresentationId> {
        let presentation_id = self.next_presentation_identifier();
        self.present_scene_with_id(presentation_id.clone(), updates)?;
        Ok(presentation_id)
    }

    /// Sends a frame-presentation packet with an application-supplied
    /// identifier. Use this when application tracing needs to correlate an
    /// exact local update with a later presentation status.
    pub fn present_scene_with_id(
        &mut self,
        presentation_id: PresentationId,
        updates: impl IntoIterator<Item = PresentationUpdate>,
    ) -> Result<()> {
        self.require(Capability::Scene)?;
        self.require(Capability::Presentation)?;
        self.ensure_open()?;
        let updates: Vec<_> = updates.into_iter().collect();
        if updates.is_empty() {
            return Err(Error::InvalidInput(
                "a presentation packet requires at least one update".to_owned(),
            ));
        }
        if updates.len() > MAX_PRESENTATION_UPDATES {
            return Err(Error::InvalidInput(format!(
                "a presentation packet may update at most {MAX_PRESENTATION_UPDATES} nodes"
            )));
        }
        if updates.iter().any(|update| update.properties.is_empty()) {
            return Err(Error::InvalidInput(
                "a presentation update must change at least one geometry property".to_owned(),
            ));
        }
        if presentation_id.as_str().is_empty()
            || presentation_id.as_str().len() > MAX_PRESENTATION_ID_BYTES
        {
            return Err(Error::InvalidInput(format!(
                "a presentation_id must contain between 1 and {MAX_PRESENTATION_ID_BYTES} bytes"
            )));
        }
        let mut nodes = BTreeSet::new();
        for update in &updates {
            if !nodes.insert(update.node.as_str()) {
                return Err(Error::InvalidInput(format!(
                    "a presentation packet updates {} more than once",
                    update.node.as_str()
                )));
            }
            for (name, value) in [
                ("x", update.properties.x),
                ("y", update.properties.y),
                ("width", update.properties.width),
                ("height", update.properties.height),
            ] {
                if value.is_some_and(|value| !value.is_finite()) {
                    return Err(Error::InvalidInput(format!(
                        "presentation {name} must be finite"
                    )));
                }
            }
        }
        let message = WireClientMessage::PresentScene {
            presentation_id,
            updates,
        };
        let payload = serde_json::to_vec(&message)?;
        if payload.len() > MAX_PRESENTATION_PACKET_BYTES {
            return Err(Error::InvalidInput(format!(
                "a presentation packet may not exceed {MAX_PRESENTATION_PACKET_BYTES} bytes"
            )));
        }
        self.write_client_frame(b'J', &payload)
    }

    /// Removes a node and all of its descendants.
    pub fn remove_node(&mut self, node: &NodeHandle) -> Result<()> {
        self.require(Capability::Scene)?;
        Self::expect_acknowledged(
            self.command(WireSceneCommand::RemoveNode { node: node.clone() }, None)?,
        )
    }

    /// Uploads RGBA8 pixels or encoded GIF bytes and returns an opaque image handle.
    pub fn upload_image(&mut self, image: ImageUpload<'_>) -> Result<ImageHandle> {
        self.require(Capability::Images)?;
        let (source, bytes) = match image {
            ImageUpload::Rgba8 {
                width,
                height,
                pixels,
            } => {
                let expected_length = rgba8_byte_len(width, height)?;
                if pixels.len() != expected_length {
                    return Err(Error::InvalidInput(format!(
                        "RGBA8 image data has {} bytes, expected {expected_length}",
                        pixels.len()
                    )));
                }
                (WireImageSource::Rgba8 { width, height }, pixels)
            }
            ImageUpload::Gif { bytes } => (
                WireImageSource::Gif {
                    byte_length: bytes.len(),
                },
                bytes,
            ),
        };
        match self.command(WireSceneCommand::UploadImage { source }, Some(bytes))? {
            WireCommandResult::Image { image } => Ok(image),
            result => Err(unexpected_command_result("image", result.name())),
        }
    }

    /// Removes an unreferenced session-owned image.
    pub fn remove_image(&mut self, image: &ImageHandle) -> Result<()> {
        self.require(Capability::Images)?;
        Self::expect_acknowledged(self.command(
            WireSceneCommand::RemoveImage {
                image: image.clone(),
            },
            None,
        )?)
    }

    /// Updates this session's application window title and/or static icon.
    ///
    /// Omitted fields preserve this session's previous claim; [`MetadataField::Clear`]
    /// serializes as JSON `null`. Clearing both fields removes the claim and
    /// lets Chymeral restore the newest remaining application claim or its
    /// terminal-derived fallback. The host rejects an icon that is not a
    /// session-owned static RGBA8 image or exceeds its icon limits.
    pub fn set_window_metadata(
        &mut self,
        metadata: WindowMetadataPatch,
    ) -> Result<WindowMetadataStatus> {
        self.require(Capability::WindowMetadata)?;
        validate_window_metadata(&metadata)?;
        let WindowMetadataPatch { title, icon } = metadata;
        match self.command(WireSceneCommand::SetWindowMetadata { title, icon }, None)? {
            WireCommandResult::WindowMetadata { icon_supported } => {
                Ok(WindowMetadataStatus { icon_supported })
            }
            result => Err(unexpected_command_result("window_metadata", result.name())),
        }
    }

    /// Returns whether any enabled same-session node overlaps `node`.
    pub fn is_overlapping(&mut self, node: &NodeHandle) -> Result<bool> {
        self.require(Capability::Scene)?;
        self.require(Capability::Overlap)?;
        match self.command(WireSceneCommand::IsOverlapping { node: node.clone() }, None)? {
            WireCommandResult::IsOverlapping { overlapping } => Ok(overlapping),
            result => Err(unexpected_command_result("is_overlapping", result.name())),
        }
    }

    /// Returns enabled same-session nodes currently overlapping `node`.
    pub fn get_overlaps(&mut self, node: &NodeHandle) -> Result<Vec<NodeHandle>> {
        self.require(Capability::Scene)?;
        self.require(Capability::Overlap)?;
        match self.command(WireSceneCommand::GetOverlaps { node: node.clone() }, None)? {
            WireCommandResult::Overlaps { nodes } => Ok(nodes),
            result => Err(unexpected_command_result("overlaps", result.name())),
        }
    }

    /// Blocks until a pending or newly received host notification arrives.
    pub fn next_notification(&mut self) -> Result<Notification> {
        if let Some(notification) = self.pending_notifications.pop_front() {
            return Ok(notification);
        }
        loop {
            let message = self.read_running_message()?;
            if let Some(notification) = self.notification_from_message(message)? {
                return Ok(notification);
            }
        }
    }

    /// Returns the next notification if one is already available without blocking.
    pub fn try_next_notification(&mut self) -> Result<Option<Notification>> {
        if let Some(notification) = self.pending_notifications.pop_front() {
            return Ok(Some(notification));
        }
        self.stream.set_nonblocking(true)?;
        let result = self.reader.try_read_json(&mut self.stream);
        let restore = self.stream.set_nonblocking(false);
        restore?;
        let message = result?;
        match message {
            Some(message) => self.notification_from_message(message),
            None => Ok(None),
        }
    }

    /// Reports a client fault and waits for the terminal `closed` message.
    pub fn report_fault(
        &mut self,
        phase: ProgramPhase,
        message: impl Into<String>,
    ) -> Result<CloseReason> {
        self.close_with(WireClientMessage::Fault {
            phase,
            message: message.into(),
        })
    }

    /// Requests clean session closure and waits for the terminal `closed` message.
    pub fn close(&mut self) -> Result<CloseReason> {
        self.close_with(WireClientMessage::Close)
    }

    fn close_with(&mut self, message: WireClientMessage) -> Result<CloseReason> {
        if let Some(reason) = &self.closed_reason {
            return Ok(reason.clone());
        }
        self.write_client_json(&message)?;
        loop {
            match self.read_running_message()? {
                WireHostMessage::Closed { session_id, reason } => {
                    self.expect_session(&session_id)?;
                    self.closed_reason = Some(reason.clone());
                    return Ok(reason);
                }
                message => self.queue_notification(message)?,
            }
        }
    }

    fn command(
        &mut self,
        command: WireSceneCommand,
        binary_payload: Option<&[u8]>,
    ) -> Result<WireCommandResult> {
        self.ensure_open()?;
        let request_id = self.next_request_id();
        self.write_client_json(&WireClientMessage::Command {
            request_id: request_id.clone(),
            command: Box::new(command),
        })?;
        if let Some(binary_payload) = binary_payload {
            for fragment in binary_payload.chunks(BINARY_FRAGMENT_BYTES) {
                self.write_client_frame(b'B', fragment)?;
            }
        }
        loop {
            match self.read_running_message()? {
                WireHostMessage::Result {
                    request_id: received,
                    result,
                } if received == request_id => return Ok(result),
                WireHostMessage::CommandError {
                    request_id: received,
                    code,
                    message,
                } if received == request_id => {
                    return Err(Error::Command {
                        request_id,
                        code,
                        message,
                    });
                }
                WireHostMessage::Result {
                    request_id: received,
                    ..
                }
                | WireHostMessage::CommandError {
                    request_id: received,
                    ..
                } => {
                    return Err(Error::Protocol(format!(
                        "received response for {received} while waiting for {request_id}"
                    )));
                }
                message => self.queue_notification(message)?,
            }
        }
    }

    fn read_running_message(&mut self) -> Result<WireHostMessage> {
        self.ensure_open()?;
        read_json(&mut self.stream, &mut self.reader)
    }

    fn write_client_json(&mut self, message: &impl Serialize) -> Result<()> {
        let payload = serde_json::to_vec(message)?;
        self.write_client_frame(b'J', &payload)
    }

    fn write_client_frame(&mut self, kind: u8, payload: &[u8]) -> Result<()> {
        let length = payload
            .len()
            .checked_add(1)
            .ok_or_else(|| Error::InvalidInput("frame length overflow".to_owned()))?;
        if length > MAX_FRAME_BYTES {
            return Err(Error::InvalidInput(format!(
                "frame length {length} exceeds {MAX_FRAME_BYTES}"
            )));
        }
        let length = u32::try_from(length)
            .map_err(|_| Error::InvalidInput("frame length exceeds u32".to_owned()))?;
        self.write_client_bytes(&length.to_be_bytes())?;
        self.write_client_bytes(&[kind])?;
        self.write_client_bytes(payload)
    }

    fn write_client_bytes(&mut self, mut bytes: &[u8]) -> Result<()> {
        let deadline = Instant::now() + IO_STALL_TIMEOUT;
        while !bytes.is_empty() {
            match self.stream.write(bytes) {
                Ok(0) => {
                    self.pump_host_while_writing()?;
                    wait_for_write_capacity(deadline)?;
                }
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if read_would_block(&error) => {
                    self.pump_host_while_writing()?;
                    wait_for_write_capacity(deadline)?;
                }
                Err(error) => return Err(Error::Io(error)),
            }
        }
        Ok(())
    }

    fn pump_host_while_writing(&mut self) -> Result<()> {
        self.stream.set_nonblocking(true)?;
        let result = self.reader.try_read_json(&mut self.stream);
        let restore = self.stream.set_nonblocking(false);
        restore?;
        if let Some(message) = result? {
            self.queue_notification(message)?;
        }
        Ok(())
    }

    fn notification_from_message(
        &mut self,
        message: WireHostMessage,
    ) -> Result<Option<Notification>> {
        match message {
            WireHostMessage::Event { session_id, event } => {
                self.expect_session(&session_id)?;
                Ok(Some(Notification::Event(event)))
            }
            WireHostMessage::Tick { session_id, dt_ns } => {
                self.expect_session(&session_id)?;
                Ok(Some(Notification::Tick(parse_duration(dt_ns)?)))
            }
            WireHostMessage::Frame { session_id, dt_ns } => {
                self.expect_session(&session_id)?;
                Ok(Some(Notification::Frame(parse_duration(dt_ns)?)))
            }
            WireHostMessage::Resize {
                session_id,
                physical_width,
                physical_height,
                logical_width,
                logical_height,
                scale_factor,
            } => {
                self.expect_session(&session_id)?;
                Ok(Some(Notification::Resize(Resize {
                    physical_width,
                    physical_height,
                    logical_width,
                    logical_height,
                    scale_factor,
                })))
            }
            WireHostMessage::DeliveryGap {
                session_id,
                categories,
            } => {
                self.expect_session(&session_id)?;
                Ok(Some(Notification::DeliveryGap(categories)))
            }
            WireHostMessage::PresentationStatus {
                session_id,
                presentation_id,
                status,
                code,
                message,
            } => {
                self.expect_session(&session_id)?;
                Ok(Some(Notification::PresentationStatus(PresentationStatus {
                    presentation_id,
                    status,
                    code,
                    message,
                })))
            }
            WireHostMessage::Closing { session_id, reason } => {
                self.expect_session(&session_id)?;
                Ok(Some(Notification::Closing(reason)))
            }
            WireHostMessage::Closed { session_id, reason } => {
                self.expect_session(&session_id)?;
                self.closed_reason = Some(reason.clone());
                Ok(Some(Notification::Closed(reason)))
            }
            WireHostMessage::Error { code, message } => Err(Error::Host { code, message }),
            other => Err(unexpected("notification", other.name())),
        }
    }

    fn queue_notification(&mut self, message: WireHostMessage) -> Result<()> {
        match self.notification_from_message(message)? {
            Some(Notification::Closed(reason)) => return Err(Error::SessionClosed(reason)),
            Some(notification) => self.pending_notifications.push_back(notification),
            None => {}
        }
        Ok(())
    }

    fn expect_session(&self, received: &SessionId) -> Result<()> {
        if received == &self.session.session_id {
            Ok(())
        } else {
            Err(Error::Protocol(format!(
                "message belongs to {received}, expected {}",
                self.session.session_id
            )))
        }
    }

    fn require(&self, capability: Capability) -> Result<()> {
        if self.has_capability(capability) {
            Ok(())
        } else {
            Err(Error::CapabilityNotGranted(capability))
        }
    }

    fn require_node_input(&self, node: &NodeInput) -> Result<()> {
        self.require(Capability::Scene)?;
        match node {
            NodeInput::Empty { properties } | NodeInput::Rectangle { properties, .. } => {
                self.require_node_properties(properties)
            }
            NodeInput::Image {
                properties,
                contact_shape,
                ..
            } => {
                self.require(Capability::Images)?;
                self.validate_image_contact_shape(*contact_shape)?;
                self.require_node_properties(properties)
            }
            NodeInput::Text { properties, .. } => {
                self.require(Capability::Text)?;
                self.require_text_properties(properties)
            }
        }
    }

    fn require_node_changes(&self, changes: &NodeChanges) -> Result<()> {
        self.require(Capability::Scene)?;
        if let Some(properties) = changes.properties {
            if properties.overlap_enabled.is_some() {
                self.require(Capability::Overlap)?;
            }
            if properties.external_drop_enabled.is_some() {
                self.require(Capability::ExternalDrop)?;
            }
        }
        if let Some(NodeVisual::Image { contact_shape, .. }) = &changes.visual {
            self.require(Capability::Images)?;
            self.validate_image_contact_shape(*contact_shape)?;
        }
        if changes.text.is_some() || changes.text_style.is_some() {
            self.require(Capability::Text)?;
        }
        Ok(())
    }

    fn require_node_properties(&self, properties: &NodeProperties) -> Result<()> {
        if properties.overlap_enabled {
            self.require(Capability::Overlap)?;
        }
        if properties.external_drop_enabled {
            self.require(Capability::ExternalDrop)?;
        }
        Ok(())
    }

    fn require_text_properties(&self, properties: &TextNodeProperties) -> Result<()> {
        if properties.overlap_enabled {
            self.require(Capability::Overlap)?;
        }
        if properties.external_drop_enabled {
            self.require(Capability::ExternalDrop)?;
        }
        Ok(())
    }

    fn validate_image_contact_shape(&self, contact_shape: ImageContactShape) -> Result<()> {
        if matches!(contact_shape, ImageContactShape::Alpha { threshold: 0 }) {
            return Err(Error::InvalidInput(
                "image alpha contact threshold must be between 1 and 255".to_owned(),
            ));
        }
        Ok(())
    }

    fn expect_acknowledged(result: WireCommandResult) -> Result<()> {
        if matches!(result, WireCommandResult::Acknowledged) {
            Ok(())
        } else {
            Err(unexpected_command_result("acknowledged", result.name()))
        }
    }

    fn ensure_open(&self) -> Result<()> {
        match &self.closed_reason {
            Some(reason) => Err(Error::SessionClosed(reason.clone())),
            None => Ok(()),
        }
    }

    fn next_request_id(&mut self) -> String {
        let request_id = format!("sdk-{}", self.next_request_id);
        self.next_request_id = self.next_request_id.checked_add(1).unwrap_or(1);
        request_id
    }

    fn next_presentation_identifier(&mut self) -> PresentationId {
        let presentation_id =
            PresentationId(format!("sdk-presentation-{}", self.next_presentation_id));
        self.next_presentation_id = self.next_presentation_id.checked_add(1).unwrap_or(1);
        presentation_id
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireClientMessage {
    Hello {
        protocol: ProtocolVersion,
        token: String,
        capabilities: Vec<Capability>,
    },
    InitComplete,
    Started,
    Fault {
        phase: ProgramPhase,
        message: String,
    },
    Close,
    Command {
        request_id: String,
        command: Box<WireSceneCommand>,
    },
    PresentScene {
        presentation_id: PresentationId,
        updates: Vec<PresentationUpdate>,
    },
    Subscribe {
        subscriptions: Vec<Subscription>,
    },
}

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum WireSceneCommand {
    CreateNode {
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<NodeHandle>,
        #[serde(skip_serializing_if = "Option::is_none")]
        layer: Option<SceneLayer>,
        node: NodeInput,
    },
    SetNode {
        node: NodeHandle,
        changes: NodeChanges,
    },
    RemoveNode {
        node: NodeHandle,
    },
    UploadImage {
        source: WireImageSource,
    },
    RemoveImage {
        image: ImageHandle,
    },
    IsOverlapping {
        node: NodeHandle,
    },
    GetOverlaps {
        node: NodeHandle,
    },
    SetWindowMetadata {
        #[serde(skip_serializing_if = "MetadataField::is_preserve")]
        title: MetadataField<String>,
        #[serde(skip_serializing_if = "MetadataField::is_preserve")]
        icon: MetadataField<ImageHandle>,
    },
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireImageSource {
    Rgba8 { width: u32, height: u32 },
    Gif { byte_length: usize },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireHostMessage {
    Init {
        protocol: ProtocolVersion,
        session_id: SessionId,
        capabilities: Vec<Capability>,
        context: InitContext,
    },
    Start {
        session_id: SessionId,
    },
    Event {
        session_id: SessionId,
        event: Event,
    },
    Tick {
        session_id: SessionId,
        dt_ns: String,
    },
    Frame {
        session_id: SessionId,
        dt_ns: String,
    },
    Resize {
        session_id: SessionId,
        physical_width: u32,
        physical_height: u32,
        logical_width: f64,
        logical_height: f64,
        scale_factor: f64,
    },
    Subscribed {
        session_id: SessionId,
        subscriptions: Vec<Subscription>,
    },
    DeliveryGap {
        session_id: SessionId,
        categories: Vec<DeliveryCategory>,
    },
    PresentationStatus {
        session_id: SessionId,
        presentation_id: PresentationId,
        status: PresentationState,
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        message: Option<String>,
    },
    Closing {
        session_id: SessionId,
        reason: CloseReason,
    },
    Closed {
        session_id: SessionId,
        reason: CloseReason,
    },
    Error {
        code: String,
        message: String,
    },
    Result {
        request_id: String,
        result: WireCommandResult,
    },
    CommandError {
        request_id: String,
        code: String,
        message: String,
    },
}

impl WireHostMessage {
    fn name(&self) -> &'static str {
        match self {
            Self::Init { .. } => "init",
            Self::Start { .. } => "start",
            Self::Event { .. } => "event",
            Self::Tick { .. } => "tick",
            Self::Frame { .. } => "frame",
            Self::Resize { .. } => "resize",
            Self::Subscribed { .. } => "subscribed",
            Self::DeliveryGap { .. } => "delivery_gap",
            Self::PresentationStatus { .. } => "presentation_status",
            Self::Closing { .. } => "closing",
            Self::Closed { .. } => "closed",
            Self::Error { .. } => "error",
            Self::Result { .. } => "result",
            Self::CommandError { .. } => "command_error",
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireCommandResult {
    Node {
        node: NodeHandle,
        #[serde(default)]
        text_metrics: Option<TextMetrics>,
    },
    NodeUpdated {
        #[serde(default)]
        text_metrics: Option<TextMetrics>,
    },
    Image {
        image: ImageHandle,
    },
    IsOverlapping {
        overlapping: bool,
    },
    Overlaps {
        nodes: Vec<NodeHandle>,
    },
    WindowMetadata {
        icon_supported: bool,
    },
    Acknowledged,
}

impl WireCommandResult {
    fn name(&self) -> &'static str {
        match self {
            Self::Node { .. } => "node",
            Self::NodeUpdated { .. } => "node_updated",
            Self::Image { .. } => "image",
            Self::IsOverlapping { .. } => "is_overlapping",
            Self::Overlaps { .. } => "overlaps",
            Self::WindowMetadata { .. } => "window_metadata",
            Self::Acknowledged => "acknowledged",
        }
    }
}

#[derive(Default)]
struct FrameReader {
    bytes: Vec<u8>,
}

enum WireFrame {
    Json(Vec<u8>),
    Binary,
}

impl FrameReader {
    fn read_json(&mut self, stream: &mut LocalSocketStream) -> Result<WireHostMessage> {
        loop {
            if let Some(frame) = self.take_complete_frame()? {
                return decode_json(frame);
            }
            let mut received = [0_u8; 8192];
            match stream.read(&mut received) {
                Ok(0) => {
                    if empty_read_is_disconnected(stream)? {
                        return Err(Error::Protocol("the CLP transport disconnected".to_owned()));
                    }
                    thread::sleep(IO_RETRY_INTERVAL);
                }
                Ok(count) => self.bytes.extend_from_slice(&received[..count]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if read_disconnected(&error) => {
                    return Err(Error::Protocol("the CLP transport disconnected".to_owned()));
                }
                Err(error) if read_would_block(&error) => thread::sleep(IO_RETRY_INTERVAL),
                Err(error) => return Err(Error::Io(error)),
            }
        }
    }

    fn try_read_json(&mut self, stream: &mut LocalSocketStream) -> Result<Option<WireHostMessage>> {
        loop {
            if let Some(frame) = self.take_complete_frame()? {
                return decode_json(frame).map(Some);
            }
            let mut received = [0_u8; 8192];
            match stream.read(&mut received) {
                Ok(0) => {
                    if empty_read_is_disconnected(stream)? {
                        return Err(Error::Protocol("the CLP transport disconnected".to_owned()));
                    }
                    return Ok(None);
                }
                Ok(count) => self.bytes.extend_from_slice(&received[..count]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if read_disconnected(&error) => {
                    return Err(Error::Protocol("the CLP transport disconnected".to_owned()));
                }
                Err(error) if read_would_block(&error) => return Ok(None),
                Err(error) => return Err(Error::Io(error)),
            }
        }
    }

    fn take_complete_frame(&mut self) -> Result<Option<WireFrame>> {
        if self.bytes.len() < 4 {
            return Ok(None);
        }
        let length = usize::try_from(u32::from_be_bytes(
            self.bytes[..4].try_into().expect("frame length is present"),
        ))
        .map_err(|_| Error::Protocol("frame length does not fit this platform".to_owned()))?;
        if length > MAX_FRAME_BYTES {
            return Err(Error::Protocol(format!(
                "frame length {length} exceeds {MAX_FRAME_BYTES}"
            )));
        }
        if length == 0 {
            return Err(Error::Protocol(
                "frame is missing its kind discriminator".to_owned(),
            ));
        }
        let frame_end = 4_usize
            .checked_add(length)
            .ok_or_else(|| Error::Protocol("frame length overflows this platform".to_owned()))?;
        if self.bytes.len() < frame_end {
            return Ok(None);
        }
        let frame = match self.bytes[4] {
            b'J' => WireFrame::Json(self.bytes[5..frame_end].to_vec()),
            b'B' => WireFrame::Binary,
            _ => Err(Error::Protocol(
                "frame has an unknown kind discriminator".to_owned(),
            ))?,
        };
        self.bytes.drain(..frame_end);
        Ok(Some(frame))
    }
}

fn read_json(stream: &mut LocalSocketStream, reader: &mut FrameReader) -> Result<WireHostMessage> {
    reader.read_json(stream)
}

fn decode_json(frame: WireFrame) -> Result<WireHostMessage> {
    let WireFrame::Json(payload) = frame else {
        return Err(Error::Protocol(
            "expected a JSON frame from the host".to_owned(),
        ));
    };
    Ok(serde_json::from_slice(&payload)?)
}

fn write_json(stream: &mut LocalSocketStream, message: &impl Serialize) -> Result<()> {
    write_frame(stream, b'J', &serde_json::to_vec(message)?)
}

fn write_frame(stream: &mut LocalSocketStream, kind: u8, payload: &[u8]) -> Result<()> {
    let length = payload
        .len()
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("frame length overflow".to_owned()))?;
    if length > MAX_FRAME_BYTES {
        return Err(Error::InvalidInput(format!(
            "frame length {length} exceeds {MAX_FRAME_BYTES}"
        )));
    }
    let length = u32::try_from(length)
        .map_err(|_| Error::InvalidInput("frame length exceeds u32".to_owned()))?;
    write_all_resilient(stream, &length.to_be_bytes())?;
    write_all_resilient(stream, &[kind])?;
    write_all_resilient(stream, payload)
}

fn write_all_resilient(stream: &mut LocalSocketStream, mut bytes: &[u8]) -> Result<()> {
    let deadline = Instant::now() + IO_STALL_TIMEOUT;
    while !bytes.is_empty() {
        match stream.write(bytes) {
            Ok(0) => wait_for_write_capacity(deadline)?,
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if read_would_block(&error) => wait_for_write_capacity(deadline)?,
            Err(error) => return Err(Error::Io(error)),
        }
    }
    Ok(())
}

fn wait_for_write_capacity(deadline: Instant) -> Result<()> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "the CLP peer stopped reading",
            ))
        })?;
    thread::sleep(remaining.min(IO_RETRY_INTERVAL));
    Ok(())
}

#[cfg(target_os = "windows")]
fn empty_read_is_disconnected(stream: &LocalSocketStream) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::{
        Foundation::{ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED},
        System::Pipes::PeekNamedPipe,
    };

    let LocalSocketStream::NamedPipe(pipe) = stream;
    // SAFETY: `pipe` owns a live named-pipe handle for the duration of this
    // call. A zero-length peek permits null optional output pointers and does
    // not read or write a caller-provided buffer.
    let available = unsafe {
        PeekNamedPipe(
            pipe.inner().as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if available != 0 {
        return Ok(false);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(code)
            if code == ERROR_BROKEN_PIPE as i32 || code == ERROR_PIPE_NOT_CONNECTED as i32 =>
        {
            Ok(true)
        }
        _ => Err(error),
    }
}

#[cfg(not(target_os = "windows"))]
fn empty_read_is_disconnected(_: &LocalSocketStream) -> io::Result<bool> {
    Ok(true)
}

fn read_disconnected(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset | io::ErrorKind::UnexpectedEof
    )
}

fn read_would_block(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        error.raw_os_error() == Some(232)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

fn parse_duration(value: String) -> Result<Duration> {
    let nanoseconds = value
        .parse::<u64>()
        .map_err(|_| Error::Protocol(format!("invalid duration nanoseconds {value:?}")))?;
    Ok(Duration::from_nanos(nanoseconds))
}

fn rgba8_byte_len(width: u32, height: u32) -> Result<usize> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| Error::InvalidInput("texture dimensions overflow".to_owned()))?;
    let bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| Error::InvalidInput("RGBA8 texture byte count overflows".to_owned()))?;
    usize::try_from(bytes).map_err(|_| {
        Error::InvalidInput("texture byte count does not fit this platform".to_owned())
    })
}

fn validate_window_metadata(metadata: &WindowMetadataPatch) -> Result<()> {
    if metadata.is_empty() {
        return Err(Error::InvalidInput(
            "a window metadata update must set or clear at least one field".to_owned(),
        ));
    }
    let MetadataField::Set(title) = &metadata.title else {
        return Ok(());
    };
    if title.is_empty() {
        return Err(Error::InvalidInput(
            "a window title must not be empty".to_owned(),
        ));
    }
    if title.len() > MAX_WINDOW_TITLE_BYTES {
        return Err(Error::InvalidInput(format!(
            "a window title may not exceed {MAX_WINDOW_TITLE_BYTES} UTF-8 bytes"
        )));
    }
    if title.chars().any(char::is_control) {
        return Err(Error::InvalidInput(
            "a window title may not contain control characters".to_owned(),
        ));
    }
    Ok(())
}

fn unexpected(expected: &str, received: &str) -> Error {
    Error::Protocol(format!("expected {expected}, received {received}"))
}

fn unexpected_command_result(expected: &str, received: &str) -> Error {
    Error::Protocol(format!(
        "expected {expected} command result, received {received}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_a_v2_hello_with_deterministic_capabilities() {
        let message = WireClientMessage::Hello {
            protocol: CLP_PROTOCOL_VERSION,
            token: "secret".to_owned(),
            capabilities: Capabilities::from([Capability::Timing, Capability::Scene])
                .iter()
                .collect(),
        };
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            json!({
                "type": "hello",
                "protocol": { "major": 2, "minor": 1 },
                "token": "secret",
                "capabilities": ["scene", "timing"]
            })
        );
    }

    #[test]
    fn serializes_a_frame_presentation_without_a_command_identifier() {
        let message = WireClientMessage::PresentScene {
            presentation_id: PresentationId::new("application-frame-7").unwrap(),
            updates: vec![
                PresentationUpdate {
                    node: NodeHandle("node-4".to_owned()),
                    properties: PresentationPropertiesPatch {
                        x: Some(48.0),
                        y: Some(24.0),
                        ..PresentationPropertiesPatch::default()
                    },
                },
                PresentationUpdate {
                    node: NodeHandle("node-5".to_owned()),
                    properties: PresentationPropertiesPatch {
                        width: Some(32.0),
                        height: Some(16.0),
                        ..PresentationPropertiesPatch::default()
                    },
                },
            ],
        };
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            json!({
                "type": "present_scene",
                "presentation_id": "application-frame-7",
                "updates": [
                    { "node": "node-4", "properties": { "x": 48.0, "y": 24.0 } },
                    { "node": "node-5", "properties": { "width": 32.0, "height": 16.0 } }
                ]
            })
        );
    }

    #[test]
    fn decodes_presentation_statuses() {
        let message: WireHostMessage = serde_json::from_value(json!({
            "type": "presentation_status",
            "session_id": "session-1",
            "presentation_id": "application-frame-7",
            "status": "rejected",
            "code": "invalid_node_change",
            "message": "node-4 has intrinsic text bounds"
        }))
        .unwrap();
        assert!(matches!(
            message,
            WireHostMessage::PresentationStatus {
                presentation_id,
                status: PresentationState::Rejected,
                code: Some(code),
                message: Some(message),
                ..
            } if presentation_id.as_str() == "application-frame-7"
                && code == "invalid_node_change"
                && message == "node-4 has intrinsic text bounds"
        ));
    }

    #[test]
    fn serializes_an_overlap_query_with_an_opaque_handle() {
        let message = WireClientMessage::Command {
            request_id: "sdk-7".to_owned(),
            command: Box::new(WireSceneCommand::GetOverlaps {
                node: NodeHandle("node-42".to_owned()),
            }),
        };
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            json!({
                "type": "command",
                "request_id": "sdk-7",
                "command": { "op": "get_overlaps", "node": "node-42" }
            })
        );
    }

    #[test]
    fn serializes_an_external_drop_opt_in_node() {
        let message = WireClientMessage::Command {
            request_id: "sdk-8".to_owned(),
            command: Box::new(WireSceneCommand::CreateNode {
                parent: None,
                layer: Some(SceneLayer::Overlay),
                node: NodeInput::Empty {
                    properties: NodeProperties {
                        external_drop_enabled: true,
                        ..NodeProperties::new(1.0, 2.0, 30.0, 40.0)
                    },
                },
            }),
        };
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            json!({
                "type": "command",
                "request_id": "sdk-8",
                "command": {
                    "op": "create_node",
                    "layer": "overlay",
                    "node": {
                        "kind": "empty",
                        "properties": {
                            "x": 1.0,
                            "y": 2.0,
                            "width": 30.0,
                            "height": 40.0,
                            "visible": true,
                            "clip_children": false,
                            "z_index": 0,
                            "overlap_enabled": false,
                            "external_drop_enabled": true,
                            "input_passthrough": false
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn serializes_intrinsic_text_commands() {
        let message = WireClientMessage::Command {
            request_id: "sdk-9".to_owned(),
            command: Box::new(WireSceneCommand::CreateNode {
                parent: None,
                layer: Some(SceneLayer::Overlay),
                node: NodeInput::Text {
                    properties: TextNodeProperties {
                        overlap_enabled: true,
                        ..TextNodeProperties::new(24.0, 48.0)
                    },
                    text: "a\tb\n界".to_owned(),
                    style: TextStyle {
                        color: Some([30, 200, 255, 255]),
                        weight: TextWeight::Bold,
                        align: TextAlign::Center,
                        ..TextStyle::default()
                    },
                },
            }),
        };
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            json!({
                "type": "command",
                "request_id": "sdk-9",
                "command": {
                    "op": "create_node",
                    "layer": "overlay",
                    "node": {
                        "kind": "text",
                        "properties": {
                            "x": 24.0,
                            "y": 48.0,
                            "visible": true,
                            "clip_children": false,
                            "z_index": 0,
                            "overlap_enabled": true,
                            "external_drop_enabled": false,
                            "input_passthrough": false
                        },
                        "text": "a\tb\n界",
                        "style": {
                            "color": [30, 200, 255, 255],
                            "weight": "bold",
                            "align": "center"
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn serializes_an_atomic_visual_and_parent_update() {
        let message = WireClientMessage::Command {
            request_id: "sdk-10".to_owned(),
            command: Box::new(WireSceneCommand::SetNode {
                node: NodeHandle("node-4".to_owned()),
                changes: NodeChanges {
                    parent: Some(NodeParent::Root),
                    visual: Some(NodeVisual::Image {
                        image: ImageHandle("image-7".to_owned()),
                        opacity: 0.75,
                        contact_shape: ImageContactShape::Bounds,
                    }),
                    ..NodeChanges::default()
                },
            }),
        };
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            json!({
                "type": "command",
                "request_id": "sdk-10",
                "command": {
                    "op": "set_node",
                    "node": "node-4",
                    "changes": {
                        "parent": { "kind": "root" },
                        "visual": {
                            "kind": "image",
                            "image": "image-7",
                            "opacity": 0.75,
                            "contact_shape": { "kind": "bounds" }
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn serializes_alpha_contact_and_input_passthrough() {
        let message = WireClientMessage::Command {
            request_id: "sdk-11".to_owned(),
            command: Box::new(WireSceneCommand::CreateNode {
                parent: None,
                layer: Some(SceneLayer::Overlay),
                node: NodeInput::Image {
                    properties: NodeProperties {
                        input_passthrough: true,
                        ..NodeProperties::new(4.0, 8.0, 32.0, 16.0)
                    },
                    image: ImageHandle("image-7".to_owned()),
                    opacity: 1.0,
                    contact_shape: ImageContactShape::Alpha { threshold: 128 },
                },
            }),
        };
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            json!({
                "type": "command",
                "request_id": "sdk-11",
                "command": {
                    "op": "create_node",
                    "layer": "overlay",
                    "node": {
                        "kind": "image",
                        "properties": {
                            "x": 4.0,
                            "y": 8.0,
                            "width": 32.0,
                            "height": 16.0,
                            "visible": true,
                            "clip_children": false,
                            "z_index": 0,
                            "overlap_enabled": false,
                            "external_drop_enabled": false,
                            "input_passthrough": true
                        },
                        "image": "image-7",
                        "opacity": 1.0,
                        "contact_shape": { "kind": "alpha", "threshold": 128 }
                    }
                }
            })
        );

        let patch = NodePropertiesPatch {
            input_passthrough: Some(false),
            ..NodePropertiesPatch::default()
        };
        assert_eq!(
            serde_json::to_value(patch).unwrap(),
            json!({ "input_passthrough": false })
        );
    }

    #[test]
    fn serializes_rgba8_and_gif_image_sources() {
        let rgba8 = WireSceneCommand::UploadImage {
            source: WireImageSource::Rgba8 {
                width: 2,
                height: 3,
            },
        };
        let gif = WireSceneCommand::UploadImage {
            source: WireImageSource::Gif { byte_length: 128 },
        };
        assert_eq!(
            serde_json::to_value(rgba8).unwrap(),
            json!({
                "op": "upload_image",
                "source": { "kind": "rgba8", "width": 2, "height": 3 }
            })
        );
        assert_eq!(
            serde_json::to_value(gif).unwrap(),
            json!({
                "op": "upload_image",
                "source": { "kind": "gif", "byte_length": 128 }
            })
        );
    }

    #[test]
    fn serializes_window_metadata_preservation_and_clearing() {
        let set_title = WireSceneCommand::SetWindowMetadata {
            title: MetadataField::Set("Chymeral SDK probe".to_owned()),
            icon: MetadataField::Preserve,
        };
        let clear = WireSceneCommand::SetWindowMetadata {
            title: MetadataField::Clear,
            icon: MetadataField::Clear,
        };
        assert_eq!(
            serde_json::to_value(set_title).unwrap(),
            json!({
                "op": "set_window_metadata",
                "title": "Chymeral SDK probe"
            })
        );
        assert_eq!(
            serde_json::to_value(clear).unwrap(),
            json!({
                "op": "set_window_metadata",
                "title": null,
                "icon": null
            })
        );
    }

    #[test]
    fn validates_window_metadata_input_and_decodes_its_result() {
        assert!(matches!(
            validate_window_metadata(&WindowMetadataPatch::default()),
            Err(Error::InvalidInput(_))
        ));
        assert!(matches!(
            validate_window_metadata(&WindowMetadataPatch {
                title: MetadataField::Set("line one\nline two".to_owned()),
                ..WindowMetadataPatch::default()
            }),
            Err(Error::InvalidInput(_))
        ));
        let message: WireHostMessage = serde_json::from_value(json!({
            "type": "result",
            "request_id": "sdk-10",
            "result": { "kind": "window_metadata", "icon_supported": false }
        }))
        .unwrap();
        assert!(matches!(
            message,
            WireHostMessage::Result {
                result: WireCommandResult::WindowMetadata {
                    icon_supported: false
                },
                ..
            }
        ));
    }

    #[test]
    fn decodes_text_node_results() {
        let message: WireHostMessage = serde_json::from_value(json!({
            "type": "result",
            "request_id": "sdk-9",
            "result": {
                "kind": "node",
                "node": "node-4",
                "text_metrics": {
                    "columns": 9,
                    "rows": 2,
                    "width": 86.4,
                    "height": 40.0
                }
            }
        }))
        .unwrap();
        let WireHostMessage::Result {
            result:
                WireCommandResult::Node {
                    node,
                    text_metrics: Some(metrics),
                },
            ..
        } = message
        else {
            panic!("unexpected command result");
        };
        assert_eq!(node.as_str(), "node-4");
        assert_eq!(metrics.columns, 9);
        assert_eq!(metrics.rows, 2);
        assert_eq!(metrics.width, 86.4);
        assert_eq!(metrics.height, 40.0);
    }

    #[test]
    fn decodes_overlap_and_close_notifications() {
        let event: WireHostMessage = serde_json::from_value(json!({
            "type": "event",
            "session_id": "session-1",
            "event": { "kind": "scene", "event": { "kind": "overlap_enter", "first": "node-2", "second": "node-3" } }
        }))
        .unwrap();
        let WireHostMessage::Event {
            event:
                Event::Scene {
                    event: SceneEvent::OverlapEnter { first, second },
                },
            ..
        } = event
        else {
            panic!("unexpected event");
        };
        assert_eq!(first.as_str(), "node-2");
        assert_eq!(second.as_str(), "node-3");

        let closed: WireHostMessage = serde_json::from_value(json!({
            "type": "closed",
            "session_id": "session-1",
            "reason": { "kind": "host_shutdown" }
        }))
        .unwrap();
        assert!(matches!(
            closed,
            WireHostMessage::Closed {
                reason: CloseReason::HostShutdown,
                ..
            }
        ));
    }

    #[test]
    fn decodes_external_drop_notifications() {
        let event: WireHostMessage = serde_json::from_value(json!({
            "type": "event",
            "session_id": "session-1",
            "event": {
                "kind": "scene",
                "event": {
                    "kind": "external_drop",
                    "target": "node-7",
                    "position": { "x": 12.0, "y": 24.0 },
                    "files": [
                        { "uri": "file:///tmp/a%20file.txt" },
                        { "uri": "file:///tmp/b.txt" }
                    ]
                }
            }
        }))
        .unwrap();
        let WireHostMessage::Event {
            event:
                Event::Scene {
                    event:
                        SceneEvent::ExternalDrop {
                            target,
                            position,
                            files,
                        },
                },
            ..
        } = event
        else {
            panic!("unexpected event");
        };
        assert_eq!(target.as_str(), "node-7");
        assert_eq!(position, Vec2 { x: 12.0, y: 24.0 });
        assert_eq!(
            files,
            vec![
                ExternalFile {
                    uri: "file:///tmp/a%20file.txt".to_owned(),
                },
                ExternalFile {
                    uri: "file:///tmp/b.txt".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn frame_reader_accepts_fragmented_json() {
        let payload = br#"{"type":"tick","session_id":"session-1","dt_ns":"16"}"#;
        let mut reader = FrameReader::default();
        let length = u32::try_from(payload.len() + 1).unwrap().to_be_bytes();
        reader.bytes.extend_from_slice(&length[..2]);
        assert!(reader.take_complete_frame().unwrap().is_none());
        reader.bytes.extend_from_slice(&length[2..]);
        reader.bytes.push(b'J');
        reader.bytes.extend_from_slice(payload);
        let WireFrame::Json(frame) = reader.take_complete_frame().unwrap().unwrap() else {
            panic!("expected JSON frame");
        };
        let message: WireHostMessage = serde_json::from_slice(&frame).unwrap();
        assert!(matches!(message, WireHostMessage::Tick { dt_ns, .. } if dt_ns == "16"));
    }

    #[test]
    fn validates_rgba8_lengths_before_writing() {
        assert_eq!(rgba8_byte_len(2, 3).unwrap(), 24);
        assert!(rgba8_byte_len(u32::MAX, u32::MAX).is_err());
    }

    #[test]
    fn parses_duration_without_precision_loss() {
        assert_eq!(
            parse_duration("18446744073709551615".to_owned()).unwrap(),
            Duration::from_nanos(u64::MAX)
        );
        assert!(parse_duration("1.5".to_owned()).is_err());
    }
}
