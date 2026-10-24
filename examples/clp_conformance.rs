use std::{
    error::Error,
    time::{Duration, Instant},
};

use chymeral_rust_sdk::{
    BatchNodeRef, Capabilities, Capability, Client, CloseReason, Event, ImageContactShape,
    ImageUpload, Key, KeyState, MetadataField, NodeChanges, NodeInput, NodeProperties,
    NodePropertiesPatch, NodeVisual, Notification, SceneBatch, SceneBatchNodeChanges, SceneLayer,
    Subscription, TextNodeProperties, TextStyle, WindowMetadataPatch,
};

const ALPHA_TOGGLE_GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0xf0, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x21, 0xff, 0x0b, 0x4e, 0x45, 0x54, 0x53, 0x43, 0x41, 0x50, 0x45, 0x32, 0x2e,
    0x30, 0x03, 0x01, 0x00, 0x00, 0x00, 0x21, 0xf9, 0x04, 0x01, 0x96, 0x00, 0x00, 0x00, 0x2c, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x21, 0xf9, 0x04,
    0x00, 0x96, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x80, 0xfb,
    0x71, 0x85, 0x00, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
];

fn main() -> Result<(), Box<dyn Error>> {
    let mut client = Client::connect_from_env(Capabilities::all())?;
    client.subscribe([
        Subscription::RawInput,
        Subscription::Terminal,
        Subscription::Timing,
    ])?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut ticks = 0;
    while Instant::now() < deadline {
        match client.next_notification()? {
            Notification::Tick(_) => ticks += 1,
            Notification::Closing(reason) | Notification::Closed(reason) => {
                return Err(format!("session closed during timing conformance: {reason:?}").into());
            }
            _ => {}
        }
    }
    if ticks == 0 {
        return Err("timing subscription did not deliver a tick".into());
    }

    let image = client.upload_image(ImageUpload::Rgba8 {
        width: 1,
        height: 1,
        pixels: &[30, 150, 240, 255],
    })?;
    if client.has_capability(Capability::WindowMetadata) {
        let initial = client.set_window_metadata(WindowMetadataPatch {
            title: MetadataField::Set("Chymeral Rust SDK conformance".to_owned()),
            icon: MetadataField::Set(image.clone()),
        })?;
        if let Some(support) = client.session().context.window_metadata {
            if initial.icon_supported != support.icon_supported {
                return Err("window metadata support disagreed with its command result".into());
            }
        }
        client.set_window_metadata(WindowMetadataPatch {
            title: MetadataField::Set("Chymeral Rust SDK conformance refreshed".to_owned()),
            ..WindowMetadataPatch::default()
        })?;
        client.set_window_metadata(WindowMetadataPatch {
            title: MetadataField::Clear,
            icon: MetadataField::Clear,
        })?;
    }
    let visual = client
        .create_node(
            None,
            Some(SceneLayer::Overlay),
            NodeInput::Image {
                properties: NodeProperties::new(16.0, 16.0, 1.0, 1.0),
                image: image.clone(),
                opacity: 1.0,
                contact_shape: ImageContactShape::alpha(),
            },
        )?
        .node;
    let animated_image = client.upload_image(ImageUpload::Gif {
        bytes: ALPHA_TOGGLE_GIF,
    })?;
    client.set_node(
        &visual,
        NodeChanges {
            visual: Some(NodeVisual::Image {
                image: animated_image.clone(),
                opacity: 0.75,
                contact_shape: ImageContactShape::alpha(),
            }),
            ..NodeChanges::default()
        },
    )?;
    client.set_node(
        &visual,
        NodeChanges {
            visual: Some(NodeVisual::Rectangle {
                color: [30, 150, 240, 255],
            }),
            ..NodeChanges::default()
        },
    )?;

    let text = client.create_node(
        None,
        Some(SceneLayer::Overlay),
        NodeInput::Text {
            properties: TextNodeProperties::new(32.0, 32.0),
            text: "a\tb\n界".to_owned(),
            style: TextStyle::default(),
        },
    )?;
    let text_metrics = text
        .text_metrics
        .expect("a created text node reports intrinsic metrics");
    if text_metrics.columns != 9 || text_metrics.rows != 2 {
        return Err("text node metrics did not follow terminal-cell layout".into());
    }
    let updated_text = client
        .set_node(
            &text.node,
            NodeChanges {
                text: Some("updated\n".to_owned()),
                ..NodeChanges::default()
            },
        )?
        .expect("text changes report intrinsic metrics");
    if updated_text.columns != 7 || updated_text.rows != 2 {
        return Err("text update did not recompute intrinsic metrics".into());
    }
    let styled_text = client
        .set_node(
            &text.node,
            NodeChanges {
                text_style: Some(TextStyle {
                    size: Some(20.0),
                    ..TextStyle::default()
                }),
                ..NodeChanges::default()
            },
        )?
        .expect("text-style changes report intrinsic metrics");
    if styled_text.columns != updated_text.columns
        || styled_text.rows != updated_text.rows
        || !styled_text.width.is_finite()
        || !styled_text.height.is_finite()
        || styled_text.width <= 0.0
        || styled_text.height <= 0.0
    {
        return Err("text style update returned invalid intrinsic metrics".into());
    }

    if client.has_capability(Capability::SceneBatch) {
        let batch = SceneBatch::new()
            .create_node(
                "batch-root",
                None,
                Some(SceneLayer::Overlay),
                NodeInput::Empty {
                    properties: NodeProperties {
                        visible: false,
                        ..NodeProperties::new(320.0, 48.0, 120.0, 72.0)
                    },
                },
            )
            .create_node(
                "batch-label",
                Some(BatchNodeRef::alias("batch-root")),
                None,
                NodeInput::Text {
                    properties: TextNodeProperties {
                        visible: false,
                        ..TextNodeProperties::new(8.0, 8.0)
                    },
                    text: "batch".to_owned(),
                    style: TextStyle::default(),
                },
            )
            .set_node(
                BatchNodeRef::alias("batch-root"),
                SceneBatchNodeChanges {
                    properties: Some(NodePropertiesPatch {
                        x: Some(336.0),
                        ..NodePropertiesPatch::default()
                    }),
                    ..SceneBatchNodeChanges::default()
                },
            )
            .create_node(
                "batch-temporary",
                None,
                Some(SceneLayer::Overlay),
                NodeInput::Empty {
                    properties: NodeProperties {
                        visible: false,
                        ..NodeProperties::new(0.0, 0.0, 1.0, 1.0)
                    },
                },
            )
            .remove_node(BatchNodeRef::alias("batch-temporary"));
        let batch = client.apply_scene_batch(batch)?;
        let batch_root = batch
            .nodes
            .get("batch-root")
            .ok_or("scene batch did not return its surviving root alias")?
            .node
            .clone();
        if !batch.nodes.contains_key("batch-label")
            || batch.nodes.contains_key("batch-temporary")
            || batch.operations.len() != 5
        {
            return Err("scene batch result did not reflect its final retained tree".into());
        }
        client.remove_node(&batch_root)?;
    }

    let collider = client
        .create_node(
            None,
            Some(SceneLayer::Overlay),
            NodeInput::Empty {
                properties: NodeProperties {
                    visible: false,
                    overlap_enabled: true,
                    ..NodeProperties::new(220.0, 80.0, 40.0, 40.0)
                },
            },
        )?
        .node;
    let mover = client
        .create_node(
            None,
            Some(SceneLayer::Overlay),
            NodeInput::Empty {
                properties: NodeProperties {
                    visible: false,
                    overlap_enabled: true,
                    ..NodeProperties::new(80.0, 80.0, 40.0, 40.0)
                },
            },
        )?
        .node;

    client.set_node(
        &mover,
        NodeChanges {
            properties: Some(NodePropertiesPatch {
                x: Some(230.0),
                ..NodePropertiesPatch::default()
            }),
            ..NodeChanges::default()
        },
    )?;
    let overlaps = client.get_overlaps(&mover)?;
    if !client.is_overlapping(&mover)? || !overlaps.iter().any(|node| node == &collider) {
        return Err("overlap query did not report the enabled same-session collider".into());
    }

    if std::env::args().any(|argument| argument == "--manual") {
        manual_interaction_demo(&mut client)?;
    }

    client.remove_node(&mover)?;
    client.remove_node(&collider)?;
    client.remove_node(&text.node)?;
    client.remove_node(&visual)?;
    client.remove_image(&image)?;
    client.remove_image(&animated_image)?;
    let reason = client.close()?;
    if reason != CloseReason::ClientRequested {
        return Err(format!("unexpected close reason: {reason:?}").into());
    }

    println!(
        "CLP v{} conformance client passed.",
        client.session().protocol_version
    );
    Ok(())
}

