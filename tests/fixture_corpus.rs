use chymeral_rust_sdk::{
    BatchNodeRef, Capabilities, Capability, Client, CloseReason, DeliveryCategory, Error, Event,
    ImageContactShape, ImageUpload, MetadataField, NodeChanges, NodeHandle, NodeInput,
    NodeProperties, NodePropertiesPatch, Notification, PresentationId, PresentationPropertiesPatch,
    PresentationState, PresentationUpdate, ProgramPhase, SceneBatch, SceneBatchNodeChanges,
    SceneBatchOperationResult, SceneEvent, SceneLayer, Subscription, TextAlign, TextNodeProperties,
    TextStyle, TextWeight, WindowMetadataPatch, WindowMetadataStatus, WindowMetadataSupport,
};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName, prelude::*};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, JoinHandle},
};

const FIXTURE_TOKEN: &str = "fixture-token-not-a-secret";
const FIXTURE_V2_0_BYTES: &str = include_str!("../fixtures/clp/v2.0/fixtures.json");
const FIXTURE_V2_0_SHA256: &str = include_str!("../fixtures/clp/v2.0/SHA256SUMS");
const FIXTURE_V2_1_BYTES: &str = include_str!("../fixtures/clp/v2.1/fixtures.json");
const FIXTURE_V2_1_SHA256: &str = include_str!("../fixtures/clp/v2.1/SHA256SUMS");
const FIXTURE_V2_4_BYTES: &str = include_str!("../fixtures/clp/v2.4/fixtures.json");
const FIXTURE_V2_4_SHA256: &str = include_str!("../fixtures/clp/v2.4/SHA256SUMS");
const MAX_FRAME_BYTES: usize = 1024 * 1024;
static NEXT_ENDPOINT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Deserialize)]
struct FixtureCorpus {
    format: String,
    fixture_version: u32,
    protocol: FixtureProtocol,
    transport: FixtureTransport,
    vectors: Vec<FixtureVector>,
}

#[derive(Clone, Deserialize)]
struct FixtureProtocol {
    major: u16,
    minor: u16,
}

#[derive(Clone, Deserialize)]
struct FixtureTransport {
    maximum_frame_bytes: usize,
    json_discriminator: String,
    binary_discriminator: String,
}

#[derive(Clone, Deserialize)]
struct FixtureVector {
    id: String,
    frames: Vec<FixtureFrame>,
}

