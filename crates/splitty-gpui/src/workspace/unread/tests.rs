use super::{Event, NotificationSettingsRead, Severity};
use crate::workspace::chrome::view_tests::with_view;
use crate::workspace::{Command, Destination, Update, WorkerCommand, Workspace};
use gpui_kit::{Entity, Modifiers, VisualTestContext, WindowVisibility};
use serde_json::json;
use splitty_client_core::workspace::{SourceId, SourceToken, WindowWorkspace};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use uuid::Uuid;

fn model(owner: Uuid) -> WindowWorkspace {
    let mut model = WindowWorkspace::new(owner);
    model.ui_test_local_screen_fixture("Work").unwrap();
    let source = model.lifecycle_token();
    model.source_mut(source.source_id()).unwrap().apply_snapshot(owner, &json!({
        "type":"mux.screen_snapshot","screen":{
            "id":"work","revision":2,
            "root":{"type":"split","id":"split","axis":"horizontal","sizes":[0.5,0.5],"children":[
                {"type":"slot","id":"left","paneIds":["p0"]},
                {"type":"slot","id":"right","paneIds":["p1"]}
            ]},
            "panes":{"p0":{"id":"p0","sessionId":"s-p0"},"p1":{"id":"p1","sessionId":"s-p1"}},
            "sessions":{"s-p0":{"id":"s-p0","status":"running"},"s-p1":{"id":"s-p1","status":"running"}}
        }
    })).unwrap();
    model
        .source_mut(source.source_id())
        .unwrap()
        .focus_pane("p0");
    model
}

fn read(owner: SourceToken, generation: u64, enabled: bool) -> NotificationSettingsRead {
    NotificationSettingsRead::ui_test_fixture(
        owner.connection_id(),
        generation,
        Ok(serde_json::from_value(json!({
            "enabled":enabled,"desktop":false,"toasts":false,
            "processFinished":true,"processExited":true,"processCrashed":true
        }))
        .unwrap()),
    )
}

fn event(rt: &Runtime, source: SourceToken, id: &str, session: &str, severity: &str) -> Update {
    let event = rt
        .block_on(Event::ui_test_json(
            source.connection_id(),
            &json!({
                "type":"process.idle","id":id,"timestamp":1,"sessionId":session,
                "notice":{"kind":"process.idle","title":id,"severity":severity}
            }),
        ))
        .unwrap();
    Update::Event {
        source,
        event: Box::new(event),
    }
}

fn deliver(window: &mut VisualTestContext, view: &Entity<Workspace>, update: Option<Update>) {
    let (_sender, mut receiver) = mpsc::channel(1);
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            workspace.poll(update, &mut receiver, window, cx);
        });
        window.draw(cx).clear(cx);
    });
}

fn setup(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    model: WindowWorkspace,
) -> mpsc::Receiver<WorkerCommand> {
    let owner = model.lifecycle_token();
    let (sender, mut commands) = mpsc::channel(64);
    window.update(|window, cx| {
        window.activate_window();
        view.update(cx, |workspace, cx| {
            workspace.commands = sender;
            workspace.destination = Destination::Screens;
            workspace.drawer_visible = true;
            workspace.reconcile(model, window, cx);
        });
        window.draw(cx).clear(cx);
        window.draw(cx).clear(cx);
    });
    window.simulate_visibility_change(WindowVisibility::Visible);
    window.deactivate_window();
    deliver(
        window,
        view,
        Some(Update::NotificationSettings {
            owner,
            read: read(owner, 2, true),
        }),
    );
    deliver(window, view, None);
    while commands.try_recv().is_ok() {}
    commands
}

fn redraw(window: &mut VisualTestContext) {
    window.update(|window, cx| window.draw(cx).clear(cx));
}

