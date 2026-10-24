// Private SDK tests retain direct access to wire and transport implementation.
// Keep them separate from the documented public client surface.

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
                "protocol": { "major": 2, "minor": 4 },
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
    fn serializes_a_scene_batch_with_aliases_and_structural_changes() {
        let batch = SceneBatch::new()
            .create_node(
                "panel",
                None,
                Some(SceneLayer::Overlay),
                NodeInput::Rectangle {
                    properties: NodeProperties {
                        z_index: 3,
                        ..NodeProperties::new(8.0, 12.0, 240.0, 120.0)
                    },
                    color: [36, 88, 216, 255],
                },
            )
            .create_node(
                "label",
                Some(BatchNodeRef::alias("panel")),
                None,
                NodeInput::Text {
                    properties: TextNodeProperties::new(16.0, 24.0),
                    text: "Ready".to_owned(),
                    style: TextStyle::default(),
                },
            )
            .set_node(
                BatchNodeRef::alias("panel"),
                SceneBatchNodeChanges {
                    properties: Some(NodePropertiesPatch {
                        x: Some(20.0),
                        clip_children: Some(true),
                        ..NodePropertiesPatch::default()
                    }),
                    ..SceneBatchNodeChanges::default()
                },
            );
        let message = WireClientMessage::Command {
            request_id: "sdk-12".to_owned(),
            command: Box::new(WireSceneCommand::SceneBatch {
                operations: batch.into_operations(),
            }),
        };

        assert_eq!(
            serde_json::to_value(message).unwrap(),
            json!({
                "type": "command",
                "request_id": "sdk-12",
                "command": {
                    "op": "scene_batch",
                    "operations": [
                        {
                            "op": "create_node",
                            "alias": "panel",
                            "layer": "overlay",
                            "node": {
                                "kind": "rectangle",
                                "properties": {
                                    "x": 8.0,
                                    "y": 12.0,
                                    "width": 240.0,
                                    "height": 120.0,
                                    "visible": true,
                                    "clip_children": false,
                                    "z_index": 3,
                                    "overlap_enabled": false,
                                    "external_drop_enabled": false,
                                    "input_passthrough": false
                                },
                                "color": [36, 88, 216, 255]
                            }
                        },
                        {
                            "op": "create_node",
                            "alias": "label",
                            "parent": { "alias": "panel" },
                            "node": {
                                "kind": "text",
                                "properties": {
                                    "x": 16.0,
                                    "y": 24.0,
                                    "visible": true,
                                    "clip_children": false,
                                    "z_index": 0,
                                    "overlap_enabled": false,
                                    "external_drop_enabled": false,
                                    "input_passthrough": false
                                },
                                "text": "Ready",
                                "style": { "weight": "normal", "align": "start" }
                            }
                        },
                        {
                            "op": "set_node",
                            "node": { "alias": "panel" },
                            "changes": {
                                "properties": { "x": 20.0, "clip_children": true }
                            }
                        }
                    ]
                }
            })
        );
    }

    #[test]
    fn decodes_scene_batch_handles_metrics_and_operation_outcomes() {
        let message: WireHostMessage = serde_json::from_value(json!({
            "type": "result",
            "request_id": "sdk-12",
            "result": {
                "kind": "scene_batch",
                "nodes": {
                    "label": {
                        "node": "node-2",
                        "text_metrics": {
                            "columns": 5,
                            "rows": 1,
                            "width": 48.0,
                            "height": 20.0
                        }
                    },
                    "panel": { "node": "node-1" }
                },
                "operations": [
                    { "kind": "created", "alias": "panel" },
                    { "kind": "created", "alias": "label" },
                    { "kind": "updated" }
                ]
            }
        }))
        .unwrap();
        let WireHostMessage::Result {
            result: WireCommandResult::SceneBatch(batch),
            ..
        } = message
        else {
            panic!("expected a scene batch command result");
        };

        assert_eq!(batch.nodes["panel"].node.as_str(), "node-1");
        assert_eq!(batch.nodes["label"].node.as_str(), "node-2");
        assert_eq!(batch.nodes["label"].text_metrics.unwrap().columns, 5);
        assert_eq!(
            batch.operations,
            vec![
                SceneBatchOperationResult::Created {
                    alias: "panel".to_owned(),
                },
                SceneBatchOperationResult::Created {
                    alias: "label".to_owned(),
                },
                SceneBatchOperationResult::Updated { text_metrics: None },
            ]
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
