//! The Keyboard page of the settings: every command with its keys, to read
//! and to rebind. It reads the keymap in effect and changes the bindings
//! through the workspace, which applies them at once and writes the file.
use super::settings::SettingsView;
use super::*;
use gpui_kit::base::Selectable as _;
use keymap::{defaults, Category, Chord, Keymap, Overrides, Platform, Reserved};

/// Key names of modifier presses, which recording waits past.
const MODIFIER_KEYS: &[&str] = &[
    "shift", "control", "ctrl", "alt", "option", "platform", "cmd", "super", "meta", "function",
    "fn", "capslock",
];

/// One change the page makes to the bindings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BindingChange {
    /// The command gets this one chord in place of its keys.
    Record(KeyCommand, Chord),
    /// The command keeps no key.
    Remove(KeyCommand),
    /// The command gets its default keys back.
    Reset(KeyCommand),
    /// Every command gets its default keys back.
    ResetAll,
}

impl BindingChange {
    /// The command whose row shows the outcome.
    fn command(&self) -> Option<KeyCommand> {
        match self {
            Self::Record(command, _) | Self::Remove(command) | Self::Reset(command) => {
                Some(*command)
            }
            Self::ResetAll => None,
        }
    }
}

/// A line under a row about its last change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Notice {
    /// The chord was taken from another command.
    Taken { from: KeyCommand, key: String },
    /// The chord is one terminal programs commonly use.
    Program { key: String },
    /// The chord is plain typing and was refused.
    Typing { key: String },
    /// The key cannot be written as a shortcut and was refused.
    Unreadable { key: String },
}

impl Notice {
    fn text(&self) -> String {
        match self {
            Self::Taken { from, key } => {
                format!("{key} was taken from {}.", from.title())
            }
            Self::Program { key } => format!(
                "{key} is commonly used by programs in the terminal; they no longer receive it."
            ),
            Self::Typing { key } => format!("{key} is plain typing and cannot be a shortcut."),
            Self::Unreadable { key } => format!("{key} cannot be used as a shortcut."),
        }
    }

    fn refused(&self) -> bool {
        matches!(self, Self::Typing { .. } | Self::Unreadable { .. })
    }
}

/// The page's own state; the bindings themselves are the keymap in effect.
pub(super) struct KeyboardPage {
    workspace: WeakEntity<Workspace>,
    search: Entity<InputState>,
    /// The command whose row waits for the next chord.
    recording: Option<KeyCommand>,
    /// The lines under the row changed last, until the next change.
    notices: Option<(KeyCommand, Vec<Notice>)>,
    /// Why the last change could not be written to the file.
    save_error: Option<String>,
    /// "Reset all" waits for its confirmation.
    confirm_reset_all: bool,
}

