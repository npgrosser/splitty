use super::{
    tests::with_view, CloseTarget, Command, Destination, ForwardAction, ForwardOutcome,
    ForwardRequest, ForwardResult, ForwardSnapshot, ForwardState, HostGroupTarget, MappingKey,
    OpenOrigin, PortFeedback, PortsView, WebUrl, WorkerCommand, Workspace,
};
use gpui_kit::{Entity, VisualTestContext};
use splitty_client_core::workspace::{ScreenKey, SourceToken, TerminalToken, WindowWorkspace};
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

fn setup(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    capacity: usize,
) -> (
    HostGroupTarget,
    ScreenKey,
    TerminalToken,
    mpsc::Receiver<WorkerCommand>,
) {
    let mut model = WindowWorkspace::new(Uuid::new_v4());
    model.ui_test_local_screen_fixture("Local").unwrap();
    let (screen, terminal) = model.ui_test_remote_terminal_fixture("vm1").unwrap();
    let group = HostGroupTarget::capture(&model, screen.source_id()).unwrap();
    let (commands, receiver) = mpsc::channel(capacity);
    window.update(|_, cx| {
        view.update(cx, |workspace, _| {
            workspace.commands = commands;
            workspace.ports.set_owner(model.lifecycle_token());
            workspace.model = Some(model);
        })
    });
    (group, screen, terminal, receiver)
}

fn admitted(receiver: &mut mpsc::Receiver<WorkerCommand>) -> (SourceToken, Uuid, ForwardRequest) {
    let command = receiver.try_recv().expect("one explicit forward");
    let Command::ManageForward {
        request_id,
        request,
    } = command.kind()
    else {
        panic!("expected forward")
    };
    (
        command.source_token().unwrap(),
        *request_id,
        request.clone(),
    )
}

#[test]
fn drawer_open_keeps_group_scope_across_navigation_and_reconnect_and_consumes_once() {
    with_view(|window, view| {
        let (group, screen, _, mut receiver) = setup(window, &view, 16);
        let key = MappingKey::new("vm1", 3000).unwrap();
        window.update(|_, cx| {
            view.update(cx, |workspace, cx| {
                workspace.submit_forward(
                    ForwardRequest::open(key.clone(), Some(4173), false, None).unwrap(),
                    OpenOrigin::Drawer(group.clone()),
                    cx,
                );
                workspace.submit_forward(
                    ForwardRequest::open(key.clone(), Some(4173), false, None).unwrap(),
                    OpenOrigin::Drawer(group.clone()),
                    cx,
                );
            })
        });
        let (owner, id, request) = admitted(&mut receiver);
        assert!(receiver.try_recv().is_err());
        assert_eq!(request.local(), Some(4173));
        assert_eq!(request.action(), ForwardAction::Open);
        assert!(!request.remember());
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                let mut model = workspace.model.clone().unwrap();
                let local = model
                    .opened()
                    .into_iter()
                    .find(|entry| entry.key().source_id() == owner.source_id())
                    .unwrap()
                    .key()
                    .clone();
                model.select(owner, &local).unwrap();
                model.disconnect(screen.source_id()).unwrap();
                model.finish_attempt(screen.source_id()).unwrap();
                model.retry(screen.source_id()).unwrap();
                assert!(group.is_current(&model));
                assert!(model.execution_token(screen.source_id()).is_none());
                workspace.revoke_missing_terminal_links(&model);
                workspace.model = Some(model);
                for destination in [
                    Destination::Hosts,
                    Destination::Settings,
                    Destination::Screens,
                ] {
                    workspace.destination = destination;
                    workspace.leave_ports(window, cx);
                    workspace.drawer_visible = false;
                    workspace.drawer_overlay_open = false;
                    assert!(workspace.ports.open_requests.contains_key(&id));
                }
                let result = ForwardResult::ui_test_fixture(
                    owner,
                    id,
                    &request,
                    ForwardOutcome::Succeeded,
                    None,
                    Some(4173),
                )
                .unwrap();
                let handle = result.eligible_open().unwrap().clone();
                workspace.forward_result(result.clone(), window, cx);
                assert!(!handle.is_active());
                assert_eq!(workspace.ports.feedback[&key].browser_opened, Some(true));
                assert!(workspace.ports.link_notice.is_none());
            })
        });
        assert_eq!(
            window.opened_url().as_deref(),
            Some("http://localhost:4173/")
        );
        // A distinct verified-looking duplicate must not dispatch or rewrite settled feedback.
        window.update(|window, cx| {
            cx.open_url("https://example.test/after-open");
            view.update(cx, |workspace, cx| {
                let duplicate = ForwardResult::ui_test_fixture(
                    owner,
                    id,
                    &request,
                    ForwardOutcome::Succeeded,
                    None,
                    Some(4173),
                )
                .unwrap();
                let handle = duplicate.eligible_open().unwrap().clone();
                workspace.forward_result(duplicate, window, cx);
                assert!(!handle.is_active());
                assert_eq!(workspace.ports.feedback[&key].browser_opened, Some(true));
            });
        });
        assert_eq!(
            window.opened_url().as_deref(),
            Some("https://example.test/after-open")
        );
        assert!(
            receiver.try_recv().is_err(),
            "navigation and reconciliation do not forward again"
        );
    });
}

