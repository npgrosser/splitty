use crate::terminal::Terminal;
use crate::workspace::{
    chrome::view_tests::with_view, Command, Destination, WorkerCommand, Workspace,
};
use gpui_kit::{
    ClipboardItem, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, VisualTestContext,
};
use splitty_client_core::workspace::{Axis, CloseTarget, WindowWorkspace};
use tokio::sync::mpsc;
use uuid::Uuid;

fn model(owner: Uuid, session: &str) -> WindowWorkspace {
    let mut model = WindowWorkspace::new(owner);
    model.ui_test_local_screen_fixture("Work").unwrap();
    let source = model.lifecycle_token();
    model.source_mut(source.source_id()).unwrap().apply_snapshot(owner,
        &serde_json::json!({"type":"mux.screen_snapshot","screen":{
            "id":"work","revision":2,
            "root":{"type":"split","id":"split","axis":"horizontal","sizes":[0.5,0.5],"children":[
                {"type":"slot","id":"left","paneIds":["p0"],"activePaneId":"p0"},
                {"type":"slot","id":"right","paneIds":["p1"],"activePaneId":"p1"}
            ]},
            "panes":{"p0":{"id":"p0","sessionId":"s0"},"p1":{"id":"p1","sessionId":session}},
            "sessions":{"s0":{"id":"s0","status":"running"},(session):{"id":session,"status":"running","busy":true}}
        }})).unwrap();
    model
        .source_mut(source.source_id())
        .unwrap()
        .focus_pane("p0");
    model
}

fn setup(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
) -> mpsc::Receiver<WorkerCommand> {
    let (sender, receiver) = mpsc::channel(128);
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            workspace.commands = sender;
            workspace.destination = Destination::Screens;
            workspace.drawer_visible = false;
            workspace.drawer_overlay_open = false;
            workspace.reconcile(model(Uuid::new_v4(), "s1"), window, cx);
        });
        window.draw(cx).clear(cx);
        window.draw(cx).clear(cx);
    });
    receiver
}

fn terminal(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    pane: &str,
) -> Entity<Terminal> {
    window.update(|_, cx| {
        let workspace = view.read(cx);
        workspace
            .panes
            .get(&workspace.selected_model().unwrap().pane_key(pane))
            .unwrap()
            .terminal
            .clone()
    })
}

fn right_click(window: &mut VisualTestContext, terminal: &Entity<Terminal>, modifiers: Modifiers) {
    let position = window.update(|_, cx| {
        terminal.update(cx, |terminal, cx| {
            terminal
                .presentation(cx)
                .expect("measured terminal")
                .bounds()
                .center()
        })
    });
    window.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position,
        click_count: 1,
        modifiers,
        first_mouse: false,
    });
    // A Shift override remains local even when Shift is released first.
    window.simulate_event(MouseUpEvent {
        button: MouseButton::Right,
        position,
        click_count: 1,
        modifiers: Modifiers::default(),
    });
    window.update(|window, cx| window.draw(cx).clear(cx));
}

fn activate_item(window: &mut VisualTestContext, view: &Entity<Workspace>, label: &str) {
    // Navigate the rendered terminal popup. Copy is disabled in these unselected fixtures;
    // disabled items are skipped by keyboard navigation and no callback is invoked directly.
    let steps = match label {
        "Paste" => 1,
        "Find" => 2,
        "Clear Buffer" => 3,
        "Split Right" => 4,
        "Split Below" | "Split Down" => 5,
        "Show Scratchpad" | "Hide Scratchpad" => 6,
        "Move to Screen" => 7,
        "Show Drawer" => 8,
        "Close Pane" => window.update(|_, cx| {
            let workspace = view.read(cx);
            if !workspace.drawer_visible && !workspace.drawer_overlay_open {
                9
            } else {
                8
            }
        }),
        _ => panic!("unsupported fixture menu item"),
    };
    for _ in 0..steps {
        window.simulate_keystrokes("down");
    }
    window.simulate_keystrokes("enter");
}

