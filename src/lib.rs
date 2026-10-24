//! A synchronous, dependency-light reference client for CLP v2.4.
//!
//! The crate speaks only the public Chymeral Link Protocol. It neither imports
//! Chymeral internals nor creates a graphics window of its own.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
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
pub const CLP_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new(2, 4);
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
/// Maximum structural operations accepted in one atomic scene batch.
pub const MAX_SCENE_BATCH_OPERATIONS: usize = 512;
/// Maximum UTF-8 byte length of one serialized scene-batch command envelope.
pub const MAX_SCENE_BATCH_BYTES: usize = 512 * 1024;
/// Maximum UTF-8 byte length of one batch-local node alias.
pub const MAX_SCENE_BATCH_ALIAS_BYTES: usize = 128;

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
    /// Bounded atomic construction or revision of retained scene structure.
    SceneBatch,
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
            Capability::SceneBatch,
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
#[derive(Clone, Debug, PartialEq, Deserialize)]
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

/// A node reference used while assembling one [`SceneBatch`].
///
/// A [`BatchNodeRef::Handle`] identifies an existing session-owned node. An
/// [`BatchNodeRef::Alias`] identifies a node created by an earlier operation
/// in the same batch. Aliases are never retained by Chymeral after the batch
/// completes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum BatchNodeRef {
    /// An opaque handle returned by an earlier successful host command.
    Handle(NodeHandle),
    /// A batch-local alias for an earlier `create_node` operation.
    Alias {
        /// The declared alias.
        alias: String,
    },
}

impl BatchNodeRef {
    /// Creates a batch-local alias reference.
    pub fn alias(alias: impl Into<String>) -> Self {
        Self::Alias {
            alias: alias.into(),
        }
    }
}

impl From<NodeHandle> for BatchNodeRef {
    fn from(handle: NodeHandle) -> Self {
        Self::Handle(handle)
    }
}

impl From<&NodeHandle> for BatchNodeRef {
    fn from(handle: &NodeHandle) -> Self {
        Self::Handle(handle.clone())
    }
}

/// A structural parent change used by a [`SceneBatchOperation::SetNode`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BatchNodeParent {
    /// Make the node a root.
    Root,
    /// Attach the node below a referenced parent.
    Node {
        /// Existing handle or earlier batch alias for the parent.
        node: BatchNodeRef,
    },
}

/// Partial node changes accepted in one [`SceneBatch`].
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SceneBatchNodeChanges {
    /// Partial geometry and interaction-property changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<NodePropertiesPatch>,
    /// Structural parent change; omit to preserve the current parent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<BatchNodeParent>,
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

impl From<NodeChanges> for SceneBatchNodeChanges {
    fn from(changes: NodeChanges) -> Self {
        Self {
            properties: changes.properties,
            parent: changes.parent.map(|parent| match parent {
                NodeParent::Root => BatchNodeParent::Root,
                NodeParent::Node { node } => BatchNodeParent::Node {
                    node: BatchNodeRef::Handle(node),
                },
            }),
            layer: changes.layer,
            visual: changes.visual,
            text: changes.text,
            text_style: changes.text_style,
        }
    }
}

/// One ordered structural operation in an atomic [`SceneBatch`].
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SceneBatchOperation {
    /// Create a root or child node and bind its batch-local alias.
    CreateNode {
        /// Alias returned in the batch result and available to later operations.
        alias: String,
        /// Parent handle or earlier alias. Omit for a root node.
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<BatchNodeRef>,
        /// Root drawing layer. It is invalid for child nodes.
        #[serde(skip_serializing_if = "Option::is_none")]
        layer: Option<SceneLayer>,
        /// Complete node content and properties.
        node: NodeInput,
    },
    /// Update an existing node or one created earlier in the batch.
    SetNode {
        /// Existing handle or earlier alias to update.
        node: BatchNodeRef,
        /// Changes to apply atomically with the rest of the batch.
        changes: SceneBatchNodeChanges,
    },
    /// Remove an existing node or one created earlier in the batch.
    RemoveNode {
        /// Existing handle or earlier alias to remove with its descendants.
        node: BatchNodeRef,
    },
}

/// Builder for a bounded atomic structural scene command.
///
/// Use [`Client::apply_scene_batch`] to send the completed builder. Chymeral
/// validates every operation, commits a successful batch as one retained scene
/// state, and requests one redraw. This is not a virtual-DOM layer and does
/// not replace incremental [`Client::create_node`] or [`Client::set_node`]
/// commands.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneBatch {
    operations: Vec<SceneBatchOperation>,
}

impl SceneBatch {
    /// Creates an empty scene batch.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a batch from explicit ordered operations.
    pub fn from_operations(operations: Vec<SceneBatchOperation>) -> Self {
        Self { operations }
    }

    /// Returns the ordered operations without consuming the builder.
    pub fn operations(&self) -> &[SceneBatchOperation] {
        &self.operations
    }

    /// Appends an explicit operation and returns the same builder.
    pub fn push(&mut self, operation: SceneBatchOperation) -> &mut Self {
        self.operations.push(operation);
        self
    }

    /// Appends a node creation and returns the builder for fluent composition.
    pub fn create_node(
        mut self,
        alias: impl Into<String>,
        parent: Option<BatchNodeRef>,
        layer: Option<SceneLayer>,
        node: NodeInput,
    ) -> Self {
        self.operations.push(SceneBatchOperation::CreateNode {
            alias: alias.into(),
            parent,
            layer,
            node,
        });
        self
    }

    /// Appends a node update and returns the builder for fluent composition.
    pub fn set_node(
        mut self,
        node: impl Into<BatchNodeRef>,
        changes: SceneBatchNodeChanges,
    ) -> Self {
        self.operations.push(SceneBatchOperation::SetNode {
            node: node.into(),
            changes,
        });
        self
    }

    /// Appends a node removal and returns the builder for fluent composition.
    pub fn remove_node(mut self, node: impl Into<BatchNodeRef>) -> Self {
        self.operations
            .push(SceneBatchOperation::RemoveNode { node: node.into() });
        self
    }

    fn into_operations(self) -> Vec<SceneBatchOperation> {
        self.operations
    }
}

/// Outcome of one [`SceneBatchOperation`] in a successful batch result.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SceneBatchOperationResult {
    /// A node was created and remains addressable through its alias.
    Created {
        /// The alias supplied by the operation.
        alias: String,
    },
    /// A node update was applied.
    Updated {
        /// Fresh metrics when the update changed intrinsic text content or style.
        #[serde(default)]
        text_metrics: Option<TextMetrics>,
    },
    /// A node and its descendants were removed.
    Removed,
}

/// Result returned by [`Client::apply_scene_batch`].
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SceneBatchResult {
    /// Opaque handles for every batch alias that remains live after commit.
    pub nodes: BTreeMap<String, CreatedNode>,
    /// Ordered outcomes aligned with the submitted operations.
    pub operations: Vec<SceneBatchOperationResult>,
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

// Synchronous public Client lifecycle and command orchestration.
include!("client.rs");
// Private serde schema and conversion boundary for CLP messages.
// The public SDK model remains above; this wire representation is deliberately
// kept close to the client without becoming part of the documented API.
include!("wire.rs");
// Private local-socket framing and bounded-write implementation.
include!("framing.rs");
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
    include!("tests.rs");
}
