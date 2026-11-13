//! Exercise authoritative transfers through the owned worker and live PTYs.
use super::*;
use crate::workspace::SourceWorkspace;
use crate::workspace::{CloseAffected, CloseIdentity};
use splitty_core::{
    ids::{PaneId, ScreenId},
    mux::Op,
    pty::Backend as _,
};
use uuid::Uuid;

async fn next(updates: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(Duration::from_secs(15), updates.recv())
        .await
        .expect("transfer update timeout")
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
async fn confirmed(
    updates: &mut mpsc::Receiver<Update>,
    request: Uuid,
    pane: &str,
    session: &str,
) -> Box<SourceWorkspace> {
    let mut latest = None;
    loop {
        match next(updates).await {
            Update::Workspace(model) => {
                let model = local_projection(&model);
                assert_eq!(
                    model.session_for(pane),
                    Some(session),
                    "no transient route loss"
                );
                latest = Some(model);
            }
            Update::Attached { session: id, .. } if id == session => {
                panic!("unchanged session reattached")
            }
            Update::ScreenTransfer(result) if result.request_id == request => {
                assert_eq!(result.outcome, ScreenTransferOutcome::Confirmed);
                return latest.expect("model published before outcome");
            }
            _ => {}
        }
    }
}
async fn output(updates: &mut mpsc::Receiver<Update>, session: &str) {
    let mut bytes = Vec::new();
    loop {
        match next(updates).await {
            Update::Event { event, .. } => {
                if let Some(chunk) = event.output() {
                    if chunk.session_id == session {
                        assert!(!chunk.replay, "transfer introduced replay");
                        bytes.extend_from_slice(chunk.bytes);
                        if bytes.windows(14).any(|part| part == b"TRANSFER_ALIVE") {
                            return;
                        }
                    }
                }
            }
            Update::Attached { session: id, .. } if id == session => {
                panic!("unchanged session reattached")
            }
            _ => {}
        }
    }
}
#[tokio::test]
async fn worker_transfers_existing_new_and_external_screens_without_reattach_or_process_change() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let state = runtime.state.clone();
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
        stop.clone(),
        cleanup.clone(),
        ColorScheme::Dark,
    ));
    let first = model_where(&mut updates, |m| {
        m.routes().len() == 1 && m.routes().iter().all(|r| m.session_ready(&r.session_id))
    })
    .await;
    let scope = first.source_token();
    let screen_a = first.shown_screen_id().unwrap().to_owned();
    let first_pane = first.routes()[0].pane_id.clone();
    commands
        .send(Command::new(
            scope,
            CommandKind::Tab {
                screen: first.screen_key(&screen_a),
                pane: first_pane.clone(),
            },
        ))
        .await
        .unwrap();
    let tabs = model_where(&mut updates, |m| {
        m.routes().len() == 2 && m.routes().iter().all(|r| m.session_ready(&r.session_id))
    })
    .await;
    let moved = tabs
        .routes()
        .into_iter()
        .find(|r| r.pane_id != first_pane)
        .unwrap();
    let process = pty.get(&moved.session_id).unwrap();
    commands
        .send(Command::new(
            scope,
            CommandKind::CreateScreen("Target".into()),
        ))
        .await
        .unwrap();
    let target = model_where(&mut updates, |m| {
        m.screens().len() == 2
            && m.routes().len() == 3
            && m.routes().iter().all(|r| m.session_ready(&r.session_id))
            && m.shown_screen_id() != Some(screen_a.as_str())
    })
    .await;
    let screen_b = target.shown_screen_id().unwrap().to_owned();
    commands
        .send(Command::new(
            scope,
            CommandKind::SwitchScreen(first.screen_key(&screen_a)),
        ))
        .await
        .unwrap();
    commands
        .send(Command::new(
            scope,
            CommandKind::Select {
                screen: first.screen_key(&screen_a),
                pane: first_pane.clone(),
            },
        ))
        .await
        .unwrap();
    let source = model_where(&mut updates, |m| {
        m.shown_screen_id() == Some(screen_a.as_str())
            && m.focused_pane() == Some(first_pane.as_str())
    })
    .await;
    let request = Uuid::new_v4();
    commands
        .send(Command::new(
            scope,
            CommandKind::TransferPane {
                request_id: request,
                source: source.capture_pane_action(&moved.pane_id).unwrap(),
                target: ScreenTransferTarget::Existing {
                    screen: first.screen_key(&screen_b),
                },
            },
        ))
        .await
        .unwrap();
    let committed = confirmed(&mut updates, request, &moved.pane_id, &moved.session_id).await;
    assert_eq!(committed.shown_screen_id(), Some(screen_b.as_str()));
    assert_eq!(committed.focused_pane(), Some(moved.pane_id.as_str()));
    assert!(committed.session_ready(&moved.session_id));
    assert!(Arc::ptr_eq(&process, &pty.get(&moved.session_id).unwrap()));
    // Same service, another actor: native must discover the target and retain
    // this exact route even though no local transfer intent exists.
    let external = Uuid::new_v4().to_string();
    state
        .mux_store
        .dispatch(Op::MovePaneToScreen {
            screen_id: ScreenId::from(screen_b.as_str()),
            pane_id: PaneId::from(moved.pane_id.as_str()),
            target_screen_id: ScreenId::from(external.as_str()),
            create_screen: true,
            screen_name: None,
        })
        .unwrap();
    state.client_state.broadcast_events(
        &[
            splitty_core::protocol::ServerEvent::mux_nudge("cli_op".into(), Some(screen_b.clone())),
            splitty_core::protocol::ServerEvent::mux_nudge("cli_op".into(), Some(external.clone())),
        ],
        None,
    );
    let observed = loop {
        match next(&mut updates).await {
            Update::Workspace(model) => {
                let model = local_projection(&model);
                assert_eq!(
                    model.session_for(&moved.pane_id),
                    Some(moved.session_id.as_str())
                );
                if model.screen_for(&moved.pane_id) == Some(external.as_str()) {
                    break model;
                }
            }
            Update::Attached { session: id, .. } if id == moved.session_id => {
                panic!("external move reattached")
            }
            _ => {}
        }
    };
    assert_eq!(
        observed.shown_screen_id(),
        Some(screen_b.as_str()),
        "external move cannot steal focus"
    );
    commands
        .send(Command::new(
            scope,
            CommandKind::SwitchScreen(first.screen_key(&external)),
        ))
        .await
        .unwrap();
    let source = model_where(&mut updates, |m| {
        m.shown_screen_id() == Some(external.as_str())
    })
    .await;
    let request = Uuid::new_v4();
    let new = Uuid::new_v4().to_string();
    commands
        .send(Command::new(
            scope,
            CommandKind::TransferPane {
                request_id: request,
                source: source.capture_pane_action(&moved.pane_id).unwrap(),
                target: ScreenTransferTarget::New {
                    screen_id: new.clone(),
                },
            },
        ))
        .await
        .unwrap();
    let final_model = confirmed(&mut updates, request, &moved.pane_id, &moved.session_id).await;
    assert!(final_model.screen(&external).is_none());
    assert_eq!(final_model.shown_screen_id(), Some(new.as_str()));
    assert_eq!(final_model.routes().len(), 3);
    assert!(Arc::ptr_eq(&process, &pty.get(&moved.session_id).unwrap()));
    commands
        .send(Command::new(
            scope,
            CommandKind::Input {
                route: TerminalToken::new(scope, &moved.pane_id, &moved.session_id),
                bytes: b"printf 'TRANSFER_%s\\n' 'ALIVE'\n".into(),
            },
        ))
        .await
        .unwrap();
    output(&mut updates, &moved.session_id).await;
    // Queue an assessed whole-window close immediately behind a new transfer.
    // Until its coherent commit, the added reconciliation marker is exactly the
    // worker's unresolved scope. If a commit wins the race, re-assess normally.
    let request = Uuid::new_v4();
    let mut plan = final_model.assess_close(900, CloseTarget::Window).unwrap();
    plan.affected.push(CloseAffected {
        identity: CloseIdentity::Reconciliation,
        unresolved: true,
    });
    plan.affected.sort_by(|a, b| a.identity.cmp(&b.identity));
    plan.unresolved_count += 1;
    commands
        .try_send(Command::new(
            scope,
            CommandKind::TransferPane {
                request_id: request,
                source: final_model.capture_pane_action(&moved.pane_id).unwrap(),
                target: ScreenTransferTarget::Existing {
                    screen: first.screen_key(&screen_a),
                },
            },
        ))
        .unwrap();
    commands
        .try_send(Command {
            source: Some(scope),
            kind: CommandKind::RequestClose {
                plan,
                confirmed: true,
            },
        })
        .unwrap();
    let mut latest = final_model;
    let mut close_id = 900;
    loop {
        match next(&mut updates).await {
            Update::Workspace(model) => latest = local_projection(&model),
            Update::CloseRejected { .. } => {
                close_id += 1;
                commands
                    .send(Command {
                        source: Some(scope),
                        kind: CommandKind::RequestClose {
                            plan: latest.assess_close(close_id, CloseTarget::Window).unwrap(),
                            confirmed: true,
                        },
                    })
                    .await
                    .unwrap();
            }
            Update::WindowCloseApproved { .. } => break,
            _ => {}
        }
    }
    worker.await.unwrap().unwrap();
    assert!(cleanup.load(Ordering::Acquire));
    assert!(pty.get(&moved.session_id).is_none());
    assert!(!path.exists());
}
