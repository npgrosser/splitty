use super::*;
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

const PLATFORMS: [Platform; 3] = [Platform::Linux, Platform::Windows, Platform::Mac];

fn chord(text: &str) -> Chord {
    Chord::parse(text).unwrap()
}

/// The default keys, as the registry documents them: `mod` is Alt on Linux
/// and Windows and Cmd on macOS; a second column applies to macOS only.
/// Id, title, category, keys, and the keys on macOS where they differ.
type Row = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    Option<&'static [&'static str]>,
);

const TABLE: &[Row] = &[
    (
        "screen.new",
        "New Screen",
        "Screens",
        &["ctrl+shift+space"],
        None,
    ),
    ("screen.next", "Next Screen", "Screens", &["ctrl+tab"], None),
    (
        "screen.previous",
        "Previous Screen",
        "Screens",
        &["ctrl+shift+tab"],
        None,
    ),
    (
        "screen.select.1",
        "Switch to Screen 1",
        "Screens",
        &["mod+1", "alt+shift+1"],
        Some(&["cmd+1"]),
    ),
    (
        "screen.select.2",
        "Switch to Screen 2",
        "Screens",
        &["mod+2", "alt+shift+2"],
        Some(&["cmd+2"]),
    ),
    (
        "screen.select.3",
        "Switch to Screen 3",
        "Screens",
        &["mod+3", "alt+shift+3"],
        Some(&["cmd+3"]),
    ),
    (
        "screen.select.4",
        "Switch to Screen 4",
        "Screens",
        &["mod+4", "alt+shift+4"],
        Some(&["cmd+4"]),
    ),
    (
        "screen.select.5",
        "Switch to Screen 5",
        "Screens",
        &["mod+5", "alt+shift+5"],
        Some(&["cmd+5"]),
    ),
    (
        "screen.select.6",
        "Switch to Screen 6",
        "Screens",
        &["mod+6", "alt+shift+6"],
        Some(&["cmd+6"]),
    ),
    (
        "screen.select.7",
        "Switch to Screen 7",
        "Screens",
        &["mod+7", "alt+shift+7"],
        Some(&["cmd+7"]),
    ),
    (
        "screen.select.8",
        "Switch to Screen 8",
        "Screens",
        &["mod+8", "alt+shift+8"],
        Some(&["cmd+8"]),
    ),
    (
        "screen.select.9",
        "Switch to Screen 9",
        "Screens",
        &["mod+9", "alt+shift+9"],
        Some(&["cmd+9"]),
    ),
    (
        "screen.close",
        "Close Screen",
        "Screens",
        &["mod+shift+w"],
        None,
    ),
    (
        "screen.rename",
        "Rename Screen",
        "Screens",
        &["mod+shift+r"],
        None,
    ),
    ("screen.move-up", "Move Screen Up", "Screens", &[], None),
    ("screen.move-down", "Move Screen Down", "Screens", &[], None),
    (
        "screen.move-to-new-window",
        "Move Screen to New Window",
        "Screens",
        &[],
        None,
    ),
    (
        "screen.move-to-next-window",
        "Move Screen to Next Window",
        "Screens",
        &[],
        None,
    ),
    (
        "screen.move-group-to-new-window",
        "Move Screen Group to New Window",
        "Screens",
        &[],
        None,
    ),
    (
        "screen.move-group-to-next-window",
        "Move Screen Group to Next Window",
        "Screens",
        &[],
        None,
    ),
    ("tab.new", "New Tab", "Tabs", &["mod+t"], None),
    (
        "tab.new-with-profile",
        "New Tab with Profile…",
        "Tabs",
        &["mod+shift+t"],
        None,
    ),
    ("tab.close", "Close Tab", "Tabs", &["mod+w"], None),
    ("tab.close-others", "Close Other Tabs", "Tabs", &[], None),
    ("tab.next", "Next Tab", "Tabs", &["mod+pagedown"], None),
    (
        "tab.previous",
        "Previous Tab",
        "Tabs",
        &["mod+pageup"],
        None,
    ),
    (
        "tab.move-right",
        "Move Tab Right",
        "Tabs",
        &["mod+shift+pagedown"],
        None,
    ),
    (
        "tab.move-left",
        "Move Tab Left",
        "Tabs",
        &["mod+shift+pageup"],
        None,
    ),
    (
        "tab.move-to-screen",
        "Move Tab to Screen…",
        "Tabs",
        &["mod+shift+m"],
        None,
    ),
    ("pane.split-right", "Split Right", "Panes", &["mod+d"], None),
    (
        "pane.split-down",
        "Split Down",
        "Panes",
        &["mod+shift+d"],
        None,
    ),
    (
        "pane.focus-left",
        "Focus Pane Left",
        "Panes",
        &["mod+shift+left"],
        None,
    ),
    (
        "pane.focus-right",
        "Focus Pane Right",
        "Panes",
        &["mod+shift+right"],
        None,
    ),
    (
        "pane.focus-up",
        "Focus Pane Above",
        "Panes",
        &["mod+shift+up"],
        None,
    ),
    (
        "pane.focus-down",
        "Focus Pane Below",
        "Panes",
        &["mod+shift+down"],
        None,
    ),
    ("pane.maximize", "Maximize Pane", "Panes", &["mod+m"], None),
    (
        "pane.float",
        "Float or Dock Pane",
        "Panes",
        &["mod+shift+f"],
        None,
    ),
    ("pane.arrange", "Arrange Panes", "Panes", &["mod+r"], None),
    (
        "pane.scratchpad",
        "Toggle Scratchpad",
        "Panes",
        &["mod+j"],
        None,
    ),
    (
        "terminal.find",
        "Find in Terminal",
        "Terminal",
        &["ctrl+shift+f"],
        Some(&["cmd+f"]),
    ),
    (
        "terminal.copy",
        "Copy",
        "Terminal",
        &["ctrl+shift+c", "super+c"],
        Some(&["cmd+c"]),
    ),
    (
        "terminal.paste",
        "Paste",
        "Terminal",
        &["ctrl+shift+v", "super+v"],
        Some(&["cmd+v"]),
    ),
    (
        "terminal.clear",
        "Clear Buffer",
        "Terminal",
        &["mod+k"],
        None,
    ),
    (
        "terminal.scroll-page-up",
        "Scroll Page Up",
        "Terminal",
        &["shift+pageup"],
        None,
    ),
    (
        "terminal.scroll-page-down",
        "Scroll Page Down",
        "Terminal",
        &["shift+pagedown"],
        None,
    ),
    (
        "terminal.scroll-top",
        "Scroll to Top",
        "Terminal",
        &["shift+home"],
        None,
    ),
    (
        "terminal.scroll-bottom",
        "Scroll to Bottom",
        "Terminal",
        &["shift+end"],
        None,
    ),
    (
        "terminal.copy-directory",
        "Copy Working Directory",
        "Terminal",
        &[],
        None,
    ),
    (
        "terminal.open-viewer",
        "Open Selection in Viewer",
        "Terminal",
        &[],
        None,
    ),
    (
        "drawer.toggle",
        "Toggle Drawer",
        "View",
        &["mod+shift+b"],
        None,
    ),
    ("drawer.focus", "Focus Drawer", "View", &["mod+e"], None),
    (
        "view.hosts",
        "Hosts and Ports",
        "View",
        &["alt+h"],
        Some(&["cmd+shift+h"]),
    ),
    ("view.templates", "Templates…", "View", &["mod+p"], None),
    (
        "view.settings",
        "Settings",
        "View",
        &["ctrl+,"],
        Some(&["cmd+,"]),
    ),
    (
        "view.palette",
        "Command Palette",
        "View",
        &["mod+shift+p"],
        None,
    ),
    (
        "view.keyboard",
        "Keyboard Shortcuts",
        "View",
        &["mod+shift+k"],
        None,
    ),
    ("host.connect", "Connect to Host…", "View", &[], None),
    (
        "zoom.in",
        "Zoom In",
        "Window",
        &["ctrl+=", "ctrl+plus"],
        Some(&["cmd+=", "cmd+plus"]),
    ),
    (
        "zoom.out",
        "Zoom Out",
        "Window",
        &["ctrl+-"],
        Some(&["cmd+-"]),
    ),
    (
        "zoom.reset",
        "Reset Zoom",
        "Window",
        &["ctrl+0"],
        Some(&["cmd+0"]),
    ),
    (
        "window.fullscreen",
        "Toggle Full Screen",
        "Window",
        &["f11"],
        Some(&["ctrl+cmd+f"]),
    ),
    (
        "window.new",
        "New Window",
        "Window",
        &["ctrl+shift+n"],
        Some(&["cmd+n"]),
    ),
    ("window.close", "Close Window", "Window", &[], None),
    ("window.quit", "Quit", "Window", &[], Some(&["cmd+q"])),
];

