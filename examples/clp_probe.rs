use std::error::Error;

use chymeral_rust_sdk::{
    Capabilities, Capability, Client, CloseReason, Event, Key, KeyState, NodeContent,
    NodeProperties, NodePropertiesPatch, Notification, SceneEvent, SceneLayer, Vec2,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut client = Client::connect_from_env(Capabilities::from([
        Capability::Scene,
        Capability::Input,
        Capability::Overlap,
        Capability::ExternalDrop,
    ]))?;
    let external_drop_available = client.has_capability(Capability::ExternalDrop);
    let blue = client.create_root(
        SceneLayer::Overlay,
        NodeProperties {
            x: 420.0,
            y: 150.0,
            width: 190.0,
            height: 190.0,
            overlap_enabled: true,
            external_drop_enabled: external_drop_available,
            ..NodeProperties::new(0.0, 0.0, 0.0, 0.0)
        },
        NodeContent::Solid {
            color: [45, 135, 220, 230],
        },
    )?;
    let mut orange_position = Vec2 { x: 140.0, y: 190.0 };
    let orange = client.create_root(
        SceneLayer::Overlay,
        NodeProperties {
            x: orange_position.x,
            y: orange_position.y,
            width: 96.0,
            height: 96.0,
            overlap_enabled: true,
            ..NodeProperties::new(0.0, 0.0, 0.0, 0.0)
        },
        NodeContent::Solid {
            color: [245, 135, 35, 255],
        },
    )?;
    println!(
        "CLP v{} probe connected. Drag the orange node onto the blue node, drop a desktop file on blue, then press q to close.",
        client.session().protocol_version
    );
    if external_drop_available {
        println!(
            "The blue node is an external-drop target. Files dropped elsewhere use terminal fallback."
        );
    } else {
        println!(
            "The active Chymeral backend does not offer external file drops; the blue node still tests overlap."
        );
    }
    println!("The program only exits when q is pressed while both nodes overlap.");

    let mut drag_offset = None;
    loop {
        match client.next_notification()? {
            Notification::Event(Event::Key {
                key: Key::Character { value },
                state: KeyState::Pressed,
                ..
            }) if value.eq_ignore_ascii_case("q") => {
                let overlaps = client.query_overlaps(&orange)?;
                if overlaps.nodes.iter().any(|node| node == &blue) {
                    println!("Overlap confirmed; closing only this CLP program session.");
                    let reason = client.close()?;
                    if reason != CloseReason::ClientRequested {
                        return Err(format!("unexpected close reason: {reason:?}").into());
                    }
                    break;
                }
                println!("q ignored: the orange and blue nodes do not overlap.");
            }
            Notification::Event(Event::Scene {
                event: SceneEvent::DragStart { source, origin, .. },
            }) if source == orange => {
                drag_offset = Some(Vec2 {
                    x: origin.x - orange_position.x,
                    y: origin.y - orange_position.y,
                });
            }
            Notification::Event(Event::Scene {
                event:
                    SceneEvent::DragMove {
                        source: Some(source),
                        position,
                        ..
                    },
            }) if source == orange => {
                if let Some(offset) = drag_offset {
                    orange_position = Vec2 {
                        x: position.x - offset.x,
                        y: position.y - offset.y,
                    };
                    client.update_node(
                        &orange,
                        NodePropertiesPatch {
                            x: Some(orange_position.x),
                            y: Some(orange_position.y),
                            ..NodePropertiesPatch::default()
                        },
                    )?;
                }
            }
            Notification::Event(Event::Scene {
                event: SceneEvent::DragEnd { source, .. },
            }) if source == Some(orange.clone()) => drag_offset = None,
            Notification::Event(Event::Scene {
                event: SceneEvent::DragCancel { source, .. },
            }) if source == orange => drag_offset = None,
            Notification::Event(Event::Scene {
                event: SceneEvent::OverlapEnter { first, second },
            }) if (first == orange && second == blue) || (first == blue && second == orange) => {
                println!("Overlap entered.");
            }
            Notification::Event(Event::Scene {
                event: SceneEvent::OverlapLeave { first, second },
            }) if (first == orange && second == blue) || (first == blue && second == orange) => {
                println!("Overlap left.");
            }
            Notification::Event(Event::Scene {
                event: SceneEvent::ExternalDropEnter { target, position },
            }) if target == blue => {
                println!(
                    "External file drag entered blue node at ({:.0}, {:.0}).",
                    position.x, position.y
                );
            }
            Notification::Event(Event::Scene {
                event: SceneEvent::ExternalDropLeave { target, position },
            }) if target == blue => {
                println!(
                    "External file drag left blue node at ({:.0}, {:.0}).",
                    position.x, position.y
                );
            }
            Notification::Event(Event::Scene {
                event:
                    SceneEvent::ExternalDrop {
                        target,
                        position,
                        files,
                    },
            }) if target == blue => {
                println!(
                    "External drop on blue node at ({:.0}, {:.0}):",
                    position.x, position.y
                );
                for file in files {
                    println!("  {}", file.uri);
                }
            }
            Notification::Closing(_) => {
                let _ = client.close()?;
                break;
            }
            Notification::Closed(reason) => {
                eprintln!("Chymeral closed the probe session: {reason:?}");
                break;
            }
            _ => {}
        }
    }

    Ok(())
}
