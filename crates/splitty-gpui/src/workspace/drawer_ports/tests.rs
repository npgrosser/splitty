use super::{HostGroupTarget, Workspace};
use crate::workspace::chrome::view_tests::with_view;
use crate::workspace::{CloseTarget, Command, Destination, Update, WorkerCommand};
use gpui_kit::{
    point, px, size, Entity, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton,
    VisualTestContext,
};
use splitty_client_core::ports::{ForwardAction, ForwardSnapshot};
use splitty_client_core::workspace::{SourceToken, WindowWorkspace};
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

fn draw(window: &mut VisualTestContext) {
    window.run_until_parked();
    window.update(|window, cx| window.draw(cx).clear(cx));
}

fn click(window: &mut VisualTestContext, selector: String) {
    let selector: &'static str = Box::leak(selector.into_boxed_str());
    let center = window.debug_bounds(selector).expect(selector).center();
    window.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
    window.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
    draw(window);
}

fn press(window: &mut VisualTestContext, key: &str) {
    let keystroke =
        window.update(|_, cx| Keystroke::parse_with_os(key, cx.platform_info().os).unwrap());
    window.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    window.simulate_event(KeyUpEvent { keystroke });
    draw(window);
}

fn snapshot(
    owner: SourceToken,
    generation: u64,
    active: Option<&str>,
    saved: Option<&str>,
) -> Arc<ForwardSnapshot> {
    Arc::new(ForwardSnapshot::ui_test_fixture(owner, generation, active, saved).unwrap())
}

const EMPTY_SAVED: &str = r#"{"version":1,"forwards":[]}"#;
const B_UP: &str = r#"[{"target":"two.test","remote":3000,"local":4173,"state":"up"}]"#;

