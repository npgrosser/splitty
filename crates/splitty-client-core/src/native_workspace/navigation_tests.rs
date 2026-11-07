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
    model_where(&mut updates, |model| {
        model.shown_screen_id() != Some(screen_a.as_str())
    })
    .await;
    worker.shutdown().unwrap();
}