#[test]
fn removed_drawer_group_never_revives_consent_even_with_full_cancel_queue() {
    for mode in [
        "detach",
        "dispose",
        "owner",
        "window",
        "before-detach-publication",
    ] {
        with_view(|window, view| {
            let (group, screen, terminal, mut receiver) = setup(window, &view, 1);
            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.submit_forward(
                        ForwardRequest::open(
                            MappingKey::new("vm1", 3000).unwrap(),
                            Some(4173),
                            false,
                            None,
                        )
                        .unwrap(),
                        OpenOrigin::Drawer(group.clone()),
                        cx,
                    );
                })
            });
            // Keep the only queue slot occupied; cancellation must revoke UI authority anyway.
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let mut model = workspace.model.clone().unwrap();
                    match mode {
                        "detach" => {
                            model.detach(group.owner(), &screen).unwrap();
                            workspace.revoke_missing_terminal_links(&model);
                            model
                                .open_existing(terminal.source_token(), &screen)
                                .unwrap();
                            assert!(
                                group.is_current(&model),
                                "same source group can be recreated"
                            );
                        }
                        "dispose" => {
                            model.begin_close(screen.source_id()).unwrap();
                            model.finish_close(screen.source_id()).unwrap();
                            workspace.revoke_missing_terminal_links(&model);
                            let (replacement, _) =
                                model.ui_test_remote_terminal_fixture("vm1").unwrap();
                            assert_ne!(replacement.source_id(), screen.source_id());
                        }
                        "owner" => {
                            model = WindowWorkspace::new(Uuid::new_v4());
                            workspace.ports_owner_changed(model.lifecycle_token(), window, cx);
                        }
                        "window" => {
                            let plan = model
                                .local_source()
                                .assess_close(99, CloseTarget::Window)
                                .unwrap();
                            workspace.revoke_close_links(&plan, cx);
                        }
                        "before-detach-publication" => {
                            workspace.revoke_screen_links(&screen, cx);
                            assert!(
                                group.is_current(&model),
                                "admitted close revokes before model publication"
                            );
                        }
                        _ => unreachable!(),
                    }
                    workspace.model = Some(model);
                    assert!(workspace.ports.open_requests.is_empty());
                })
            });
            let (owner, id, request) = admitted(&mut receiver);
            assert!(
                receiver.try_recv().is_err(),
                "revocation never sends Stop or replays Open"
            );
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let result = ForwardResult::ui_test_fixture(
                        owner,
                        id,
                        &request,
                        ForwardOutcome::Succeeded,
                        None,
                        Some(4173),
                    )
                    .unwrap();
                    let handle = result.eligible_open().unwrap().clone();
                    workspace.forward_result(result, window, cx);
                    assert!(!handle.is_active());
                    if mode != "owner" {
                        assert_eq!(
                            workspace.ports.feedback[request.key()].browser_opened,
                            Some(false)
                        );
                    }
                })
            });
            assert!(window.opened_url().is_none());
        });
    }
}

#[test]
fn drawer_open_rejects_wrong_mapping_and_terminal_override_and_reports_failures_without_replay() {
    with_view(|window, view| {
        let (group, _, terminal, mut receiver) = setup(window, &view, 8);
        let key = MappingKey::new("vm1", 3000).unwrap();
        window.update(|_, cx| {
            view.update(cx, |workspace, cx| {
                workspace.submit_forward(
                    ForwardRequest::open(MappingKey::new("vm2", 3000).unwrap(), None, false, None)
                        .unwrap(),
                    OpenOrigin::Drawer(group.clone()),
                    cx,
                );
                assert!(workspace.ports.open_requests.is_empty());
                assert!(workspace
                    .ports
                    .link_notice
                    .as_deref()
                    .unwrap()
                    .contains("no longer available"));
            })
        });
        assert!(receiver.try_recv().is_err());
        for (outcome, reason, override_terminal) in [
            (ForwardOutcome::Failed, "local port conflict", false),
            (ForwardOutcome::Uncertain, "reply lost", false),
            (ForwardOutcome::Rejected, "capacity refused", false),
            (ForwardOutcome::Succeeded, "", true),
        ] {
            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.submit_forward(
                        ForwardRequest::open(key.clone(), Some(4173), false, None).unwrap(),
                        OpenOrigin::Drawer(group.clone()),
                        cx,
                    )
                })
            });
            let (owner, id, request) = admitted(&mut receiver);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    assert!(workspace
                        .ports
                        .link_notice
                        .as_deref()
                        .unwrap()
                        .contains("pending"));
                    // A passive snapshot cannot turn an unresolved request into success.
                    workspace.ports.accept(Arc::new(
                        ForwardSnapshot::ui_test_fixture(
                            owner,
                            1,
                            Some("[]"),
                            Some(r#"{"version":1,"forwards":[]}"#),
                        )
                        .unwrap(),
                    ));
                    assert_eq!(
                        workspace.ports.feedback[&key].outcome,
                        ForwardOutcome::Pending
                    );
                    let request = if override_terminal {
                        ForwardRequest::open_link(
                            key.clone(),
                            terminal.clone(),
                            WebUrl::parse("http://localhost:3000/").unwrap(),
                        )
                        .unwrap()
                    } else {
                        request
                    };
                    let result = ForwardResult::ui_test_fixture(
                        owner,
                        id,
                        &request,
                        outcome,
                        Some(reason.into()),
                        override_terminal.then_some(4173),
                    )
                    .unwrap();
                    workspace.forward_result(result, window, cx);
                    assert!(workspace.ports.open_requests.is_empty());
                    assert!(workspace.ports.link_notice.is_some());
                    assert_eq!(workspace.ports.feedback[&key].outcome, outcome);
                    assert_eq!(workspace.ports.feedback[&key].browser_opened, Some(false));
                })
            });
            assert!(receiver.try_recv().is_err());
            assert!(window.opened_url().is_none());
        }
    });
}