#[derive(Clone, Deserialize)]
struct FixtureFrame {
    direction: FixtureDirection,
    kind: FixtureKind,
    #[serde(default)]
    message: Option<Value>,
    #[serde(default)]
    bytes_hex: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FixtureDirection {
    ClientToHost,
    HostToClient,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FixtureKind {
    Json,
    Binary,
}

#[test]
fn replays_the_vendored_canonical_clp_v2_fixture_corpus_through_the_public_client() {
    let corpus = load_fixture_corpus(FIXTURE_V2_0_BYTES, FIXTURE_V2_0_SHA256, 0);
    replay_lifecycle(&corpus);
    replay_scene_and_overlap(&corpus);
    replay_images(&corpus);
    replay_delivery_errors_and_fault_close(&corpus);
}

#[test]
fn replays_the_vendored_clp_v2_1_fixture_corpus_through_the_public_client() {
    let corpus = load_fixture_corpus(FIXTURE_V2_1_BYTES, FIXTURE_V2_1_SHA256, 1);
    let (endpoint, host) = spawn_fixture_host(with_client_protocol_minor(
        vector(&corpus, "frame-presentation").frames.clone(),
        4,
    ));
    let mut client = Client::connect(
        &endpoint,
        FIXTURE_TOKEN,
        Capabilities::from([
            Capability::Scene,
            Capability::Images,
            Capability::Input,
            Capability::Timing,
            Capability::Overlap,
            Capability::ExternalDrop,
            Capability::Text,
            Capability::Presentation,
            Capability::WindowMetadata,
        ]),
    )
    .expect("v2.1 fixture handshake should succeed");

    assert_eq!(client.session().protocol_version.minor, 1);
    assert!(client.has_capability(chymeral_rust_sdk::Capability::Presentation));
    let first = PresentationId::new("fixture-presentation-1").unwrap();
    let node_one: NodeHandle = serde_json::from_str("\"node-1\"").unwrap();
    let node_two: NodeHandle = serde_json::from_str("\"node-2\"").unwrap();
    client
        .present_scene_with_id(
            first.clone(),
            [
                PresentationUpdate {
                    node: node_one,
                    properties: PresentationPropertiesPatch {
                        x: Some(96.0),
                        y: Some(64.0),
                        ..PresentationPropertiesPatch::default()
                    },
                },
                PresentationUpdate {
                    node: node_two,
                    properties: PresentationPropertiesPatch {
                        width: Some(48.0),
                        height: Some(24.0),
                        ..PresentationPropertiesPatch::default()
                    },
                },
            ],
        )
        .expect("presentation packet should be written without a command response");
    assert!(matches!(
        client.next_notification().expect("presented status"),
        Notification::PresentationStatus(status)
            if status.presentation_id == first
                && status.status == PresentationState::Presented
                && status.code.is_none()
                && status.message.is_none()
    ));

    let second = PresentationId::new("fixture-presentation-2").unwrap();
    let node_three: NodeHandle = serde_json::from_str("\"node-3\"").unwrap();
    client
        .present_scene_with_id(
            second.clone(),
            [PresentationUpdate {
                node: node_three,
                properties: PresentationPropertiesPatch {
                    width: Some(88.0),
                    ..PresentationPropertiesPatch::default()
                },
            }],
        )
        .expect("rejected presentation packet should still be sent");
    assert!(matches!(
        client.next_notification().expect("rejected status"),
        Notification::PresentationStatus(status)
            if status.presentation_id == second
                && status.status == PresentationState::Rejected
                && status.code.as_deref() == Some("invalid_node_change")
                && status.message.as_deref()
                    == Some("node-3 has intrinsic text bounds that cannot be set directly")
    ));

    assert_eq!(
        client.close().expect("fixture close should succeed"),
        CloseReason::ClientRequested
    );
    join_host(host);
    replay_window_metadata(&corpus);
}

#[test]
fn replays_the_vendored_clp_v2_4_scene_batch_fixture_through_the_public_client() {
    let corpus = load_fixture_corpus(FIXTURE_V2_4_BYTES, FIXTURE_V2_4_SHA256, 4);
    let (endpoint, host) = spawn_fixture_host(vector(&corpus, "atomic-scene-batch").frames.clone());
    let mut client = Client::connect(
        &endpoint,
        FIXTURE_TOKEN,
        Capabilities::from([Capability::Scene, Capability::Text, Capability::SceneBatch]),
    )
    .expect("v2.4 fixture handshake should succeed");

    assert_eq!(client.session().protocol_version.minor, 4);
    assert!(client.has_capability(Capability::SceneBatch));
    let result = client
        .apply_scene_batch(
            SceneBatch::new()
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
                )
                .create_node(
                    "temporary",
                    None,
                    Some(SceneLayer::Overlay),
                    NodeInput::Empty {
                        properties: NodeProperties {
                            visible: false,
                            ..NodeProperties::new(0.0, 0.0, 1.0, 1.0)
                        },
                    },
                )
                .remove_node(BatchNodeRef::alias("temporary")),
        )
        .expect("scene batch should match the v2.4 fixture");
    assert_eq!(result.nodes["panel"].node.as_str(), "node-1");
    assert_eq!(result.nodes["label"].node.as_str(), "node-2");
    assert!(!result.nodes.contains_key("temporary"));
    assert_eq!(result.nodes["label"].text_metrics.unwrap().columns, 5);
    assert_eq!(
        result.operations,
        vec![
            SceneBatchOperationResult::Created {
                alias: "panel".to_owned(),
            },
            SceneBatchOperationResult::Created {
                alias: "label".to_owned(),
            },
            SceneBatchOperationResult::Updated { text_metrics: None },
            SceneBatchOperationResult::Created {
                alias: "temporary".to_owned(),
            },
            SceneBatchOperationResult::Removed,
        ]
    );
    assert_eq!(
        client.close().expect("fixture close should succeed"),
        CloseReason::ClientRequested
    );
    join_host(host);
}

fn replay_window_metadata(corpus: &FixtureCorpus) {
    let (endpoint, host) = spawn_fixture_host(with_client_protocol_minor(
        vector(corpus, "application-window-metadata").frames.clone(),
        4,
    ));
    let mut client = Client::connect(
        &endpoint,
        FIXTURE_TOKEN,
        Capabilities::from([Capability::Images, Capability::WindowMetadata]),
    )
    .expect("window metadata fixture handshake should succeed");

    assert!(client.has_capability(Capability::WindowMetadata));
    assert_eq!(
        client.session().context.window_metadata,
        Some(WindowMetadataSupport {
            icon_supported: true
        })
    );
    let image = client
        .upload_image(ImageUpload::Rgba8 {
            width: 1,
            height: 1,
            pixels: &[255, 127, 0, 255],
        })
        .expect("fixture icon upload should succeed");
    assert_eq!(image.as_str(), "image-1");
    assert_eq!(
        client
            .set_window_metadata(WindowMetadataPatch {
                title: MetadataField::Set("Fixture metadata".to_owned()),
                icon: MetadataField::Set(image),
            })
            .expect("fixture metadata set should succeed"),
        WindowMetadataStatus {
            icon_supported: true
        }
    );
    assert_eq!(
        client
            .set_window_metadata(WindowMetadataPatch {
                title: MetadataField::Set("Refreshed fixture metadata".to_owned()),
                ..WindowMetadataPatch::default()
            })
            .expect("fixture title replacement should preserve the icon"),
        WindowMetadataStatus {
            icon_supported: true
        }
    );
    assert_eq!(
        client
            .set_window_metadata(WindowMetadataPatch {
                title: MetadataField::Clear,
                icon: MetadataField::Clear,
            })
            .expect("fixture metadata clear should succeed"),
        WindowMetadataStatus {
            icon_supported: true
        }
    );
    assert_eq!(
        client.close().expect("fixture close should succeed"),
        CloseReason::ClientRequested
    );
    join_host(host);
}

fn load_fixture_corpus(
    fixture_bytes: &str,
    fixture_checksum: &str,
    expected_minor: u16,
) -> FixtureCorpus {
    let expected_hash = fixture_checksum
        .split_whitespace()
        .next()
        .expect("fixture SHA256SUMS must contain a checksum");
    let actual_hash = format!("{:x}", Sha256::digest(fixture_bytes.as_bytes()));
    assert_eq!(actual_hash, expected_hash);

    let corpus: FixtureCorpus =
        serde_json::from_str(fixture_bytes).expect("fixture corpus must be valid JSON");
    assert_eq!(corpus.format, "chymeral-clp-fixture-corpus");
    assert_eq!(corpus.fixture_version, 1);
    assert_eq!(corpus.protocol.major, 2);
    assert_eq!(corpus.protocol.minor, expected_minor);
    assert_eq!(corpus.transport.maximum_frame_bytes, MAX_FRAME_BYTES);
    assert_eq!(corpus.transport.json_discriminator, "J");
    assert_eq!(corpus.transport.binary_discriminator, "B");
    corpus
}

fn replay_lifecycle(corpus: &FixtureCorpus) {
    let (mut client, host) = connect_fixture_session(corpus, "lifecycle-and-subscriptions");
    assert_eq!(client.session().session_id.as_str(), "session-1");
    assert_eq!(
        client.session().context.arguments,
        vec!["--fixture".to_owned()]
    );

    let subscriptions = client
        .subscribe([
            Subscription::RawInput,
            Subscription::Terminal,
            Subscription::Timing,
        ])
        .expect("subscription should succeed");
    assert_eq!(
        subscriptions,
        vec![
            Subscription::RawInput,
            Subscription::Terminal,
            Subscription::Timing,
        ]
    );
    assert!(matches!(
        client.next_notification().expect("resize notification"),
        Notification::Resize(resize)
            if resize.physical_width == 1600
                && resize.physical_height == 900
                && resize.logical_width == 800.0
                && resize.logical_height == 450.0
                && resize.scale_factor == 2.0
    ));
    assert!(matches!(
        client.next_notification().expect("tick notification"),
        Notification::Tick(duration) if duration.as_nanos() == 16_666_667
    ));
    assert!(matches!(
        client.next_notification().expect("frame notification"),
        Notification::Frame(duration) if duration.as_nanos() == 16_666_667
    ));
    assert_eq!(
        client.close().expect("clean close"),
        CloseReason::ClientRequested
    );
    join_host(host);
}

fn replay_scene_and_overlap(corpus: &FixtureCorpus) {
    let (mut client, host) = connect_fixture_session(corpus, "scene-text-and-overlap");
    let rectangle = client
        .create_node(
            None,
            Some(SceneLayer::Overlay),
            NodeInput::Rectangle {
                properties: NodeProperties {
                    x: 24.0,
                    y: 32.0,
                    width: 160.0,
                    height: 72.0,
                    visible: true,
                    clip_children: false,
                    z_index: 2,
                    overlap_enabled: true,
                    external_drop_enabled: false,
                    input_passthrough: false,
                },
                color: [37, 99, 235, 230],
            },
        )
        .expect("rectangle creation should succeed");
    assert_eq!(rectangle.node.as_str(), "node-1");

    let text = client
        .create_node(
            None,
            Some(SceneLayer::Overlay),
            NodeInput::Text {
                properties: TextNodeProperties {
                    x: 48.0,
                    y: 56.0,
                    visible: true,
                    clip_children: false,
                    z_index: 3,
                    overlap_enabled: true,
                    external_drop_enabled: false,
                    input_passthrough: false,
                },
                text: "Status:\tready\n界".to_owned(),
                style: TextStyle {
                    color: Some([220, 230, 255, 255]),
                    weight: TextWeight::Bold,
                    align: TextAlign::Start,
                    ..TextStyle::default()
                },
            },
        )
        .expect("text creation should succeed");
    assert_eq!(text.node.as_str(), "node-2");
    assert_eq!(
        text.text_metrics.expect("text metrics").columns,
        13,
        "host text metrics must be exposed"
    );

    assert_eq!(
        client
            .set_node(
                &rectangle.node,
                NodeChanges {
                    properties: Some(NodePropertiesPatch {
                        x: Some(64.0),
                        input_passthrough: Some(true),
                        ..NodePropertiesPatch::default()
                    }),
                    ..NodeChanges::default()
                },
            )
            .expect("node update should succeed"),
        None
    );
    assert!(matches!(
        client.next_notification().expect("overlap event"),
        Notification::Event(Event::Scene {
            event: SceneEvent::OverlapEnter { first, second }
        }) if first == rectangle.node && second == text.node
    ));
    assert!(
        client
            .is_overlapping(&rectangle.node)
            .expect("overlap query should succeed")
    );
    assert_eq!(
        client
            .get_overlaps(&rectangle.node)
            .expect("overlap list should succeed"),
        vec![text.node.clone()]
    );
    client
        .remove_node(&text.node)
        .expect("text removal should succeed");
    client
        .remove_node(&rectangle.node)
        .expect("rectangle removal should succeed");
    join_host(host);
}

fn replay_images(corpus: &FixtureCorpus) {
    let (mut client, host) = connect_fixture_session(corpus, "image-upload-and-cleanup");
    let rgba8 = client
        .upload_image(ImageUpload::Rgba8 {
            width: 1,
            height: 1,
            pixels: &[0xff, 0x80, 0x00, 0xff],
        })
        .expect("RGBA8 upload should succeed");
    assert_eq!(rgba8.as_str(), "image-1");

    let image_node = client
        .create_node(
            None,
            Some(SceneLayer::Overlay),
            NodeInput::Image {
                properties: NodeProperties {
                    x: 96.0,
                    y: 64.0,
                    width: 48.0,
                    height: 48.0,
                    visible: true,
                    clip_children: false,
                    z_index: 1,
                    overlap_enabled: true,
                    external_drop_enabled: true,
                    input_passthrough: false,
                },
                image: rgba8.clone(),
                opacity: 0.75,
                contact_shape: ImageContactShape::alpha(),
            },
        )
        .expect("alpha-contact image node should succeed");
    assert_eq!(image_node.node.as_str(), "node-3");
    client
        .remove_node(&image_node.node)
        .expect("image node removal should succeed");
    client
        .remove_image(&rgba8)
        .expect("RGBA8 image removal should succeed");

    let gif_bytes = decode_hex(
        "47494638396101000100800000000000ffffff21f90401000000002c00000000010001000002024c01003b",
    )
    .expect("fixture GIF bytes must decode");
    let gif = client
        .upload_image(ImageUpload::Gif { bytes: &gif_bytes })
        .expect("GIF upload should succeed");
    assert_eq!(gif.as_str(), "image-2");
    client
        .remove_image(&gif)
        .expect("GIF image removal should succeed");
    join_host(host);
}

fn replay_delivery_errors_and_fault_close(corpus: &FixtureCorpus) {
    let (mut client, host) = connect_fixture_session(corpus, "delivery-errors-and-fault-close");
    let unknown_node: NodeHandle =
        serde_json::from_str("\"node-99\"").expect("fixture node handle must deserialize");
    let error = client
        .is_overlapping(&unknown_node)
        .expect_err("fixture host rejects a foreign node");
    match error {
        Error::Command { code, message, .. } => {
            assert_eq!(code, "not_owner");
            assert_eq!(message, "node-99 is not owned by this session");
        }
        other => panic!("expected a command error, received {other:?}"),
    }
    assert!(matches!(
        client.next_notification().expect("external drop event"),
        Notification::Event(Event::Scene {
            event: SceneEvent::ExternalDrop {
                target,
                position,
                files,
            }
        }) if target.as_str() == "node-5"
            && position.x == 120.0
            && position.y == 80.0
            && files.len() == 1
            && files[0].uri == "file:///tmp/fixture.txt"
    ));
    assert_eq!(
        client.next_notification().expect("delivery gap"),
        Notification::DeliveryGap(vec![DeliveryCategory::Scene, DeliveryCategory::Timing])
    );
    assert_eq!(
        client
            .report_fault(ProgramPhase::Event, "fixture program fault")
            .expect("fault close should succeed"),
        CloseReason::ProgramFault {
            phase: ProgramPhase::Event,
            message: "fixture program fault".to_owned(),
        }
    );
    join_host(host);
}

fn connect_fixture_session(
    corpus: &FixtureCorpus,
    vector_id: &str,
) -> (Client, JoinHandle<std::result::Result<(), String>>) {
    let lifecycle = vector(corpus, "lifecycle-and-subscriptions");
    let frames = if vector_id == lifecycle.id {
        lifecycle.frames.clone()
    } else {
        lifecycle
            .frames
            .iter()
            .take(5)
            .cloned()
            .chain(vector(corpus, vector_id).frames.iter().cloned())
            .collect()
    };
    let (endpoint, host) =
        spawn_fixture_host_with_legacy_fallback(frames, corpus.protocol.minor == 0);
    let client = Client::connect(&endpoint, FIXTURE_TOKEN, Capabilities::all())
        .expect("fixture handshake should succeed");
    (client, host)
}

fn vector<'a>(corpus: &'a FixtureCorpus, id: &str) -> &'a FixtureVector {
    corpus
        .vectors
        .iter()
        .find(|vector| vector.id == id)
        .unwrap_or_else(|| panic!("fixture corpus does not contain vector {id}"))
}

