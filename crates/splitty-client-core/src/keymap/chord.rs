//! A key with its modifiers, in the form a keybindings file writes and the
//! form the interface shows.
use std::fmt;

/// The operating system a keymap is for: it decides what `mod` means, how
/// the platform key is named and how chords are labeled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Mac,
    Windows,
    Linux,
}

impl Platform {
    /// The platform this build runs on.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    pub fn is_mac(self) -> bool {
        self == Self::Mac
    }
}

/// One key combination. `platform` is Cmd on macOS and the Super/Windows key
/// elsewhere; `key` is GPUI's lowercase key name (`a`, `1`, `tab`, `space`,
/// `pageup`, `left`, `f11`, `,`, `+`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
    pub key: String,
}

/// Why a written chord was not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordError(String);

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ChordError {}

/// Chords a binding must not take, or should take only knowingly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reserved {
    /// Plain input: a binding would make the key impossible to type.
    Typing,
    /// A key terminal programs commonly use; binding it takes it from them.
    Program,
}

/// The named keys besides single characters.
const NAMED_KEYS: &[&str] = &[
    "space",
    "tab",
    "enter",
    "escape",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "up",
    "down",
    "left",
    "right",
];

/// Keys that edit or move in text: no binding takes them without a modifier.
const EDITING_KEYS: &[&str] = &[
    "enter",
    "tab",
    "escape",
    "backspace",
    "delete",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
];

/// The keys that scroll a terminal's history with Shift.
const HISTORY_KEYS: &[&str] = &["home", "end", "pageup", "pagedown"];

fn function_key(name: &str) -> bool {
    name.strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (1..=24).contains(&number))
        && !name.starts_with("f0")
}

/// A key that types a character: one character, or Space.
fn character_key(name: &str) -> bool {
    name == "space" || name.chars().count() == 1
}

fn key_alias(name: &str) -> &str {
    match name {
        "plus" => "+",
        "esc" => "escape",
        "return" => "enter",
        "del" => "delete",
        "pgup" => "pageup",
        "pgdn" => "pagedown",
        "arrowup" => "up",
        "arrowdown" => "down",
        "arrowleft" => "left",
        "arrowright" => "right",
        other => other,
    }
}

