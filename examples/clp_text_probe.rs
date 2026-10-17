use std::error::Error;

use chymeral_rust_sdk::{
    Capabilities, Capability, Client, CloseReason, Event, Key, KeyState, NodeChanges, NodeInput,
    NodeProperties, Notification, SceneLayer, TextAlign, TextNodeProperties, TextStyle, TextWeight,
};

fn main() -> Result<(), Box<dyn Error>> {
    let font_alias = std::env::var("CHYMERAL_TEXT_PROBE_FONT_ALIAS")
        .ok()
        .filter(|alias| !alias.is_empty());
    let mut client = Client::connect_from_env(Capabilities::from([
        Capability::Scene,
        Capability::Input,
        Capability::Text,
    ]))?;
    let panel = client
        .create_node(
            None,
            Some(SceneLayer::Overlay),
            NodeInput::Rectangle {
                properties: NodeProperties::new(48.0, 72.0, 620.0, 220.0),
                color: [20, 34, 56, 224],
            },
        )?
        .node;
    let title = client.create_node(
        Some(&panel),
        None,
        NodeInput::Text {
            properties: TextNodeProperties::new(24.0, 20.0),
            text: "Chymeral intrinsic text\nPress U to update, B for bold, or Q to close."
                .to_owned(),
            style: TextStyle {
                font: font_alias.clone(),
                ..TextStyle::default()
            },
        },
    )?;
    let sample = client.create_node(
        Some(&panel),
        None,
        NodeInput::Text {
            properties: TextNodeProperties::new(24.0, 100.0),
            text: "Tabs use terminal cells: A\tB\nWide Unicode cell: 界".to_owned(),
            style: TextStyle {
                font: font_alias.clone(),
                color: Some([105, 210, 255, 255]),
                weight: TextWeight::Normal,
                align: TextAlign::Center,
                ..TextStyle::default()
            },
        },
    )?;
    let title_metrics = title
        .text_metrics
        .expect("a created text node reports intrinsic metrics");
    let sample_metrics = sample
        .text_metrics
        .expect("a created text node reports intrinsic metrics");
    println!(
        "CLP v{} text probe connected. Title: {} columns × {} rows; sample: {} columns × {} rows.",
        client.session().protocol_version,
        title_metrics.columns,
        title_metrics.rows,
        sample_metrics.columns,
        sample_metrics.rows,
    );
    match font_alias.as_deref() {
        Some(alias) => println!(
            "Text requests configured font alias {alias:?}. Confirm the panel, its text-child offsets, Unicode, and tab spacing. Press q to close only this session."
        ),
        None => println!(
            "Text uses terminal font defaults. Confirm the panel, its text-child offsets, Unicode, and tab spacing. Press q to close only this session."
        ),
    }

    loop {
        match client.next_notification()? {
            Notification::Event(Event::Key {
                key: Key::Character { value },
                state: KeyState::Pressed,
                ..
            }) if value.eq_ignore_ascii_case("u") => {
                let metrics = client
                    .set_node(
                        &title.node,
                        NodeChanges {
                            text: Some(
                                "Updated text node\nIts intrinsic rectangle was recomputed."
                                    .to_owned(),
                            ),
                            ..NodeChanges::default()
                        },
                    )?
                    .expect("text changes report intrinsic metrics");
                println!(
                    "Text updated: {} columns × {} rows ({} × {} logical pixels).",
                    metrics.columns, metrics.rows, metrics.width, metrics.height
                );
            }
            Notification::Event(Event::Key {
                key: Key::Character { value },
                state: KeyState::Pressed,
                ..
            }) if value.eq_ignore_ascii_case("b") => {
                let metrics = client
                    .set_node(
                        &sample.node,
                        NodeChanges {
                            text_style: Some(TextStyle {
                                font: font_alias.clone(),
                                color: Some([255, 196, 94, 255]),
                                weight: TextWeight::Bold,
                                align: TextAlign::End,
                                ..TextStyle::default()
                            }),
                            ..NodeChanges::default()
                        },
                    )?
                    .expect("text-style changes report intrinsic metrics");
                println!(
                    "Sample style updated: {} columns × {} rows.",
                    metrics.columns, metrics.rows
                );
            }
            Notification::Event(Event::Key {
                key: Key::Character { value },
                state: KeyState::Pressed,
                ..
            }) if value.eq_ignore_ascii_case("q") => {
                let reason = client.close()?;
                if reason != CloseReason::ClientRequested {
                    return Err(format!("unexpected close reason: {reason:?}").into());
                }
                break;
            }
            Notification::Closing(_) => {
                let _ = client.close()?;
                break;
            }
            Notification::Closed(reason) => {
                eprintln!("Chymeral closed the text probe session: {reason:?}");
                break;
            }
            _ => {}
        }
    }

    Ok(())
}
