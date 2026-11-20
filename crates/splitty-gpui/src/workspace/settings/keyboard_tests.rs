//! The Keyboard page in the window: it lists the registry, filters, records,
//! removes and resets keys, applies them at once and writes the file.
use super::Section;
use crate::workspace::keyboard::Notice;
use crate::workspace::{chrome::view_tests::with_view, Destination, Workspace};
use gpui_kit::{
    Bounds, Entity, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, Pixels, VisualTestContext,
};
use splitty_client_core::keymap::{Chord, Command, KeybindingsFile};
use std::path::{Path, PathBuf};

/// Opens the Keyboard page in a window that keeps its bindings in `file`.
fn open(window: &mut VisualTestContext, view: &Entity<Workspace>, file: PathBuf) {
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            workspace.use_keybindings(KeybindingsFile::open(file), cx);
            workspace.run_command(Command::KeyboardShortcuts, window, cx);
        });
        window.draw(cx).clear(cx);
    });
}

/// Shows only the rows that match `query`, so that they are in view.
fn search(window: &mut VisualTestContext, view: &Entity<Workspace>, query: &str) {
    window.update(|window, cx| {
        let search = view.read(cx).settings.read(cx).keyboard.search().clone();
        search.update(cx, |search, cx| search.set_value(query, window, cx));
        window.draw(cx).clear(cx);
    });
}

fn draw(window: &mut VisualTestContext) {
    window.update(|window, cx| window.draw(cx).clear(cx));
}

/// Where `selector` was painted, if it was.
fn bounds(window: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    window.debug_bounds(selector.to_owned().leak())
}

fn shown(window: &mut VisualTestContext, selector: &str) -> bool {
    bounds(window, selector).is_some()
}

fn click(window: &mut VisualTestContext, selector: &str) {
    let center = bounds(window, selector)
        .unwrap_or_else(|| panic!("{selector} is shown"))
        .center();
    window.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: center,
        click_count: 1,
        modifiers: Modifiers::default(),
        first_mouse: false,
    });
    window.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: center,
        click_count: 1,
        modifiers: Modifiers::default(),
    });
    draw(window);
}

/// `chord` as the test window's keystroke text.
fn keys(chord: &Chord) -> String {
    let mut text = String::new();
    for (held, name) in [
        (chord.ctrl, "ctrl-"),
        (chord.alt, "alt-"),
        (chord.shift, "shift-"),
        (chord.platform, "cmd-"),
    ] {
        if held {
            text.push_str(name);
        }
    }
    text.push_str(&chord.key);
    text
}

fn keymap(window: &mut VisualTestContext) -> std::rc::Rc<splitty_client_core::keymap::Keymap> {
    window.update(|_, cx| crate::keys::keymap(cx))
}

fn file_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn notices(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    command: Command,
) -> Vec<Notice> {
    window.update(|_, cx| {
        view.read(cx)
            .settings
            .read(cx)
            .keyboard
            .notices(command)
            .to_vec()
    })
}

// Level: in-process (the workspace view with the settings open).
#[test]
fn the_keyboard_page_lists_every_command_and_its_search_filters_by_title_and_key() {
    with_view(|window, view| {
        let dir = tempfile::tempdir().unwrap();
        open(window, &view, dir.path().join("keybindings.json"));
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert!(workspace.destination == Destination::Settings);
            assert_eq!(workspace.settings.read(cx).section, Section::Keyboard);
        });
        for command in Command::all() {
            assert!(
                shown(window, &format!("keyboard-row-{}", command.id())),
                "{} is listed",
                command.id()
            );
        }
        // Several default keys are all shown; an unbound command shows none.
        let zoom = format!("keyboard-key-{}-1", Command::ZoomIn.id());
        assert!(shown(window, &zoom));
        let connect = format!("keyboard-key-{}-0", Command::ConnectHost.id());
        assert!(!shown(window, &connect));

        // The search field has the focus: typing filters by title.
        window.simulate_input("split down");
        draw(window);
        assert!(shown(window, "keyboard-row-pane.split-down"));
        assert!(!shown(window, "keyboard-row-pane.split-right"));
        assert!(!shown(window, "keyboard-row-tab.new"));
        // And by key, as written in the file.
        search(window, &view, "ctrl+tab");
        assert!(shown(window, "keyboard-row-screen.next"));
        assert!(!shown(window, "keyboard-row-screen.previous"));
    });
}

// Level: in-process (the workspace view with the settings open).
#[test]
fn a_recorded_chord_applies_at_once_is_written_and_reset_removes_it() {
    with_view(|window, view| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.json");
        open(window, &view, path.clone());
        let chord = Chord::new("n").ctrl().shift();
        click(window, "keyboard-record-screen.new");
        window.update(|_, cx| {
            assert_eq!(
                view.read(cx).settings.read(cx).keyboard.recording(),
                Some(Command::NewScreen)
            );
        });
        // A modifier alone is waited past.
        window.simulate_keystrokes("shift");
        window.simulate_keystrokes(&keys(&chord));
        draw(window);
        let bound = keymap(window);
        assert_eq!(bound.lookup(&chord), Some(Command::NewScreen));
        assert_eq!(
            bound.bindings(Command::NewScreen),
            std::slice::from_ref(&chord)
        );
        let written = file_text(&path);
        assert!(
            written.contains("\"screen.new\"") && written.contains("ctrl+shift+n"),
            "{written}"
        );
        assert!(shown(window, "keyboard-key-screen.new-0"));
        assert!(!shown(window, "keyboard-key-screen.new-1"));
        window.update(|_, cx| {
            assert_eq!(view.read(cx).settings.read(cx).keyboard.recording(), None);
        });

        click(window, "keyboard-reset-screen.new");
        assert!(keymap(window).is_default(Command::NewScreen));
        assert!(
            !file_text(&path).contains("screen.new"),
            "{}",
            file_text(&path)
        );

        // Remove stores an empty list.
        click(window, "keyboard-remove-screen.new");
        assert!(keymap(window).bindings(Command::NewScreen).is_empty());
        assert!(file_text(&path).contains("\"screen.new\": []"));
        assert!(!shown(window, "keyboard-key-screen.new-0"));
    });
}

