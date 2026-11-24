//! Several windows of the application on one worker: each view reads its part
//! of the one stream, a Screen changes windows with its terminal views, and a
//! window closes by handing its Screens over.
use super::{Commands, Update, WorkerLink, Workspace};
use crate::windows::Windows;
use gpui_kit::{App, AppContext, Entity, EntityId, TestAppContext, WindowOptions};
use splitty_client_core::keymap::Command as KeyCommand;
use splitty_client_core::native_workspace::{
    Command as WorkerCommand, CommandKind, WindowPresenceCell, WindowStart,
};
use splitty_client_core::workspace::{ScreenKey, WindowId, WindowWorkspace};
use std::sync::{atomic::AtomicBool, Arc};
use tokio::sync::mpsc;
use uuid::Uuid;

const SECOND: WindowId = WindowId::new(1);

fn open_test_window(id: WindowId, link: WorkerLink, cx: &mut App) -> anyhow::Result<()> {
    cx.open_window(WindowOptions::default(), |window, cx| {
        let view = cx.new(|cx| {
            Workspace::new_in_app(false, Arc::new(AtomicBool::new(false)), link, window, cx)
        });
        Windows::register(id, window.window_handle(), &view, cx);
        cx.new(|cx| gpui_kit::base::Root::new(view, window, cx))
    })?;
    Ok(())
}

/// An application with its first window, and what its windows send.
fn application() -> (TestAppContext, mpsc::Receiver<WorkerCommand>) {
    let app = TestAppContext::single();
    app.update(gpui_kit::init);
    let (commands, sent) = mpsc::channel(32);
    let (_, stream) = mpsc::channel(1);
    let link = WorkerLink {
        window: WindowId::FIRST,
        commands: Commands::new(commands, WindowId::FIRST),
        presence: WindowPresenceCell::default(),
        stopping: Arc::default(),
        cleanup_complete: Arc::default(),
        worker_finished: Arc::default(),
    };
    let first = link.for_window(WindowId::FIRST, WindowPresenceCell::default());
    app.update(|cx| {
        Windows::start(link, stream, open_test_window, cx);
        open_test_window(WindowId::FIRST, first, cx).unwrap();
    });
    (app, sent)
}

fn view(app: &mut TestAppContext, id: WindowId) -> Option<Entity<Workspace>> {
    app.update(|cx| Windows::ui_test_window(id, cx).map(|(_, view)| view))
}

/// What the worker sends window `id` when the model changed.
fn model_for(app: &mut TestAppContext, model: &WindowWorkspace, id: WindowId) {
    let update = Update::Workspace(Box::new(model.projection(id).expect("an open window")));
    let update = if id == WindowId::FIRST {
        update
    } else {
        Update::ForWindow {
            window: id,
            update: Box::new(update),
        }
    };
    app.update(|cx| Windows::ui_test_deliver(update, cx));
    app.run_until_parked();
}

fn terminal(
    app: &mut TestAppContext,
    id: WindowId,
    model: &WindowWorkspace,
    pane: &str,
) -> Option<EntityId> {
    let key = model.local_source().pane_key(pane);
    let view = view(app, id)?;
    app.update(|cx| {
        view.read(cx)
            .panes
            .get(&key)
            .map(|pane| pane.terminal.entity_id())
    })
}

fn shown(app: &mut TestAppContext, id: WindowId) -> Option<ScreenKey> {
    let view = view(app, id)?;
    app.update(|cx| view.read(cx).current_screen_key())
}

fn run(app: &mut TestAppContext, id: WindowId, command: KeyCommand) {
    let (handle, view) = app
        .update(|cx| Windows::ui_test_window(id, cx))
        .expect("an open window");
    app.update(|cx| {
        handle
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| view.run_command(command, window, cx));
            })
            .unwrap();
    });
    app.run_until_parked();
}

fn sent(commands: &mut mpsc::Receiver<WorkerCommand>) -> Vec<WorkerCommand> {
    std::iter::from_fn(|| commands.try_recv().ok()).collect()
}

