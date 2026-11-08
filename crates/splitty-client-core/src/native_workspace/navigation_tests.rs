//! Navigation intents cross the worker without relying on a fresh UI model.
use super::*;
use crate::workspace::SourceWorkspace;
use splitty_core::ids::PaneId;
use std::time::Duration;

async fn next(updates: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(Duration::from_secs(15), updates.recv())
        .await
        .expect("navigation update timeout")
        .expect("worker alive")
}

async fn model_where(
    updates: &mut mpsc::Receiver<Update>,
    test: impl Fn(&SourceWorkspace) -> bool,
) -> Box<SourceWorkspace> {
    loop {
        if let Update::Workspace(model) = next(updates).await {
            let model = local_projection(&model);
            if test(&model) {
                return model;
            }
        }
    }
}

async fn rejected(updates: &mut mpsc::Receiver<Update>, reason: &str) {
    loop {
        if let Update::Status(status) = next(updates).await {
            if status.contains(reason) {
                return;
            }
        }
    }
}

#[tokio::test]
async fn worker_steps_latest_screen_and_hovered_group_without_retargeting() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (commands, receiver) = mpsc::channel(64);
    let (sender, mut updates) = mpsc::channel(64);
    let worker = Worker::start(receiver, sender);
    let first = model_where(&mut updates, |model| {
        model.catalog_initialized()
            && model.screens().len() == 1
            && model.routes().len() == 1
            && model
                .routes()
                .iter()
                .all(|route| model.session_ready(&route.session_id))
    })
    .await;
    let owner = first.connection_id();
    let scope = first.source_token();
    let screen_a = first.shown_screen_id().unwrap().to_owned();
    let pane_a = first.routes()[0].pane_id.clone();
    commands
        .send(Command::new(
            scope,
            CommandKind::Split {
                screen: first.screen_key(&screen_a),
                pane: pane_a.clone(),
                axis: Axis::Horizontal,
            },
        ))
        .await
        .unwrap();
    let split = model_where(&mut updates, |model| {
        model.routes().len() == 2
            && model
                .routes()
                .iter()
                .all(|route| model.session_ready(&route.session_id))
    })
    .await;
    let pane_b = split
        .routes()
        .into_iter()
        .find(|route| route.pane_id != pane_a)
        .unwrap()
        .pane_id;
    // Focus is worker-owned presentation intent; it carries the exact session,
    // and an unrelated publication must retain its result.
    commands
        .send(Command::new(
            scope,
            CommandKind::Focus {
                screen: split.screen_key(&screen_a),
                route: split.terminal_token(&pane_a).unwrap(),
            },
        ))
        .await
        .unwrap();
    model_where(&mut updates, |model| {
        model.focused_pane() == Some(pane_a.as_str())
    })
    .await;
    commands
        .send(Command::new(
            scope,
            CommandKind::Tab {
                screen: first.screen_key(&screen_a),
                pane: pane_b.clone(),
            },
        ))
        .await
        .unwrap();
    let tabs = model_where(&mut updates, |model| {
        model.routes().len() == 3
            && model
                .routes()
                .iter()
                .all(|route| model.session_ready(&route.session_id))
    })
    .await;
    let slot = tabs
        .snapshot()
        .unwrap()
        .root
        .find_slot(&PaneId::from(pane_b.as_str()))
        .unwrap();
    let slot_id = slot.id().to_string();
    let splitty_core::mux::LayoutNode::Slot { pane_ids, .. } = slot else {
        unreachable!("find_slot returns a slot");
    };
    let selected = tabs.selected_pane(slot).unwrap();
    let target = pane_ids
        .iter()
        .find(|pane| pane.as_str() != selected)
        .unwrap()
        .to_string();
    let step = if pane_ids[0].as_str() == selected {
        TabStep::Next
    } else {
        TabStep::Previous
    };
    let sessions: HashMap<_, _> = tabs
        .routes()
        .into_iter()
        .map(|route| (route.pane_id, route.session_id))
        .collect();
    commands
        .send(Command::new(
            scope,
            CommandKind::Select {
                screen: first.screen_key(&screen_a),
                pane: pane_a.clone(),
            },
        ))
        .await
        .unwrap();
    model_where(&mut updates, |model| {
        model.focused_pane() == Some(pane_a.as_str())
    })
    .await;
    commands
        .send(Command::new(
            scope,
            CommandKind::CycleTabInSlot {
                connection: owner,
                screen: screen_a.clone(),
                slot: slot_id.clone(),
                step,
            },
        ))
        .await
        .unwrap();
    let hovered = model_where(&mut updates, |model| {
        model.focused_pane() == Some(target.as_str())
    })
    .await;
    for (pane, session) in &sessions {
        assert_eq!(hovered.session_for(pane), Some(session.as_str()));
    }
    commands
        .send(Command::new(
            scope,
            CommandKind::CreateScreen("Second".into()),
        ))
        .await
        .unwrap();
    let second = model_where(&mut updates, |model| {
        model.screens().len() == 2 && model.shown_screen_id() != Some(screen_a.as_str())
    })
    .await;
    let screen_b = second.shown_screen_id().unwrap().to_owned();
    commands
        .send(Command::new(
            scope,
            CommandKind::CreateScreen("Third".into()),
        ))
        .await
        .unwrap();
    let third = model_where(&mut updates, |model| model.screens().len() == 3).await;
    let screen_c = third.shown_screen_id().unwrap().to_owned();
    assert_ne!(screen_b, screen_c);
    // Both commands are queued before the view receives either result.
    for _ in 0..2 {
        commands
            .send(Command::new(
                scope,
                CommandKind::CycleScreen {
                    step: TabStep::Previous,
                },
            ))
            .await
            .unwrap();
    }
    let returned = model_where(&mut updates, |model| {
        model.shown_screen_id() == Some(screen_a.as_str())
    })
    .await;
    assert_eq!(returned.focused_pane(), Some(target.as_str()));
    for (pane, session) in &sessions {
        assert_eq!(returned.session_for(pane), Some(session.as_str()));
    }
    commands
        .send(Command::new(
            SourceWorkspace::new(uuid::Uuid::new_v4()).source_token(),
            CommandKind::CycleScreen {
                step: TabStep::Next,
            },
        ))
        .await
        .unwrap();
    rejected(&mut updates, "source connection").await;
    commands
        .send(Command::new(
            scope,
            CommandKind::CycleTabInSlot {
                connection: owner,
                screen: screen_b,
                slot: slot_id,
                step: TabStep::Next,
            },
        ))
        .await
        .unwrap();
    rejected(&mut updates, "previous Screen").await;
    commands
        .send(Command::new(
            scope,
            CommandKind::CycleScreen {
                step: TabStep::Next,
            },
        ))
        .await
        .unwrap();
    let navigated = model_where(&mut updates, |model| {
        model.shown_screen_id() != Some(screen_a.as_str())
    })
    .await;
    let selected_before = navigated.shown_screen_id().unwrap().to_owned();
    commands
        .send(Command::new(
            scope,
            CommandKind::MoveScreen {
                screen: navigated.screen_key(&screen_c),
                before: Some(navigated.screen_key(&screen_a)),
            },
        ))
        .await
        .unwrap();
    let moved = model_where(&mut updates, |model| {
        model
            .screens()
            .first()
            .is_some_and(|entry| entry.id.as_str() == screen_c)
    })
    .await;
    assert_eq!(moved.shown_screen_id(), Some(selected_before.as_str()));
    for (pane, session) in &sessions {
        assert_eq!(moved.session_for(pane), Some(session.as_str()));
    }
    commands
        .send(Command::new(
            SourceWorkspace::new(uuid::Uuid::new_v4()).source_token(),
            CommandKind::MoveScreen {
                screen: moved.screen_key(&screen_a),
                before: None,
            },
        ))
        .await
        .unwrap();
    rejected(&mut updates, "source connection").await;
    // Creation reaches the same ordered worker before placement captured from
    // the previous catalog; relative placement must not lose the fourth Screen.
    commands
        .send(Command::new(
            scope,
            CommandKind::CreateScreen("Fourth".into()),
        ))
        .await
        .unwrap();
    commands
        .send(Command::new(
            scope,
            CommandKind::MoveScreen {
                screen: moved.screen_key(&screen_a),
                before: None,
            },
        ))
        .await
        .unwrap();
    let final_model = model_where(&mut updates, |model| {
        model.screens().len() == 4
            && model
                .screens()
                .last()
                .is_some_and(|entry| entry.id.as_str() == screen_a)
    })
    .await;
    assert_eq!(final_model.screens()[0].id.as_str(), screen_c);
    assert_eq!(final_model.screens()[2].name.as_deref(), Some("Fourth"));
    assert_eq!(
        final_model.shown_screen_id(),
        Some(final_model.screens()[2].id.as_str())
    );
    for (pane, session) in &sessions {
        assert_eq!(final_model.session_for(pane), Some(session.as_str()));
    }
    let route = final_model.terminal_token(&pane_a).unwrap();
    commands
        .send(Command::new(
            scope,
            CommandKind::Input {
                route: route.clone(),
                bytes: b"printf 'ORDER_%s\\n' OUTPUT_OK\r".to_vec(),
            },
        ))
        .await
        .unwrap();
    let mut output = Vec::new();
    loop {
        if let Update::Event { event, .. } = next(&mut updates).await {
            if let Some(chunk) = event.output() {
                if chunk.session_id == route.session_id() {
                    output.extend_from_slice(chunk.bytes);
                    if output
                        .windows(b"ORDER_OUTPUT_OK".len())
                        .any(|bytes| bytes == b"ORDER_OUTPUT_OK")
                    {
                        break;
                    }
                    assert!(output.len() < 65536, "bounded terminal evidence");
                }
            }
        }
    }
    worker.shutdown().unwrap();
}

#[tokio::test]
async fn remote_screen_order_dispatch_requires_no_live_transport() {
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let client = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(client.connection_id());
    let owner = window.lifecycle_token();
    let keys = window
        .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B")], Some("offline-order.test"))
        .unwrap();
    let id = keys[0].source_id();
    window.disconnect(id).unwrap();
    window.finish_attempt(id).unwrap();
    client.sender().close();
    assert!(client.request_workspace().is_err());
    let local = source::SourceRuntime::new(client, owner);
    let selected = window.shown_key();
    assert!(move_window_screen(&mut window, &local, owner, &keys[1], Some(&keys[0])).unwrap());
    assert_eq!(
        window
            .opened()
            .iter()
            .map(|entry| entry.key().screen_id())
            .collect::<Vec<_>>(),
        ["b", "a"]
    );
    assert_eq!(window.shown_key(), selected);
    assert_eq!(
        window.source_status(id),
        Some(crate::workspace::SourceStatus::Offline)
    );
    assert!(!move_window_screen(&mut window, &local, owner, &keys[1], Some(&keys[0])).unwrap());
    local.client.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}