// Level: in-process (the workspace view with the settings open).
#[test]
fn while_recording_a_key_runs_no_command_and_a_held_chord_names_who_lost_it() {
    with_view(|window, view| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.json");
        open(window, &view, path.clone());
        // The Hosts key works on the settings page, and leaves it.
        let hosts = keymap(window).bindings(Command::Hosts)[0].clone();
        search(window, &view, "tab.new");
        click(window, "keyboard-record-tab.new");
        window.simulate_keystrokes(&keys(&hosts));
        draw(window);
        window.update(|_, cx| {
            assert!(view.read(cx).destination == Destination::Settings);
        });
        let keymap_now = keymap(window);
        assert_eq!(keymap_now.lookup(&hosts), Some(Command::NewTab));
        assert!(keymap_now.bindings(Command::Hosts).is_empty());
        assert_eq!(
            notices(window, &view, Command::NewTab)[0],
            Notice::Taken {
                from: Command::Hosts,
                key: hosts.label(keymap_now.platform()),
            }
        );
        assert!(shown(window, "keyboard-notice-tab.new-0"));

        // Escape cancels recording and leaves the settings open.
        click(window, "keyboard-record-tab.new");
        window.simulate_keystrokes("escape");
        draw(window);
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert!(workspace.destination == Destination::Settings);
            assert_eq!(workspace.settings.read(cx).keyboard.recording(), None);
        });
        assert_eq!(keymap(window).lookup(&hosts), Some(Command::NewTab));

        // A typing chord is refused and changes nothing.
        let before = file_text(&path);
        click(window, "keyboard-record-tab.new");
        window.simulate_keystrokes("a");
        draw(window);
        assert_eq!(keymap(window).lookup(&hosts), Some(Command::NewTab));
        assert_eq!(file_text(&path), before);
        assert_eq!(
            notices(window, &view, Command::NewTab),
            [Notice::Typing { key: "A".into() }]
        );
    });
}

// Level: in-process (the workspace view with the settings open).
#[test]
fn a_change_that_cannot_be_written_stays_in_effect_and_the_page_says_so() {
    with_view(|window, view| {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, b"").unwrap();
        open(window, &view, blocker.join("keybindings.json"));
        search(window, &view, "split right");
        click(window, "keyboard-remove-pane.split-right");
        assert!(keymap(window).bindings(Command::SplitRight).is_empty());
        window.update(|_, cx| {
            assert!(view
                .read(cx)
                .settings
                .read(cx)
                .keyboard
                .save_error()
                .is_some());
        });
        assert!(shown(window, "keyboard-save-error"));

        // Reset All asks first, then returns every command to its defaults.
        click(window, "keyboard-reset-all");
        assert!(!keymap(window).is_default(Command::SplitRight));
        click(window, "keyboard-reset-all-confirm");
        assert!(keymap(window).is_default(Command::SplitRight));
    });
}

// Level: in-process (the workspace view and its palette).
#[test]
fn the_palette_entry_opens_the_keyboard_page() {
    with_view(|window, view| {
        let dir = tempfile::tempdir().unwrap();
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.use_keybindings(
                    KeybindingsFile::open(dir.path().join("keybindings.json")),
                    cx,
                );
                workspace.toggle_palette(window, cx);
            });
            window.draw(cx).clear(cx);
        });
        window.simulate_input("Keyboard Shortcuts");
        window.simulate_keystrokes("enter");
        draw(window);
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert!(workspace.palette.is_none());
            assert!(workspace.destination == Destination::Settings);
            assert_eq!(workspace.settings.read(cx).section, Section::Keyboard);
        });
    });
}

// Level: in-process (the workspace view with the settings open).
#[test]
fn tab_reaches_a_row_and_enter_starts_recording() {
    with_view(|window, view| {
        let dir = tempfile::tempdir().unwrap();
        open(window, &view, dir.path().join("keybindings.json"));
        search(window, &view, "screen.new");
        let enter = || Keystroke::parse("enter").unwrap();
        // From the search field past the disabled Reset All to the row.
        window.simulate_keystrokes("tab");
        draw(window);
        window.simulate_event(KeyDownEvent {
            keystroke: enter(),
            is_held: false,
            prefer_character_input: false,
        });
        window.simulate_event(KeyUpEvent { keystroke: enter() });
        draw(window);
        window.update(|_, cx| {
            assert_eq!(
                view.read(cx).settings.read(cx).keyboard.recording(),
                Some(Command::NewScreen),
                "past the search field and Reset All, Enter on the row records"
            );
        });
    });
}
