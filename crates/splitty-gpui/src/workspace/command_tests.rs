//! The keyboard commands that had no key of their own: each runs through
//! the path its pointer control takes.
use crate::workspace::{chrome::view_tests::with_view, Command, Destination, WorkerCommand};
use crate::workspace::{ScreenKey, Workspace};
use gpui_kit::{Entity, Focusable, KeyUpEvent, Keystroke, VisualTestContext};
use splitty_client_core::workspace::WindowWorkspace;
use tokio::sync::mpsc;
use uuid::Uuid;

/// A local Screen "work" with the given tiled layout and floating groups,
/// the pane `focus` focused.
fn model(
    root: serde_json::Value,
    floating: serde_json::Value,
    panes: &[&str],
    focus: &str,
) -> WindowWorkspace {
    let owner = Uuid::new_v4();
    let mut model = WindowWorkspace::new(owner);
    model.ui_test_local_screen_fixture("Work").unwrap();
    let source = model.lifecycle_token();
    let mut pane_states = serde_json::Map::new();
    let mut sessions = serde_json::Map::new();
    for (index, pane) in panes.iter().enumerate() {
        let session = format!("s{index}");
        pane_states.insert(
            (*pane).into(),
            serde_json::json!({"id": pane, "sessionId": session}),
        );
        sessions.insert(
            session.clone(),
            serde_json::json!({"id": session, "status": "running"}),
        );
    }
    {
        let mut local = model.source_mut(source.source_id()).unwrap();
        local
            .apply_snapshot(
                owner,
                &serde_json::json!({"type":"mux.screen_snapshot","screen":{
                    "id":"work","revision":2,"root":root,"floating":floating,
                    "panes":pane_states,"sessions":sessions
                }}),
            )
            .unwrap();
        for (index, pane) in panes.iter().enumerate() {
            local.observe_spawn(owner, pane, &format!("s{index}"));
        }
        local.set_tab_placement_supported(true);
        local.focus_pane(focus);
    }
    model
}

fn tabs(panes: &[&str], active: &str) -> serde_json::Value {
    serde_json::json!({"type":"slot","id":"slot","paneIds":panes,"activePaneId":active})
}

fn side_by_side() -> serde_json::Value {
    serde_json::json!({"type":"split","id":"split","axis":"horizontal","sizes":[1,1],"children":[
        {"type":"slot","id":"left","paneIds":["p0"],"activePaneId":"p0"},
        {"type":"slot","id":"right","paneIds":["p1"],"activePaneId":"p1"}
    ]})
}

fn no_floats() -> serde_json::Value {
    serde_json::json!([])
}

fn setup(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    model: WindowWorkspace,
) -> mpsc::Receiver<WorkerCommand> {
    let (sender, mut receiver) = mpsc::channel(128);
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            workspace.commands = sender.into();
            workspace.destination = Destination::Screens;
            workspace.drawer_visible = false;
            workspace.drawer_overlay_open = false;
            workspace.reconcile(model, window, cx);
        });
        window.draw(cx).clear(cx);
        view.update(cx, |workspace, cx| {
            workspace.focus.focus(window, cx);
            workspace.focus_current(window, cx);
        });
        window.draw(cx).clear(cx);
    });
    while receiver.try_recv().is_ok() {}
    receiver
}

/// The commands a key press sent, after the window drew its result.
fn press(
    window: &mut VisualTestContext,
    receiver: &mut mpsc::Receiver<WorkerCommand>,
    keys: &str,
) -> Vec<WorkerCommand> {
    window.simulate_keystrokes(keys);
    window.update(|window, cx| window.draw(cx).clear(cx));
    std::iter::from_fn(|| receiver.try_recv().ok())
        .filter(|command| !matches!(command.kind(), Command::Visible { .. }))
        .collect()
}

/// The terminal input bytes among `commands`.
fn inputs(commands: &[WorkerCommand]) -> Vec<Vec<u8>> {
    commands
        .iter()
        .filter_map(|command| match command.kind() {
            Command::Input { bytes, .. } => Some(bytes.clone()),
            _ => None,
        })
        .collect()
}

/// `keys` with the platform's command modifier in front.
fn modded(window: &mut VisualTestContext, keys: &str) -> String {
    let mac = window.update(|_, cx| cx.platform_info().is_mac());
    format!("{}-{keys}", if mac { "cmd" } else { "alt" })
}

fn is_mac(window: &mut VisualTestContext) -> bool {
    window.update(|_, cx| cx.platform_info().is_mac())
}