#[test]
fn a_screen_changes_windows_with_its_terminal_view() {
    let (mut app, mut commands) = application();
    let mut model = WindowWorkspace::new(Uuid::new_v4());
    let keys = model
        .ui_test_local_screens_fixture(&["work", "notes"])
        .unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    assert_eq!(shown(&mut app, WindowId::FIRST), Some(keys[0].clone()));
    let work = terminal(&mut app, WindowId::FIRST, &model, "p-work").expect("its terminal view");
    let notes = terminal(&mut app, WindowId::FIRST, &model, "p-notes").expect("its terminal view");
    assert_ne!(work, notes);
    sent(&mut commands);

    // The shown Screen goes to a new window: the window opens, and the
    // worker hears from the window that asked what the new one starts with.
    run(&mut app, WindowId::FIRST, KeyCommand::MoveScreenToNewWindow);
    assert_eq!(app.update(|cx| Windows::count(cx)), 2);
    let opened = sent(&mut commands);
    assert!(opened.iter().any(|command| {
        command.window() == WindowId::FIRST
            && matches!(command.kind(), CommandKind::OpenWindow { window, start: WindowStart::Screen(screen), .. }
                if *window == SECOND && screen == &keys[0])
    }));

    // The worker's models: the window that gave the Screen reads first.
    model.ui_test_open_window(SECOND).unwrap();
    model.ui_test_move_screen(&keys[0], SECOND).unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    assert_eq!(terminal(&mut app, WindowId::FIRST, &model, "p-work"), None);
    model_for(&mut app, &model, SECOND);
    assert_eq!(
        terminal(&mut app, SECOND, &model, "p-work"),
        Some(work),
        "the same terminal view, with what it shows"
    );
    assert_eq!(
        terminal(&mut app, WindowId::FIRST, &model, "p-notes"),
        Some(notes)
    );
    assert_eq!(shown(&mut app, SECOND), Some(keys[0].clone()));
    assert_eq!(shown(&mut app, WindowId::FIRST), Some(keys[1].clone()));

    // Each window's commands name it, and act on what it shows.
    sent(&mut commands);
    run(&mut app, SECOND, KeyCommand::MoveScreenToNextWindow);
    let moved = sent(&mut commands);
    assert!(moved.iter().any(|command| {
        command.window() == SECOND
            && matches!(command.kind(), CommandKind::MoveScreenToWindow { screen, target }
                if screen == &keys[0] && *target == WindowId::FIRST)
    }));

    // The second window asked for a Screen the first one holds: the first
    // window shows it, by its own command.
    app.update(|cx| {
        Windows::ui_test_deliver(
            Update::ForWindow {
                window: SECOND,
                update: Box::new(Update::ScreenElsewhere {
                    screen: keys[1].clone(),
                    window: WindowId::FIRST,
                }),
            },
            cx,
        )
    });
    app.run_until_parked();
    let shown = sent(&mut commands);
    assert!(shown.iter().any(|command| {
        command.window() == WindowId::FIRST
            && matches!(command.kind(), CommandKind::SwitchScreen(screen) if screen == &keys[1])
    }));
    assert!(!shown.iter().any(|command| command.window() == SECOND));
}