#[test]
fn every_command_has_the_tables_id_title_category_and_defaults_in_its_order() {
    let all = Command::all();
    assert_eq!(all.len(), TABLE.len());
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(sorted, all, "the declaration order is the registry's");
    for (command, (id, title, category, keys, mac_keys)) in all.into_iter().zip(TABLE) {
        assert_eq!(command.id(), *id);
        assert_eq!(Command::from_id(id), Some(command));
        assert_eq!(command.title(), *title);
        assert_eq!(command.category().title(), *category);
        for platform in PLATFORMS {
            let primary = if platform.is_mac() { "cmd" } else { "alt" };
            let texts = match (platform, mac_keys) {
                (Platform::Mac, Some(mac)) => *mac,
                _ => *keys,
            };
            let expected: Vec<_> = texts
                .iter()
                // `alt+shift+N` is the second Screen key of Linux and Windows.
                .filter(|text| !(platform.is_mac() && text.starts_with("alt+shift+")))
                .map(|text| chord(&text.replace("mod", primary)))
                .collect();
            assert_eq!(
                defaults(platform, command),
                expected,
                "{id} on {platform:?}"
            );
        }
    }
    assert_eq!(Command::from_id("screen.select.0"), None);
    assert_eq!(Command::from_id("nothing"), None);
    let categories: Vec<_> = Command::all().into_iter().map(Command::category).collect();
    let mut ordered = categories.clone();
    ordered.sort();
    assert_eq!(categories, ordered);
}