fn update<T>(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    run: impl FnOnce(&mut Workspace, &mut gpui_kit::Window, &mut gpui_kit::Context<Workspace>) -> T,
) -> T {
    window.update(|window, cx| view.update(cx, |workspace, cx| run(workspace, window, cx)))
}

#[test]
fn next_and_previous_screen_wrap_in_drawer_order_across_sources() {
    with_view(|window, view| {
        let mut model = WindowWorkspace::new(Uuid::new_v4());
        let local = model
            .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B")], None)
            .unwrap();
        let remote = model
            .ui_test_screen_order_fixture(&[("c", "C")], Some("host.test"))
            .unwrap();
        let owner = model.lifecycle_token();
        model.select(owner, &remote[0]).unwrap();
        let mut receiver = setup(window, &view, model);
        let switched = |commands: Vec<WorkerCommand>| -> Vec<ScreenKey> {
            commands
                .iter()
                .filter_map(|command| match command.kind() {
                    Command::SwitchScreen(key) => Some(key.clone()),
                    _ => None,
                })
                .collect()
        };
        // The host's Screen is the last one: the next is the first local one.
        assert_eq!(
            switched(press(window, &mut receiver, "ctrl-tab")),
            [local[0].clone()]
        );
        update(window, &view, |workspace, window, cx| {
            let mut model = workspace.model.clone().unwrap();
            model.select(model.lifecycle_token(), &local[0]).unwrap();
            workspace.reconcile(model, window, cx);
            workspace.focus.focus(window, cx);
        });
        while receiver.try_recv().is_ok() {}
        assert_eq!(
            switched(press(window, &mut receiver, "ctrl-shift-tab")),
            [remote[0].clone()]
        );
        assert_eq!(
            switched(press(window, &mut receiver, "ctrl-tab")),
            [local[1].clone()]
        );
    });
}

#[test]
fn close_screen_assesses_a_local_screen_and_detaches_a_hosts_one() {
    with_view(|window, view| {
        let mut model = WindowWorkspace::new(Uuid::new_v4());
        let local = model
            .ui_test_screen_order_fixture(&[("a", "A")], None)
            .unwrap();
        let remote = model
            .ui_test_screen_order_fixture(&[("c", "C")], Some("host.test"))
            .unwrap();
        let owner = model.lifecycle_token();
        model.select(owner, &local[0]).unwrap();
        let mut receiver = setup(window, &view, model);
        let close = modded(window, "shift-w");
        let sent = press(window, &mut receiver, &close);
        assert!(
            sent.iter()
                .any(|command| matches!(command.kind(), Command::AssessClose { .. })),
            "a local Screen is closed after its assessment"
        );
        update(window, &view, |workspace, window, cx| {
            workspace.pending_close = None;
            let mut model = workspace.model.clone().unwrap();
            model.select(model.lifecycle_token(), &remote[0]).unwrap();
            workspace.reconcile(model, window, cx);
            workspace.focus.focus(window, cx);
        });
        while receiver.try_recv().is_ok() {}
        let sent = press(window, &mut receiver, &close);
        assert!(
            sent.iter().any(|command| matches!(
                command.kind(),
                Command::DetachRemoteScreen { screen } if *screen == remote[0]
            )),
            "a host's Screen is detached"
        );
    });
}

#[test]
fn rename_screen_reveals_the_drawer_with_the_field_focused() {
    with_view(|window, view| {
        let model = model(tabs(&["p0"], "p0"), no_floats(), &["p0"], "p0");
        let mut receiver = setup(window, &view, model);
        let rename = modded(window, "shift-r");
        let sent = press(window, &mut receiver, &rename);
        assert!(inputs(&sent).is_empty(), "the key reaches no terminal");
        update(window, &view, |workspace, window, cx| {
            assert!(workspace.drawer_overlay_open, "the hidden Drawer shows");
            let editor = workspace.rename.as_ref().expect("the rename editor");
            assert!(editor.input.read(cx).focus_handle(cx).is_focused(window));
        });
        window.simulate_keystrokes("escape");
        update(window, &view, |workspace, _, _| {
            assert!(workspace.rename.is_none());
            assert!(
                !workspace.drawer_overlay_open,
                "the Drawer the rename revealed hides again"
            );
        });
    });
}

