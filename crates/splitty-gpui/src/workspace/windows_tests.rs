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
fn a_window_closes_by_handing_its_screens_over_and_the_last_one_ends_the_application() {
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

    // One of two windows: nothing is assessed, the worker is asked to
    // remove the window, and the window stays until the worker did.
    run(&mut app, SECOND, KeyCommand::CloseWindow);
    let asked = sent(&mut commands);
    assert!(asked
        .iter()
        .any(|command| command.window() == SECOND
            && matches!(command.kind(), CommandKind::CloseWindow)));
    assert!(!asked.iter().any(|command| matches!(
        command.kind(),
        CommandKind::AssessClose { .. } | CommandKind::RequestClose { .. }
    )));
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