#[test]
fn no_two_commands_share_a_default_on_any_platform() {
    for platform in PLATFORMS {
        let mut seen: HashMap<Chord, Command> = HashMap::new();
        for command in Command::all() {
            for chord in defaults(platform, command) {
                if let Some(other) = seen.insert(chord.clone(), command) {
                    panic!("{chord:?} of {command:?} and {other:?} on {platform:?}");
                }
                assert_ne!(
                    chord.reserved(platform),
                    Some(Reserved::Typing),
                    "{chord:?}"
                );
            }
        }
        let keymap = Keymap::defaults(platform);
        assert!(keymap.problems().is_empty());
        for command in Command::all() {
            assert!(keymap.is_default(command));
            for chord in keymap.bindings(command) {
                assert_eq!(keymap.lookup(chord), Some(command));
            }
        }
    }
}

#[test]
fn chords_parse_print_and_label_per_platform() {
    for (text, canonical) in [
        ("ctrl+shift+tab", "ctrl+shift+tab"),
        ("Shift+Ctrl+Tab", "ctrl+shift+tab"),
        ("ctrl+plus", "ctrl+plus"),
        ("ctrl++", "ctrl+plus"),
        ("alt+=", "alt+="),
        ("ctrl+,", "ctrl+,"),
        ("win+c", "super+c"),
        ("meta+c", "super+c"),
        ("cmd+shift+left", "shift+super+left"),
        ("f11", "f11"),
        ("ctrl+esc", "ctrl+escape"),
        ("alt+PageDown", "alt+pagedown"),
    ] {
        let parsed = chord(text);
        assert_eq!(parsed.text(Platform::Linux), canonical, "{text}");
        assert_eq!(chord(&parsed.text(Platform::Linux)), parsed);
        assert_eq!(chord(&parsed.text(Platform::Mac)), parsed);
    }
    assert_eq!(chord("super+shift+k").text(Platform::Mac), "shift+cmd+k");
    for invalid in [
        "",
        "ctrl+",
        "ctrl+hyper+a",
        "ctrl+ctrl+a",
        "ctrl+banana",
        "f0",
        "f25",
    ] {
        assert!(Chord::parse(invalid).is_err(), "{invalid}");
    }
    for (text, linux, windows, mac) in [
        (
            "ctrl+shift+tab",
            "Ctrl+Shift+Tab",
            "Ctrl+Shift+Tab",
            "⌃⇧Tab",
        ),
        ("alt+t", "Alt+T", "Alt+T", "⌥T"),
        ("cmd+shift+d", "Shift+Super+D", "Shift+Win+D", "⇧⌘D"),
        ("alt+shift+left", "Alt+Shift+←", "Alt+Shift+←", "⌥⇧←"),
        (
            "ctrl+alt+shift+cmd+pageup",
            "Ctrl+Alt+Shift+Super+PageUp",
            "Ctrl+Alt+Shift+Win+PageUp",
            "⌃⌥⇧⌘PageUp",
        ),
        ("ctrl+,", "Ctrl+,", "Ctrl+,", "⌃,"),
        ("ctrl+plus", "Ctrl++", "Ctrl++", "⌃+"),
        ("f11", "F11", "F11", "F11"),
        (
            "ctrl+shift+space",
            "Ctrl+Shift+Space",
            "Ctrl+Shift+Space",
            "⌃⇧Space",
        ),
    ] {
        let parsed = chord(text);
        assert_eq!(parsed.label(Platform::Linux), linux);
        assert_eq!(parsed.label(Platform::Windows), windows);
        assert_eq!(parsed.label(Platform::Mac), mac);
    }
    let keymap = Keymap::defaults(Platform::Mac);
    assert_eq!(keymap.label(Command::SplitDown).as_deref(), Some("⇧⌘D"));
    assert_eq!(keymap.label(Command::Hosts).as_deref(), Some("⇧⌘H"));
    let keymap = Keymap::defaults(Platform::Linux);
    assert_eq!(keymap.label(Command::Settings).as_deref(), Some("Ctrl+,"));
    assert_eq!(keymap.label(Command::Hosts).as_deref(), Some("Alt+H"));
    assert_eq!(keymap.label(Command::ConnectHost), None);
}