fn setup(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
) -> (HostGroupTarget, mpsc::Receiver<WorkerCommand>) {
    let mut model = WindowWorkspace::new(Uuid::new_v4());
    let local = model.ui_test_local_screen_fixture("Work").unwrap();
    model.ui_test_remote_terminal_fixture("one.test").unwrap();
    let (remote, _) = model.ui_test_remote_terminal_fixture("two.test").unwrap();
    let group = HostGroupTarget::capture(&model, remote.source_id()).unwrap();
    let owner = model.lifecycle_token();
    model.select(owner, &local).unwrap();
    let (commands, mut receiver) = mpsc::channel(64);
    window.update(|window, cx| {
        cx.set_reduce_motion(true);
        window.resize(size(px(900.), px(620.)));
        view.update(cx, |workspace, cx| {
            workspace.commands = commands;
            workspace.reconcile(model, window, cx);
            assert!(workspace
                .ports
                .accept(snapshot(owner, 1, Some(B_UP), Some(EMPTY_SAVED))));
            workspace.drawer_visible = true;
            workspace.status.clear();
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    while receiver.try_recv().is_ok() {}
    (group, receiver)
}

#[test]
fn host_plus_pointer_sends_one_exact_remote_request_and_projects_pending_reference() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        window.update(|_, cx| {
            view.update(cx, |workspace, cx| {
                workspace.destination = Destination::Screens;
                cx.notify();
            })
        });
        draw(window);
        let plus = format!("drawer-host-new-{}", group.source_id());
        assert!(window
            .debug_bounds(Box::leak(plus.clone().into_boxed_str()))
            .is_some());
        assert!(
            receiver.try_recv().is_err(),
            "rendering the Plus action is passive"
        );
        click(window, plus.clone());
        let command = receiver.try_recv().expect("one remote creation command");
        assert_eq!(
            command.source_token(),
            Some(group.owner()),
            "local owner orchestrates navigation"
        );
        let Command::CreateRemoteScreen { request } = command.kind() else {
            panic!("Plus must send remote creation");
        };
        assert_eq!(request.target().source_id(), group.source_id());
        assert_eq!(request.target().target(), group.target());
        let captured = request.clone();
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert!(workspace.destination == Destination::Screens);
            assert_eq!(workspace.remote_create_enqueues.len(), 1);
            assert_eq!(
                workspace
                    .model
                    .as_ref()
                    .unwrap()
                    .shown_key()
                    .unwrap()
                    .source_id(),
                group.owner().source_id()
            );
        });
        click(window, plus);
        assert!(
            receiver.try_recv().is_err(),
            "the enqueue latch blocks repeat activation"
        );

        let before = window.update(|_, cx| view.read(cx).model.clone().unwrap());
        let mut pending = before.clone();
        pending
            .ui_test_remote_creation_fixture(&captured, false)
            .unwrap();
        let requested = pending
            .opened()
            .into_iter()
            .find(|entry| {
                entry.key().source_id() == group.source_id()
                    && entry.key().screen_id()
                        == pending
                            .source(group.source_id())
                            .unwrap()
                            .remote_screen_creation()
                            .unwrap()
                            .screen_id()
            })
            .unwrap()
            .key()
            .clone();
        let mut missing = pending.clone();
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| workspace.reconcile(pending, window, cx))
        });
        draw(window);
        let pending_label = format!("drawer-screen-label-{requested}-Creating Screen…");
        assert!(window
            .debug_bounds(Box::leak(pending_label.into_boxed_str()))
            .is_some());
        window.update(|_, cx| assert!(view.read(cx).remote_create_enqueues.is_empty()));

        missing
            .ui_test_remote_creation_settle_missing_fixture(&captured)
            .unwrap();
        let missing_id = missing
            .source(group.source_id())
            .unwrap()
            .remote_screen_creation()
            .unwrap()
            .screen_id();
        let missing_entry = missing
            .opened()
            .into_iter()
            .find(|entry| entry.key() == &requested)
            .unwrap();
        assert_eq!(requested.screen_id(), missing_id);
        assert_eq!(missing_entry.name(), None);
        assert_eq!(
            missing_entry.state(),
            splitty_client_core::workspace::OpenedScreenState::Missing
        );
        let missing_key = missing_entry.key().clone();
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| workspace.reconcile(missing, window, cx))
        });
        draw(window);
        let missing_label = format!("drawer-screen-label-{missing_key}-Unnamed Screen");
        assert!(window
            .debug_bounds(Box::leak(missing_label.into_boxed_str()))
            .is_some());
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert!(workspace
                .remote_create_notice
                .as_ref()
                .unwrap()
                .message()
                .contains("currently missing"));
        });
        assert!(
            receiver.try_recv().is_err(),
            "projection and reconciliation submit no extra command"
        );
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.request_close_screen(missing_key.clone(), window, cx)
            })
        });
        assert!(matches!(
            receiver.try_recv().unwrap().kind(),
            Command::DetachRemoteScreen { screen } if screen == &missing_key
        ));
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                let mut detached = workspace.model.clone().unwrap();
                detached
                    .detach(detached.lifecycle_token(), &missing_key)
                    .unwrap();
                workspace.reconcile(detached, window, cx);
                assert!(workspace.remote_create_notice.is_none());
            })
        });
    });
}

#[test]
fn qualified_worker_rejection_of_host_plus_keeps_visible_feedback() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        click(window, format!("drawer-host-new-{}", group.source_id()));
        let command = receiver.try_recv().expect("Plus activation");
        let Command::CreateRemoteScreen { request } = command.kind() else {
            panic!("Plus must use the creation path");
        };
        let request = request.clone();
        assert_eq!(command.source_token(), Some(group.owner()));
        assert_eq!(request.target().target(), group.target());
        assert!(receiver.try_recv().is_err());
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                let mut rebound = workspace.model.clone().unwrap();
                rebound
                    .ui_test_remote_terminal_fixture(group.target())
                    .unwrap();
                assert_ne!(
                    rebound.execution_token(group.source_id()),
                    Some(request.source())
                );
                workspace.reconcile(rebound, window, cx);
                assert!(workspace.remote_create_enqueues.is_empty());
                assert!(workspace.remote_create_latest.is_some());
            })
        });
        assert!(
            !request.is_follow_requested(),
            "a new execution token revokes navigation immediately"
        );
        let stale = Update::RemoteScreenCreateRejected {
            owner: group.owner(),
            source: request.source(),
            request_id: Uuid::new_v4(),
            reason: "old".into(),
        };
        let rejection = Update::RemoteScreenCreateRejected {
            owner: group.owner(),
            source: request.source(),
            request_id: request.request_id(),
            reason: "catalog changed".into(),
        };
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                let (_, mut empty) = mpsc::channel(1);
                workspace.poll(Some(stale), &mut empty, window, cx);
                assert!(workspace.remote_create_notice.is_none());
                workspace.poll(Some(rejection), &mut empty, window, cx);
                assert!(workspace.remote_create_enqueues.is_empty());
                assert!(workspace
                    .remote_create_notice
                    .as_ref()
                    .unwrap()
                    .message()
                    .contains("catalog changed"));
                workspace.reconcile(workspace.model.clone().unwrap(), window, cx);
                assert!(
                    workspace.remote_create_notice.is_some(),
                    "normal Workspace publication must not erase the rejection"
                );
            })
        });
        draw(window);
        assert!(window.debug_bounds("workspace-status").is_some());
    });
}

