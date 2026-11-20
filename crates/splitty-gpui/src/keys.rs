//! The application's keymap as every view reads it, and the chord of a key
//! event. The workspace replaces the keymap when the user's bindings change.
use gpui_kit::{App, Global, Keystroke, SharedString};
use splitty_client_core::keymap::{Chord, Command, Keymap, Platform};
use std::{cell::RefCell, rc::Rc};

struct ActiveKeymap(Rc<Keymap>);
impl Global for ActiveKeymap {}

thread_local! {
    /// The defaults per platform, for an application that has set none.
    static DEFAULTS: RefCell<Vec<Rc<Keymap>>> = const { RefCell::new(Vec::new()) };
}

/// The platform the keymap is for, as the application reports it.
pub(crate) fn platform(cx: &App) -> Platform {
    if cx.platform_info().is_mac() {
        Platform::Mac
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Linux
    }
}

/// The keymap in effect: the one last set, or the platform's defaults.
pub(crate) fn keymap(cx: &App) -> Rc<Keymap> {
    let platform = platform(cx);
    if let Some(active) = cx
        .try_global::<ActiveKeymap>()
        .filter(|active| active.0.platform() == platform)
    {
        return active.0.clone();
    }
    DEFAULTS.with(|defaults| {
        let mut defaults = defaults.borrow_mut();
        if let Some(keymap) = defaults.iter().find(|keymap| keymap.platform() == platform) {
            return keymap.clone();
        }
        let keymap = Rc::new(Keymap::defaults(platform));
        defaults.push(keymap.clone());
        keymap
    })
}

/// Use `keymap` from now on and repaint every window's labels.
pub(crate) fn set_keymap(keymap: Keymap, cx: &mut App) {
    cx.set_global(ActiveKeymap(Rc::new(keymap)));
    cx.refresh_windows();
}

/// The label of the first key of `command`; none when it is unbound.
pub(crate) fn label(command: Command, cx: &App) -> Option<SharedString> {
    keymap(cx).label(command).map(SharedString::from)
}

/// The command a key event runs. `raw_shift` is the window's physical Shift
/// state and `layout` the keyboard layout's id, which recognize Shift with a
/// digit where the layout turns it into a symbol.
pub(crate) fn command(
    keystroke: &Keystroke,
    raw_shift: bool,
    layout: &str,
    cx: &App,
) -> Option<Command> {
    lookup(&keymap(cx), chord(keystroke, raw_shift, layout)?)
}

/// The command of `chord`. A symbol typed with Shift, such as `+`, names its
/// key already, so it also runs the command of the symbol without Shift.
fn lookup(keymap: &Keymap, chord: Chord) -> Option<Command> {
    keymap.lookup(&chord).or_else(|| {
        let symbol =
            chord.key.chars().count() == 1 && !chord.key.chars().all(char::is_alphanumeric);
        (symbol && chord.shift)
            .then(|| {
                keymap.lookup(&Chord {
                    shift: false,
                    ..chord
                })
            })
            .flatten()
    })
}

/// The chord of a key event; none while the Fn modifier is held.
pub(crate) fn chord(keystroke: &Keystroke, raw_shift: bool, layout: &str) -> Option<Chord> {
    let modifiers = keystroke.modifiers;
    if modifiers.function {
        return None;
    }
    let mut chord = Chord {
        ctrl: modifiers.control,
        alt: modifiers.alt,
        shift: modifiers.shift,
        platform: modifiers.platform,
        key: keystroke.key.to_lowercase(),
    };
    if raw_shift {
        if let Some(digit) = shifted_digit(keystroke, layout) {
            chord.key = digit.to_string();
            chord.shift = true;
        }
    }
    Some(chord)
}

// GPUI's XKB conversion clears Keystroke.modifiers.shift for punctuation.
// Window::modifiers retains the physical Shift state. Symbol -> digit is only
// known for these exact XKB layouts; an unknown layout passes through to PTY.
fn shifted_digit(key: &Keystroke, layout: &str) -> Option<usize> {
    if let Ok(digit) = key.key.parse::<usize>() {
        if (1..=9).contains(&digit) {
            return Some(digit);
        }
    }
    let symbols = match layout {
        "English (US)" => ["!", "@", "#", "$", "%", "^", "&", "*", "("],
        "German" => ["!", "\"", "§", "$", "%", "&", "/", "(", ")"],
        _ => return None,
    };
    let printed = key.key_char.as_deref().unwrap_or(&key.key);
    symbols
        .iter()
        .position(|symbol| *symbol == printed)
        .map(|index| index + 1)
}

#[cfg(test)]
mod tests {
    use super::{chord, shifted_digit};
    use gpui_kit::{Keystroke, Modifiers};
    use splitty_client_core::keymap::{Command, Keymap, Platform};

