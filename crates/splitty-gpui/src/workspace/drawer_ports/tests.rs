use super::{HostGroupTarget, Workspace, MAX_MENU_PORTS};
use crate::workspace::chrome::view_tests::with_view;
use crate::workspace::{Command, Destination, WorkerCommand};
use gpui_kit::{
    point, px, size, Entity, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton,
    VisualTestContext,
};
use serde_json::json;
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
fn real_kit_popup_inspection_escape_and_keyboard_choice_keep_exact_host() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        let trigger = format!("drawer-ports-{}", group.source_id());
        click(window, trigger.clone());
        let first = window.update(|_, cx| view.read(cx).drawer_port_menu.as_ref().unwrap().id);
        assert!(receiver.try_recv().is_err(), "opening/rendering is passive");
        press(window, "escape");
        window.update(|_, cx| assert!(view.read(cx).drawer_port_menu.is_none()));
        assert!(receiver.try_recv().is_err(), "dismissal is passive");
        click(window, trigger);
        window
            .update(|_, cx| assert_ne!(view.read(cx).drawer_port_menu.as_ref().unwrap().id, first));
        press(window, "down");
        press(window, "enter");
        let command = receiver
            .try_recv()
            .expect("keyboard choice submits exactly once");
        assert_eq!(command.source_token(), Some(group.owner()));
        assert!(
            matches!(command.kind(), Command::ManageForward { request, .. }
            if request.action() == ForwardAction::Open && request.key().target() == "two.test"
                && request.key().remote() == 3000 && request.local() == Some(4173) && !request.remember())
        );
        assert!(receiver.try_recv().is_err());
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert!(workspace.drawer_port_menu.is_none());
            assert!(
                workspace.drawer_port_presentation(&group).entries[0].disabled,
                "pending mapping cannot admit a duplicate choice"
            );
        });
    });
}

#[test]
fn unpinned_drawer_stays_revealed_for_popup_then_resumes_autohide() {
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
        window.simulate_mouse_move(point(px(500.), px(180.)), None, Modifiers::default());
        draw(window);
        window.update(|_, cx| {
            assert!(
                view.read(cx).drawer_overlay_open,
                "travelling to an open popup must not dismiss its Drawer"
            )
        });
        press(window, "escape");
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert!(!workspace.drawer_overlay_open);
            assert!(workspace.drawer_port_menu.is_none());
        });
        assert!(receiver.try_recv().is_err());
    });
}

#[test]
fn caption_pointer_navigates_without_connect_or_forward() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        let selector = format!("drawer-host-{}", group.source_id());
        click(window, selector.clone());
        window.update(|_, cx| assert!(view.read(cx).destination == Destination::Hosts));
        while let Ok(command) = receiver.try_recv() {
            assert!(matches!(
                command.kind(),
                Command::RefreshForwards | Command::Visible { .. }
            ));
        }
    });
}

#[test]
fn stale_popup_choices_cannot_restart_changed_ports_or_revive_after_detach() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        let trigger = format!("drawer-ports-{}", group.source_id());
        click(window, trigger.clone());
        window.update(|_, cx| view.update(cx, |workspace, _| {
            assert!(workspace.ports.accept(snapshot(group.owner(), 2, Some("[]"), Some(
                r#"{"version":1,"forwards":[{"target":"two.test","remote":3000,"local":4173,"label":"App","enabled":false,"lastError":null}]}"#))));
        }));
        draw(window);
        // Kit deliberately caches open menu rows. The old Up item now asks for a fresh choice.
        press(window, "down");
        press(window, "enter");
        assert!(receiver.try_recv().is_err());
        window.update(|_, cx| {
            assert!(view
                .read(cx)
                .ports
                .link_notice
                .as_ref()
                .unwrap()
                .contains("choose again"))
        });
        click(window, trigger.clone());
        let (old, choice, screen) = window.update(|_, cx| {
            let workspace = view.read(cx);
            let screen = workspace
                .model
                .as_ref()
                .unwrap()
                .opened()
                .into_iter()
                .find(|entry| entry.key().source_id() == group.source_id())
                .unwrap()
                .key()
                .clone();
            (
                workspace.drawer_port_menu.clone().unwrap(),
                workspace.drawer_port_presentation(&group).entries[0]
                    .choice
                    .clone(),
                screen,
            )
        });
        press(window, "escape");
        click(window, trigger);
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.choose_drawer_port(&old, &choice, cx);
                assert!(
                    workspace.drawer_port_menu.is_some(),
                    "old callback cannot consume a new menu"
                );
                let immediate = workspace.drawer_port_menu.clone().unwrap();
                workspace.request_close_screen(screen.clone(), window, cx);
                assert!(
                    workspace.drawer_port_menu.is_none(),
                    "final detach revokes before model acknowledgement"
                );
                workspace.choose_drawer_port(&immediate, &choice, cx);
                let mut model = workspace.model.clone().unwrap();
                let remote_owner = model.source(group.source_id()).unwrap().source_token();
                model.detach(group.owner(), &screen).unwrap();
                workspace.reconcile(model.clone(), window, cx);
                assert!(workspace.drawer_port_menu.is_none());
                model.open_existing(remote_owner, &screen).unwrap();
                workspace.reconcile(model, window, cx);
                assert!(
                    group.is_current(workspace.model.as_ref().unwrap()),
                    "same source group reappears"
                );
                workspace.choose_drawer_port(&old, &choice, cx);
                assert!(
                    workspace.drawer_port_menu.is_none(),
                    "reappearing group cannot revive a capture"
                );
            })
        });
        while let Ok(command) = receiver.try_recv() {
            assert!(!matches!(command.kind(), Command::ManageForward { .. }));
        }
    });
}