impl KeyboardPage {
    pub(super) fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<SettingsView>,
    ) -> (Self, Subscription) {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search commands and keys…"));
        let changes = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let page = Self {
            workspace,
            search,
            recording: None,
            notices: None,
            save_error: None,
            confirm_reset_all: false,
        };
        (page, changes)
    }

    /// The command whose row waits for a chord.
    pub(super) fn recording(&self) -> Option<KeyCommand> {
        self.recording
    }

    pub(super) fn cancel_recording(&mut self) {
        self.recording = None;
    }

    pub(super) fn focus_search(&self, window: &mut Window, cx: &mut App) {
        self.search.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// Take the outcome of a change: the notes of its row, and whether the
    /// file was written (`None` when nothing was changed).
    fn settle(
        &mut self,
        command: Option<KeyCommand>,
        notices: Vec<Notice>,
        saved: Option<std::io::Result<()>>,
    ) {
        self.recording = None;
        self.confirm_reset_all = false;
        self.notices = command
            .filter(|_| !notices.is_empty())
            .map(|command| (command, notices));
        match saved {
            Some(Ok(())) => self.save_error = None,
            Some(Err(error)) => {
                self.save_error = Some(format!(
                    "The keybindings file could not be written ({error}); \
                     the change applies until Splitty quits."
                ))
            }
            None => {}
        }
    }

    /// The page in the settings' content area.
    pub(super) fn render(&self, cx: &mut Context<SettingsView>) -> AnyElement {
        let keymap = crate::keys::keymap(cx);
        let appearance = self
            .workspace
            .upgrade()
            .map(|workspace| workspace.read(cx).appearance.clone())
            .unwrap_or_else(|| NativeAppearance::fallback(&cx.theme().font_family));
        let query = self.search.read(cx).value().to_string();
        let muted = cx.theme().muted_foreground;
        let mut page = div().flex().flex_col().gap_4().w_full();

        let mut messages = div().flex().flex_col().gap_1();
        for problem in keymap.problems() {
            messages = messages.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().warning)
                    .child(problem.clone()),
            );
        }
        if let Some(error) = &self.save_error {
            messages = messages.child(
                div()
                    .when(cfg!(all(test, feature = "ui-test-support")), |line| {
                        line.debug_selector(|| "keyboard-save-error".into())
                    })
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone()),
            );
        }
        page = page.child(messages);

        let all_default = KeyCommand::all()
            .into_iter()
            .all(|command| keymap.is_default(command));
        let reset_all = Button::new("keyboard-reset-all")
            .when(cfg!(all(test, feature = "ui-test-support")), |button| {
                button.debug_selector(|| "keyboard-reset-all".into())
            })
            .label("Reset All…")
            .ghost()
            .small()
            .disabled(all_default || self.confirm_reset_all)
            .on_click(cx.listener(|this, _, _, cx| {
                this.keyboard.recording = None;
                this.keyboard.confirm_reset_all = true;
                cx.notify();
            }));
        page = page.child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .when(cfg!(all(test, feature = "ui-test-support")), |field| {
                            field.debug_selector(|| "keyboard-search".into())
                        })
                        .child(gpui_kit::component::input::Input::new(&self.search).small()),
                )
                .child(div().flex_none().child(reset_all)),
        );
        if self.confirm_reset_all {
            let workspace = self.workspace.clone();
            page = page.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .child("Every command gets its default keys back."),
                    )
                    .child(
                        Button::new("keyboard-reset-all-confirm")
                            .when(cfg!(all(test, feature = "ui-test-support")), |button| {
                                button.debug_selector(|| "keyboard-reset-all-confirm".into())
                            })
                            .label("Reset All")
                            .danger()
                            .small()
                            .on_click(move |_, _, cx| {
                                let _ = workspace.update(cx, |workspace, cx| {
                                    workspace.change_key_binding(BindingChange::ResetAll, cx)
                                });
                            }),
                    )
                    .child(
                        Button::new("keyboard-reset-all-cancel")
                            .label("Cancel")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.keyboard.confirm_reset_all = false;
                                cx.notify();
                            })),
                    ),
            );
        }

        let groups = keyboard_groups(&keymap, &query);
        if groups.is_empty() {
            page = page.child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("No command matches."),
            );
        }
        for (category, commands) in groups {
            let mut group = div().flex().flex_col().gap_1().child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child(category.title()),
            );
            for command in commands {
                group = group.child(self.render_row(command, &keymap, &appearance, cx));
            }
            page = page.child(group);
        }
        page.into_any_element()
    }

    fn render_row(
        &self,
        command: KeyCommand,
        keymap: &Keymap,
        appearance: &NativeAppearance,
        cx: &mut Context<SettingsView>,
    ) -> AnyElement {
        let id = command.id();
        let test = cfg!(all(test, feature = "ui-test-support"));
        let platform = keymap.platform();
        let recording = self.recording == Some(command);
        let bindings = keymap.bindings(command);
        let mut keys = div().flex().flex_none().items_center().gap_1();
        if recording {
            keys = keys.child(
                key_cap(appearance, "Press a key…")
                    .bg(rgb(appearance.active))
                    .text_color(rgb(appearance.active_text)),
            );
        } else if bindings.is_empty() {
            keys = keys.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No key"),
            );
        }
        if !recording {
            for (index, chord) in bindings.iter().enumerate() {
                let selector = format!("keyboard-key-{id}-{index}");
                keys = keys.child(
                    key_cap(appearance, chord.label(platform))
                        .when(test, |cap| cap.debug_selector(move || selector)),
                );
            }
        }
        let workspace = self.workspace.clone();
        let remove_workspace = workspace.clone();
        let label = command.title();
        let record_id = format!("keyboard-record-{id}");
        let remove_id = format!("keyboard-remove-{id}");
        let reset_id = format!("keyboard-reset-{id}");
        let actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .child(
                Button::new(SharedString::from(record_id.clone()))
                    .when(test, |button| button.debug_selector(move || record_id))
                    // The keys themselves are the control that records.
                    .child(keys)
                    .tooltip(if recording {
                        "Cancel"
                    } else {
                        "Record a new key"
                    })
                    .accessibility_label(format!("Record a key for {label}"))
                    .selected(recording)
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let page = &mut this.keyboard;
                        page.confirm_reset_all = false;
                        page.recording = (page.recording != Some(command)).then_some(command);
                        cx.notify();
                    })),
            )
            .child(
                Button::new(SharedString::from(remove_id.clone()))
                    .when(test, |button| button.debug_selector(move || remove_id))
                    .icon(IconName::Close)
                    .tooltip("Remove the key")
                    .accessibility_label(format!("Remove the key of {label}"))
                    .ghost()
                    .xsmall()
                    .disabled(bindings.is_empty())
                    .on_click(move |_, _, cx| {
                        let _ = remove_workspace.update(cx, |workspace, cx| {
                            workspace.change_key_binding(BindingChange::Remove(command), cx)
                        });
                    }),
            )
            .child(
                Button::new(SharedString::from(reset_id.clone()))
                    .when(test, |button| button.debug_selector(move || reset_id))
                    .icon(IconName::Undo2)
                    .tooltip("Reset to the default")
                    .accessibility_label(format!("Reset the keys of {label}"))
                    .ghost()
                    .xsmall()
                    .disabled(keymap.is_default(command))
                    .on_click(move |_, _, cx| {
                        let _ = workspace.update(cx, |workspace, cx| {
                            workspace.change_key_binding(BindingChange::Reset(command), cx)
                        });
                    }),
            );
        let row_id = format!("keyboard-row-{id}");
        let mut row = div()
            .when(test, |row| row.debug_selector(move || row_id))
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_sm()
                            .child(command.title()),
                    )
                    .child(actions),
            );
        if recording {
            row = row.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Press the new key combination; Esc cancels."),
            );
        }
        if let Some((_, notices)) = self
            .notices
            .as_ref()
            .filter(|(changed, _)| *changed == command)
        {
            for (index, notice) in notices.iter().enumerate() {
                let selector = format!("keyboard-notice-{id}-{index}");
                row = row.child(
                    div()
                        .when(test, |line| line.debug_selector(move || selector))
                        .text_sm()
                        .text_color(if notice.refused() {
                            cx.theme().danger
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(notice.text()),
                );
            }
        }
        row.into_any_element()
    }

    /// The lines under `command`'s row.
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn notices(&self, command: KeyCommand) -> &[Notice] {
        self.notices
            .as_ref()
            .filter(|(changed, _)| *changed == command)
            .map_or(&[], |(_, notices)| notices.as_slice())
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn save_error(&self) -> Option<&str> {
        self.save_error.as_deref()
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn search(&self) -> &Entity<InputState> {
        &self.search
    }
}

/// A key as a small raised chip in the theme's look.
fn key_cap(appearance: &NativeAppearance, label: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .edged(appearance, Relief::Raised)
        .rounded(px(appearance.corner(4.)))
        .bg(rgb(appearance.raised))
        .text_color(rgb(appearance.text))
        .text_xs()
        .whitespace_nowrap()
        .child(label.into())
}

/// Whether `command` matches the search `query`: by its title, its
/// identifier, or one of its keys as written in the file or as shown.
fn command_matches(command: KeyCommand, keymap: &Keymap, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let platform = keymap.platform();
    command.title().to_lowercase().contains(&query)
        || command.id().contains(&query)
        || keymap.bindings(command).iter().any(|chord| {
            chord.text(platform).contains(&query)
                || chord.label(platform).to_lowercase().contains(&query)
        })
}

/// The commands the page lists for `query`, grouped by category, both in
/// the registry's order.
pub(super) fn keyboard_groups(keymap: &Keymap, query: &str) -> Vec<(Category, Vec<KeyCommand>)> {
    let mut groups: Vec<(Category, Vec<KeyCommand>)> = Vec::new();
    for command in KeyCommand::all() {
        if !command_matches(command, keymap, query) {
            continue;
        }
        match groups.last_mut() {
            Some((category, commands)) if *category == command.category() => commands.push(command),
            _ => groups.push((command.category(), vec![command])),
        }
    }
    groups
}

/// The overrides after `change` and the notes for its row, or why it is
/// refused. A chord another command holds is taken from it, out of its
/// override where it has one; what was skipped while reading the file is
/// not carried over, since the file is written anew.
fn changed_overrides(
    overrides: &Overrides,
    keymap: &Keymap,
    change: &BindingChange,
) -> Result<(Overrides, Vec<Notice>), Notice> {
    let platform = keymap.platform();
    let mut next = Overrides::default();
    for (command, chords) in overrides.iter() {
        // Kept overrides hold no typing chord, so none is refused here.
        let _ = next.set(command, chords.to_vec());
    }
    let mut notices = Vec::new();
    let mut take = |next: &mut Overrides, command: KeyCommand, chord: &Chord| {
        let Some(holder) = keymap.lookup(chord).filter(|holder| *holder != command) else {
            return;
        };
        if let Some(chords) = next.get(holder) {
            let kept = chords
                .iter()
                .filter(|held| *held != chord)
                .cloned()
                .collect();
            let _ = next.set(holder, kept);
        }
        notices.push(Notice::Taken {
            from: holder,
            key: chord.label(platform),
        });
    };
    match change {
        BindingChange::Record(command, chord) => {
            let key = chord.label(platform);
            let reserved = chord.reserved(platform);
            if reserved == Some(Reserved::Typing) {
                return Err(Notice::Typing { key });
            }
            take(&mut next, *command, chord);
            next.set(*command, vec![chord.clone()])
                .map_err(|_| Notice::Typing { key: key.clone() })?;
            if reserved == Some(Reserved::Program) {
                notices.push(Notice::Program { key });
            }
        }
        BindingChange::Remove(command) => {
            let _ = next.set(*command, Vec::new());
        }
        BindingChange::Reset(command) => {
            next.reset(*command);
            for chord in defaults(platform, *command) {
                take(&mut next, *command, &chord);
            }
        }
        BindingChange::ResetAll => next = Overrides::default(),
    }
    Ok((next, notices))
}

/// The chord a recorded key press stands for: `None` for a modifier alone,
/// which recording waits past, or the reason the key cannot be bound.
fn recorded_chord(chord: Chord, platform: Platform) -> Option<Result<Chord, Notice>> {
    if MODIFIER_KEYS.contains(&chord.key.as_str()) || chord.key.is_empty() {
        return None;
    }
    // Only a chord the file can hold is taken.
    if Chord::parse(&chord.text(platform)).ok().as_ref() != Some(&chord) {
        return Some(Err(Notice::Unreadable {
            key: chord.label(platform),
        }));
    }
    Some(Ok(chord))
}

impl Workspace {
    /// Show the Keyboard page of the settings.
    pub(super) fn show_keyboard_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_settings(window, cx);
        if self.destination == Destination::Settings {
            self.settings
                .update(cx, |view, cx| view.show_keyboard(window, cx));
        }
    }

    /// While a row of the Keyboard page waits for a chord, the key press is
    /// its chord: it runs no command and reaches no terminal. Escape cancels
    /// and a modifier alone is waited past. True when the press was taken.
    pub(super) fn record_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.destination != Destination::Settings {
            return false;
        }
        let Some(command) = self.settings.read(cx).keyboard.recording() else {
            return false;
        };
        cx.stop_propagation();
        let keystroke = &event.keystroke;
        if event.is_held {
            return true;
        }
        if keystroke.key == "escape" && !keystroke.modifiers.modified() {
            self.settings.update(cx, |view, cx| {
                view.keyboard.cancel_recording();
                cx.notify();
            });
            return true;
        }
        let Some(chord) = crate::keys::chord(
            keystroke,
            window.modifiers().shift,
            cx.keyboard_layout().id(),
        ) else {
            return true;
        };
        match recorded_chord(chord, crate::keys::platform(cx)) {
            None => {}
            Some(Ok(chord)) => self.change_key_binding(BindingChange::Record(command, chord), cx),
            Some(Err(refusal)) => self.settings.update(cx, |view, cx| {
                view.keyboard.settle(Some(command), vec![refusal], None);
                cx.notify();
            }),
        }
        true
    }

    /// Make `change` to the user's bindings: apply it at once, write the
    /// file, and tell the Keyboard page how it went.
    pub(super) fn change_key_binding(&mut self, change: BindingChange, cx: &mut Context<Self>) {
        let keymap = crate::keys::keymap(cx);
        let (notices, saved) = match changed_overrides(&self.key_overrides, &keymap, &change) {
            Ok((next, notices)) => (notices, Some(self.apply_key_overrides(next, cx))),
            Err(refusal) => (vec![refusal], None),
        };
        self.settings.update(cx, |view, cx| {
            view.keyboard.settle(change.command(), notices, saved);
            cx.notify();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{changed_overrides, keyboard_groups, recorded_chord, BindingChange, Notice};
    use splitty_client_core::keymap::{
        defaults, Category, Chord, Command, Keymap, Overrides, Platform,
    };

    // Level: unit.
    #[test]
    fn the_page_lists_every_command_by_category_and_filters_by_title_id_and_key() {
        let linux = Keymap::defaults(Platform::Linux);
        let listed: Vec<Command> = keyboard_groups(&linux, "")
            .into_iter()
            .flat_map(|(_, commands)| commands)
            .collect();
        assert_eq!(listed, Command::all());
        let categories: Vec<Category> = keyboard_groups(&linux, "")
            .into_iter()
            .map(|(category, _)| category)
            .collect();
        let mut sorted = categories.clone();
        sorted.dedup();
        assert_eq!(categories, sorted, "each category is one group");

        let commands = |keymap: &Keymap, query: &str| -> Vec<Command> {
            keyboard_groups(keymap, query)
                .into_iter()
                .flat_map(|(_, commands)| commands)
                .collect()
        };
        assert_eq!(commands(&linux, "nothing here"), Vec::<Command>::new());
        assert_eq!(commands(&linux, "Split Down"), [Command::SplitDown]);
        assert_eq!(commands(&linux, "tab.move-left"), [Command::MoveTabLeft]);
        // By key, as written in the file and as shown.
        assert_eq!(
            commands(&linux, "ctrl+tab"),
            [Command::NextScreen],
            "ctrl+shift+tab does not contain ctrl+tab"
        );
        assert!(commands(&linux, "Alt+Shift+D").contains(&Command::SplitDown));
        let mac = Keymap::defaults(Platform::Mac);
        assert!(commands(&mac, "shift+cmd+d").contains(&Command::SplitDown));
        assert!(commands(&mac, "⇧⌘D").contains(&Command::SplitDown));
        // "Tab" finds the tab commands by title and the screens by key.
        let tab = commands(&linux, "Tab");
        assert!(tab.contains(&Command::NewTab));
        assert!(tab.contains(&Command::PreviousScreen));
    }

    // Level: unit.
    #[test]
    fn every_default_chord_belongs_to_one_command() {
        for platform in [Platform::Linux, Platform::Windows, Platform::Mac] {
            let mut seen: Vec<(Chord, Command)> = Vec::new();
            for command in Command::all() {
                for chord in defaults(platform, command) {
                    if let Some((_, other)) = seen.iter().find(|(held, _)| *held == chord) {
                        panic!("{chord:?} is a default of {other:?} and {command:?}");
                    }
                    seen.push((chord, command));
                }
            }
        }
    }

    // Level: unit.
    #[test]
    fn recording_takes_a_chord_from_its_holder_and_reset_takes_the_defaults_back() {
        let platform = Platform::Linux;
        let keymap = Keymap::defaults(platform);
        let maximize = Chord::new("m").alt();
        // A default chord of another command: the keymap takes it.
        let (overrides, notices) = changed_overrides(
            &Overrides::default(),
            &keymap,
            &BindingChange::Record(Command::NewTab, maximize.clone()),
        )
        .unwrap();
        assert_eq!(
            overrides.get(Command::NewTab),
            Some(&[maximize.clone()][..])
        );
        assert_eq!(overrides.get(Command::MaximizePane), None);
        // Alt with a letter is also a key terminal programs use.
        assert_eq!(
            notices,
            [
                Notice::Taken {
                    from: Command::MaximizePane,
                    key: "Alt+M".into()
                },
                Notice::Program {
                    key: "Alt+M".into()
                }
            ]
        );
        // A chord of another override leaves that override.
        let keymap = Keymap::new(platform, &overrides);
        let (overrides, notices) = changed_overrides(
            &overrides,
            &keymap,
            &BindingChange::Record(Command::SplitRight, maximize.clone()),
        )
        .unwrap();
        assert_eq!(overrides.get(Command::NewTab), Some(&[][..]));
        assert_eq!(
            notices[0],
            Notice::Taken {
                from: Command::NewTab,
                key: "Alt+M".into()
            }
        );
        let keymap = Keymap::new(platform, &overrides);
        assert!(keymap.problems().is_empty());
        assert_eq!(keymap.lookup(&maximize), Some(Command::SplitRight));
        // Resetting the command that lost its default takes it back.
        let (overrides, notices) = changed_overrides(
            &overrides,
            &keymap,
            &BindingChange::Reset(Command::MaximizePane),
        )
        .unwrap();
        assert_eq!(overrides.get(Command::SplitRight), Some(&[][..]));
        assert_eq!(notices.len(), 1);
        let keymap = Keymap::new(platform, &overrides);
        assert!(keymap.is_default(Command::MaximizePane));
        // Remove stores an empty list; Reset All empties the file.
        let (removed, _) = changed_overrides(
            &Overrides::default(),
            &keymap,
            &BindingChange::Remove(Command::Hosts),
        )
        .unwrap();
        assert_eq!(removed.get(Command::Hosts), Some(&[][..]));
        let (all, _) = changed_overrides(&removed, &keymap, &BindingChange::ResetAll).unwrap();
        assert!(all.is_empty());
    }

    // Level: unit.
    #[test]
    fn typing_is_refused_program_keys_are_noted_and_problems_of_the_file_are_dropped() {
        let platform = Platform::Linux;
        let keymap = Keymap::defaults(platform);
        let read =
            Overrides::from_json(r#"{"bindings":{"no.such":["ctrl+x"],"tab.new":["alt+n"]}}"#);
        assert_eq!(read.problems().len(), 1);
        assert_eq!(
            changed_overrides(
                &read,
                &keymap,
                &BindingChange::Record(Command::NewScreen, Chord::new("a").shift())
            ),
            Err(Notice::Typing {
                key: "Shift+A".into()
            })
        );
        let (overrides, notices) = changed_overrides(
            &read,
            &keymap,
            &BindingChange::Record(Command::NewScreen, Chord::new("b").ctrl()),
        )
        .unwrap();
        assert_eq!(
            notices,
            [Notice::Program {
                key: "Ctrl+B".into()
            }]
        );
        assert!(overrides.problems().is_empty());
        assert_eq!(
            overrides.get(Command::NewTab),
            Some(&[Chord::new("n").alt()][..])
        );
    }

    // Level: unit.
    #[test]
    fn a_recorded_press_waits_past_modifiers_and_refuses_keys_the_file_cannot_hold() {
        let platform = Platform::Linux;
        assert_eq!(recorded_chord(Chord::new("shift").shift(), platform), None);
        assert_eq!(recorded_chord(Chord::new("control").ctrl(), platform), None);
        let chord = Chord::new("n").ctrl().shift();
        assert_eq!(recorded_chord(chord.clone(), platform), Some(Ok(chord)));
        let plus = Chord::new("+").ctrl();
        assert_eq!(recorded_chord(plus.clone(), platform), Some(Ok(plus)));
        assert!(matches!(
            recorded_chord(Chord::new("menu").alt(), platform),
            Some(Err(Notice::Unreadable { .. }))
        ));
    }
}