fn inputs(receiver: &mut mpsc::Receiver<WorkerCommand>) -> Vec<(String, String, Vec<u8>)> {
    std::iter::from_fn(|| receiver.try_recv().ok())
        .filter_map(|command| match command.kind() {
            Command::Input { route, bytes } => Some((
                route.pane_id().to_owned(),
                route.session_id().to_owned(),
                bytes.clone(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn body_menu_uses_clicked_split_for_paste_find_split_and_assessed_close() {
    with_view(|window, view| {
        let mut receiver = setup(window, &view);
        let clicked = terminal(window, &view, "p1");
        window.update(|_, cx| {
            clicked.update(cx, |terminal, cx| terminal.feed(b"\x1b[?2004h", false, cx));
            cx.write_to_clipboard(ClipboardItem::new_string("a\nb".into()));
        });
        inputs(&mut receiver);
        right_click(window, &clicked, Modifiers::default());
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert_eq!(
                workspace.selected_model().unwrap().focused_pane(),
                Some("p1")
            );
            assert!(workspace
                .terminal_menu
                .as_ref()
                .unwrap()
                .target
                .group
                .is_some());
        });
        activate_item(window, &view, "Paste");
        assert_eq!(
            inputs(&mut receiver),
            vec![("p1".into(), "s1".into(), b"\x1b[200~a\rb\x1b[201~".to_vec())]
        );
        window.simulate_input("x");
        assert_eq!(
            inputs(&mut receiver),
            vec![("p1".into(), "s1".into(), b"x".to_vec())]
        );

        right_click(window, &clicked, Modifiers::default());
        activate_item(window, &view, "Split Right");
        let sent: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();
        assert_eq!(sent.iter().filter(|command| matches!(command.kind(), Command::Split { pane, axis: Axis::Horizontal, .. } if pane == "p1")).count(), 1);

        right_click(window, &clicked, Modifiers::default());
        activate_item(window, &view, "Find");
        window.update(|window, cx| assert!(clicked.read(cx).search_input_focused(window, cx)));
        window.simulate_keystrokes("escape");
        assert!(inputs(&mut receiver).is_empty());

        right_click(window, &clicked, Modifiers::default());
        activate_item(window, &view, "Show Drawer");
        window.update(|_, cx| {
            assert!(view.read(cx).drawer_overlay_open);
            assert!(!view.read(cx).drawer_visible);
        });
        right_click(window, &clicked, Modifiers::default());
        activate_item(window, &view, "Close Pane");
        window.update(|_, cx| {
            let close = view
                .read(cx)
                .pending_close
                .as_ref()
                .expect("assessed close");
            assert_eq!(
                close.plan.target,
                CloseTarget::Pane {
                    screen: "work".into(),
                    pane: "p1".into()
                }
            );
            assert!(close.plan.needs_confirmation());
            assert!(close.prompting);
        });
        assert!(!std::iter::from_fn(|| receiver.try_recv().ok())
            .any(|command| matches!(command.kind(), Command::RequestClose { .. })));
    });
}

#[test]
fn application_right_button_wins_and_shift_menu_keys_stay_local() {
    with_view(|window, view| {
        let mut receiver = setup(window, &view);
        let clicked = terminal(window, &view, "p1");
        window.update(|_, cx| {
            clicked.update(cx, |terminal, cx| {
                terminal.feed(b"\x1b[?1000h\x1b[?1006h", false, cx)
            });
            cx.write_to_clipboard(ClipboardItem::new_string("paste".into()));
        });
        inputs(&mut receiver);
        right_click(window, &clicked, Modifiers::default());
        window.update(|_, cx| assert!(view.read(cx).terminal_menu.is_none()));
        let reports = inputs(&mut receiver);
        assert_eq!(reports.len(), 2);
        assert!(reports.iter().all(|(pane, session, bytes)| pane == "p1"
            && session == "s1"
            && bytes.starts_with(b"\x1b[<2;")));
        assert!(reports[0].2.ends_with(b"M"));
        assert!(reports[1].2.ends_with(b"m"));

        right_click(
            window,
            &clicked,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        window.update(|_, cx| assert!(view.read(cx).terminal_menu.is_some()));
        assert!(inputs(&mut receiver).is_empty());
        window.simulate_keystrokes("down escape");
        window.update(|_, cx| assert!(view.read(cx).terminal_menu.is_none()));
        assert!(inputs(&mut receiver).is_empty());
        window.simulate_input("z");
        assert_eq!(
            inputs(&mut receiver),
            vec![("p1".into(), "s1".into(), b"z".to_vec())]
        );

        right_click(
            window,
            &clicked,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        window.simulate_keystrokes("down enter");
        assert_eq!(
            inputs(&mut receiver),
            vec![("p1".into(), "s1".into(), b"paste".to_vec())]
        );
    });
}

#[test]
fn obsolete_menu_callbacks_never_retarget_input_layout_or_close() {
    for change in 0..5 {
        for action in ["Paste", "Split Below", "Close Pane"] {
            with_view(|window, view| {
                let mut receiver = setup(window, &view);
                let clicked = terminal(window, &view, "p1");
                window.update(|_, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string("secret".into()))
                });
                right_click(window, &clicked, Modifiers::default());
                inputs(&mut receiver);
                window.update(|_, cx| {
                    view.update(cx, |workspace, cx| {
                        let owner = workspace
                            .model
                            .as_ref()
                            .unwrap()
                            .lifecycle_token()
                            .connection_id();
                        // Keep the already rendered popup to exercise its captured callback
                        // racing a model change, before ordinary reconcile dismisses it.
                        match change {
                            0 => workspace.model = Some(model(Uuid::new_v4(), "s1")),
                            1 => workspace.model = Some(model(owner, "replacement")),
                            2 => workspace.destination = Destination::Hosts,
                            3 => clicked.update(cx, |terminal, cx| terminal.reset(1, cx)),
                            _ => {
                                let mut removed = WindowWorkspace::new(owner);
                                removed.ui_test_local_screen_fixture("Work").unwrap();
                                workspace.model = Some(removed);
                            }
                        }
                    });
                });
                activate_item(window, &view, action);
                assert!(
                    std::iter::from_fn(|| receiver.try_recv().ok()).all(|command| !matches!(
                        command.kind(),
                        Command::Input { .. }
                            | Command::Split { .. }
                            | Command::RequestClose { .. }
                            | Command::Focus { .. }
                    ))
                );
                window.update(|_, cx| assert!(view.read(cx).pending_close.is_none()));
            });
        }
    }
}

#[test]
fn offline_retained_menu_copies_and_finds_without_live_command_authority() {
    with_view(|window, view| {
        let mut receiver = setup(window, &view);
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                let mut remote = WindowWorkspace::new(Uuid::new_v4());
                let (screen, _) = remote.ui_test_remote_terminal_fixture("me@192.0.2.1").unwrap();
                let source = remote.source(screen.source_id()).unwrap().source_token();
                remote.source_mut(screen.source_id()).unwrap().apply_snapshot(source.connection_id(),
                    &serde_json::json!({"type":"mux.screen_snapshot","screen":{
                    "id":"work","revision":2,
                    "root":{"type":"split","id":"split","axis":"horizontal","sizes":[0.5,0.5],"children":[
                        {"type":"slot","id":"left","paneIds":["p0"],"activePaneId":"p0"},
                        {"type":"slot","id":"right","paneIds":["p1"],"activePaneId":"p1"}]},
                    "panes":{"p0":{"id":"p0","sessionId":"s0"},"p1":{"id":"p1","sessionId":"s1"}},
                    "sessions":{"s0":{"id":"s0","status":"running"},"s1":{"id":"s1","status":"running"}}}})).unwrap();
                remote.source_mut(screen.source_id()).unwrap().focus_pane("p0");
                remote.disconnect(screen.source_id()).unwrap();
                workspace.reconcile(remote, window, cx);
                // This test concerns retained menu authority, not outgoing motion.
                workspace.screen_motion.clear();
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        let clicked = terminal(window, &view, "p1");
        window.update(|_, cx| {
            clicked.update(cx, |terminal, cx| terminal.feed(b"retained", false, cx))
        });
        right_click(window, &clicked, Modifiers::default());
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert!(workspace
                .terminal_menu
                .as_ref()
                .unwrap()
                .target
                .authority
                .is_none());
            assert_eq!(
                workspace.selected_model().unwrap().focused_pane(),
                Some("p1")
            );
        });
        // Copy has no selection and every live action is disabled: Find is first.
        window.simulate_keystrokes("down enter");
        window.update(|window, cx| assert!(clicked.read(cx).search_input_focused(window, cx)));
        window.simulate_keystrokes("escape");
        let sent: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();
        assert!(sent.iter().any(
            |command| matches!(command.kind(), Command::Focus {route,..} if route.pane_id()=="p1")
        ));
        assert!(sent.iter().all(|command| !matches!(
            command.kind(),
            Command::Input { .. }
                | Command::Split { .. }
                | Command::RequestClose { .. }
                | Command::ConnectHost { .. }
                | Command::RetryHost { .. }
        )));
        let position = window.update(|_, cx| {
            clicked.update(cx, |terminal, cx| {
                terminal.presentation(cx).unwrap().bounds().origin
                    + gpui_kit::point(gpui_kit::px(10.), gpui_kit::px(10.))
            })
        });
        window.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        window.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position,
            click_count: 2,
            modifiers: Modifiers::default(),
        });
        right_click(window, &clicked, Modifiers::default());
        window.simulate_keystrokes("down enter");
        window.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("retained")
            )
        });
        assert!(inputs(&mut receiver).is_empty());
    });
}