fn with_client_protocol_minor(mut frames: Vec<FixtureFrame>, minor: u16) -> Vec<FixtureFrame> {
    let hello = frames
        .first_mut()
        .and_then(|frame| frame.message.as_mut())
        .expect("fixture vector must begin with a JSON hello");
    let protocol_minor = hello
        .pointer_mut("/protocol/minor")
        .expect("fixture hello must include protocol.minor");
    *protocol_minor = Value::from(minor);
    frames
}

fn spawn_fixture_host(
    frames: Vec<FixtureFrame>,
) -> (String, JoinHandle<std::result::Result<(), String>>) {
    spawn_fixture_host_with_legacy_fallback(frames, false)
}

fn spawn_fixture_host_with_legacy_fallback(
    frames: Vec<FixtureFrame>,
    legacy_fallback: bool,
) -> (String, JoinHandle<std::result::Result<(), String>>) {
    let endpoint = format!(
        "chymeral-rust-sdk-fixture-{}-{}",
        std::process::id(),
        NEXT_ENDPOINT_ID.fetch_add(1, Ordering::Relaxed)
    );
    let name = endpoint
        .clone()
        .to_ns_name::<GenericNamespaced>()
        .expect("fixture endpoint must be valid");
    let listener = ListenerOptions::new()
        .name(name)
        .create_sync()
        .expect("fixture listener must start");
    let host = thread::spawn(move || {
        if legacy_fallback {
            let mut stream = listener
                .accept()
                .map_err(|error| format!("fixture fallback accept failed: {error}"))?;
            let (kind, payload) = read_frame(&mut stream)?;
            if kind != b'J' {
                return Err("fixture fallback expected a client JSON hello".to_owned());
            }
            let hello: Value = serde_json::from_slice(&payload).map_err(|error| {
                format!("fixture fallback received invalid hello JSON: {error}")
            })?;
            if message_type(&hello)? != "hello"
                || hello.pointer("/protocol/minor").and_then(Value::as_u64) != Some(4)
            {
                return Err("fixture fallback expected a CLP v2.4 hello".to_owned());
            }
            let rejection = serde_json::json!({
                "type": "error",
                "code": "unsupported_version",
                "message": "fixture host only supports CLP v2.0"
            });
            let rejection = serde_json::to_vec(&rejection)
                .map_err(|error| format!("fixture fallback error encode failed: {error}"))?;
            write_frame(&mut stream, b'J', &rejection)?;
        }
        let mut stream = listener
            .accept()
            .map_err(|error| format!("fixture host accept failed: {error}"))?;
        replay_frames(&mut stream, &frames)
    });
    (endpoint, host)
}

