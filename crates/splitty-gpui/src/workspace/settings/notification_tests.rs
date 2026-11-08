use super::{SettingsDomain, SettingsField, SettingsValue, WriteState};
use crate::workspace::{chrome::view_tests::with_view, Command, Update, WorkerCommand, Workspace};
use gpui_kit::{
    point, px, size, Entity, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton,
    ScrollDelta, ScrollEvent, TouchPhase, VisualTestContext,
};
use serde_json::json;
use splitty_client_core::{
    appearance::test_support::{settings_snapshot, Availability},
    local_service::{
        NotificationSettingsRead, SettingsReconciled, SettingsWriteOutcome, SettingsWriteResult,
    },
    workspace::{SourceToken, WindowWorkspace},
};
use tokio::sync::mpsc;
use uuid::Uuid;

fn read(
    owner: SourceToken,
    generation: u64,
    enabled: bool,
    defaults: bool,
) -> NotificationSettingsRead {
    let settings = serde_json::from_value(json!({
        "enabled":enabled,"toasts":true,"desktop":false,
        "processFinished":true,"processExited":false,"processCrashed":true
    }))
    .unwrap();
    if defaults {
        NotificationSettingsRead::ui_test_with_defaults(
            owner.connection_id(),
            generation,
            settings,
            serde_json::from_value(json!({
                "enabled":true,"toasts":false,"desktop":true,
                "processFinished":false,"processExited":false,"processCrashed":true
            }))
            .unwrap(),
        )
    } else {
        NotificationSettingsRead::ui_test_fixture(owner.connection_id(), generation, Ok(settings))
    }
}

fn deliver(window: &mut VisualTestContext, view: &Entity<Workspace>, update: Update) {
    let (_, mut updates) = mpsc::channel(1);
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            workspace.poll(Some(update), &mut updates, window, cx)
        });
        window.draw(cx).clear(cx);
    });
}

fn setup(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
) -> (SourceToken, mpsc::Receiver<WorkerCommand>) {
    let mut model = WindowWorkspace::new(Uuid::new_v4());
    model.ui_test_local_screen_fixture("Work").unwrap();
    let owner = model.lifecycle_token();
    let (remote, _) = model
        .ui_test_remote_terminal_fixture("vm.example.test")
        .unwrap();
    model.select(owner, &remote).unwrap();
    let (commands, mut receiver) = mpsc::channel(32);
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            workspace.commands = commands;
            workspace.reconcile(model, window, cx);
        });
        window.draw(cx).clear(cx);
    });
    deliver(
        window,
        view,
        Update::NotificationSettings {
            owner,
            read: read(owner, 1, true, true),
        },
    );
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| workspace.show_settings(window, cx));
        window.draw(cx).clear(cx);
    });
    click(window, "settings-nav-notifications", false);
    while receiver.try_recv().is_ok() {}
    (owner, receiver)
}

fn click(window: &mut VisualTestContext, selector: &'static str, switch: bool) {
    let bounds = window.debug_bounds(selector).expect(selector);
    // The existing control container starts at the Switch track's leading edge.
    let point = if switch {
        point(bounds.left() + px(8.), bounds.center().y)
    } else {
        bounds.center()
    };
    window.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    window.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    window.run_until_parked();
    window.update(|window, cx| window.draw(cx).clear(cx));
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
    window.run_until_parked();
    window.update(|window, cx| window.draw(cx).clear(cx));
}

fn request(
    receiver: &mut mpsc::Receiver<WorkerCommand>,
    owner: SourceToken,
    field: SettingsField,
    value: bool,
) -> Uuid {
    let command = receiver.try_recv().expect("explicit settings intent");
    match command.kind() {
        Command::UpdateSettings {
            connection,
            request_id,
            edit,
        } => {
            assert_eq!(
                *connection,
                owner.connection_id(),
                "selected remote never changes settings owner"
            );
            assert_eq!(edit.field(), field);
            assert_eq!(edit.value(), &SettingsValue::Bool(value));
            *request_id
        }
        _ => panic!("expected one field intent"),
    }
}

