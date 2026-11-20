//! Key dispatch through the keymap: a bound chord runs its command and
//! never reaches the PTY, an unbound one does, and labels follow the keymap.
use crate::workspace::{chrome::view_tests::with_view, Command, Destination, WorkerCommand};
use crate::workspace::{palette::PaletteCommand, Workspace};
use gpui_kit::{Entity, VisualTestContext};
use splitty_client_core::keymap::{Chord, Command as KeyCommand, Overrides};
use splitty_client_core::workspace::{Axis, WindowWorkspace};
use tokio::sync::mpsc;
use uuid::Uuid;

fn model(owner: Uuid) -> WindowWorkspace {
    let mut model = WindowWorkspace::new(owner);
    model.ui_test_local_screen_fixture("Work").unwrap();
    let source = model.lifecycle_token();
    model
        .source_mut(source.source_id())
        .unwrap()
        .apply_snapshot(
            owner,
            &serde_json::json!({"type":"mux.screen_snapshot","screen":{
                "id":"work","revision":2,
                "root":{"type":"slot","id":"only","paneIds":["p0"],"activePaneId":"p0"},
                "panes":{"p0":{"id":"p0","sessionId":"s0"}},
                "sessions":{"s0":{"id":"s0","status":"running"}}
            }}),
        )
        .unwrap();
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
            workspace.reconcile(model(Uuid::new_v4()), window, cx);
        });
        window.draw(cx).clear(cx);
        view.update(cx, |workspace, cx| workspace.focus_current(window, cx));
        window.draw(cx).clear(cx);
    });
    let mut receiver = receiver;
    while receiver.try_recv().is_ok() {}
    receiver
}

/// What the key sent: the terminal input bytes, and whether it split.
fn press(
    window: &mut VisualTestContext,
    receiver: &mut mpsc::Receiver<WorkerCommand>,
    keys: &str,
) -> (Vec<Vec<u8>>, bool) {
    window.simulate_keystrokes(keys);
    let mut inputs = Vec::new();
    let mut split = false;
    while let Ok(command) = receiver.try_recv() {
        match command.kind() {
            Command::Input { bytes, .. } => inputs.push(bytes.clone()),
            Command::Split { axis, .. } => split |= *axis == Axis::Horizontal,
            _ => {}
        }
    }
    (inputs, split)
}

fn is_mac(window: &mut VisualTestContext) -> bool {
    window.update(|_, cx| cx.platform_info().is_mac())
}

#[test]
fn a_default_key_runs_its_command_and_unbound_control_keys_reach_the_terminal() {
    with_view(|window, view| {
        let mut receiver = setup(window, &view);
        let split = if is_mac(window) { "cmd-d" } else { "alt-d" };
        assert_eq!(press(window, &mut receiver, split), (Vec::new(), true));
        // Ctrl+B and Ctrl+F run no command by default; Ctrl+C never does.
        for (keys, byte) in [("ctrl-b", 0x02), ("ctrl-f", 0x06), ("ctrl-c", 0x03)] {
            assert_eq!(
                press(window, &mut receiver, keys),
                (vec![vec![byte]], false),
                "{keys}"
            );
        }
    });
}

#[test]
fn an_override_moves_the_command_to_its_new_key_and_frees_the_old_one() {
    with_view(|window, view| {
        let mut receiver = setup(window, &view);
        let mut overrides = Overrides::default();
        overrides
            .set(KeyCommand::SplitRight, vec![Chord::new("x").ctrl().alt()])
            .unwrap();
        overrides.set(KeyCommand::SplitDown, Vec::new()).unwrap();
        window.update(|_, cx| {
            view.update(cx, |workspace, cx| {
                workspace.apply_key_overrides(overrides, cx).unwrap()
            })
        });
        assert_eq!(
            press(window, &mut receiver, "ctrl-alt-x"),
            (Vec::new(), true)
        );
        if !is_mac(window) {
            // Alt+D is the program's again: Meta-d reaches the terminal.
            assert_eq!(
                press(window, &mut receiver, "alt-d"),
                (vec![b"\x1bd".to_vec()], false)
            );
        }
        // The palette shows the new key, and no key for an unbound command.
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.toggle_palette(window, cx);
                let entries = workspace.palette_matches(cx);
                let shortcut = |command| {
                    entries
                        .iter()
                        .find(|entry| entry.command == PaletteCommand::Run(command))
                        .map(|entry| entry.shortcut.clone())
                        .expect("a palette row")
                };
                let mac = cx.platform_info().is_mac();
                assert_eq!(
                    shortcut(KeyCommand::SplitRight).as_deref(),
                    Some(if mac { "⌃⌥X" } else { "Ctrl+Alt+X" })
                );
                assert_eq!(shortcut(KeyCommand::SplitDown), None);
                workspace.close_palette(window, cx);
            })
        });
    });
}
