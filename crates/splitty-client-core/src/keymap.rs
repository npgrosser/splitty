//! The window's keyboard commands: a registry with default keys per
//! platform, the user's overrides kept in a file of the application, and the
//! effective keymap a window looks its keys up in. UI-free: the window turns
//! its own key events into [`Chord`]s.
mod chord;
mod command;
mod file;
#[cfg(test)]
mod tests;

pub use chord::{Chord, ChordError, Platform, Reserved};
pub use command::{defaults, Category, Command, Direction};
pub use file::KeybindingsFile;

use std::collections::{BTreeMap, HashMap};

/// The user's bindings: a command's list of chords replaces its defaults,
/// and an empty list unbinds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    bindings: BTreeMap<Command, Vec<Chord>>,
    problems: Vec<String>,
}

/// An override that was not taken: one of its chords is plain typing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypingChord(pub Chord);

impl Overrides {
    /// Reads the file form, `{"bindings": {"screen.next": ["ctrl+tab"]}}`.
    /// Unknown ids, chords that do not parse and typing chords are skipped
    /// and reported; the rest is kept.
    pub fn from_json(text: &str) -> Self {
        let mut overrides = Self::default();
        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(error) => {
                overrides.problems.push(format!(
                    "The keybindings file is not valid JSON ({error}); the default keys apply."
                ));
                return overrides;
            }
        };
        let Some(bindings) = value.get("bindings").and_then(|value| value.as_object()) else {
            overrides.problems.push(
                "The keybindings file has no \"bindings\" object; the default keys apply.".into(),
            );
            return overrides;
        };
        for (id, chords) in bindings {
            let Some(command) = Command::from_id(id) else {
                overrides
                    .problems
                    .push(format!("\"{id}\" is not a command; its keys are ignored."));
                continue;
            };
            let Some(chords) = chords.as_array() else {
                overrides.problems.push(format!(
                    "The keys of {} are not a list; the default keys apply.",
                    command.title()
                ));
                continue;
            };
            let mut taken = Vec::new();
            for chord in chords {
                let Some(text) = chord.as_str() else {
                    overrides.problems.push(format!(
                        "A key of {} is not text; it is ignored.",
                        command.title()
                    ));
                    continue;
                };
                match Chord::parse(text) {
                    Ok(chord) if chord.reserved(Platform::current()) == Some(Reserved::Typing) => {
                        overrides.problems.push(typing_problem(&chord, command))
                    }
                    Ok(chord) => {
                        if !taken.contains(&chord) {
                            taken.push(chord);
                        }
                    }
                    Err(error) => overrides.problems.push(format!(
                        "\"{text}\" for {} is not a key combination ({error}); it is ignored.",
                        command.title()
                    )),
                }
            }
            overrides.bindings.insert(command, taken);
        }
        overrides
    }

    /// The file form, chords written as `platform` names its keys.
    pub fn to_json(&self, platform: Platform) -> String {
        let bindings: serde_json::Map<String, serde_json::Value> = self
            .bindings
            .iter()
            .map(|(command, chords)| {
                (
                    command.id(),
                    chords
                        .iter()
                        .map(|chord| serde_json::Value::from(chord.text(platform)))
                        .collect(),
                )
            })
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({ "bindings": bindings }))
            .expect("a JSON object")
    }

    /// Bind `command` to `chords` instead of its defaults; an empty list
    /// unbinds it. A typing chord is refused and nothing changes.
    pub fn set(&mut self, command: Command, chords: Vec<Chord>) -> Result<(), TypingChord> {
        if let Some(chord) = chords
            .iter()
            .find(|chord| chord.reserved(Platform::current()) == Some(Reserved::Typing))
        {
            return Err(TypingChord(chord.clone()));
        }
        let mut taken: Vec<Chord> = Vec::new();
        for chord in chords {
            if !taken.contains(&chord) {
                taken.push(chord);
            }
        }
        self.bindings.insert(command, taken);
        Ok(())
    }

    /// Give `command` its defaults again.
    pub fn reset(&mut self, command: Command) {
        self.bindings.remove(&command);
    }

    /// The chords the user gave `command`, if any.
    pub fn get(&self, command: Command) -> Option<&[Chord]> {
        self.bindings.get(&command).map(Vec::as_slice)
    }

    /// Every overridden command with its chords, in registry order.
    pub fn iter(&self) -> impl Iterator<Item = (Command, &[Chord])> {
        self.bindings
            .iter()
            .map(|(command, chords)| (*command, chords.as_slice()))
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// What was skipped while reading, each as a sentence.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }
}

fn typing_problem(chord: &Chord, command: Command) -> String {
    format!(
        "{} for {} is plain typing and cannot be a shortcut; it is ignored.",
        chord.label(Platform::current()),
        command.title()
    )
}

/// The effective bindings of a window: the defaults of its platform with the
/// user's overrides applied.
#[derive(Clone, Debug)]
pub struct Keymap {
    platform: Platform,
    bindings: BTreeMap<Command, Vec<Chord>>,
    index: HashMap<Chord, Command>,
    problems: Vec<String>,
}

impl Keymap {
    /// An override replaces a command's defaults. A chord an override binds
    /// is taken from any command that has it by default; two overrides with
    /// the same chord leave it with the first command in registry order.
    pub fn new(platform: Platform, overrides: &Overrides) -> Self {
        let mut problems = overrides.problems.clone();
        let mut bindings = BTreeMap::new();
        let mut claimed: HashMap<Chord, Command> = HashMap::new();
        for (command, chords) in overrides.iter() {
            let mut kept = Vec::new();
            for chord in chords {
                match claimed.get(chord) {
                    Some(first) => problems.push(format!(
                        "{} is bound to both {} and {}; it stays with {}.",
                        chord.label(platform),
                        first.title(),
                        command.title(),
                        first.title()
                    )),
                    None => {
                        claimed.insert(chord.clone(), command);
                        kept.push(chord.clone());
                    }
                }
            }
            bindings.insert(command, kept);
        }
        for command in Command::all() {
            bindings.entry(command).or_insert_with(|| {
                defaults(platform, command)
                    .into_iter()
                    .filter(|chord| !claimed.contains_key(chord))
                    .collect()
            });
        }
        let mut index = HashMap::new();
        for (command, chords) in &bindings {
            for chord in chords {
                index.entry(chord.clone()).or_insert(*command);
            }
        }
        Self {
            platform,
            bindings,
            index,
            problems,
        }
    }

    /// The defaults of `platform`.
    pub fn defaults(platform: Platform) -> Self {
        Self::new(platform, &Overrides::default())
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// The command `chord` runs.
    pub fn lookup(&self, chord: &Chord) -> Option<Command> {
        self.index.get(chord).copied()
    }

    /// The chords that run `command`, the first one the one shown.
    pub fn bindings(&self, command: Command) -> &[Chord] {
        self.bindings.get(&command).map_or(&[], Vec::as_slice)
    }

    /// The label of the first chord of `command`; none when it is unbound.
    pub fn label(&self, command: Command) -> Option<String> {
        self.bindings(command)
            .first()
            .map(|chord| chord.label(self.platform))
    }

    /// Whether `command` has exactly its default chords.
    pub fn is_default(&self, command: Command) -> bool {
        self.bindings(command) == defaults(self.platform, command).as_slice()
    }

    /// What the overrides could not do, each as a sentence for the settings:
    /// unknown ids, unreadable or typing chords and conflicts.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }
}