fn join_host(host: JoinHandle<std::result::Result<(), String>>) {
    host.join()
        .expect("fixture host thread panicked")
        .expect("fixture host replay failed");
}

fn replay_frames(
    stream: &mut impl ReadWrite,
    frames: &[FixtureFrame],
) -> std::result::Result<(), String> {
    let mut request_ids = BTreeMap::new();
    for (index, frame) in frames.iter().enumerate() {
        match (frame.direction, frame.kind) {
            (FixtureDirection::ClientToHost, FixtureKind::Json) => {
                let (kind, bytes) = read_frame(stream)?;
                if kind != b'J' {
                    return Err(format!("frame {index}: expected a client JSON frame"));
                }
                let actual: Value = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("frame {index}: invalid client JSON: {error}"))?;
                let mut expected = frame_message(frame)?.clone();
                normalize_client_request(&mut expected, &actual, &mut request_ids)?;
                if actual != expected {
                    return Err(format!(
                        "frame {index}: client JSON differs\nexpected: {expected}\nactual: {actual}"
                    ));
                }
            }
            (FixtureDirection::ClientToHost, FixtureKind::Binary) => {
                let (kind, actual) = read_frame(stream)?;
                if kind != b'B' {
                    return Err(format!("frame {index}: expected a client binary frame"));
                }
                let expected = decode_hex(frame_bytes(frame)?)?;
                if actual != expected {
                    return Err(format!("frame {index}: client binary payload differs"));
                }
            }
            (FixtureDirection::HostToClient, FixtureKind::Json) => {
                let mut message = frame_message(frame)?.clone();
                normalize_host_request(&mut message, &request_ids)?;
                let bytes = serde_json::to_vec(&message)
                    .map_err(|error| format!("frame {index}: host JSON encode failed: {error}"))?;
                write_frame(stream, b'J', &bytes)?;
            }
            (FixtureDirection::HostToClient, FixtureKind::Binary) => {
                write_frame(stream, b'B', &decode_hex(frame_bytes(frame)?)?)?;
            }
        }
    }
    Ok(())
}