    fn key(name: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            key: name.into(),
            modifiers,
            key_char: None,
        }
    }

    fn lookup(
        platform: Platform,
        keystroke: &Keystroke,
        shift: bool,
        layout: &str,
    ) -> Option<Command> {
        super::lookup(
            &Keymap::defaults(platform),
            chord(keystroke, shift, layout)?,
        )
    }

    #[test]
    fn workspace_chords_do_not_claim_terminal_control_or_clipboard_keys() {
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        let cmd = Modifiers {
            platform: true,
            ..Default::default()
        };
        let ctrl = Modifiers {
            control: true,
            ..Default::default()
        };
        for (platform, base) in [(Platform::Linux, alt), (Platform::Mac, cmd)] {
            let run = |name: &str, modifiers| lookup(platform, &key(name, modifiers), false, "");
            let shifted = Modifiers {
                shift: true,
                ..base
            };
            assert_eq!(run("t", base), Some(Command::NewTab));
            assert_eq!(run("d", base), Some(Command::SplitRight));
            assert_eq!(run("w", base), Some(Command::CloseTab));
            assert_eq!(run("p", base), Some(Command::Templates));
            assert_eq!(run("pageup", base), Some(Command::PreviousTab));
            assert_eq!(run("pagedown", base), Some(Command::NextTab));
            assert_eq!(run("d", shifted), Some(Command::SplitDown));
            assert_eq!(run("1", base), Some(Command::SelectScreen(1)));
            // The Fn modifier claims nothing.
            assert_eq!(
                run(
                    "d",
                    Modifiers {
                        function: true,
                        ..base
                    }
                ),
                None
            );
            assert_eq!(
                run(
                    "d",
                    Modifiers {
                        control: true,
                        ..base
                    }
                ),
                None
            );
            // Ctrl+C, Ctrl+B and Ctrl+F are the terminal's.
            for name in ["c", "b", "f", "v"] {
                assert_eq!(run(name, ctrl), None, "ctrl+{name}");
            }
            // Zoom by its logical key, also with Shift held for `+`.
            let zoom = if platform.is_mac() { cmd } else { ctrl };
            assert_eq!(run("=", zoom), Some(Command::ZoomIn));
            assert_eq!(run("+", zoom), Some(Command::ZoomIn));
            assert_eq!(
                run(
                    "+",
                    Modifiers {
                        shift: true,
                        ..zoom
                    }
                ),
                Some(Command::ZoomIn)
            );
            assert_eq!(run("-", zoom), Some(Command::ZoomOut));
            assert_eq!(run("0", zoom), Some(Command::ZoomReset));
            assert_eq!(run("=", Modifiers { alt: true, ..zoom }), None);
        }
        // Hosts: Alt+H, and Cmd+Shift+H where the system owns Cmd+H.
        let mac_shift = Modifiers {
            platform: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            lookup(Platform::Linux, &key("h", alt), false, ""),
            Some(Command::Hosts)
        );
        assert_eq!(
            lookup(Platform::Mac, &key("h", mac_shift), false, ""),
            Some(Command::Hosts)
        );
        assert_eq!(lookup(Platform::Mac, &key("h", cmd), false, ""), None);
        // Copy and paste with Ctrl+Shift are the terminal's commands.
        let ctrl_shift = Modifiers {
            control: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            lookup(Platform::Linux, &key("c", ctrl_shift), false, ""),
            Some(Command::Copy)
        );
        assert_eq!(
            lookup(Platform::Linux, &key("space", ctrl_shift), false, ""),
            Some(Command::NewScreen)
        );
    }

    #[test]
    fn shifted_screen_digits_need_physical_shift_and_a_known_layout() {
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        let linux = |keystroke: &Keystroke, shift, layout| {
            lookup(Platform::Linux, keystroke, shift, layout)
        };
        let shifted_alt = Modifiers { shift: true, ..alt };
        assert_eq!(
            linux(&key("1", shifted_alt), true, "unknown"),
            Some(Command::SelectScreen(1))
        );
        assert_eq!(
            linux(&key("&", alt), true, "English (US)"),
            Some(Command::SelectScreen(7))
        );
        assert_eq!(
            linux(&key("&", alt), true, "German"),
            Some(Command::SelectScreen(6))
        );
        assert_eq!(
            linux(&key("/", alt), true, "German"),
            Some(Command::SelectScreen(7))
        );
        assert_eq!(
            linux(&key("(", alt), true, "German"),
            Some(Command::SelectScreen(8))
        );
        assert_eq!(
            linux(&key("(", alt), true, "English (US)"),
            Some(Command::SelectScreen(9))
        );
        assert_eq!(linux(&key("&", alt), false, "German"), None);
        assert_eq!(linux(&key("&", alt), true, "unknown"), None);
        assert_eq!(
            linux(
                &key(
                    "&",
                    Modifiers {
                        control: true,
                        ..alt
                    }
                ),
                true,
                "German"
            ),
            None
        );
        assert_eq!(shifted_digit(&key("0", alt), "German"), None);
    }
}