#[test]
fn a_window_asks_before_its_screens_close_and_can_hand_them_over_instead() {
    let (mut app, mut commands) = application();
    let mut model = WindowWorkspace::new(Uuid::new_v4());
    let keys = model
        .ui_test_local_screens_fixture(&["work", "notes"])
        .unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    run(&mut app, WindowId::FIRST, KeyCommand::NewWindow);
    assert!(sent(&mut commands).iter().any(|command| matches!(
        command.kind(),
        CommandKind::OpenWindow { window, start: WindowStart::NewScreen, .. } if *window == SECOND
    )));
    model.ui_test_open_window(SECOND).unwrap();
    model.ui_test_move_screen(&keys[1], SECOND).unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    model_for(&mut app, &model, SECOND);
    let notes = terminal(&mut app, SECOND, &model, "p-notes").expect("its terminal view");

    // One of two windows: its Screen is assessed, and since a program runs
    // in it one question is asked. Nothing closes and nothing moves before
    // the answer.
    let (handle, second) = app
        .update(|cx| Windows::ui_test_window(SECOND, cx))
        .unwrap();
    let mut window = gpui_kit::VisualTestContext::from_window(handle, &app);
    let ask = |app: &mut TestAppContext,
               commands: &mut mpsc::Receiver<WorkerCommand>,
               window: &mut gpui_kit::VisualTestContext| {
        run(app, SECOND, KeyCommand::CloseWindow);
        let asked = sent(commands);
        assert!(!asked.iter().any(|command| matches!(
            command.kind(),
            CommandKind::CloseWindow | CommandKind::RequestClose { .. }
        )));
        let assess = asked
            .into_iter()
            .find(|command| {
                command.window() == SECOND
                    && matches!(command.kind(), CommandKind::AssessClose { .. })
            })
            .expect("the window's Screens are assessed");
        // The service's answer: a program runs in the window's Screen.
        let CommandKind::AssessClose { request_id, .. } = assess.kind() else {
            unreachable!("found as an assessment");
        };
        let (source, request_id) = (assess.source_token().expect("its source"), *request_id);
        let assessment: splitty_client_core::workspace::CloseAssessment =
            serde_json::from_value(serde_json::json!({
                "confirm": true, "running": 1, "starting": 0, "scratchpads": 0,
                "annotations": 0,
                "entries": [{
                    "screenId": "notes", "paneId": "p-notes", "sessionId": "s-notes",
                    "running": true, "starting": false, "scratchpad": false,
                    "annotations": 0,
                }],
            }))
            .expect("assessment");
        window.update(|window, cx| {
            second.update(cx, |workspace, cx| {
                workspace.close_assessed(
                    source,
                    request_id,
                    splitty_client_core::workspace::CloseImpact::Assessed(assessment),
                    window,
                    cx,
                );
            })
        });
        let (title, detail) = window.pending_prompt().expect("one question");
        assert_eq!(title, "Close this window?");
        assert!(detail.contains("Its Screen closes"), "{detail}");
        assert!(detail.contains("Running sessions: 1"), "{detail}");
    };
    ask(&mut app, &mut commands, &mut window);
    window.simulate_prompt_answer("Keep open");
    window.run_until_parked();
    assert!(
        sent(&mut commands).is_empty(),
        "the window and its Screen stay"
    );
    assert_eq!(app.update(|cx| Windows::count(cx)), 2);

    // Closing: the Screen's close is submitted, confirmed. The window goes
    // once it is empty, as after any last Screen.
    ask(&mut app, &mut commands, &mut window);
    window.simulate_prompt_answer("Close");
    window.run_until_parked();
    let closing = sent(&mut commands);
    assert!(closing.iter().any(|command| command.window() == SECOND
        && matches!(command.kind(), CommandKind::RequestClose { .. })));
    assert!(!closing
        .iter()
        .any(|command| matches!(command.kind(), CommandKind::CloseWindow)));
    window.update(|_, cx| second.update(cx, |workspace, _| workspace.pending_close = None));

    // Moving instead: the worker is asked to remove the window, whose
    // Screens it hands to another one, and the window stays until it did.
    ask(&mut app, &mut commands, &mut window);
    window.simulate_prompt_answer("Move to Other Window");
    window.run_until_parked();
    let asked = sent(&mut commands);
    assert!(asked
        .iter()
        .any(|command| command.window() == SECOND
            && matches!(command.kind(), CommandKind::CloseWindow)));
    assert!(!asked
        .iter()
        .any(|command| matches!(command.kind(), CommandKind::RequestClose { .. })));
    assert_eq!(app.update(|cx| Windows::count(cx)), 2);

    // The worker removed it: the window goes, and the heir shows the same
    // terminal view when it reads that it holds the Screen.
    model.ui_test_close_window(SECOND, WindowId::FIRST).unwrap();
    app.update(|cx| {
        Windows::ui_test_deliver(
            Update::ForWindow {
                window: SECOND,
                update: Box::new(Update::WindowClosed { window: SECOND }),
            },
            cx,
        )
    });
    app.run_until_parked();
    assert_eq!(app.update(|cx| Windows::count(cx)), 1);
    assert!(view(&mut app, SECOND).is_none());
    model_for(&mut app, &model, WindowId::FIRST);
    assert_eq!(
        terminal(&mut app, WindowId::FIRST, &model, "p-notes"),
        Some(notes)
    );
    assert_eq!(shown(&mut app, WindowId::FIRST), Some(keys[0].clone()));

    // The last window closes the application: its close is assessed.
    sent(&mut commands);
    run(&mut app, WindowId::FIRST, KeyCommand::CloseWindow);
    let last = sent(&mut commands);
    assert!(!last
        .iter()
        .any(|command| matches!(command.kind(), CommandKind::CloseWindow)));
    assert!(last.iter().any(|command| matches!(
        command.kind(),
        CommandKind::AssessClose { .. } | CommandKind::RequestClose { .. }
    )));
}