#[test]
fn reserved_chords_are_typing_or_programs_keys() {
    for platform in PLATFORMS {
        for typing in [
            "a",
            "1",
            "space",
            ",",
            "shift+a",
            "shift+1",
            "shift+space",
            "shift+tab",
            "shift+enter",
            "shift+backspace",
            "shift+left",
            "enter",
            "tab",
            "escape",
            "backspace",
            "delete",
            "left",
            "up",
            "home",
            "end",
            "pageup",
            "pagedown",
        ] {
            assert_eq!(
                chord(typing).reserved(platform),
                Some(Reserved::Typing),
                "{typing}"
            );
        }
        for program in [
            "ctrl+c",
            "ctrl+b",
            "ctrl+f",
            "ctrl+,",
            "ctrl+shift+c",
            "ctrl+shift+v",
        ] {
            assert_eq!(
                chord(program).reserved(platform),
                Some(Reserved::Program),
                "{program}"
            );
        }
        for free in [
            "shift+pageup",
            "shift+pagedown",
            "shift+home",
            "shift+end",
            "f11",
            "ctrl+tab",
            "ctrl+shift+space",
            "ctrl+1",
            "alt+shift+d",
            "ctrl+shift+f",
            "cmd+t",
        ] {
            assert_eq!(chord(free).reserved(platform), None, "{free}");
        }
    }
    assert_eq!(
        chord("alt+t").reserved(Platform::Linux),
        Some(Reserved::Program)
    );
    assert_eq!(
        chord("alt+t").reserved(Platform::Windows),
        Some(Reserved::Program)
    );
    assert_eq!(chord("alt+t").reserved(Platform::Mac), None);
}

#[test]
fn overrides_replace_unbind_steal_and_keep_the_first_of_two() {
    let platform = Platform::Linux;
    let mut overrides = Overrides::default();
    // Replace: the old key is free, the new one runs the command.
    overrides
        .set(Command::NextScreen, vec![chord("alt+n")])
        .unwrap();
    // Unbind.
    overrides.set(Command::ToggleDrawer, Vec::new()).unwrap();
    // Steal: Alt+T is New Tab's default.
    overrides
        .set(Command::CloseOtherTabs, vec![chord("alt+t")])
        .unwrap();
    // Two overrides with one chord: the first in registry order keeps it.
    overrides
        .set(Command::Quit, vec![chord("ctrl+shift+q")])
        .unwrap();
    overrides
        .set(
            Command::RenameScreen,
            vec![chord("ctrl+shift+q"), chord("f2")],
        )
        .unwrap();
    let keymap = Keymap::new(platform, &overrides);
    assert_eq!(keymap.lookup(&chord("alt+n")), Some(Command::NextScreen));
    assert_eq!(keymap.lookup(&chord("ctrl+tab")), None);
    assert!(!keymap.is_default(Command::NextScreen));
    assert_eq!(keymap.bindings(Command::ToggleDrawer), &[] as &[Chord]);
    assert_eq!(keymap.label(Command::ToggleDrawer), None);
    assert_eq!(keymap.lookup(&chord("alt+shift+b")), None);
    assert_eq!(
        keymap.lookup(&chord("alt+t")),
        Some(Command::CloseOtherTabs)
    );
    assert_eq!(keymap.bindings(Command::NewTab), &[] as &[Chord]);
    assert!(!keymap.is_default(Command::NewTab));
    assert_eq!(
        keymap.lookup(&chord("ctrl+shift+q")),
        Some(Command::RenameScreen)
    );
    assert_eq!(keymap.bindings(Command::Quit), &[] as &[Chord]);
    assert_eq!(keymap.lookup(&chord("f2")), Some(Command::RenameScreen));
    assert_eq!(
        keymap.problems(),
        ["Ctrl+Shift+Q is bound to both Rename Screen and Quit; it stays with Rename Screen."]
    );
    assert!(keymap.is_default(Command::SplitRight));

    // A typing chord is refused and changes nothing.
    assert_eq!(
        overrides.set(Command::NewTab, vec![chord("alt+x"), chord("shift+x")]),
        Err(TypingChord(chord("shift+x")))
    );
    assert_eq!(overrides.get(Command::NewTab), None);
    overrides.reset(Command::NextScreen);
    assert_eq!(overrides.get(Command::NextScreen), None);
    let keymap = Keymap::new(platform, &overrides);
    assert_eq!(keymap.lookup(&chord("ctrl+tab")), Some(Command::NextScreen));
}