#[test]
fn exact_detach_revokes_follow_only_after_queue_admission() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        click(window, format!("drawer-host-new-{}", group.source_id()));
        let command = receiver.try_recv().unwrap();
        let Command::CreateRemoteScreen { request } = command.kind() else {
            panic!("expected remote creation");
        };
        let request = request.clone();
        let mut pending = window.update(|_, cx| view.read(cx).model.clone().unwrap());
        pending
            .ui_test_remote_creation_fixture(&request, false)
            .unwrap();
        let screen = pending
            .opened()
            .into_iter()
            .find(|entry| {
                entry.key().source_id() == group.source_id()
                    && entry.key().screen_id()
                        == pending
                            .source(group.source_id())
                            .unwrap()
                            .remote_screen_creation()
                            .unwrap()
                            .screen_id()
            })
            .unwrap()
            .key()
            .clone();
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| workspace.reconcile(pending, window, cx))
        });
        assert!(request.is_follow_requested());

        let (full_sender, mut full_receiver) = mpsc::channel(1);
        full_sender
            .try_send(WorkerCommand::new(
                group.owner(),
                Command::ReleaseHostBrowsing,
            ))
            .unwrap();
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.commands = full_sender;
                workspace.request_close_screen(screen.clone(), window, cx);
            })
        });
        assert!(request.is_follow_requested());
        assert!(matches!(
            full_receiver.try_recv().unwrap().kind(),
            Command::ReleaseHostBrowsing
        ));

        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.request_close_screen(screen.clone(), window, cx)
            })
        });
        assert!(matches!(
            full_receiver.try_recv().unwrap().kind(),
            Command::DetachRemoteScreen { screen: sent } if sent == &screen
        ));
        assert!(!request.is_follow_requested());
    });
}

#[test]
fn accepted_window_close_revokes_follow_before_worker_acknowledgement() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        click(window, format!("drawer-host-new-{}", group.source_id()));
        let command = receiver.try_recv().unwrap();
        let Command::CreateRemoteScreen { request } = command.kind() else {
            panic!("expected remote creation");
        };
        let request = request.clone();
        let plan = window.update(|_, cx| {
            view.read(cx)
                .local_model()
                .unwrap()
                .assess_close(77, CloseTarget::Window)
                .unwrap()
        });
        let (full_sender, mut full_receiver) = mpsc::channel(1);
        full_sender
            .try_send(WorkerCommand::new(
                group.owner(),
                Command::ReleaseHostBrowsing,
            ))
            .unwrap();
        window.update(|_, cx| {
            view.update(cx, |workspace, cx| {
                workspace.commands = full_sender;
                workspace.submit_close(plan.clone(), false, cx);
            })
        });
        assert!(request.is_follow_requested());
        assert!(matches!(
            full_receiver.try_recv().unwrap().kind(),
            Command::ReleaseHostBrowsing
        ));
        window.update(|_, cx| {
            view.update(cx, |workspace, cx| workspace.submit_close(plan, false, cx))
        });
        assert!(matches!(
            full_receiver.try_recv().unwrap().kind(),
            Command::RequestClose { .. }
        ));
        assert!(!request.is_follow_requested());
    });
}