#[test]
fn notification_switches_and_reset_use_real_local_settings_event_path() {
    with_view(|window, view| {
        let (owner, mut commands) = setup(window, &view);
        for (field, next) in [
            (SettingsField::NotificationEnabled, false),
            (SettingsField::NotificationProcessFinished, false),
            (SettingsField::NotificationProcessExited, true),
            (SettingsField::NotificationProcessCrashed, false),
        ] {
            let selector = Box::leak(format!("settings-control-{}", field.path()).into_boxed_str());
            click(window, selector, true);
            let id = request(&mut commands, owner, field, next);
            assert!(commands.try_recv().is_err(), "one native switch request");
            press(window, "space");
            window.run_until_parked();
            assert!(
                commands.try_recv().is_err(),
                "pending control cannot repeat through keyboard"
            );
            window.update(|_, cx| {
                let settings = view.read(cx).settings.read(cx);
                assert_eq!(
                    settings.value(field),
                    Some(SettingsValue::Bool(!next)),
                    "controlled switch keeps confirmed value"
                );
                assert!(!settings.enabled(field));
            });
            if field == SettingsField::NotificationEnabled {
                deliver(
                    window,
                    &view,
                    Update::SettingsWrite(SettingsWriteResult::ui_test_fixture(
                        owner.connection_id(),
                        id,
                        field,
                        3,
                        SettingsWriteOutcome::Confirmed,
                    )),
                );
                deliver(
                    window,
                    &view,
                    Update::Appearance(settings_snapshot(
                        owner.connection_id(),
                        99,
                        Availability::Full,
                    )),
                );
                window.update(|_, cx| {
                    assert!(
                        !view.read(cx).settings.read(cx).enabled(field),
                        "appearance cannot complete notification edit"
                    );
                });
                deliver(
                    window,
                    &view,
                    Update::NotificationSettings {
                        owner,
                        read: read(owner, 3, false, true),
                    },
                );
                window.update(|_, cx| {
                    let settings = view.read(cx).settings.read(cx);
                    assert_eq!(settings.value(field), Some(SettingsValue::Bool(false)));
                    assert!(settings.enabled(field));
                    let values = settings.notification.as_ref().unwrap().result().unwrap();
                    assert!(values.process_finished && values.process_crashed && values.toasts);
                    assert!(
                        !values.process_exited && !values.desktop,
                        "master preserves independent switches"
                    );
                });
                click(window, "settings-reset-notifications.enabled", false);
                let reset = request(&mut commands, owner, field, true);
                deliver(
                    window,
                    &view,
                    Update::SettingsWrite(SettingsWriteResult::ui_test_fixture(
                        owner.connection_id(),
                        reset,
                        field,
                        4,
                        SettingsWriteOutcome::Rejected {
                            reason: "rejected".into(),
                        },
                    )),
                );
                window.update(|_, cx| {
                    let settings = view.read(cx).settings.read(cx);
                    assert_eq!(settings.value(field), Some(SettingsValue::Bool(false)));
                    assert!(settings.status(field).unwrap().contains("rejected"));
                });
            }
        }
    });
}

