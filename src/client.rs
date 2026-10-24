// Synchronous public Client lifecycle and command orchestration.
// Textual inclusion keeps the documented public API at the crate root.

/// A synchronous CLP v2.4 client with one-time CLP v2.0 fallback.
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
                // metadata capabilities plus v2.4 scene batches, and may
                // reject either newer enum value before version negotiation.
                // Reconnect exactly once with the frozen v2.0 capability set.
                Self::connect_once(
                    endpoint,
                    token,
                    requested_capabilities
                        .without(Capability::Presentation)
                        .without(Capability::WindowMetadata)
                        .without(Capability::SceneBatch),
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

    /// Applies a bounded structural scene batch as one retained-scene state.
    ///
    /// Chymeral validates the whole batch before making its final state
    /// observable and requests one redraw after a successful commit. Aliases
    /// only refer to nodes created by earlier operations in the same batch;
    /// use the returned [`SceneBatchResult`] to retain their opaque handles.
    ///
    /// This method deliberately does **not** replay a batch through sequential
    /// [`Client::create_node`] or [`Client::set_node`] calls when the host does
    /// not grant [`Capability::SceneBatch`]. It instead returns
    /// [`Error::CapabilityNotGranted`], so an application can explicitly
    /// choose its own compatible incremental path without risking a visible
    /// partial first paint.
    pub fn apply_scene_batch(&mut self, batch: SceneBatch) -> Result<SceneBatchResult> {
        self.require(Capability::Scene)?;
        self.require(Capability::SceneBatch)?;
        let operations = batch.into_operations();
        self.validate_scene_batch(&operations)?;
        match self.command(WireSceneCommand::SceneBatch { operations }, None)? {
            WireCommandResult::SceneBatch(batch) => Ok(batch),
            result => Err(unexpected_command_result("scene_batch", result.name())),
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
        let scene_batch = matches!(&command, WireSceneCommand::SceneBatch { .. });
        let message = WireClientMessage::Command {
            request_id: request_id.clone(),
            command: Box::new(command),
        };
        let payload = serde_json::to_vec(&message)?;
        if scene_batch && payload.len() > MAX_SCENE_BATCH_BYTES {
            return Err(Error::InvalidInput(format!(
                "a scene batch may not exceed {MAX_SCENE_BATCH_BYTES} bytes"
            )));
        }
        self.write_client_frame(b'J', &payload)?;
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

    fn validate_scene_batch(&self, operations: &[SceneBatchOperation]) -> Result<()> {
        if operations.is_empty() {
            return Err(Error::InvalidInput(
                "a scene batch requires at least one operation".to_owned(),
            ));
        }
        if operations.len() > MAX_SCENE_BATCH_OPERATIONS {
            return Err(Error::InvalidInput(format!(
                "a scene batch may contain at most {MAX_SCENE_BATCH_OPERATIONS} operations"
            )));
        }

        let mut aliases = BTreeSet::new();
        for operation in operations {
            match operation {
                SceneBatchOperation::CreateNode {
                    alias,
                    parent,
                    layer,
                    node,
                } => {
                    Self::validate_scene_batch_alias(alias)?;
                    if aliases.contains(alias) {
                        return Err(Error::InvalidInput(format!(
                            "the scene batch alias {alias:?} is declared more than once"
                        )));
                    }
                    if let Some(parent) = parent {
                        self.validate_scene_batch_node_ref(parent, &aliases)?;
                        if layer.is_some() {
                            return Err(Error::InvalidInput(
                                "only root nodes may specify a layer".to_owned(),
                            ));
                        }
                    }
                    self.require_node_input(node)?;
                    aliases.insert(alias.clone());
                }
                SceneBatchOperation::SetNode { node, changes } => {
                    self.validate_scene_batch_node_ref(node, &aliases)?;
                    self.require_scene_batch_node_changes(changes)?;
                    if let Some(parent) = &changes.parent {
                        match parent {
                            BatchNodeParent::Root => {}
                            BatchNodeParent::Node { node } => {
                                self.validate_scene_batch_node_ref(node, &aliases)?;
                                if changes.layer.is_some() {
                                    return Err(Error::InvalidInput(
                                        "only root nodes may specify a layer".to_owned(),
                                    ));
                                }
                            }
                        }
                    }
                }
                SceneBatchOperation::RemoveNode { node } => {
                    self.validate_scene_batch_node_ref(node, &aliases)?;
                }
            }
        }
        Ok(())
    }

    fn validate_scene_batch_alias(alias: &str) -> Result<()> {
        if alias.is_empty() {
            return Err(Error::InvalidInput(
                "a scene batch alias must not be empty".to_owned(),
            ));
        }
        if alias.len() > MAX_SCENE_BATCH_ALIAS_BYTES {
            return Err(Error::InvalidInput(format!(
                "a scene batch alias may not exceed {MAX_SCENE_BATCH_ALIAS_BYTES} bytes"
            )));
        }
        if alias.chars().any(char::is_control) {
            return Err(Error::InvalidInput(
                "a scene batch alias may not contain control characters".to_owned(),
            ));
        }
        Ok(())
    }

    fn validate_scene_batch_node_ref(
        &self,
        node: &BatchNodeRef,
        aliases: &BTreeSet<String>,
    ) -> Result<()> {
        match node {
            BatchNodeRef::Handle(_) => Ok(()),
            BatchNodeRef::Alias { alias } => {
                Self::validate_scene_batch_alias(alias)?;
                if aliases.contains(alias) {
                    Ok(())
                } else {
                    Err(Error::InvalidInput(format!(
                        "the scene batch alias {alias:?} has not been declared"
                    )))
                }
            }
        }
    }

    fn require_scene_batch_node_changes(&self, changes: &SceneBatchNodeChanges) -> Result<()> {
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