#[test]
fn reading_skips_and_reports_what_it_cannot_use() {
    let overrides = Overrides::from_json(
        r#"{"bindings": {
            "screen.next": ["ctrl+n", "ctrl+banana", 3, "x"],
            "drawer.toggle": [],
            "screen.teleport": ["alt+z"],
            "pane.float": ["ctlr+m"],
            "tab.new": "alt+y"
        }}"#,
    );
    // A list of which nothing could be read leaves the defaults.
    assert_eq!(overrides.get(Command::from_id("pane.float").unwrap()), None);
    assert!(overrides
        .problems()
        .iter()
        .any(|problem| problem.contains("default keys apply")));
    assert_eq!(
        overrides.get(Command::NextScreen),
        Some(&[chord("ctrl+n")][..])
    );
    assert_eq!(overrides.get(Command::ToggleDrawer), Some(&[][..]));
    assert_eq!(overrides.get(Command::NewTab), None);
    assert_eq!(overrides.problems().len(), 7, "{:?}", overrides.problems());
    assert!(overrides
        .problems()
        .iter()
        .any(|problem| problem.contains("ctrl+banana")));
    assert!(overrides
        .problems()
        .iter()
        .any(|problem| problem.contains("screen.teleport")));
    assert!(overrides
        .problems()
        .iter()
        .any(|problem| problem.contains("plain typing")));
    let keymap = Keymap::new(Platform::Linux, &overrides);
    assert_eq!(keymap.problems(), overrides.problems());
    assert_eq!(keymap.lookup(&chord("ctrl+n")), Some(Command::NextScreen));
    for text in ["", "[]", "{}", r#"{"bindings": []}"#] {
        let overrides = Overrides::from_json(text);
        assert!(overrides.is_empty());
        assert_eq!(overrides.problems().len(), 1, "{text}");
    }
}

fn directory() -> PathBuf {
    std::env::temp_dir().join(format!("splitty-keybindings-{}", Uuid::new_v4().simple()))
}

#[test]
fn the_file_round_trips_and_a_missing_or_broken_one_holds_no_overrides() {
    let directory = directory();
    let path = directory.join("keybindings.json");
    let mut file = KeybindingsFile::open(path.clone());
    assert!(file.overrides().is_empty());
    assert!(
        file.overrides().problems().is_empty(),
        "a missing file is no problem"
    );
    let mut overrides = Overrides::default();
    overrides
        .set(Command::ZoomIn, vec![chord("ctrl+plus"), chord("super+=")])
        .unwrap();
    overrides.set(Command::ToggleDrawer, Vec::new()).unwrap();
    file.save(overrides.clone()).unwrap();
    assert_eq!(file.overrides(), &overrides);
    let read = KeybindingsFile::open(path.clone());
    assert_eq!(read.overrides(), &overrides);
    let text = std::fs::read_to_string(&path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["bindings"]["drawer.toggle"], serde_json::json!([]));
    assert_eq!(value["bindings"]["zoom.in"][0], "ctrl+plus");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "a private file");
    }

    std::fs::write(&path, "not json").unwrap();
    let broken = KeybindingsFile::open(path.clone());
    assert!(broken.overrides().is_empty());
    assert_eq!(broken.overrides().problems().len(), 1);
    std::fs::write(&path, " ".repeat(64 * 1024 + 1)).unwrap();
    let oversized = KeybindingsFile::open(path.clone());
    assert!(oversized.overrides().is_empty());
    assert!(oversized.overrides().problems()[0].contains("too large"));
    std::fs::remove_dir_all(&directory).unwrap();

    // A directory in the file's place cannot be replaced: the save fails and
    // the kept overrides stay as they were.
    std::fs::create_dir_all(&path).unwrap();
    let mut blocked = KeybindingsFile::open(path.clone());
    assert!(blocked.save(overrides).is_err());
    assert!(blocked.overrides().is_empty());
    std::fs::remove_dir_all(&directory).unwrap();
}