#[test]
fn move_screen_down_places_it_after_its_neighbor() {
    with_view(|window, view| {
        let mut model = WindowWorkspace::new(Uuid::new_v4());
        let keys = model
            .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B"), ("c", "C")], None)
            .unwrap();
        let mut receiver = setup(window, &view, model);
        let sent: Vec<_> = update(window, &view, |workspace, window, cx| {
            workspace.run_command(
                splitty_client_core::keymap::Command::MoveScreenDown,
                window,
                cx,
            );
            std::iter::from_fn(|| receiver.try_recv().ok()).collect()
        });
        assert!(sent.iter().any(|command| matches!(
            command.kind(),
            Command::MoveScreen { screen, before }
                if *screen == keys[0] && before.as_ref() == Some(&keys[2])
        )));
        // The first Screen cannot move up.
        let sent: Vec<_> = update(window, &view, |workspace, window, cx| {
            workspace.run_command(
                splitty_client_core::keymap::Command::MoveScreenUp,
                window,
                cx,
            );
            std::iter::from_fn(|| receiver.try_recv().ok()).collect()
        });
        assert!(!sent
            .iter()
            .any(|command| matches!(command.kind(), Command::MoveScreen { .. })));
    });
}

#[test]
fn a_tab_moves_one_place_and_stops_at_the_ends() {
    with_view(|window, view| {
        let model = model(
            tabs(&["p0", "p1", "p2"], "p1"),
            no_floats(),
            &["p0", "p1", "p2"],
            "p1",
        );
        let mut receiver = setup(window, &view, model);
        let right = modded(window, "shift-pagedown");
        let left = modded(window, "shift-pageup");
        let placed = |commands: Vec<WorkerCommand>| -> Vec<(String, Option<String>)> {
            commands
                .iter()
                .filter_map(|command| match command.kind() {
                    Command::PlaceTab {
                        drag,
                        target,
                        before,
                    } => {
                        assert_eq!(target, "slot");
                        Some((drag.pane_id().to_owned(), before.clone()))
                    }
                    _ => None,
                })
                .collect()
        };
        let sent = press(window, &mut receiver, &right);
        assert!(inputs(&sent).is_empty());
        assert_eq!(placed(sent), [("p1".to_owned(), None)]);
        assert_eq!(
            placed(press(window, &mut receiver, &left)),
            [("p1".to_owned(), Some("p0".to_owned()))]
        );
        for (pane, key) in [("p2", &right), ("p0", &left)] {
            update(window, &view, |workspace, window, cx| {
                let mut model = workspace.model.clone().unwrap();
                let source = model.lifecycle_token().source_id();
                model.source_mut(source).unwrap().focus_pane(pane);
                workspace.reconcile(model, window, cx);
            });
            while receiver.try_recv().is_ok() {}
            let sent = press(window, &mut receiver, key);
            assert!(placed(sent).is_empty(), "{pane} is at the end");
        }
    });
}

#[test]
fn float_floats_a_docked_tab_and_docks_a_floating_group() {
    with_view(|window, view| {
        let docked = model(side_by_side(), no_floats(), &["p0", "p1"], "p0");
        let mut receiver = setup(window, &view, docked);
        let float = modded(window, "shift-f");
        let sent = press(window, &mut receiver, &float);
        assert!(inputs(&sent).is_empty());
        assert!(sent.iter().any(|command| matches!(
            command.kind(),
            Command::FloatPane { source, .. } if source.pane_id == "p0"
        )));

        let floating = model(
            tabs(&["p0"], "p0"),
            serde_json::json!([{"id":"float","root":
                {"type":"slot","id":"float-slot","paneIds":["p1"],"activePaneId":"p1"},
                "bounds":{"x":0.25,"y":0.25,"w":0.5,"h":0.5}}]),
            &["p0", "p1"],
            "p1",
        );
        update(window, &view, |workspace, _, _| {
            workspace.pending_float = None
        });
        let mut receiver = setup(window, &view, floating);
        let sent = press(window, &mut receiver, &float);
        assert!(sent.iter().any(|command| matches!(
            command.kind(),
            Command::DockFloat { target, .. } if target.floating_id() == "float"
        )));
    });
}

