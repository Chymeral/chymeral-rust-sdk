use std::{
    error::Error,
    time::{Duration, Instant},
};

use chymeral_rust_sdk::{
    Capabilities, Client, CloseReason, ImageUpload, NodeChanges, NodeInput, NodeProperties,
    NodePropertiesPatch, NodeVisual, Notification, SceneLayer, Subscription, TextNodeProperties,
    TextStyle,
};

const ONE_PIXEL_GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x00\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";

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
    let visual = client
        .create_node(
            None,
            Some(SceneLayer::Overlay),
            NodeInput::Image {
                properties: NodeProperties::new(16.0, 16.0, 1.0, 1.0),
                image: image.clone(),
                opacity: 1.0,
            },
        )?
        .node;
    let animated_image = client.upload_image(ImageUpload::Gif {
        bytes: ONE_PIXEL_GIF,
    })?;
    client.set_node(
        &visual,
        NodeChanges {
            visual: Some(NodeVisual::Image {
                image: animated_image.clone(),
                opacity: 0.75,
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