#[test]
fn notification_badges_follow_watched_focus_and_deliberate_pane_activation() {
    let rt = Runtime::new().unwrap();
    with_view(|window, view| {
        let mut model = model(Uuid::new_v4());
        let owner = model.lifecycle_token();
        let screen = model.local_source().screen_key("work");
        let mut commands = setup(window, &view, model.clone());
        for (id, session, severity) in [
            ("First", "s-p0", "info"),
            ("Second", "s-p0", "error"),
            ("Third", "s-p1", "warning"),
        ] {
            deliver(
                window,
                &view,
                Some(event(&rt, owner, id, session, severity)),
            );
        }
        let selector: &'static str =
            Box::leak(format!("drawer-unread-{screen}-3").into_boxed_str());
        assert!(window.debug_bounds(selector).is_some());
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert_eq!(
                workspace.screen_unread_summary(&screen).as_deref(),
                Some("3 unread: Third · Second · First")
            );
            assert_eq!(
                workspace
                    .notifications
                    .severity(&model.local_source().pane_key("p0")),
                Some(Severity::Error)
            );
        });
        assert!(
            commands.try_recv().is_err(),
            "incoming badges do not send workspace commands"
        );

        window.update(|window, _| window.activate_window());
        window.executor().run_until_parked();
        redraw(window);
        window.update(|_, cx| assert_eq!(view.read(cx).notifications.unread_count(&screen), 1));
        assert!(
            commands.try_recv().is_err(),
            "focus return only changes local read state"
        );

        window.simulate_visibility_change(WindowVisibility::Hidden);
        window.executor().run_until_parked();
        deliver(
            window,
            &view,
            Some(event(&rt, owner, "Hidden", "s-p0", "info")),
        );
        window.update(|_, cx| assert_eq!(view.read(cx).notifications.unread_count(&screen), 2));
        window.simulate_visibility_change(WindowVisibility::Visible);
        window.executor().run_until_parked();
        deliver(
            window,
            &view,
            Some(event(&rt, owner, "Watched", "s-p0", "info")),
        );
        window.update(|_, cx| assert_eq!(view.read(cx).notifications.unread_count(&screen), 1));
        assert!(
            commands.try_recv().is_err(),
            "visibility and watched arrival are local read observations"
        );

        let pane_selector: &'static str =
            Box::leak(format!("drawer-pane-{screen}-p1").into_boxed_str());
        let target = window.debug_bounds(pane_selector).unwrap().center();
        window.simulate_click(target, Modifiers::default());
        let mut selected = false;
        while let Ok(command) = commands.try_recv() {
            match command.kind() {
                Command::SwitchScreen(chosen) => assert_eq!(chosen, &screen),
                Command::Select {
                    screen: chosen,
                    pane,
                } => {
                    assert_eq!(chosen, &screen);
                    assert_eq!(pane, "p1");
                    selected = true;
                }
                _ => panic!("pane activation sent an unrelated command"),
            }
        }
        assert!(selected, "actual pane control activates its captured pane");
        model
            .source_mut(owner.source_id())
            .unwrap()
            .focus_pane("p1");
        deliver(window, &view, Some(Update::Workspace(Box::new(model))));
        window.update(|_, cx| assert_eq!(view.read(cx).notifications.unread_count(&screen), 0));
    });
}