#[test]
fn clear_buffer_clears_the_focused_terminals_history() {
    with_view(|window, view| {
        let model = model(tabs(&["p0"], "p0"), no_floats(), &["p0"], "p0");
        let mut receiver = setup(window, &view, model);
        let terminal = update(window, &view, |workspace, _, _| {
            workspace.focused_terminal().expect("the focused terminal")
        });
        let mut output = Vec::new();
        for line in 0..200 {
            output.extend_from_slice(format!("line {line}\r\n").as_bytes());
        }
        let history = |window: &mut VisualTestContext| {
            window.update(|window, cx| {
                window.draw(cx).clear(cx);
                let scrollback = terminal.read(cx).ui_test_scrollback().expect("scrollback");
                scrollback.total.saturating_sub(scrollback.rows)
            })
        };
        window
            .update(|_, cx| terminal.update(cx, |terminal, cx| terminal.feed(&output, false, cx)));
        assert!(history(window) > 100);
        let clear = modded(window, "k");
        let sent = press(window, &mut receiver, &clear);
        assert!(inputs(&sent).is_empty(), "the key reaches no program");
        assert_eq!(history(window), 0);
    });
}

#[test]
fn history_keys_scroll_the_normal_screen_and_reach_the_program_on_the_alternate_one() {
    with_view(|window, view| {
        let model = model(tabs(&["p0"], "p0"), no_floats(), &["p0"], "p0");
        let mut receiver = setup(window, &view, model);
        let terminal = update(window, &view, |workspace, _, _| {
            workspace.focused_terminal().expect("the focused terminal")
        });
        let mut output = Vec::new();
        for line in 0..200 {
            output.extend_from_slice(format!("line {line}\r\n").as_bytes());
        }
        window
            .update(|_, cx| terminal.update(cx, |terminal, cx| terminal.feed(&output, false, cx)));
        let behind = |window: &mut VisualTestContext| {
            window.update(|window, cx| {
                window.draw(cx).clear(cx);
                terminal
                    .read(cx)
                    .ui_test_scrollback()
                    .map(|scrollback| scrollback.behind())
            })
        };
        assert_eq!(behind(window), Some(0));
        let sent = press(window, &mut receiver, "shift-pageup");
        assert!(inputs(&sent).is_empty(), "the normal screen scrolls");
        assert!(behind(window).is_some_and(|rows| rows > 0));
        assert!(press(window, &mut receiver, "shift-home").is_empty());
        assert!(press(window, &mut receiver, "shift-end").is_empty());
        assert_eq!(behind(window), Some(0));

        window.update(|_, cx| {
            terminal.update(cx, |terminal, cx| terminal.feed(b"\x1b[?1049h", false, cx))
        });
        let sent = press(window, &mut receiver, "shift-pageup");
        assert_eq!(
            inputs(&sent),
            [b"\x1b[5;2~".to_vec()],
            "the program receives Shift+PageUp"
        );
    });
}

#[test]
fn full_screen_toggles_the_window() {
    with_view(|window, view| {
        let model = model(tabs(&["p0"], "p0"), no_floats(), &["p0"], "p0");
        let mut receiver = setup(window, &view, model);
        let key = if is_mac(window) { "ctrl-cmd-f" } else { "f11" };
        assert!(!window.update(|window, _| window.is_fullscreen()));
        let sent = press(window, &mut receiver, key);
        assert!(inputs(&sent).is_empty());
        assert!(window.update(|window, _| window.is_fullscreen()));
        press(window, &mut receiver, key);
        assert!(!window.update(|window, _| window.is_fullscreen()));
    });
}

#[test]
fn focus_drawer_moves_the_focus_to_the_shown_row_and_back() {
    with_view(|window, view| {
        let mut model = WindowWorkspace::new(Uuid::new_v4());
        let keys = model
            .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B")], None)
            .unwrap();
        let mut receiver = setup(window, &view, model);
        let focus = modded(window, "e");
        press(window, &mut receiver, &focus);
        update(window, &view, |workspace, window, cx| {
            assert!(workspace.drawer_overlay_open, "the hidden Drawer shows");
            assert!(workspace.drawer_focus.is_focused(window));
            let _ = cx;
        });
        // Down moves to the next row; Enter shows its Screen and returns.
        press(window, &mut receiver, "down");
        assert_eq!(
            window.update(|_, cx| view.read(cx).drawer_focus_row.clone()),
            Some(keys[1].clone())
        );
        let sent = press(window, &mut receiver, "enter");
        // A focused button activates when Enter is released.
        window.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("enter").unwrap(),
        });
        let sent: Vec<_> = sent
            .into_iter()
            .chain(std::iter::from_fn(|| receiver.try_recv().ok()))
            .collect();
        assert!(sent.iter().any(|command| matches!(
            command.kind(),
            Command::SwitchScreen(key) if *key == keys[1]
        )));
        update(window, &view, |workspace, window, _| {
            assert!(!workspace.drawer_focus.is_focused(window));
            assert!(!workspace.drawer_overlay_open);
        });
        // The same key returns the focus.
        press(window, &mut receiver, &focus);
        assert!(update(window, &view, |workspace, window, _| workspace
            .drawer_focus
            .is_focused(window)));
        press(window, &mut receiver, &focus);
        update(window, &view, |workspace, window, _| {
            assert!(!workspace.drawer_focus.is_focused(window));
            assert!(!workspace.drawer_overlay_open);
        });
    });
}