#[test]
fn older_host_rejection_releases_only_its_enqueue_latch() {
    with_view(|window, view| {
        let (group_b, mut receiver) = setup(window, &view);
        let group_a = window.update(|_, cx| {
            let model = view.read(cx).model.as_ref().unwrap();
            let source = model.source_for_target("one.test").unwrap();
            HostGroupTarget::capture(model, source).unwrap()
        });
        click(window, format!("drawer-host-new-{}", group_a.source_id()));
        let command_a = receiver.try_recv().unwrap();
        let Command::CreateRemoteScreen { request: a } = command_a.kind() else {
            panic!("first host creation expected");
        };
        let a = a.clone();
        click(window, format!("drawer-host-new-{}", group_b.source_id()));
        let command_b = receiver.try_recv().unwrap();
        let Command::CreateRemoteScreen { request: b } = command_b.kind() else {
            panic!("second host creation expected");
        };
        let b = b.clone();
        assert_ne!(a.request_id(), b.request_id());
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                assert_eq!(workspace.remote_create_enqueues.len(), 2);
                let (_, mut empty) = mpsc::channel(1);
                workspace.poll(
                    Some(Update::RemoteScreenCreateRejected {
                        owner: group_a.owner(),
                        source: a.source(),
                        request_id: a.request_id(),
                        reason: "first host refused".into(),
                    }),
                    &mut empty,
                    window,
                    cx,
                );
                assert!(!workspace
                    .remote_create_enqueues
                    .contains_key(&group_a.source_id()));
                assert_eq!(
                    workspace.remote_create_enqueues[&group_b.source_id()].request_id(),
                    b.request_id()
                );
                assert!(workspace.remote_create_latest.is_some());
                assert_eq!(
                    workspace
                        .remote_create_follow
                        .as_ref()
                        .unwrap()
                        .request_id(),
                    b.request_id()
                );
                assert!(workspace.remote_create_notice.is_none());
            })
        });
        assert!(b.is_follow_requested());
        draw(window);
        click(window, format!("drawer-host-new-{}", group_a.source_id()));
        let again = receiver.try_recv().unwrap();
        assert!(matches!(
            again.kind(),
            Command::CreateRemoteScreen { request } if request.target().source_id() == group_a.source_id()
        ));
    });
}

#[test]
fn settings_departure_cancels_follow_but_keeps_later_missing_outcome_visible() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        click(window, format!("drawer-host-new-{}", group.source_id()));
        let command = receiver.try_recv().unwrap();
        let Command::CreateRemoteScreen { request } = command.kind() else {
            panic!("expected remote creation");
        };
        let request = request.clone();
        assert!(request.is_follow_requested());
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.show_settings(window, cx);
                assert!(workspace.destination == Destination::Settings);
                assert!(workspace.remote_create_latest.is_some());
            })
        });
        assert!(!request.is_follow_requested());
        while let Ok(command) = receiver.try_recv() {
            assert!(
                !matches!(command.kind(), Command::CreateRemoteScreen { .. }),
                "Settings may refresh its own data but cannot create another Screen"
            );
        }

        let mut pending = window.update(|_, cx| view.read(cx).model.clone().unwrap());
        pending
            .ui_test_remote_creation_fixture(&request, false)
            .unwrap();
        let mut missing = pending.clone();
        missing
            .ui_test_remote_creation_settle_missing_fixture(&request)
            .unwrap();
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| workspace.reconcile(pending, window, cx))
        });
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.reconcile(missing, window, cx);
                assert!(workspace
                    .remote_create_notice
                    .as_ref()
                    .unwrap()
                    .message()
                    .contains("currently missing"));
                assert!(workspace.destination == Destination::Settings);
            })
        });
        draw(window);
        assert!(window.debug_bounds("workspace-status").is_some());
        while let Ok(command) = receiver.try_recv() {
            assert!(!matches!(
                command.kind(),
                Command::CreateRemoteScreen { .. }
            ));
        }
    });
}

#[test]
fn remote_plus_is_inert_offline_and_uses_a_fresh_ready_capture() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        let plus = format!("drawer-host-new-{}", group.source_id());
        let old_source = window.update(|_, cx| {
            view.read(cx)
                .model
                .as_ref()
                .unwrap()
                .execution_token(group.source_id())
                .unwrap()
        });
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                let mut offline = workspace.model.clone().unwrap();
                offline.disconnect(group.source_id()).unwrap();
                workspace.reconcile(offline, window, cx);
            })
        });
        draw(window);
        click(window, plus.clone());
        assert!(receiver.try_recv().is_err());

        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                let mut ready = workspace.model.clone().unwrap();
                ready
                    .ui_test_remote_terminal_fixture(group.target())
                    .unwrap();
                workspace.reconcile(ready, window, cx);
            })
        });
        draw(window);
        while let Ok(command) = receiver.try_recv() {
            assert!(!matches!(
                command.kind(),
                Command::CreateRemoteScreen { .. }
            ));
        }
        click(window, plus);
        let command = receiver.try_recv().unwrap();
        let Command::CreateRemoteScreen { request } = command.kind() else {
            panic!("a fresh ready capture must create");
        };
        assert_eq!(command.source_token(), Some(group.owner()));
        assert_eq!(request.target().target(), group.target());
        assert_ne!(request.source(), old_source);
        assert!(receiver.try_recv().is_err());
    });
}