#[test]
fn a_window_whose_screens_end_nothing_closes_them_without_a_question() {
    let (mut app, mut commands) = application();
    let mut model = WindowWorkspace::new(Uuid::new_v4());
    let keys = model
        .ui_test_local_screens_fixture(&["work", "notes"])
        .unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    run(&mut app, WindowId::FIRST, KeyCommand::NewWindow);
    model.ui_test_open_window(SECOND).unwrap();
    model.ui_test_move_screen(&keys[1], SECOND).unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    model_for(&mut app, &model, SECOND);
    sent(&mut commands);

    run(&mut app, SECOND, KeyCommand::CloseWindow);
    let assess = sent(&mut commands)
        .into_iter()
        .find(|command| matches!(command.kind(), CommandKind::AssessClose { .. }))
        .expect("the window's Screens are assessed");
    let (handle, second) = app
        .update(|cx| Windows::ui_test_window(SECOND, cx))
        .unwrap();
    let mut window = gpui_kit::VisualTestContext::from_window(handle, &app);
    window.update(|window, cx| {
        second.update(cx, |workspace, cx| {
            assert!(workspace.answer_close_assessment_for_test(&assess, window, cx));
        })
    });
    assert!(!window.has_pending_prompt(), "an idle shell asks nothing");
    let closing = sent(&mut commands);
    assert!(closing.iter().any(|command| command.window() == SECOND
        && matches!(command.kind(), CommandKind::RequestClose { .. })));
    assert!(!closing
        .iter()
        .any(|command| matches!(command.kind(), CommandKind::CloseWindow)));
}

#[test]
fn quit_closes_the_application_from_any_window_and_an_emptied_window_closes() {
    let (mut app, mut commands) = application();
    let mut model = WindowWorkspace::new(Uuid::new_v4());
    let keys = model
        .ui_test_local_screens_fixture(&["work", "notes"])
        .unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    run(&mut app, WindowId::FIRST, KeyCommand::NewWindow);
    model.ui_test_open_window(SECOND).unwrap();
    model.ui_test_move_screen(&keys[1], SECOND).unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    model_for(&mut app, &model, SECOND);
    sent(&mut commands);

    // Quit is the application's close whichever window it comes from.
    run(&mut app, SECOND, KeyCommand::Quit);
    let quit = sent(&mut commands);
    assert!(!quit
        .iter()
        .any(|command| matches!(command.kind(), CommandKind::CloseWindow)));
    assert!(quit.iter().any(|command| {
        command.window() == SECOND
            && matches!(
                command.kind(),
                CommandKind::AssessClose { .. } | CommandKind::RequestClose { .. }
            )
    }));

    // A window whose last Screen left it asks to be removed.
    model
        .ui_test_move_screen(&keys[1], WindowId::FIRST)
        .unwrap();
    model_for(&mut app, &model, SECOND);
    assert!(sent(&mut commands)
        .iter()
        .any(|command| command.window() == SECOND
            && matches!(command.kind(), CommandKind::CloseWindow)));
}