#[test]
fn narrow_settings_keeps_navigation_close_and_scrolled_controls_accessible() {
    with_view(|window, view| {
        let (owner, mut commands) = setup(window, &view);
        window.simulate_resize(size(px(900.), px(600.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        let wide_nav = window.debug_bounds("settings-nav").unwrap();
        let wide_content = window.debug_bounds("settings-content").unwrap();
        assert_eq!(wide_nav.right(), wide_content.left());
        assert_eq!(wide_nav.top(), wide_content.top());
        window.simulate_resize(size(px(560.), px(420.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        let drawer = window.debug_bounds("drawer-slide").unwrap();
        let settings = window.debug_bounds("settings-view").unwrap();
        let nav = window.debug_bounds("settings-nav").unwrap();
        let content = window.debug_bounds("settings-content").unwrap();
        let close = window.debug_bounds("settings-close").unwrap();
        assert_eq!(drawer.size.width, px(200.));
        assert_eq!(settings.left(), drawer.right());
        assert_eq!(settings.size.width, px(360.));
        assert_eq!(nav.right(), settings.right());
        assert_eq!(content.top(), nav.bottom());
        assert!(close.left() >= content.left() && close.right() <= content.right());
        assert!(close.bottom() <= content.bottom());
        assert!(
            window
                .debug_bounds("settings-nav-notifications")
                .unwrap()
                .right()
                <= nav.right()
        );
        assert!(commands.try_recv().is_err());
        click(window, "settings-close", false);
        window.update(|_, cx| {
            assert!(view.read(cx).destination != crate::workspace::Destination::Settings);
        });
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| workspace.show_settings(window, cx));
            window.draw(cx).clear(cx);
        });
        while let Ok(command) = commands.try_recv() {
            assert!(
                !matches!(command.kind(), Command::UpdateSettings { .. }),
                "layout and close do not submit settings"
            );
        }
        let position = window.debug_bounds("settings-content").unwrap().center();
        window.simulate_event(ScrollEvent {
            position,
            delta: ScrollDelta::Lines(point(0., -12.)),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let crashed = window
            .debug_bounds("settings-control-notifications.processCrashed")
            .unwrap();
        assert!(crashed.top() >= content.top() && crashed.bottom() <= content.bottom());
        click(
            window,
            "settings-control-notifications.processCrashed",
            true,
        );
        request(
            &mut commands,
            owner,
            SettingsField::NotificationProcessCrashed,
            false,
        );
        assert!(
            commands.try_recv().is_err(),
            "one explicit settings command"
        );
    });
}

#[test]
fn notification_controls_keep_last_good_values_and_fence_uncertainty_and_old_owner() {
    with_view(|window, view| {
        let (owner, mut commands) = setup(window, &view);
        let field = SettingsField::NotificationEnabled;
        deliver(
            window,
            &view,
            Update::AppearanceError {
                connection: owner.connection_id(),
                reason: "theme failed".into(),
            },
        );
        deliver(
            window,
            &view,
            Update::NotificationSettings {
                owner,
                read: read(owner, 2, true, false),
            },
        );
        click(window, "settings-reset-notifications.enabled", false);
        assert!(
            commands.try_recv().is_err(),
            "missing default cannot submit Reset"
        );
        click(window, "settings-control-notifications.enabled", true);
        let id = request(&mut commands, owner, field, false);
        deliver(
            window,
            &view,
            Update::SettingsWrite(SettingsWriteResult::ui_test_fixture(
                owner.connection_id(),
                id,
                field,
                3,
                SettingsWriteOutcome::Uncertain {
                    reason: "lost reply".into(),
                },
            )),
        );
        deliver(
            window,
            &view,
            Update::SettingsReconciled(SettingsReconciled::ui_test_fixture(
                owner.connection_id(),
                99,
                SettingsDomain::Appearance,
                vec![(field, id)],
            )),
        );
        window.update(|_, cx| assert!(!view.read(cx).settings.read(cx).enabled(field)));
        deliver(
            window,
            &view,
            Update::NotificationSettings {
                owner,
                read: read(owner, 3, true, false),
            },
        );
        deliver(
            window,
            &view,
            Update::SettingsReconciled(SettingsReconciled::ui_test_fixture(
                owner.connection_id(),
                3,
                SettingsDomain::Notifications,
                vec![(field, id)],
            )),
        );
        window.update(|_, cx| {
            let settings = view.read(cx).settings.read(cx);
            assert!(settings.enabled(field));
            assert!(matches!(
                settings.writes.get(&field),
                Some(WriteState::Uncertain { blocked: false, .. })
            ));
        });
        assert!(
            commands.try_recv().is_err(),
            "uncertain mutation never replays"
        );
        press(window, "space");
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        let keyboard = request(&mut commands, owner, field, false);
        deliver(
            window,
            &view,
            Update::SettingsWrite(SettingsWriteResult::ui_test_fixture(
                owner.connection_id(),
                keyboard,
                field,
                4,
                SettingsWriteOutcome::Rejected {
                    reason: "keyboard edit rejected".into(),
                },
            )),
        );
        deliver(
            window,
            &view,
            Update::NotificationSettings {
                owner,
                read: NotificationSettingsRead::ui_test_fixture(
                    owner.connection_id(),
                    4,
                    Err("read failed".into()),
                ),
            },
        );
        deliver(
            window,
            &view,
            Update::NotificationSettings {
                owner,
                read: read(owner, 3, false, true),
            },
        );
        window.update(|_, cx| {
            let settings = view.read(cx).settings.read(cx);
            assert_eq!(
                settings.value(field),
                Some(SettingsValue::Bool(true)),
                "failed and superseded reads keep last good"
            );
            assert!(settings
                .notification_notice()
                .unwrap()
                .contains("read failed"));
        });
        let replacement = WindowWorkspace::new(Uuid::new_v4());
        let next = replacement.lifecycle_token();
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.reconcile(replacement, window, cx)
            })
        });
        deliver(
            window,
            &view,
            Update::NotificationSettings {
                owner: next,
                read: read(next, 1, true, true),
            },
        );
        window.update(|_, cx| {
            let settings = view.read(cx).settings.clone();
            settings.update(cx, |settings, cx| {
                settings.request_captured(
                    Some(owner.connection_id()),
                    field,
                    SettingsValue::Bool(false),
                    cx,
                );
                settings.reset_captured(
                    Some(owner.connection_id()),
                    field,
                    SettingsValue::Bool(true),
                    cx,
                );
            });
            assert_eq!(
                settings.read(cx).value(field),
                Some(SettingsValue::Bool(true))
            );
            assert!(settings.read(cx).writes.is_empty());
        });
        while let Ok(command) = commands.try_recv() {
            assert!(
                !matches!(command.kind(), Command::UpdateSettings { .. }),
                "obsolete controls cannot change replacement owner"
            );
        }
    });
}