#[test]
fn compact_zoomed_drawer_keeps_host_create_action_in_view() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        window.update(|window, cx| {
            window.resize(size(px(560.), px(420.)));
            view.update(cx, |workspace, cx| {
                workspace.apply_zoom(
                    splitty_client_core::zoom::Zoom::from_values(
                        1.25,
                        "window",
                        &Default::default(),
                    ),
                    window,
                    cx,
                );
            })
        });
        draw(window);
        let plus = format!("drawer-host-new-{}", group.source_id());
        let bounds = window
            .debug_bounds(Box::leak(plus.clone().into_boxed_str()))
            .unwrap();
        assert!(bounds.origin.x >= px(0.));
        assert!(bounds.origin.y >= px(0.));
        assert!(bounds.origin.x + bounds.size.width <= px(560.));
        assert!(bounds.origin.y + bounds.size.height <= px(420.));
        assert!(receiver.try_recv().is_err());
        click(window, plus);
        assert!(matches!(
            receiver.try_recv().unwrap().kind(),
            Command::CreateRemoteScreen { .. }
        ));
    });
}

#[test]
fn ports_panel_is_passive_until_a_port_is_chosen_and_opens_it_in_the_browser() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        let trigger = format!("drawer-ports-{}", group.source_id());
        click(window, trigger.clone());
        assert!(window.debug_bounds("drawer-popover").is_some());
        while let Ok(command) = receiver.try_recv() {
            assert!(
                matches!(command.kind(), Command::RefreshForwards),
                "opening only refreshes what it lists"
            );
        }
        press(window, "escape");
        window.update(|_, cx| assert!(view.read(cx).drawer_popover.is_none()));
        assert!(receiver.try_recv().is_err(), "dismissal is passive");

        click(window, trigger.clone());
        while receiver.try_recv().is_ok() {}
        press(window, "enter");
        let sent = receiver.try_recv().expect("the listed port opens");
        assert_eq!(sent.source_token(), Some(group.owner()));
        assert!(matches!(
            sent.kind(),
            Command::ManageForward { request, .. }
                if request.action() == ForwardAction::Open
                    && request.key().target() == "two.test"
                    && request.key().remote() == 3000
                    && request.local() == Some(4173)
        ));
        window.update(|_, cx| assert!(view.read(cx).drawer_popover.is_none()));

        click(window, trigger);
        while receiver.try_recv().is_ok() {}
        window.simulate_input("5173");
        draw(window);
        press(window, "enter");
        let sent = receiver.try_recv().expect("a typed port opens");
        assert!(matches!(
            sent.kind(),
            Command::ManageForward { request, .. }
                if request.action() == ForwardAction::Open
                    && request.key().target() == "two.test"
                    && request.key().remote() == 5173
                    && request.local().is_none()
        ));
        assert!(receiver.try_recv().is_err());
    });
}

#[test]
fn forwarded_port_row_copies_its_address_and_stops_without_opening() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        let trigger = format!("drawer-ports-{}", group.source_id());
        click(window, trigger.clone());
        while receiver.try_recv().is_ok() {}
        click(window, "popover-port-3000-action-0".into());
        assert!(receiver.try_recv().is_err(), "copying sends nothing");
        window.update(|_, cx| {
            assert!(view.read(cx).drawer_popover.is_none());
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("localhost:4173")
            );
        });
        click(window, trigger);
        while receiver.try_recv().is_ok() {}
        click(window, "popover-port-3000-action-1".into());
        let sent = receiver.try_recv().expect("Stop is sent");
        assert!(matches!(
            sent.kind(),
            Command::ManageForward { request, .. }
                if request.action() == ForwardAction::Stop && request.key().remote() == 3000
        ));
        assert!(receiver.try_recv().is_err());
    });
}