#[test]
fn a_question_of_the_local_service_is_asked_once_for_all_windows() {
    let (mut app, mut commands) = application();
    let mut model = WindowWorkspace::new(Uuid::new_v4());
    let keys = model
        .ui_test_local_screens_fixture(&["work", "notes"])
        .unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    run(&mut app, WindowId::FIRST, KeyCommand::NewWindow);
    model.ui_test_open_window(SECOND).unwrap();
    model.ui_test_move_screen(&keys[1], SECOND).unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    model_for(&mut app, &model, SECOND);
    sent(&mut commands);

    // The worker tells every window of the question.
    let question = Arc::new(vec![splitty_client_core::native_workspace::SshPrompt {
        id: "a".repeat(64),
        target: "me@192.0.2.1".into(),
        prompt: "Enter passphrase".into(),
        confirm: false,
    }]);
    for window in [WindowId::FIRST, SECOND] {
        let update = Update::SshPrompts(question.clone());
        let update = if window == WindowId::FIRST {
            update
        } else {
            Update::ForWindow {
                window,
                update: Box::new(update),
            }
        };
        app.update(|cx| Windows::ui_test_deliver(update, cx));
        app.run_until_parked();
    }
    assert_eq!(
        app.update(|cx| cx.global::<crate::askpass::Prompts>().asked()),
        1,
        "one dialog asks"
    );
    // The question went: its dialog goes for the whole application.
    app.update(|cx| Windows::ui_test_deliver(Update::SshPrompts(Arc::new(Vec::new())), cx));
    app.run_until_parked();
    assert_eq!(
        app.update(|cx| cx.global::<crate::askpass::Prompts>().asked()),
        0
    );
}

#[test]
fn a_whole_group_goes_to_another_window_with_one_command() {
    let (mut app, mut commands) = application();
    let mut model = WindowWorkspace::new(Uuid::new_v4());
    let keys = model
        .ui_test_local_screens_fixture(&["work", "notes"])
        .unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    sent(&mut commands);

    // To a new window: it opens, and starts with the group.
    run(
        &mut app,
        WindowId::FIRST,
        KeyCommand::MoveScreenGroupToNewWindow,
    );
    assert_eq!(app.update(|cx| Windows::count(cx)), 2);
    assert!(sent(&mut commands).iter().any(|command| matches!(
        command.kind(),
        CommandKind::OpenWindow { window, start: WindowStart::Screens(screens), .. }
            if *window == SECOND && screens == &keys
    )));
    model.ui_test_open_window(SECOND).unwrap();
    for key in keys.iter().rev() {
        model.ui_test_move_screen(key, SECOND).unwrap();
    }
    model_for(&mut app, &model, WindowId::FIRST);
    model_for(&mut app, &model, SECOND);
    sent(&mut commands);

    // To the next open window: one command names every Screen of the group.
    run(&mut app, SECOND, KeyCommand::MoveScreenGroupToNextWindow);
    assert!(sent(&mut commands).iter().any(|command| {
        command.window() == SECOND
            && matches!(command.kind(), CommandKind::MoveScreensToWindow { screens, target }
                if screens == &keys && *target == WindowId::FIRST)
    }));
}