#[test]
fn notification_settings_reject_stale_owners_and_keep_last_good_after_failure() {
    let rt = Runtime::new().unwrap();
    with_view(|window, view| {
        let model = model(Uuid::new_v4());
        let owner = model.lifecycle_token();
        let screen = model.local_source().screen_key("work");
        let mut commands = setup(window, &view, model);
        deliver(
            window,
            &view,
            Some(event(&rt, owner, "First", "s-p0", "info")),
        );
        deliver(
            window,
            &view,
            Some(Update::NotificationSettings {
                owner,
                read: read(owner, 1, false),
            }),
        );
        let foreign = WindowWorkspace::new(Uuid::new_v4()).lifecycle_token();
        deliver(
            window,
            &view,
            Some(Update::NotificationSettings {
                owner: foreign,
                read: read(foreign, 99, false),
            }),
        );
        let failure = NotificationSettingsRead::ui_test_fixture(
            owner.connection_id(),
            3,
            Err("unavailable".into()),
        );
        deliver(
            window,
            &view,
            Some(Update::NotificationSettings {
                owner,
                read: failure,
            }),
        );
        deliver(
            window,
            &view,
            Some(event(&rt, owner, "Second", "s-p1", "success")),
        );
        deliver(
            window,
            &view,
            Some(event(&rt, foreign, "Foreign", "s-p0", "error")),
        );
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert_eq!(workspace.notifications.unread_count(&screen), 2);
            assert_eq!(
                workspace.notification_settings_error.as_deref(),
                Some("unavailable")
            );
        });
        deliver(
            window,
            &view,
            Some(Update::NotificationSettings {
                owner,
                read: read(owner, 4, false),
            }),
        );
        deliver(
            window,
            &view,
            Some(Update::NotificationSettings {
                owner,
                read: read(owner, 5, true),
            }),
        );
        window.update(|_, cx| assert_eq!(view.read(cx).notifications.unread_count(&screen), 0));
        let replacement = self::model(Uuid::new_v4());
        let replacement_owner = replacement.lifecycle_token();
        let replacement_screen = replacement.local_source().screen_key("work");
        deliver(
            window,
            &view,
            Some(Update::Workspace(Box::new(replacement))),
        );
        deliver(
            window,
            &view,
            Some(event(&rt, owner, "Retired", "s-p0", "error")),
        );
        deliver(
            window,
            &view,
            Some(event(
                &rt,
                replacement_owner,
                "Before settings",
                "s-p0",
                "info",
            )),
        );
        window.update(|_, cx| {
            assert_eq!(
                view.read(cx)
                    .notifications
                    .unread_count(&replacement_screen),
                0
            )
        });
        deliver(
            window,
            &view,
            Some(Update::NotificationSettings {
                owner: replacement_owner,
                read: read(replacement_owner, 1, true),
            }),
        );
        deliver(
            window,
            &view,
            Some(event(
                &rt,
                replacement_owner,
                "After settings",
                "s-p0",
                "info",
            )),
        );
        window.update(|_, cx| {
            assert_eq!(
                view.read(cx)
                    .notifications
                    .unread_count(&replacement_screen),
                1
            )
        });
        while let Ok(command) = commands.try_recv() {
            assert!(
                matches!(
                    command.kind(),
                    Command::ResizePane { .. } | Command::Visible { .. } | Command::SetColorScheme { .. }
                ),
                "diagnostic layout and replacement owner may only report geometry or initialize OS scheme"
            );
        }
    });
}

#[test]
fn notification_delivery_uses_retained_offline_source_and_rejects_replaced_epoch() {
    let rt = Runtime::new().unwrap();
    with_view(|window, view| {
        let mut model = model(Uuid::new_v4());
        let local = model.lifecycle_token();
        let local_screen = model.local_source().screen_key("work");
        let (remote_screen, route) = model.ui_test_remote_terminal_fixture("badge.test").unwrap();
        let remote = route.source_token();
        let mut commands = setup(window, &view, model.clone());
        model.disconnect(remote.source_id()).unwrap();
        model.finish_attempt(remote.source_id()).unwrap();
        deliver(
            window,
            &view,
            Some(Update::Workspace(Box::new(model.clone()))),
        );
        while commands.try_recv().is_ok() {}
        deliver(
            window,
            &view,
            Some(event(&rt, remote, "Collision", "s-p0", "error")),
        );
        deliver(
            window,
            &view,
            Some(event(&rt, local, "Collision", "s-p0", "info")),
        );
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert_eq!(workspace.notifications.unread_count(&remote_screen), 1);
            assert_eq!(workspace.notifications.unread_count(&local_screen), 1);
        });
        assert!(
            commands.try_recv().is_err(),
            "offline facts never connect or execute commands"
        );
        model.ui_test_remote_terminal_fixture("badge.test").unwrap();
        deliver(window, &view, Some(Update::Workspace(Box::new(model))));
        deliver(
            window,
            &view,
            Some(event(&rt, remote, "Obsolete", "s-p0", "error")),
        );
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert_eq!(workspace.notifications.unread_count(&remote_screen), 1);
            assert_eq!(workspace.notifications.unread_count(&local_screen), 1);
        });
    });
}

