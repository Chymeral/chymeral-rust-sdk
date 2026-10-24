// Private serde schema and conversion boundary for CLP messages.
// It is textually included by lib.rs to preserve existing private visibility.

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
    SceneBatch {
        operations: Vec<SceneBatchOperation>,
    },
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
    SceneBatch(SceneBatchResult),
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
            Self::SceneBatch(_) => "scene_batch",
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