#[test]
fn the_screens_menu_moves_it_to_a_new_window_by_a_click() {
    use gpui_kit::{Modifiers, VisualTestContext};
    let (mut app, mut commands) = application();
    let mut model = WindowWorkspace::new(Uuid::new_v4());
    let keys = model
        .ui_test_local_screens_fixture(&["work", "notes"])
        .unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    sent(&mut commands);
    let (handle, view) = app
        .update(|cx| Windows::ui_test_window(WindowId::FIRST, cx))
        .unwrap();
    let mut window = VisualTestContext::from_window(handle, &app);
    let key = keys[1].clone();
    // The menu a right click on the Screen's row opens.
    window.update(|window, cx| {
        cx.set_reduce_motion(true);
        window.resize(gpui_kit::size(gpui_kit::px(900.), gpui_kit::px(620.)));
        view.update(cx, |workspace, cx| {
            workspace.drawer_visible = true;
            let at = gpui_kit::point(gpui_kit::px(60.), gpui_kit::px(120.));
            workspace.open_screen_menu(&key, at, window, cx);
        });
        window.draw(cx).clear(cx);
    });
    let entry = window
        .debug_bounds("popover-move-to-new-window")
        .expect("the menu's entry");
    window.simulate_click(entry.center(), Modifiers::default());
    window.run_until_parked();
    assert!(sent(&mut commands).iter().any(|command| matches!(
        command.kind(),
        CommandKind::OpenWindow { start: WindowStart::Screen(screen), .. } if screen == &keys[1]
    )));
    assert_eq!(app.update(|cx| Windows::count(cx)), 2);
}

#[test]
fn a_hosts_screen_changes_windows_with_its_terminal_view() {
    let (mut app, mut commands) = application();
    let mut model = WindowWorkspace::new(Uuid::new_v4());
    let local = model.ui_test_local_screen_fixture("work").unwrap();
    let (remote, _) = model.ui_test_remote_terminal_fixture("host.test").unwrap();
    assert_ne!(remote.source_id(), local.source_id());
    model_for(&mut app, &model, WindowId::FIRST);
    assert_eq!(shown(&mut app, WindowId::FIRST), Some(remote.clone()));
    let views = |app: &mut TestAppContext, id: WindowId| -> Vec<EntityId> {
        let Some(view) = view(app, id) else {
            return Vec::new();
        };
        app.update(|cx| {
            view.read(cx)
                .panes
                .iter()
                .filter(|(pane, _)| pane.source_id() == remote.source_id())
                .map(|(_, pane)| pane.terminal.entity_id())
                .collect()
        })
    };
    let before = views(&mut app, WindowId::FIRST);
    assert_eq!(before.len(), 1, "the host Screen's terminal view");
    sent(&mut commands);

    // The host's Screen goes to a new window over the connection the
    // application already has: no command connects the host again.
    run(&mut app, WindowId::FIRST, KeyCommand::MoveScreenToNewWindow);
    assert_eq!(app.update(|cx| Windows::count(cx)), 2);
    let opened = sent(&mut commands);
    assert!(opened.iter().any(|command| matches!(
        command.kind(),
        CommandKind::OpenWindow { window, start: WindowStart::Screen(screen), .. }
            if *window == SECOND && screen == &remote
    )));
    assert!(!opened
        .iter()
        .any(|command| matches!(command.kind(), CommandKind::ConnectHost { .. })));

    model.ui_test_open_window(SECOND).unwrap();
    model.ui_test_move_screen(&remote, SECOND).unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    model_for(&mut app, &model, SECOND);
    assert_eq!(shown(&mut app, SECOND), Some(remote.clone()));
    assert_eq!(shown(&mut app, WindowId::FIRST), Some(local));
    assert!(views(&mut app, WindowId::FIRST).is_empty());
    assert_eq!(
        views(&mut app, SECOND),
        before,
        "the same terminal view, with what it shows"
    );

    // Closing the window that holds only the host's Screen detaches it:
    // its sessions stay on the host, so nothing is assessed or asked.
    sent(&mut commands);
    run(&mut app, SECOND, KeyCommand::CloseWindow);
    let closing = sent(&mut commands);
    assert!(closing.iter().any(|command| matches!(
        command.kind(),
        CommandKind::DetachRemoteScreen { screen } if screen == &remote
    )));
    assert!(!closing.iter().any(|command| matches!(
        command.kind(),
        CommandKind::AssessClose { .. } | CommandKind::CloseWindow
    )));
}