#[test]
fn new_tab_with_profile_opens_the_focused_groups_picker() {
    with_view(|window, view| {
        let model = model(side_by_side(), no_floats(), &["p0", "p1"], "p1");
        let mut receiver = setup(window, &view, model);
        let key = modded(window, "shift-t");
        let sent = press(window, &mut receiver, &key);
        assert!(inputs(&sent).is_empty());
        assert!(update(window, &view, |workspace, _, _| workspace
            .profile_picker_open_for("right")));
        let sent = press(window, &mut receiver, "escape");
        assert!(inputs(&sent).is_empty(), "Escape closes the picker");
        assert!(update(window, &view, |workspace, _, _| workspace
            .profile_picker
            .is_none()));
        assert_eq!(
            inputs(&press(window, &mut receiver, "x")),
            [b"x".to_vec()],
            "the keyboard returns to the terminal"
        );
    });
}

#[test]
fn close_other_tabs_assesses_the_other_tabs_of_the_group() {
    with_view(|window, view| {
        let model = model(
            tabs(&["p0", "p1", "p2"], "p1"),
            no_floats(),
            &["p0", "p1", "p2"],
            "p1",
        );
        let mut receiver = setup(window, &view, model);
        let sent: Vec<_> = update(window, &view, |workspace, window, cx| {
            workspace.run_command(
                splitty_client_core::keymap::Command::CloseOtherTabs,
                window,
                cx,
            );
            std::iter::from_fn(|| receiver.try_recv().ok()).collect()
        });
        assert!(sent
            .iter()
            .any(|command| matches!(command.kind(), Command::AssessClose { .. })));
    });
}

#[test]
fn move_tab_to_screen_opens_the_chooser_with_the_keyboard_on_it() {
    with_view(|window, view| {
        let model = model(tabs(&["p0", "p1"], "p1"), no_floats(), &["p0", "p1"], "p1");
        let mut receiver = setup(window, &view, model);
        let key = modded(window, "shift-m");
        let sent = press(window, &mut receiver, &key);
        assert!(inputs(&sent).is_empty());
        update(window, &view, |workspace, _, _| {
            let menu = workspace.tab_context_menu.as_ref().expect("the tab menu");
            assert_eq!(menu.source.pane_id, "p1");
            assert!(menu.keyboard_in_transfer(), "the chooser has the keyboard");
        });
        press(window, &mut receiver, "escape");
        press(window, &mut receiver, "escape");
        assert!(update(window, &view, |workspace, _, _| workspace
            .tab_context_menu
            .is_none()));
    });
}

#[test]
fn open_selection_in_viewer_does_nothing_without_a_selection() {
    with_view(|window, view| {
        let model = model(tabs(&["p0"], "p0"), no_floats(), &["p0"], "p0");
        let mut receiver = setup(window, &view, model);
        let sent: Vec<_> = update(window, &view, |workspace, window, cx| {
            workspace.run_command(
                splitty_client_core::keymap::Command::OpenSelectionInViewer,
                window,
                cx,
            );
            std::iter::from_fn(|| receiver.try_recv().ok()).collect()
        });
        assert!(!sent
            .iter()
            .any(|command| matches!(command.kind(), Command::OpenTextViewer { .. })));
    });
}

#[test]
fn quit_closes_the_window_after_its_assessment() {
    with_view(|window, view| {
        let model = model(tabs(&["p0"], "p0"), no_floats(), &["p0"], "p0");
        let mut receiver = setup(window, &view, model);
        // The fixture's worker has ended, which would end the app at once.
        let sent: Vec<_> = update(window, &view, |workspace, window, cx| {
            workspace.worker_finished = std::sync::Arc::default();
            workspace.cleanup_complete = std::sync::Arc::default();
            workspace.run_command(splitty_client_core::keymap::Command::Quit, window, cx);
            std::iter::from_fn(|| receiver.try_recv().ok()).collect()
        });
        assert!(sent
            .iter()
            .any(|command| matches!(command.kind(), Command::AssessClose { .. })));
    });
}