#[test]
fn projection_is_bounded_truthful_and_uses_shared_mapping_states() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        window.update(|_, cx| view.update(cx, |workspace, _| {
            let model = workspace.model.as_mut().unwrap();
            model.ui_test_port_observation(group.source_id(), &[(3000, "node"), (3001, "worker")]).unwrap();
            let saved = json!({"version":1,"forwards":(3002..3016).map(|remote|
                json!({"target":"two.test","remote":remote,"local":remote+1000,"label":"App","enabled":remote!=3003,
                    "lastError":if remote==3004 { json!({"code":"fixture","message":"Failed","hint":null}) } else { serde_json::Value::Null }})).collect::<Vec<_>>()});
            let active = r#"[{"target":"two.test","remote":3000,"local":4173,"state":"up"},{"target":"two.test","remote":3002,"local":4002,"state":"down"},{"target":"one.test","remote":9999,"local":9999,"state":"up"}]"#;
            assert!(workspace.ports.accept(snapshot(group.owner(), 2, Some(active), Some(&saved.to_string()))));
            let projected = workspace.drawer_port_presentation(&group);
            assert_eq!(projected.total, 16);
            assert_eq!(projected.entries.len(), MAX_MENU_PORTS);
            assert!(projected.status.is_none());
            assert!(projected.entries[0].label.starts_with("Open 3000 · localhost:4173"));
            assert!(projected.entries[1].label.contains("detected"));
            assert!(projected.entries[2].label.contains("reconnecting"));
            assert!(projected.entries[3].label.contains("paused, preferred local 4003"));
            assert!(projected.entries[4].label.contains("failed, preferred local 4004"));
            assert!(projected.entries[5].label.contains("saved, preferred local 4005"));
            assert!(projected.entries.iter().all(|entry| entry.choice.key.target() == group.target()));
            assert!(workspace.ports.accept(snapshot(group.owner(), 3, None, Some(EMPTY_SAVED))));
            assert_eq!(workspace.drawer_port_presentation(&group).status, Some("Some port information is unavailable"));
            workspace.model.as_mut().unwrap().ui_test_port_observation(group.source_id(), &[]).unwrap();
            assert!(workspace.ports.accept(snapshot(group.owner(), 4, Some("[]"), Some(EMPTY_SAVED))));
            assert_eq!(workspace.drawer_port_presentation(&group).status, Some("No ports reported"));
            workspace.model.as_mut().unwrap().disconnect(group.source_id()).unwrap();
            assert_eq!(workspace.drawer_port_presentation(&group).status, Some("Host has not reported port discovery"));
        }));
        assert!(
            receiver.try_recv().is_err(),
            "projection never submits a command"
        );
    });
}

#[test]
fn menu_owner_replacement_and_consumed_choice_cannot_submit_again() {
    with_view(|window, view| {
        let (group, mut receiver) = setup(window, &view);
        click(window, format!("drawer-ports-{}", group.source_id()));
        let (capture, choice) = window.update(|_, cx| {
            let workspace = view.read(cx);
            (
                workspace.drawer_port_menu.clone().unwrap(),
                workspace.drawer_port_presentation(&group).entries[0]
                    .choice
                    .clone(),
            )
        });
        window.update(|_, cx| {
            view.update(cx, |workspace, cx| {
                workspace.choose_drawer_port(&capture, &choice, cx);
                workspace.choose_drawer_port(&capture, &choice, cx);
            })
        });
        assert!(matches!(
            receiver.try_recv().unwrap().kind(),
            Command::ManageForward { .. }
        ));
        assert!(receiver.try_recv().is_err());
        press(window, "escape");
        click(window, format!("drawer-ports-{}", group.source_id()));
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                let capture = workspace.drawer_port_menu.clone().unwrap();
                let replacement = WindowWorkspace::new(Uuid::new_v4());
                workspace.reconcile(replacement, window, cx);
                assert!(workspace.drawer_port_menu.is_none());
                workspace.choose_drawer_port(&capture, &choice, cx);
            })
        });
        while let Ok(command) = receiver.try_recv() {
            assert!(!matches!(command.kind(), Command::ManageForward { .. }));
        }
    });
}
