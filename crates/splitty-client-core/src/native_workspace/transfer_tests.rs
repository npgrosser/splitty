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
            CommandKind::CreateScreen(Some("Target".into())),
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

/// The outcome of transfer `request`, keeping the latest model published
/// before it; none of the `moved` sessions is attached again meanwhile.
async fn transfer_outcome(
    updates: &mut mpsc::Receiver<Update>,
    request: Uuid,
    latest: Box<SourceWorkspace>,
    moved: &[String],
) -> (ScreenTransferOutcome, Box<SourceWorkspace>) {
    let mut latest = latest;
    loop {
        match next(updates).await {
            Update::Workspace(model) => latest = local_projection(&model),
            Update::Attached { session, .. } if moved.contains(&session) => {
                panic!("session {session} reattached during {request}")
            }
            Update::ScreenTransfer(result) if result.request_id == request => {
                return (result.outcome, latest)
            }
            _ => {}
        }
    }
}

/// workspace-model, Native Tab Group Transfer, in process with live PTYs: a
/// group the service refuses (its target went away behind the window's
/// back) settles as an ordinary rejection with the group where it was, and
/// the next transfer of the same group to a new Screen moves it whole: its
/// processes keep running, the emptied source goes, the window follows to
/// the tab it showed.
#[tokio::test]
async fn worker_transfers_a_tab_group_after_a_refused_one() {
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
    let new_tab = tabs
        .routes()
        .into_iter()
        .find(|r| r.pane_id != first_pane)
        .unwrap()
        .pane_id;
    commands
        .send(Command::new(
            scope,
            CommandKind::CreateScreen(Some("Target".into())),
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
                pane: new_tab.clone(),
            },
        ))
        .await
        .unwrap();
    let grouped = model_where(&mut updates, |m| {
        m.shown_screen_id() == Some(screen_a.as_str()) && m.focused_pane() == Some(new_tab.as_str())
    })
    .await;
    let group = grouped.capture_group_action(&first_pane).unwrap();
    assert_eq!(group.tab_count(), 2);
    let shown = group.shown.pane_id.clone();
    assert_ne!(shown, first_pane, "the window shows the new tab");
    let processes: Vec<_> = group
        .tabs
        .iter()
        .map(|(pane, _)| {
            let session = grouped.session_for(pane).unwrap().to_owned();
            (pane.clone(), session.clone(), pty.get(&session).unwrap())
        })
        .collect();
    let sessions: Vec<String> = processes
        .iter()
        .map(|(_, session, _)| session.clone())
        .collect();

    // The target goes behind the window's back: the service refuses.
    state
        .mux_store
        .dispatch(Op::CloseScreen {
            screen_id: ScreenId::from(screen_b.as_str()),
        })
        .unwrap();
    let request = Uuid::new_v4();
    commands
        .send(Command::new(
            scope,
            CommandKind::TransferGroup {
                request_id: request,
                source: group,
                target: ScreenTransferTarget::Existing {
                    screen: first.screen_key(&screen_b),
                },
            },
        ))
        .await
        .unwrap();
    let (outcome, refused) = transfer_outcome(&mut updates, request, grouped, &sessions).await;
    assert!(
        matches!(&outcome, ScreenTransferOutcome::Rejected(reason) if reason.contains("rejected")),
        "{outcome:?}"
    );
    for (pane, session, _) in &processes {
        assert_eq!(refused.screen_for(pane), Some(screen_a.as_str()));
        assert_eq!(refused.session_for(pane), Some(session.as_str()));
    }

    // Still usable: the same group moves to a new Screen.
    let settled = |m: &SourceWorkspace| {
        !m.transfer_pending() && !m.routes_reconciling() && m.screen(&screen_b).is_none()
    };
    let source = if settled(&refused) {
        refused
    } else {
        model_where(&mut updates, settled).await
    };
    let request = Uuid::new_v4();
    let new = Uuid::new_v4().to_string();
    commands
        .send(Command::new(
            scope,
            CommandKind::TransferGroup {
                request_id: request,
                source: source.capture_group_action(&first_pane).unwrap(),
                target: ScreenTransferTarget::New {
                    screen_id: new.clone(),
                },
            },
        ))
        .await
        .unwrap();
    let (outcome, moved) = transfer_outcome(&mut updates, request, source, &sessions).await;
    assert_eq!(outcome, ScreenTransferOutcome::Confirmed);
    assert!(moved.screen(&screen_a).is_none(), "the emptied source goes");
    assert_eq!(moved.shown_screen_id(), Some(new.as_str()));
    assert_eq!(moved.focused_pane(), Some(shown.as_str()));
    for (pane, session, process) in &processes {
        assert_eq!(moved.screen_for(pane), Some(new.as_str()));
        assert_eq!(moved.session_for(pane), Some(session.as_str()));
        assert!(Arc::ptr_eq(process, &pty.get(session).unwrap()));
    }

    let mut latest = moved;
    let mut close_id = 900;
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
    assert!(!path.exists());
}
