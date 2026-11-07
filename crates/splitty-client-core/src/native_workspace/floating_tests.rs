//! Supported floating actions against the real owned local service.
use super::*;
use splitty_core::pty::Backend as _;
use uuid::Uuid;
async fn next(updates: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(Duration::from_secs(15), updates.recv())
        .await
        .expect("floating update timeout")
        .expect("worker alive")
}
async fn model_where(
    updates: &mut mpsc::Receiver<Update>,
    test: impl Fn(&LocalWorkspace) -> bool,
) -> Box<LocalWorkspace> {
    loop {
        if let Update::Workspace(model) = next(updates).await {
            if test(&model) {
                return model;
            }
        }
    }
}
async fn settled(
    updates: &mut mpsc::Receiver<Update>,
    request: Uuid,
    session: &str,
) -> Box<LocalWorkspace> {
    let mut latest = None;
    loop {
        match next(updates).await {
            Update::Workspace(model) => latest = Some(model),
            Update::Attached { session: id, .. } if id == session => {
                panic!("floating action reattached unchanged terminal")
            }
            Update::FloatingAction(result) if result.request_id == request => {
                assert_eq!(result.outcome, FloatingActionOutcome::Confirmed);
                return latest.expect("model published before result");
            }
            _ => {}
        }
    }
}
#[tokio::test]
async fn floating_tabs_move_and_dock_into_empty_root_keep_live_sessions_and_profile() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let pty = runtime.pty.clone();
    let service = crate::EmbeddedService::from_runtime(runtime);
    let (commands, receiver) = mpsc::channel(64);
    let (sender, mut updates) = mpsc::channel(64);
    let stop = Arc::new(AtomicBool::new(false));
    let cleanup = Arc::new(AtomicBool::new(false));
    let worker = tokio::spawn(run_service(
        service,
        receiver,
        sender,
        stop,
        cleanup.clone(),
        ColorScheme::Dark,
    ));
    let initial = model_where(&mut updates, |m| {
        m.routes().len() == 1 && m.routes().iter().all(|r| m.session_ready(&r.session_id))
    })
    .await;
    let first = initial.routes()[0].clone();
    let scope = initial.source_token();
    commands
        .send(Command::new(
            scope,
            CommandKind::Tab {
                screen: initial.screen_key(initial.shown_screen_id().unwrap()),
                pane: first.pane_id.clone(),
            },
        ))
        .await
        .unwrap();
    let root = model_where(&mut updates, |m| {
        m.routes().len() == 2 && m.routes().iter().all(|r| m.session_ready(&r.session_id))
    })
    .await;
    let moving = root
        .routes()
        .into_iter()
        .find(|r| r.pane_id != first.pane_id)
        .unwrap();
    // Ready model is published just before the initial attachment notification.
    // Drain that notification before testing that subsequent moves never reattach.
    loop {
        if matches!(next(&mut updates).await, Update::Attached { session: id, .. } if id == moving.session_id)
        {
            break;
        }
    }
    let process = pty.get(&moving.session_id).unwrap();
    let request = Uuid::new_v4();
    commands
        .send(Command::new(
            scope,
            CommandKind::FloatPane {
                request_id: request,
                source: root.capture_pane_action(&moving.pane_id).unwrap(),
            },
        ))
        .await
        .unwrap();
    let floated = settled(&mut updates, request, &moving.session_id).await;
    assert!(floated.is_displayed_pane(&first.pane_id));
    assert!(floated.is_displayed_pane(&moving.pane_id));
    assert_eq!(floated.focused_pane(), Some(moving.pane_id.as_str()));
    assert!(Arc::ptr_eq(&process, &pty.get(&moving.session_id).unwrap()));
    let float_id = floated
        .floating_for_pane(&moving.pane_id)
        .unwrap()
        .id
        .to_string();
    commands
        .send(Command::new(
            scope,
            CommandKind::Tab {
                screen: floated.screen_key(floated.shown_screen_id().unwrap()),
                pane: moving.pane_id.clone(),
            },
        ))
        .await
        .unwrap();
    let stacked = model_where(&mut updates, |m| {
        m.routes().len() == 3
            && m.routes().iter().all(|r| m.session_ready(&r.session_id))
            && m.focused_pane()
                .is_some_and(|p| p != moving.pane_id && p != first.pane_id)
    })
    .await;
    let added = stacked
        .routes()
        .into_iter()
        .find(|r| r.pane_id != moving.pane_id && r.pane_id != first.pane_id)
        .unwrap();
    assert_eq!(
        stacked
            .floating_for_pane(&added.pane_id)
            .unwrap()
            .id
            .as_str(),
        float_id
    );
    assert_eq!(
        pty.get(&added.session_id).unwrap().shell_profile_id(),
        process.shell_profile_id()
    );
    let request = Uuid::new_v4();
    let bounds = FloatBounds {
        x: 0.1,
        y: 0.15,
        w: 0.7,
        h: 0.6,
    };
    commands
        .send(Command::new(
            scope,
            CommandKind::MoveFloat {
                request_id: request,
                target: stacked.capture_float(&float_id).unwrap(),
                bounds: bounds.clone(),
            },
        ))
        .await
        .unwrap();
    let moved = settled(&mut updates, request, &moving.session_id).await;
    assert_eq!(moved.floating(&float_id).unwrap().bounds, bounds);
    let slot = moved.focused_slot_id().unwrap().to_owned();
    commands
        .send(Command::new(
            scope,
            CommandKind::SetMaximized {
                screen: moved.shown_screen_id().unwrap().into(),
                connection: moved.connection_id(),
                slot: Some(slot.clone()),
                pane: Some(added.pane_id.clone()),
            },
        ))
        .await
        .unwrap();
    let maximized = model_where(&mut updates, |m| {
        m.maximized_slot_id() == Some(slot.as_str())
    })
    .await;
    assert_eq!(
        maximized.displayed_panes().as_slice(),
        std::slice::from_ref(&added.pane_id)
    );
    assert!(maximized.presented_floats().is_empty());
    commands
        .send(Command::new(
            scope,
            CommandKind::SetMaximized {
                screen: maximized.shown_screen_id().unwrap().into(),
                connection: maximized.connection_id(),
                slot: None,
                pane: None,
            },
        ))
        .await
        .unwrap();
    let restored = model_where(&mut updates, |m| m.maximized_slot_id().is_none()).await;
    let close = restored
        .assess_close(
            10,
            CloseTarget::Pane {
                screen: restored.shown_screen_id().unwrap().into(),
                pane: first.pane_id.clone(),
            },
        )
        .unwrap();
    commands
        .send(Command {
            source: Some(scope),
            kind: CommandKind::RequestClose {
                plan: close,
                confirmed: true,
            },
        })
        .await
        .unwrap();
    let mut empty = None;
    let mut close_settled = false;
    while empty.is_none() || !close_settled {
        match next(&mut updates).await {
            Update::Workspace(model)
                if model
                    .snapshot()
                    .is_some_and(|s| s.root.collect_pane_ids().is_empty())
                    && !model.routes_reconciling() =>
            {
                empty = Some(model)
            }
            Update::CloseSettled { request_id: 10, .. } => close_settled = true,
            Update::CloseRejected { reason, .. } => panic!("close rejected: {reason}"),
            _ => {}
        }
    }
    let empty = empty.unwrap();
    assert_eq!(empty.routes().len(), 2);
    let request = Uuid::new_v4();
    commands
        .send(Command::new(
            scope,
            CommandKind::DockFloat {
                request_id: request,
                target: empty.capture_float(&float_id).unwrap(),
            },
        ))
        .await
        .unwrap();
    let docked = settled(&mut updates, request, &moving.session_id).await;
    assert!(docked.snapshot().unwrap().floating.is_empty());
    assert_eq!(docked.routes().len(), 2);
    assert_eq!(docked.snapshot().unwrap().root.collect_pane_ids().len(), 2);
    assert!(Arc::ptr_eq(&process, &pty.get(&moving.session_id).unwrap()));
    commands
        .send(Command::new(
            scope,
            CommandKind::Input {
                route: TerminalToken::new(scope, &moving.pane_id, &moving.session_id),
                bytes: b"printf 'FLOAT_%s\\n' 'ALIVE'\n".into(),
            },
        ))
        .await
        .unwrap();
    let mut bytes = Vec::new();
    loop {
        if let Update::Event { event, .. } = next(&mut updates).await {
            if let Some(chunk) = event.output() {
                if chunk.session_id == moving.session_id {
                    assert!(!chunk.replay);
                    bytes.extend_from_slice(chunk.bytes);
                    if bytes.windows(11).any(|part| part == b"FLOAT_ALIVE") {
                        break;
                    }
                }
            }
        }
    }
    commands
        .send(Command {
            source: Some(scope),
            kind: CommandKind::RequestClose {
                plan: docked.assess_close(20, CloseTarget::Window).unwrap(),
                confirmed: true,
            },
        })
        .await
        .unwrap();
    loop {
        if matches!(
            next(&mut updates).await,
            Update::WindowCloseApproved { request_id: 20, .. }
        ) {
            break;
        }
    }
    worker.await.unwrap().unwrap();
    assert!(cleanup.load(Ordering::Acquire));
    assert!(pty.get(&moving.session_id).is_none());
    assert!(!path.exists());
}