#[test]
fn drawer_up_and_saved_requests_keep_local_choice_capacity_and_stop_guards() {
    for state in [ForwardState::Up, ForwardState::Paused, ForwardState::Saved] {
        with_view(|window, view| {
            let (group, _, _, mut receiver) = setup(window, &view, 8);
            window.update(|_, cx| view.update(cx, |workspace, cx| {
                let active = if state == ForwardState::Up { r#"[{"target":"vm1","remote":3000,"local":4173,"state":"up"}]"# } else { "[]" };
                let saved = if state == ForwardState::Paused {
                    r#"{"version":1,"forwards":[{"target":"vm1","remote":3000,"local":4173,"label":"App","enabled":false}]}"#
                } else {
                    r#"{"version":1,"forwards":[{"target":"vm1","remote":3000,"local":4173,"label":"App","enabled":true}]}"#
                };
                workspace.ports.accept(Arc::new(ForwardSnapshot::ui_test_fixture(group.owner(),1,Some(active),Some(saved)).unwrap()));
                let row = workspace.ports.rows(workspace.model.as_ref()).pop().unwrap();
                assert_eq!(row.state(),state);
                workspace.submit_forward(ForwardRequest::open(row.key().clone(),row.local(),false,None).unwrap(),OpenOrigin::Drawer(group.clone()),cx);
            }));
            let (owner, id, request) = admitted(&mut receiver);
            assert_eq!(request.local(), Some(4173));
            assert!(!request.remember());
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    for port in 3001..3064 {
                        workspace.ports.record(
                            MappingKey::new("vm1", port).unwrap(),
                            PortFeedback {
                                owner,
                                request_id: Uuid::new_v4(),
                                action: ForwardAction::Start,
                                outcome: ForwardOutcome::Pending,
                                reason: None,
                                browser_opened: None,
                                settle_after: 0,
                            },
                        );
                    }
                    assert_eq!(workspace.ports.feedback.len(), PortsView::MAX_FEEDBACK);
                    workspace.submit_forward(
                        ForwardRequest::open(
                            MappingKey::new("vm1", 4000).unwrap(),
                            None,
                            false,
                            None,
                        )
                        .unwrap(),
                        OpenOrigin::Drawer(group.clone()),
                        cx,
                    );
                    assert!(receiver.try_recv().is_err());
                    assert!(workspace
                        .ports
                        .link_notice
                        .as_deref()
                        .unwrap()
                        .contains("Too many"));
                    workspace.submit_forward(
                        ForwardRequest::stop(request.key().clone()),
                        OpenOrigin::Hosts,
                        cx,
                    );
                    assert!(!workspace.ports.open_requests.contains_key(&id));
                    let late = ForwardResult::ui_test_fixture(
                        owner,
                        id,
                        &request,
                        ForwardOutcome::Succeeded,
                        None,
                        Some(4173),
                    )
                    .unwrap();
                    let handle = late.eligible_open().unwrap().clone();
                    workspace.forward_result(late, window, cx);
                    assert!(!handle.is_active());
                    assert_eq!(
                        workspace.ports.feedback[request.key()].action,
                        ForwardAction::Stop
                    );
                    assert!(workspace
                        .ports
                        .link_notice
                        .as_deref()
                        .unwrap()
                        .contains("canceled by Stop"));
                })
            });
            assert!(
                matches!(receiver.try_recv().unwrap().kind(),Command::CancelForwardOpen {request_id} if *request_id==id)
            );
            let (_, _, stop) = admitted(&mut receiver);
            assert_eq!(stop.action(), ForwardAction::Stop);
            assert!(receiver.try_recv().is_err());
            assert!(window.opened_url().is_none());
        });
    }
}