#[test]
fn notification_control_changes_badges_only_after_authoritative_readback() {
    use splitty_client_core::{
        local_service::{SettingsWriteOutcome, SettingsWriteResult},
        settings::SettingsField,
    };
    let rt = Runtime::new().unwrap();
    with_view(|window, view| {
        let model = model(Uuid::new_v4());
        let owner = model.lifecycle_token();
        let screen = model.local_source().screen_key("work");
        let mut commands = setup(window, &view, model);
        deliver(
            window,
            &view,
            Some(event(&rt, owner, "Unread", "s-p1", "error")),
        );
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| workspace.show_settings(window, cx));
            window.draw(cx).clear(cx);
        });
        let click = |window: &mut VisualTestContext, selector: &'static str, switch: bool| {
            let bounds = window.debug_bounds(selector).unwrap();
            let position = if switch {
                gpui_kit::point(bounds.left() + gpui_kit::px(8.), bounds.center().y)
            } else {
                bounds.center()
            };
            window.simulate_mouse_down(position, gpui_kit::MouseButton::Left, Modifiers::default());
            window.simulate_mouse_up(position, gpui_kit::MouseButton::Left, Modifiers::default());
            window.run_until_parked();
            redraw(window);
        };
        click(window, "settings-nav-notifications", false);
        while commands.try_recv().is_ok() {}
        for rejected in [true, false] {
            click(window, "settings-control-notifications.enabled", true);
            let command = commands.try_recv().expect("native explicit switch intent");
            let request = match command.kind() {
                Command::UpdateSettings {
                    connection,
                    request_id,
                    edit,
                } => {
                    assert_eq!(*connection, owner.connection_id());
                    assert_eq!(edit.field(), SettingsField::NotificationEnabled);
                    *request_id
                }
                _ => panic!("expected settings intent"),
            };
            window.update(|_, cx| assert_eq!(view.read(cx).notifications.unread_count(&screen), 1));
            deliver(
                window,
                &view,
                Some(Update::SettingsWrite(SettingsWriteResult::ui_test_fixture(
                    owner.connection_id(),
                    request,
                    SettingsField::NotificationEnabled,
                    3,
                    if rejected {
                        SettingsWriteOutcome::Rejected {
                            reason: "not saved".into(),
                        }
                    } else {
                        SettingsWriteOutcome::Confirmed
                    },
                ))),
            );
            window.update(|_, cx| assert_eq!(view.read(cx).notifications.unread_count(&screen), 1));
            assert!(
                commands.try_recv().is_err(),
                "acknowledgement does not replay the change"
            );
        }
        deliver(
            window,
            &view,
            Some(Update::NotificationSettings {
                owner,
                read: read(owner, 3, false),
            }),
        );
        window.update(|_, cx| assert_eq!(view.read(cx).notifications.unread_count(&screen), 0));
        let foreign = WindowWorkspace::new(Uuid::new_v4()).lifecycle_token();
        deliver(
            window,
            &view,
            Some(Update::NotificationSettings {
                owner: foreign,
                read: read(foreign, 99, true),
            }),
        );
        deliver(
            window,
            &view,
            Some(event(&rt, owner, "Still disabled", "s-p1", "error")),
        );
        window.update(|_, cx| assert_eq!(view.read(cx).notifications.unread_count(&screen), 0));
    });
}

fn status_event(
    session: &str,
    pane: &str,
    screen: &str,
    status: serde_json::Value,
) -> serde_json::Value {
    let id = format!("status-{session}-{}", status["updatedAt"]);
    let mut event = json!({"type":"program_status.changed","id":id,"timestamp":1,
        "sessionId":session,"paneId":pane,"screenId":screen,"status":status});
    // The server marks and classifies only these states.
    let severity = match event["status"]["state"].as_str() {
        Some("blocked") => Some("warning"),
        Some("done") => Some("success"),
        Some("error") => Some("error"),
        _ => None,
    };
    if let Some(severity) = severity {
        event["notify"] = json!(true);
        event["notice"] = json!({"kind":"program-status","severity":severity,
            "title":format!("{screen}: status"),"setting":"programStatus"});
    }
    event
}

/// The worker applies a report to the window's model, publishes the model,
/// then the event.
fn report(
    rt: &Runtime,
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    model: &mut WindowWorkspace,
    source: SourceToken,
    event: serde_json::Value,
) {
    model
        .ui_test_program_status(source.source_id(), &event)
        .unwrap();
    deliver(
        window,
        view,
        Some(Update::Workspace(Box::new(model.clone()))),
    );
    let event = rt
        .block_on(Event::ui_test_json(source.connection_id(), &event))
        .unwrap();
    deliver(
        window,
        view,
        Some(Update::Event {
            source,
            event: Box::new(event),
        }),
    );
}

