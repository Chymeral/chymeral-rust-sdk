use std::{
    error::Error,
    time::{Duration, Instant},
};

use chymeral_rust_sdk::{
    Capabilities, Client, CloseReason, NodeContent, NodeProperties, NodePropertiesPatch,
    Notification, SceneLayer, Subscription,
};

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

    let texture = client.upload_texture(1, 1, &[30, 150, 240, 255])?;
    let visual = client.create_root(
        SceneLayer::Overlay,
        NodeProperties::new(16.0, 16.0, 1.0, 1.0),
        NodeContent::Image {
            texture: texture.clone(),
            opacity: 1.0,
        },
    )?;

    let collider = client.create_root(
        SceneLayer::Overlay,
        NodeProperties {
            visible: false,
            overlap_enabled: true,
            ..NodeProperties::new(220.0, 80.0, 40.0, 40.0)
        },
        NodeContent::Empty,
    )?;
    let mover = client.create_root(
        SceneLayer::Overlay,
        NodeProperties {
            visible: false,
            overlap_enabled: true,
            ..NodeProperties::new(80.0, 80.0, 40.0, 40.0)
        },
        NodeContent::Empty,
    )?;

    client.update_node(
        &mover,
        NodePropertiesPatch {
            x: Some(230.0),
            ..NodePropertiesPatch::default()
        },
    )?;
    let overlaps = client.query_overlaps(&mover)?;
    if !overlaps.overlapping || !overlaps.nodes.iter().any(|node| node == &collider) {
        return Err("overlap query did not report the enabled same-session collider".into());
    }

    client.remove_node(&mover)?;
    client.remove_node(&collider)?;
    client.remove_node(&visual)?;
    client.destroy_texture(&texture)?;
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