#[test]
fn drawer_pinning_is_per_window_and_persists_when_all_windows_agree() {
    use splitty_client_core::appearance::{
        test_support::{settings_snapshot_with_pin, Availability},
        DrawerVisibilityOutcome, DrawerVisibilityResult,
    };

    let (mut app, mut commands) = application();
    let owner = Uuid::new_v4();
    let mut model = WindowWorkspace::new(owner);
    let keys = model
        .ui_test_local_screens_fixture(&["work", "notes"])
        .unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    sent(&mut commands);

    // Open a second window and prime both with saved `drawer.pinned = true`.
    run(&mut app, WindowId::FIRST, KeyCommand::MoveScreenToNewWindow);
    model.ui_test_open_window(SECOND).unwrap();
    model.ui_test_move_screen(&keys[1], SECOND).unwrap();
    model_for(&mut app, &model, WindowId::FIRST);
    model_for(&mut app, &model, SECOND);
    let deliver_appearance = |app: &mut TestAppContext, generation: u64, pinned: bool| {
        let snapshot = settings_snapshot_with_pin(owner, generation, Availability::Full, pinned);
        for id in [WindowId::FIRST, SECOND] {
            let update = Update::Appearance(snapshot.clone());
            let update = if id == WindowId::FIRST {
                update
            } else {
                Update::ForWindow {
                    window: id,
                    update: Box::new(update),
                }
            };
            app.update(|cx| Windows::ui_test_deliver(update, cx));
        }
        app.run_until_parked();
    };
    deliver_appearance(&mut app, 1, true);
    sent(&mut commands);

    let pinned = |app: &mut TestAppContext, id: WindowId| {
        let view = view(app, id).unwrap();
        app.update(|cx| view.read(cx).drawer_pinned())
    };
    let toggle = |app: &mut TestAppContext, id: WindowId| {
        let view = view(app, id).unwrap();
        app.update(|cx| view.update(cx, |workspace, cx| workspace.toggle_drawer(cx)));
    };
    assert!(pinned(&mut app, WindowId::FIRST));
    assert!(pinned(&mut app, SECOND));

    // Unpinning only the second window leaves the first pinned and does not
    // overwrite the persisted default.
    toggle(&mut app, SECOND);
    assert!(pinned(&mut app, WindowId::FIRST));
    assert!(!pinned(&mut app, SECOND));
    assert!(!sent(&mut commands)
        .iter()
        .any(|c| matches!(c.kind(), CommandKind::SetDrawerVisibility { .. })));

    // An unrelated appearance broadcast (same `drawer.pinned`) keeps the
    // second window unpinned.
    deliver_appearance(&mut app, 2, true);
    assert!(pinned(&mut app, WindowId::FIRST));
    assert!(!pinned(&mut app, SECOND));

    // Unpinning the first window too makes all open windows agree on `false`,
    // so the new default is persisted.
    toggle(&mut app, WindowId::FIRST);
    assert!(!pinned(&mut app, WindowId::FIRST));
    assert!(!pinned(&mut app, SECOND));
    let request_id = sent(&mut commands)
        .into_iter()
        .find_map(|c| match c.kind() {
            CommandKind::SetDrawerVisibility {
                request_id,
                pinned: false,
                ..
            } => Some(*request_id),
            _ => None,
        })
        .expect("agreeing windows persist drawer.pinned = false");
    app.update(|cx| {
        Windows::ui_test_deliver(
            Update::DrawerVisibility(DrawerVisibilityResult {
                connection_id: owner,
                request_id,
                outcome: DrawerVisibilityOutcome::Confirmed(false),
            }),
            cx,
        );
    });
    deliver_appearance(&mut app, 3, false);

    // Pinning only the second window again stays local and does not persist.
    sent(&mut commands);
    toggle(&mut app, SECOND);
    assert!(!pinned(&mut app, WindowId::FIRST));
    assert!(pinned(&mut app, SECOND));
    assert!(!sent(&mut commands)
        .iter()
        .any(|c| matches!(c.kind(), CommandKind::SetDrawerVisibility { .. })));

    // An explicit Settings change to `drawer.pinned = true` updates all windows.
    deliver_appearance(&mut app, 4, true);
    assert!(pinned(&mut app, WindowId::FIRST));
    assert!(pinned(&mut app, SECOND));
}