#[test]
fn rejected_focus_admission_does_not_open_menu_or_strand_keyboard_focus() {
    with_view(|window, view| {
        let receiver = setup(window, &view);
        drop(receiver);
        let clicked = terminal(window, &view, "p1");
        let original = terminal(window, &view, "p0");
        let original_focus = window.update(|window, cx| {
            original.update(cx, |terminal, cx| terminal.focus_pty(window, cx));
            window.focused(cx)
        });
        right_click(window, &clicked, Modifiers::default());
        window.update(|window, cx| {
            let workspace = view.read(cx);
            assert!(workspace.terminal_menu.is_none());
            assert_eq!(
                workspace.selected_model().unwrap().focused_pane(),
                Some("p0")
            );
            assert_eq!(window.focused(cx), original_focus);
        });
    });
}

#[test]
fn body_menu_supports_scratchpad_cwd_and_move_to_screen() {
    with_view(|window, view| {
        let mut receiver = setup(window, &view);
        let clicked = terminal(window, &view, "p1");
        // Toggle Show Scratchpad via context menu.
        right_click(window, &clicked, Modifiers::default());
        activate_item(window, &view, "Show Scratchpad");
        window.update(|window, cx| {
            let workspace = view.read(cx);
            assert_eq!(workspace.scratchpad_state_for_pane("p1"), Some(true));
            assert!(workspace.focused_pane_scratchpad_focused(window, cx));
        });
        // Type into the scratchpad and hide it via context menu; verify SetScratchpad is sent.
        window.simulate_input("note for s1");
        right_click(window, &clicked, Modifiers::default());
        activate_item(window, &view, "Hide Scratchpad");
        window.update(|_, cx| {
            assert_eq!(view.read(cx).scratchpad_state_for_pane("p1"), Some(false));
        });
        let sent: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();
        assert!(sent.iter().any(|command| matches!(
            command.kind(),
            Command::SetScratchpad { screen, session, text, .. }
                if screen == "work" && session == "s1" && text == "note for s1"
        )));

        // Move to Screen -> New Screen via keyboard submenu navigation.
        right_click(window, &clicked, Modifiers::default());
        for _ in 0..7 {
            window.simulate_keystrokes("down");
        }
        window.simulate_keystrokes("right enter");
        let sent: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();
        assert!(sent.iter().any(|command| matches!(
            command.kind(),
            Command::TransferPane { source, .. } if source.pane_id == "p1"
        )));

        // Hovering between "Open Folder in" and "Move to Screen" keeps only one submenu open.
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                let source = workspace.model.as_ref().unwrap().lifecycle_token();
                workspace
                    .model
                    .as_mut()
                    .unwrap()
                    .source_mut(source.source_id())
                    .unwrap()
                    .apply_snapshot(
                        source.connection_id(),
                        &serde_json::json!({"type":"mux.screen_snapshot","screen":{
                            "id":"work","revision":3,
                            "root":{"type":"split","id":"split","axis":"horizontal","sizes":[0.5,0.5],"children":[
                                {"type":"slot","id":"left","paneIds":["p0"],"activePaneId":"p0"},
                                {"type":"slot","id":"right","paneIds":["p1"],"activePaneId":"p1"}
                            ]},
                            "panes":{"p0":{"id":"p0","sessionId":"s0"},"p1":{"id":"p1","sessionId":"s1"}},
                            "sessions":{
                                "s0":{"id":"s0","status":"running"},
                                "s1":{"id":"s1","status":"running","cwd":"/home/demo/project"}
                            }
                        }}),
                    )
                    .unwrap();
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        right_click(window, &clicked, Modifiers::default());
        let open_editor_center = window
            .debug_bounds("terminal-context-open-folder")
            .expect("open-folder row")
            .center();
        window.simulate_mouse_move(open_editor_center, None, Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(window
            .debug_bounds("terminal-context-editor-menu")
            .is_some());
        assert!(window.debug_bounds("terminal-context-move-menu").is_none());

        let move_center = window
            .debug_bounds("terminal-context-move")
            .expect("move row")
            .center();
        window.simulate_mouse_move(move_center, None, Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(window
            .debug_bounds("terminal-context-editor-menu")
            .is_none());
        assert!(window.debug_bounds("terminal-context-move-menu").is_some());

        window.simulate_mouse_move(open_editor_center, None, Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(window
            .debug_bounds("terminal-context-editor-menu")
            .is_some());
        assert!(window.debug_bounds("terminal-context-move-menu").is_none());
    });
}