fn manual_interaction_demo(client: &mut Client) -> Result<(), Box<dyn Error>> {
    let external_drop_enabled = client.has_capability(Capability::ExternalDrop);
    let pixels = alpha_frame_pixels(140, 90);
    let image = client.upload_image(ImageUpload::Rgba8 {
        width: 140,
        height: 90,
        pixels: &pixels,
    })?;
    let base = client
        .create_node(
            None,
            Some(SceneLayer::Overlay),
            NodeInput::Rectangle {
                properties: NodeProperties {
                    overlap_enabled: true,
                    external_drop_enabled,
                    ..NodeProperties::new(80.0, 180.0, 280.0, 180.0)
                },
                color: [37, 99, 235, 230],
            },
        )?
        .node;
    let alpha = client
        .create_node(
            None,
            Some(SceneLayer::Overlay),
            NodeInput::Image {
                properties: NodeProperties {
                    z_index: 1,
                    overlap_enabled: true,
                    external_drop_enabled,
                    ..NodeProperties::new(150.0, 240.0, 140.0, 90.0)
                },
                image: image.clone(),
                opacity: 1.0,
                contact_shape: ImageContactShape::alpha(),
            },
        )?
        .node;
    let animated_image = client.upload_image(ImageUpload::Gif {
        bytes: ALPHA_TOGGLE_GIF,
    })?;
    let animated_alpha = client
        .create_node(
            None,
            Some(SceneLayer::Overlay),
            NodeInput::Image {
                properties: NodeProperties {
                    z_index: 2,
                    overlap_enabled: true,
                    external_drop_enabled,
                    ..NodeProperties::new(300.0, 240.0, 50.0, 90.0)
                },
                image: animated_image.clone(),
                opacity: 1.0,
                contact_shape: ImageContactShape::alpha(),
            },
        )?
        .node;
    let passthrough = client
        .create_node(
            None,
            Some(SceneLayer::Overlay),
            NodeInput::Rectangle {
                properties: NodeProperties {
                    z_index: 3,
                    external_drop_enabled,
                    input_passthrough: true,
                    ..NodeProperties::new(80.0, 180.0, 280.0, 36.0)
                },
                color: [255, 255, 255, 120],
            },
        )?
        .node;

    println!("CLP v2.1 manual interaction scene is ready.");
    println!(
        "- Click the blue area or the transparent center of the orange frame: the blue node receives the event."
    );
    println!("- Click or drag the orange frame itself: the alpha image receives the event.");
    println!(
        "- The pink tile alternates between visible and transparent every 1.5 seconds; it receives events and overlaps only while visible."
    );
    println!(
        "- Click the white top band: it is visible but input_passthrough, so the blue node receives the event."
    );
    if external_drop_enabled {
        println!(
            "- Drop a desktop file on the orange frame, its transparent center, the pink tile, and the white band to inspect alpha and passthrough targeting."
        );
    } else {
        println!(
            "- This backend did not grant external_drop; pointer and overlap checks remain available."
        );
    }
    println!("Click a scene node, then press q to close only this conformance session.");

    loop {
        match client.next_notification()? {
            Notification::Event(Event::Key {
                key: Key::Character { value },
                state: KeyState::Pressed,
                ..
            }) if value.eq_ignore_ascii_case("q") => break,
            Notification::Event(Event::Scene { event }) => println!("Scene event: {event:?}"),
            Notification::Closing(reason) | Notification::Closed(reason) => {
                return Err(format!("session closed during manual validation: {reason:?}").into());
            }
            Notification::Event(_)
            | Notification::Tick(_)
            | Notification::Frame(_)
            | Notification::Resize(_)
            | Notification::DeliveryGap(_)
            | Notification::PresentationStatus(_) => {}
        }
    }

    client.remove_node(&passthrough)?;
    client.remove_node(&animated_alpha)?;
    client.remove_node(&alpha)?;
    client.remove_node(&base)?;
    client.remove_image(&animated_image)?;
    client.remove_image(&image)?;
    Ok(())
}

fn alpha_frame_pixels(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let border = x < 12 || y < 12 || x >= width - 12 || y >= height - 12;
            pixels.extend_from_slice(&[249, 115, 22, if border { 255 } else { 0 }]);
        }
    }
    pixels
}