#[test]
fn unpinned_drawer_stays_revealed_for_its_panel_then_resumes_autohide() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        window.update(|_, cx| {
            view.update(cx, |workspace, cx| {
                workspace.drawer_visible = false;
                workspace.drawer_overlay_open = true;
                cx.notify();
            })
        });
        draw(window);
        click(window, format!("drawer-ports-{}", group.source_id()));
        while receiver.try_recv().is_ok() {}
        window.simulate_mouse_move(point(px(500.), px(180.)), None, Modifiers::default());
        draw(window);
        window.update(|_, cx| {
            assert!(
                view.read(cx).drawer_overlay_open,
                "travelling to an open panel must not dismiss its Drawer"
            )
        });
        press(window, "escape");
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert!(!workspace.drawer_overlay_open);
            assert!(workspace.drawer_popover.is_none());
        });
        assert!(receiver.try_recv().is_err());
    });
}

#[test]
fn host_caption_shows_its_screens_beside_the_drawer_without_connecting() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        click(window, format!("drawer-host-{}", group.source_id()));
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert!(workspace.destination == Destination::Screens);
            assert_eq!(workspace.drawer_popover_host(), Some("two.test"));
        });
        assert!(
            window.debug_bounds("popover-host-settings").is_some(),
            "the manager stays one step away"
        );
        assert!(receiver.try_recv().is_err(), "looking connects nothing");
        press(window, "escape");
        window.update(|_, cx| assert!(view.read(cx).drawer_popover.is_none()));
        while let Ok(command) = receiver.try_recv() {
            assert!(matches!(command.kind(), Command::ReleaseHostBrowsing));
        }
    });
}

#[test]
fn panels_close_with_their_host_group_and_never_outlive_their_owner() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        let trigger = format!("drawer-ports-{}", group.source_id());
        click(window, trigger.clone());
        let screen = window.update(|_, cx| {
            view.read(cx)
                .model
                .as_ref()
                .unwrap()
                .opened()
                .into_iter()
                .find(|entry| entry.key().source_id() == group.source_id())
                .unwrap()
                .key()
                .clone()
        });
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.request_close_screen(screen.clone(), window, cx);
                assert!(
                    workspace.drawer_popover.is_none(),
                    "the last detach closes the panel before the model answers"
                );
            })
        });
        draw(window);
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.pending_close = None;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        click(window, trigger);
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                assert!(workspace.drawer_popover.is_some());
                workspace.reconcile(WindowWorkspace::new(Uuid::new_v4()), window, cx);
                assert!(workspace.drawer_popover.is_none());
            })
        });
        while let Ok(command) = receiver.try_recv() {
            assert!(!matches!(command.kind(), Command::ManageForward { .. }));
        }
    });
}

#[test]
fn dragging_remote_caption_moves_the_complete_group_without_selecting_or_connecting() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        let (first, selected) = window.update(|_, cx| {
            let model = view.read(cx).model.as_ref().unwrap();
            (
                model.source_for_target("one.test").unwrap(),
                model.shown_key(),
            )
        });
        let bounds = |window: &mut VisualTestContext, id| {
            window
                .debug_bounds(Box::leak(format!("drawer-group-{id}").into_boxed_str()))
                .unwrap()
        };
        let start = bounds(window, group.source_id()).origin + point(px(70.), px(22.));
        let end = bounds(window, first).origin + point(px(70.), px(2.));
        window.simulate_mouse_move(start, None, Modifiers::default());
        window.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        window.simulate_mouse_move(
            start - point(px(0.), px(8.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        draw(window);
        window.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        draw(window);
        assert!(
            window.update(|_, cx| view.read(cx).drawer_host_drag.is_some()),
            "caption drag is owned by the complete group"
        );
        window.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        draw(window);
        let command = receiver.try_recv().expect("one metadata mutation");
        assert!(matches!(command.kind(), Command::ManageHost { request, .. }
            if request.action() == splitty_client_core::hub::HostManagementAction::SetOrder && request.target() == "two.test"));
        assert!(
            receiver.try_recv().is_err(),
            "dragging neither connects nor selects"
        );
        window.update(|_, cx| {
            assert_eq!(view.read(cx).model.as_ref().unwrap().shown_key(), selected)
        });
    });
}