trait ReadWrite: Read + Write {}

impl<T: Read + Write> ReadWrite for T {}

fn frame_message(frame: &FixtureFrame) -> std::result::Result<&Value, String> {
    frame
        .message
        .as_ref()
        .ok_or_else(|| "fixture JSON frame has no message".to_owned())
}

fn frame_bytes(frame: &FixtureFrame) -> std::result::Result<&str, String> {
    frame
        .bytes_hex
        .as_deref()
        .ok_or_else(|| "fixture binary frame has no bytes_hex".to_owned())
}

fn normalize_client_request(
    expected: &mut Value,
    actual: &Value,
    request_ids: &mut BTreeMap<String, String>,
) -> std::result::Result<(), String> {
    let expected_request_id = request_id(expected)?;
    let actual_request_id = request_id(actual)?;
    match (expected_request_id, actual_request_id) {
        (None, None) => Ok(()),
        (Some(expected_id), Some(actual_id)) => {
            if message_type(expected)? != "command" {
                return Err("only fixture commands may carry request IDs".to_owned());
            }
            request_ids.insert(expected_id, actual_id.clone());
            expected
                .as_object_mut()
                .expect("fixture message was checked as an object")
                .insert("request_id".to_owned(), Value::String(actual_id));
            Ok(())
        }
        (None, Some(_)) => Err("client unexpectedly sent a request ID".to_owned()),
        (Some(_), None) => Err("client command omitted its request ID".to_owned()),
    }
}