impl Chord {
    /// `key` without modifiers.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into().to_lowercase(),
            ..Self::default()
        }
    }

    pub fn ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }

    pub fn alt(mut self) -> Self {
        self.alt = true;
        self
    }

    pub fn shift(mut self) -> Self {
        self.shift = true;
        self
    }

    pub fn platform(mut self) -> Self {
        self.platform = true;
        self
    }

    /// Whether any modifier is held.
    pub fn modified(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.platform
    }

    /// Reads the file form, `ctrl+shift+tab`: modifiers `ctrl`, `alt`,
    /// `shift` and the platform key as `cmd`, `super`, `win` or `meta`, then
    /// one key; case does not matter, and the `+` key is written `plus`.
    pub fn parse(text: &str) -> Result<Self, ChordError> {
        let text = text.trim().to_lowercase();
        if text.is_empty() {
            return Err(ChordError("an empty key combination".into()));
        }
        // `ctrl++` names the `+` key as well as `ctrl+plus` does.
        let (modifiers, key) = if text == "+" {
            ("", "+")
        } else if let Some(modifiers) = text.strip_suffix("++") {
            (modifiers, "+")
        } else {
            match text.rsplit_once('+') {
                Some((modifiers, key)) => (modifiers, key),
                None => ("", text.as_str()),
            }
        };
        let key = key_alias(key.trim());
        if key.is_empty() {
            return Err(ChordError(format!("\"{text}\" names no key")));
        }
        if !(NAMED_KEYS.contains(&key) || function_key(key) || key.chars().count() == 1) {
            return Err(ChordError(format!("\"{key}\" is not a key")));
        }
        let mut chord = Chord::new(key);
        for name in modifiers.split('+').filter(|name| !name.is_empty()) {
            let held = match name.trim() {
                "ctrl" | "control" => &mut chord.ctrl,
                "alt" | "option" | "opt" => &mut chord.alt,
                "shift" => &mut chord.shift,
                "cmd" | "command" | "super" | "win" | "meta" => &mut chord.platform,
                other => return Err(ChordError(format!("\"{other}\" is not a modifier"))),
            };
            if *held {
                return Err(ChordError(format!("\"{name}\" is written twice")));
            }
            *held = true;
        }
        Ok(chord)
    }

    /// The file form with the platform key named as `platform` names it
    /// (`cmd` on macOS, `super` elsewhere), modifiers in a fixed order.
    pub fn text(&self, platform: Platform) -> String {
        let mut text = String::new();
        for (held, name) in [
            (self.ctrl, "ctrl"),
            (self.alt, "alt"),
            (self.shift, "shift"),
            (
                self.platform,
                if platform.is_mac() { "cmd" } else { "super" },
            ),
        ] {
            if held {
                text.push_str(name);
                text.push('+');
            }
        }
        text.push_str(if self.key == "+" { "plus" } else { &self.key });
        text
    }

    /// What the interface shows: `Ctrl+Shift+Tab` and `Alt+T`, or on macOS
    /// the symbols `⌃⌥⇧⌘` before the key.
    pub fn label(&self, platform: Platform) -> String {
        let key = key_label(&self.key);
        if platform.is_mac() {
            let mut label = String::new();
            for (held, symbol) in [
                (self.ctrl, "⌃"),
                (self.alt, "⌥"),
                (self.shift, "⇧"),
                (self.platform, "⌘"),
            ] {
                if held {
                    label.push_str(symbol);
                }
            }
            label.push_str(&key);
            return label;
        }
        let mut label = String::new();
        for (held, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (
                self.platform,
                if platform == Platform::Windows {
                    "Win"
                } else {
                    "Super"
                },
            ),
        ] {
            if held {
                label.push_str(name);
                label.push('+');
            }
        }
        label.push_str(&key);
        label
    }

    /// Whether a binding must not take this chord (`Typing`), or takes it
    /// from terminal programs (`Program`).
    pub fn reserved(&self, platform: Platform) -> Option<Reserved> {
        let key = self.key.as_str();
        // Shift alone does not make a key a command's: Shift with Tab, an
        // arrow or Enter is input as much as the key itself. The history
        // keys are the exception, which terminals take with Shift.
        let typed = !self.ctrl && !self.alt && !self.platform;
        let history = self.shift && HISTORY_KEYS.contains(&key);
        if typed && (character_key(key) || EDITING_KEYS.contains(&key)) && !history {
            return Some(Reserved::Typing);
        }
        let letter = key.len() == 1 && key.chars().all(|c| c.is_ascii_alphabetic());
        let punctuation = key.len() == 1 && key.chars().all(|c| c.is_ascii_punctuation());
        let only_ctrl = self.ctrl && !self.alt && !self.shift && !self.platform;
        let ctrl_shift = self.ctrl && self.shift && !self.alt && !self.platform;
        let only_alt = self.alt && !self.ctrl && !self.shift && !self.platform;
        if (only_ctrl && (letter || punctuation))
            || (ctrl_shift && (key == "c" || key == "v"))
            || (only_alt && letter && !platform.is_mac())
        {
            return Some(Reserved::Program);
        }
        None
    }
}

fn key_label(key: &str) -> String {
    match key {
        "space" => "Space".into(),
        "tab" => "Tab".into(),
        "enter" => "Enter".into(),
        "escape" => "Esc".into(),
        "backspace" => "Backspace".into(),
        "delete" => "Delete".into(),
        "insert" => "Insert".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        other => other.to_uppercase(),
    }
}

/// The canonical file form for the platform this build runs on.
impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text(Platform::current()))
    }
}
