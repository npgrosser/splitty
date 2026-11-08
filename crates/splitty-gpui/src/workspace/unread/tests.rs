use super::{Event, NotificationSettingsRead, Severity};
use crate::workspace::chrome::view_tests::with_view;
use crate::workspace::{Command, Destination, Update, WorkerCommand, Workspace};
use gpui_kit::{Entity, Modifiers, VisualTestContext, WindowVisibility};
use serde_json::json;
use splitty_client_core::workspace::{SourceToken, WindowWorkspace};
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
                "type":"notification","id":id,"timestamp":1,"sessionId":session,
                "title":id,"severity":severity
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