fn has(window: &mut VisualTestContext, selector: String) -> bool {
    window
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .is_some()
}

#[test]
fn program_status_marks_tabs_by_state_and_drops_a_watched_done() {
    let rt = Runtime::new().unwrap();
    with_view(|window, view| {
        let mut model = model(Uuid::new_v4());
        let owner = model.lifecycle_token();
        let local = owner.source_id();
        let screen = model.local_source().screen_key("work");
        let pane = model.local_source().pane_key("p1");
        let mut commands = setup(window, &view, model.clone());
        let states = ["idle", "working", "blocked", "done", "error"];
        let marks = |window: &mut VisualTestContext, pane: &str| {
            states
                .iter()
                .filter(|state| has(window, format!("tab-status-{pane}-{state}")))
                .copied()
                .collect::<Vec<_>>()
        };
        for (at, (record, mark, summary)) in [
            (
                json!({"state":"working","progress":40}),
                "working",
                "Working, 40%",
            ),
            (
                json!({"state":"blocked","kind":"permission","msg":"Allow edit?"}),
                "blocked",
                "Waiting for permission: Allow edit?",
            ),
            (json!({"state":"done","title":"Edit"}), "done", "Done: Edit"),
            (json!({"state":"error"}), "error", "Failed"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut record = record;
            record["updatedAt"] = json!(at + 1);
            report(
                &rt,
                window,
                &view,
                &mut model,
                owner,
                status_event("s-p1", "p1", "work", record),
            );
            assert_eq!(marks(window, "p1"), [mark]);
            assert!(marks(window, "p0").is_empty());
            assert!(
                has(window, format!("drawer-status-p1-{mark}")),
                "the pane's row in the Drawer shows the state too"
            );
            assert_eq!(
                has(window, format!("drawer-blocked-{screen}")),
                mark == "blocked",
                "only a waiting pane marks its Screen"
            );
            window.update(|_, cx| {
                assert_eq!(
                    view.read(cx)
                        .tab_program_status(&pane)
                        .map(|status| status.summary())
                        .as_deref(),
                    Some(summary)
                );
            });
        }
        report(
            &rt,
            window,
            &view,
            &mut model,
            owner,
            status_event("s-p1", "p1", "work", json!({"state":"idle","updatedAt":5})),
        );
        assert!(
            marks(window, "p1").is_empty(),
            "an idle program shows no state to notice"
        );
        for rest in ["tab-status-p1-rest", "drawer-status-p1-rest"] {
            assert!(
                has(window, rest.into()),
                "an idle program shows that it rests"
            );
        }
        assert!(
            !has(window, "tab-status-p0-rest".into())
                && !has(window, "drawer-status-p0-rest".into()),
            "a pane without a record shows no mark"
        );
        // The window was not watched: blocked, done and error notified.
        window.update(|_, cx| assert_eq!(view.read(cx).notifications.unread_count(&screen), 3));

        // The switch silences the notifications, not the marks.
        let off = NotificationSettingsRead::ui_test_fixture(
            owner.connection_id(),
            3,
            Ok(serde_json::from_value(json!({
                "enabled":true,"desktop":false,"toasts":false,"processFinished":true,
                "processExited":true,"processCrashed":true,"programStatus":false
            }))
            .unwrap()),
        );
        deliver(
            window,
            &view,
            Some(Update::NotificationSettings { owner, read: off }),
        );
        report(
            &rt,
            window,
            &view,
            &mut model,
            owner,
            status_event("s-p1", "p1", "work", json!({"state":"done","updatedAt":6})),
        );
        assert_eq!(marks(window, "p1"), ["done"]);
        window.update(|_, cx| assert_eq!(view.read(cx).notifications.unread_count(&screen), 0));

        // An active window watches its focused pane only.
        window.update(|window, _| window.activate_window());
        window.executor().run_until_parked();
        redraw(window);
        assert_eq!(marks(window, "p1"), ["done"]);
        model.source_mut(local).unwrap().focus_pane("p1");
        deliver(
            window,
            &view,
            Some(Update::Workspace(Box::new(model.clone()))),
        );
        assert!(
            marks(window, "p1").is_empty(),
            "a watched done loses its mark"
        );
        assert!(
            has(window, "tab-status-p1-rest".into()),
            "a seen done shows as a program at rest"
        );
        window.deactivate_window();
        window.executor().run_until_parked();
        deliver(
            window,
            &view,
            Some(Update::Workspace(Box::new(model.clone()))),
        );
        assert!(marks(window, "p1").is_empty(), "the seen record stays seen");
        report(
            &rt,
            window,
            &view,
            &mut model,
            owner,
            status_event("s-p1", "p1", "work", json!({"state":"error","updatedAt":7})),
        );
        assert_eq!(marks(window, "p1"), ["error"], "a newer record shows again");
        while let Ok(command) = commands.try_recv() {
            assert!(
                !matches!(
                    command.kind(),
                    Command::Select { .. } | Command::SwitchScreen(_)
                ),
                "marks never move the window"
            );
        }
    });
}

#[test]
fn a_blocked_pane_on_a_host_screen_not_opened_marks_that_screen_and_the_host_group() {
    let rt = Runtime::new().unwrap();
    with_view(|window, view| {
        let mut model = model(Uuid::new_v4());
        let local = model.lifecycle_token();
        let local_screen = model.local_source().screen_key("work");
        let (remote_screen, route) = model.ui_test_remote_terminal_fixture("badge.test").unwrap();
        let remote = route.source_token();
        let host: SourceId = remote.source_id();
        model
            .ui_test_remote_catalog(host, &["work", "review"])
            .unwrap();
        let mut commands = setup(window, &view, model.clone());
        assert!(!has(window, format!("drawer-host-blocked-{host}")));

        // The same session id on the local source marks only the local pane.
        report(
            &rt,
            window,
            &view,
            &mut model,
            local,
            status_event(
                "s-p0",
                "p0",
                "work",
                json!({"state":"blocked","kind":"question","updatedAt":1}),
            ),
        );
        assert!(has(window, format!("drawer-blocked-{local_screen}")));
        assert!(!has(window, format!("drawer-host-blocked-{host}")));
        assert!(!has(window, format!("drawer-blocked-{remote_screen}")));

        report(
            &rt,
            window,
            &view,
            &mut model,
            remote,
            status_event(
                "s-review",
                "p9",
                "review",
                json!({"state":"blocked","kind":"question","msg":"Which branch?","updatedAt":2}),
            ),
        );
        // A Screen the host does not list marks nothing.
        report(
            &rt,
            window,
            &view,
            &mut model,
            remote,
            status_event(
                "s-gone",
                "p8",
                "gone",
                json!({"state":"blocked","updatedAt":3}),
            ),
        );
        assert!(has(window, format!("drawer-host-blocked-{host}")));
        assert!(!has(window, format!("drawer-blocked-{remote_screen}")));
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert!(workspace.host_screen_blocked("badge.test", "review"));
            assert!(!workspace.host_screen_blocked("badge.test", "work"));
            assert!(!workspace.host_screen_blocked("badge.test", "gone"));
        });
        while commands.try_recv().is_ok() {}
        let caption = window
            .debug_bounds(Box::leak(format!("drawer-host-{host}").into_boxed_str()))
            .unwrap()
            .center();
        window.simulate_click(caption, Modifiers::default());
        window.run_until_parked();
        redraw(window);
        assert!(
            has(window, "popover-screen-review-detail-Waiting".into()),
            "the host's panel words the Screen not opened as waiting"
        );
        assert!(has(window, "popover-screen-work-detail-1 pane".into()));
        assert!(
            commands.try_recv().is_err(),
            "showing a mark connects and opens nothing"
        );

        // A host that is not connected shows no status.
        model.disconnect(host).unwrap();
        deliver(window, &view, Some(Update::Workspace(Box::new(model))));
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert!(!workspace.source_blocked(host));
            assert!(!workspace.host_screen_blocked("badge.test", "review"));
            assert!(workspace.source_blocked(local.source_id()));
        });
    });
}