fn normalize_host_request(
    message: &mut Value,
    request_ids: &BTreeMap<String, String>,
) -> std::result::Result<(), String> {
    let Some(fixture_request_id) = request_id(message)? else {
        return Ok(());
    };
    let client_request_id = request_ids.get(&fixture_request_id).ok_or_else(|| {
        format!("host response references {fixture_request_id} before its command")
    })?;
    message
        .as_object_mut()
        .expect("fixture message was checked as an object")
        .insert(
            "request_id".to_owned(),
            Value::String(client_request_id.clone()),
        );
    Ok(())
}

fn request_id(message: &Value) -> std::result::Result<Option<String>, String> {
    let object = message
        .as_object()
        .ok_or_else(|| "fixture JSON message must be an object".to_owned())?;
    match object.get("request_id") {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err("fixture request_id must be a string".to_owned()),
    }
}

fn message_type(message: &Value) -> std::result::Result<&str, String> {
    message
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        .ok_or_else(|| "fixture JSON message must have a string type".to_owned())
}

fn read_frame(stream: &mut impl Read) -> std::result::Result<(u8, Vec<u8>), String> {
    let mut length = [0_u8; 4];
    stream
        .read_exact(&mut length)
        .map_err(|error| format!("fixture host could not read frame length: {error}"))?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(format!(
            "fixture host received invalid frame length {length}"
        ));
    }
    let mut frame = vec![0_u8; length];
    stream
        .read_exact(&mut frame)
        .map_err(|error| format!("fixture host could not read frame payload: {error}"))?;
    Ok((frame[0], frame[1..].to_vec()))
}

fn write_frame(
    stream: &mut impl Write,
    kind: u8,
    payload: &[u8],
) -> std::result::Result<(), String> {
    let length = payload
        .len()
        .checked_add(1)
        .ok_or_else(|| "fixture host frame length overflowed".to_owned())?;
    if length > MAX_FRAME_BYTES {
        return Err(format!(
            "fixture host frame length {length} exceeds the CLP limit"
        ));
    }
    let length =
        u32::try_from(length).map_err(|_| "fixture host frame length exceeds u32".to_owned())?;
    stream
        .write_all(&length.to_be_bytes())
        .and_then(|()| stream.write_all(&[kind]))
        .and_then(|()| stream.write_all(payload))
        .and_then(|()| stream.flush())
        .map_err(|error| format!("fixture host could not write a frame: {error}"))
}

fn decode_hex(value: &str) -> std::result::Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
        return Err("fixture hexadecimal payload has an odd length".to_owned());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .map_err(|error| format!("fixture hexadecimal payload is not UTF-8: {error}"))
                .and_then(|hex| {
                    u8::from_str_radix(hex, 16)
                        .map_err(|error| format!("fixture hexadecimal payload is invalid: {error}"))
                })
        })
        .collect()
}
