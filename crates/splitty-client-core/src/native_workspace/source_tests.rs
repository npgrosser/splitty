//! Source admission through the real local worker; no remote transport fixture.
use super::*;
use uuid::Uuid;

async fn next(updates: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(Duration::from_secs(15), updates.recv())
        .await
        .expect("source update timeout")
        .expect("worker alive")
}

#[tokio::test]
async fn foreign_source_envelopes_cannot_create_split_select_input_or_enqueue_settings() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let state = runtime.state.clone();
    let (commands, receiver) = mpsc::channel(32);
    let (sender, mut updates) = mpsc::channel(64);
    let stop = Arc::new(AtomicBool::new(false));
    let cleanup = Arc::new(AtomicBool::new(false));
    let worker = tokio::spawn(run_service(
        crate::EmbeddedService::from_runtime(runtime),
        receiver,
        sender,
        stop.clone(),
        cleanup.clone(),
        ColorScheme::Dark,
    ));
    let model = loop {
        if let Update::Workspace(model) = next(&mut updates).await {
            if model.routes().len() == 1 && model.session_ready(&model.routes()[0].session_id) {
                break model;
            }
        }
    };
    let owner = model.source_token();
    let inactive_pane = model.routes()[0].pane_id.clone();
    commands
        .send(Command::new(
            owner,
            CommandKind::Tab {
                screen: model.screen_key(model.shown_screen_id().unwrap()),
                pane: inactive_pane.clone(),
            },
        ))
        .await
        .unwrap();
    let model = loop {
        if let Update::Workspace(model) = next(&mut updates).await {
            if model.routes().len() == 2
                && model
                    .routes()
                    .iter()
                    .all(|route| model.session_ready(&route.session_id))
                && model
                    .focused_pane()
                    .is_some_and(|pane| pane != inactive_pane)
            {
                break model;
            }
        }
    };
    let screen = model.shown_screen_id().unwrap();
    let route = model.terminal_token(model.focused_pane().unwrap()).unwrap();
    let mut foreign = LocalWorkspace::new(Uuid::new_v4());
    foreign.reset_connection(owner.connection_id());
    let foreign_owner = foreign.source_token();
    assert_eq!(foreign_owner.connection_id(), owner.connection_id());
    assert_ne!(foreign_owner.source_id(), owner.source_id());
    let zoom_request = Uuid::new_v4();
    for kind in [
        CommandKind::CreateScreen("foreign".into()),
        CommandKind::Split {
            screen: model.screen_key(screen),
            pane: route.pane_id().into(),
            axis: Axis::Horizontal,
        },
        CommandKind::Select {
            screen: model.screen_key(screen),
            pane: inactive_pane,
        },
        CommandKind::Input {
            route: route.clone(),
            bytes: b"printf 'FOREIGN_EFFECT'\n".to_vec(),
        },
        CommandKind::Zoom {
            connection: owner.connection_id(),
            request_id: zoom_request,
            action: ZoomAction::In,
        },
    ] {
        commands
            .send(Command::new(foreign_owner, kind))
            .await
            .unwrap();
    }
    // A valid envelope cannot upgrade an old nested terminal target either.
    let old = TerminalToken::new(
        SourceToken::initial(Uuid::new_v4()),
        route.pane_id(),
        route.session_id(),
    );
    commands
        .send(Command::new(
            owner,
            CommandKind::Input {
                route: old,
                bytes: b"printf 'OLD_EFFECT'\n".to_vec(),
            },
        ))
        .await
        .unwrap();
    commands
        .send(Command::new(
            owner,
            CommandKind::Input {
                route: route.clone(),
                bytes: b"printf 'SOURCE_%s\\n' 'ALIVE'\n".to_vec(),
            },
        ))
        .await
        .unwrap();
    let mut output = Vec::new();
    let mut rejected = 0;
    let mut zoom_rejected = false;
    while rejected < 5 || !zoom_rejected || !output.windows(12).any(|part| part == b"SOURCE_ALIVE")
    {
        match next(&mut updates).await {
            Update::Status(reason) if reason.contains("source connection") => rejected += 1,
            Update::Zoom(result) if result.request_id == zoom_request => {
                assert!(matches!(
                    result.outcome,
                    crate::zoom::ZoomOutcome::Rejected { .. }
                ));
                zoom_rejected = true;
            }
            Update::Event { source, event } => {
                assert_eq!(source, owner);
                assert_eq!(event.connection_id, source.connection_id());
                if let Some(chunk) = event.output() {
                    if chunk.session_id == route.session_id() {
                        output.extend_from_slice(chunk.bytes);
                    }
                }
            }
            Update::Attached { source, .. } => assert_eq!(source, owner),
            _ => {}
        }
    }
    assert!(!String::from_utf8_lossy(&output).contains("FOREIGN_EFFECT"));
    assert!(!String::from_utf8_lossy(&output).contains("OLD_EFFECT"));
    assert_eq!(state.mux_store.workspace_catalog().0.len(), 1);
    let snapshot = state
        .mux_store
        .get_screen(&splitty_core::ids::ScreenId::from(screen))
        .unwrap();
    assert_eq!(snapshot.panes.len(), 2);
    let splitty_core::mux::LayoutNode::Slot { active_pane_id, .. } = &snapshot.root else {
        panic!("foreign split must not change the original slot");
    };
    assert_eq!(
        active_pane_id.as_ref().map(|pane| pane.as_str()),
        Some(route.pane_id()),
        "foreign Select must not activate the other existing tab"
    );
    stop.store(true, Ordering::Release);
    worker.await.unwrap().unwrap();
    assert!(cleanup.load(Ordering::Acquire));
    assert!(!path.exists());
}

#[tokio::test]
async fn queued_event_envelope_retains_charge_until_consumer_drops_it() {
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service = crate::EmbeddedService::from_runtime(runtime);
    let mut client = service.connect().await.unwrap();
    let budget = client.sender().context().budget;
    let source = SourceToken::initial(client.connection_id());
    let event = client.next_event().await.unwrap().unwrap();
    assert_eq!(event.json().unwrap()["type"], "mux.welcome");
    let (sender, mut updates) = mpsc::channel(1);
    sender
        .send(Update::Event {
            source,
            event: Box::new(event),
        })
        .await
        .ok()
        .unwrap();
    // Drain all transport ownership while the queued view update remains alive.
    // Its frame and decoded JSON must remain charged independently of the client.
    tokio::time::timeout(Duration::from_secs(15), async {
        client.finish().await.unwrap();
        service.close().await.unwrap();
    })
    .await
    .expect("cleanup cannot depend on consuming a queued view update");
    let retained = budget.used();
    assert!(retained > 0, "queued Event owns its charged allocations");
    let update = updates.recv().await.unwrap();
    let Update::Event { source, event } = &update else {
        panic!("expected event envelope");
    };
    assert_eq!(source.connection_id(), event.connection_id);
    assert_eq!(
        budget.used(),
        retained,
        "dequeue does not release the lease"
    );
    drop(update);
    assert_eq!(budget.used(), 0, "consumer drop releases the entire lease");
    assert!(!path.exists());
}

#[tokio::test]
async fn startup_close_consent_survives_workspace_publication_without_ui_consumption() {
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let state = runtime.state.clone();
    let (commands, receiver) = mpsc::channel(8);
    let (sender, mut updates) = mpsc::channel(64);
    let stop = Arc::new(AtomicBool::new(false));
    let cleanup = Arc::new(AtomicBool::new(false));
    let worker = tokio::spawn(run_service(
        crate::EmbeddedService::from_runtime(runtime),
        receiver,
        sender,
        stop,
        cleanup.clone(),
        ColorScheme::Dark,
    ));
    // No update has been consumed: the view can still hold its startup prompt.
    tokio::time::timeout(Duration::from_secs(15), async {
        while state.mux_store.workspace_catalog().0.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("workspace discovery before startup prompt resolves");
    commands
        .send(Command::close(ClosePlan::startup_window(1), false))
        .await
        .unwrap();
    loop {
        if let Update::CloseRejected { request_id: 1, .. } = next(&mut updates).await {
            break;
        }
    }
    assert!(
        !cleanup.load(Ordering::Acquire),
        "unconfirmed startup close cannot end the ready service"
    );
    commands
        .send(Command::close(ClosePlan::startup_window(2), true))
        .await
        .unwrap();
    loop {
        if let Update::WindowCloseApproved {
            request_id: 2,
            source,
        } = next(&mut updates).await
        {
            assert_eq!(
                source, None,
                "approval matches the original channel-owned prompt"
            );
            break;
        }
    }
    worker.await.unwrap().unwrap();
    assert!(cleanup.load(Ordering::Acquire));
    assert!(!path.exists());
}

#[test]
fn absent_or_rewrapped_authority_never_admits_a_non_lifecycle_intent() {
    let current = LocalWorkspace::new(Uuid::new_v4());
    let missing = Command {
        source: None,
        kind: CommandKind::CreateScreen("x".into()),
    };
    assert!(missing.validate_source(current.source_token()).is_err());
    let foreign = LocalWorkspace::new(Uuid::new_v4());
    let old_plan = foreign.assess_close(1, CloseTarget::Window).unwrap();
    let rewrapped = Command::new(
        current.source_token(),
        CommandKind::RequestClose {
            plan: old_plan,
            confirmed: true,
        },
    );
    assert!(rewrapped.validate_source(current.source_token()).is_err());
    match rejected_command(
        Some(foreign.source_token()),
        rewrapped.kind(),
        "stale".into(),
    ) {
        Update::CloseRejected {
            source, request_id, ..
        } => {
            assert_eq!(
                source,
                Some(foreign.source_token()),
                "a rejection retains its command's captured owner"
            );
            assert_eq!(request_id, 1);
            assert_ne!(source, Some(current.source_token()));
        }
        _ => panic!("close rejection expected"),
    }
}
