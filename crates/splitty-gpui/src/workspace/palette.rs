//! The command palette: one centered panel over the window, opened by a
//! shortcut. It filters the window's commands; choosing one runs it as its
//! own control or shortcut would. Every command's keys are listed and
//! changed on the Keyboard page of the settings.
use super::*;

/// What a palette row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaletteCommand {
    Run(KeyCommand),
}

pub(super) struct PaletteEntry {
    pub(super) label: String,
    pub(super) shortcut: Option<String>,
    pub(super) command: PaletteCommand,
}

/// Whether the palette lists `command`: every command but its own, so one
/// without a key can still be run.
fn listed(command: KeyCommand) -> bool {
    command != KeyCommand::CommandPalette
}

/// Every command of the window in the registry's order, with its key in
/// `keymap`.
pub(super) fn palette_entries(keymap: &keymap::Keymap) -> Vec<PaletteEntry> {
    KeyCommand::all()
        .into_iter()
        .filter(|command| listed(*command))
        .map(|command| PaletteEntry {
            label: command.title(),
            shortcut: keymap.label(command),
            command: PaletteCommand::Run(command),
        })
        .collect()
}

/// The entries whose label contains every word of the query.
pub(super) fn filter_entries(entries: Vec<PaletteEntry>, query: &str) -> Vec<PaletteEntry> {
    let query = query.to_lowercase();
    let words: Vec<_> = query.split_whitespace().collect();
    entries
        .into_iter()
        .filter(|entry| {
            let label = entry.label.to_lowercase();
            words.iter().all(|word| label.contains(word))
        })
        .collect()
}

pub(super) enum Palette {
    Commands {
        input: Entity<InputState>,
        selected: usize,
        scroll: ScrollHandle,
        _changes: Subscription,
    },
}

const PANEL_WIDTH: f32 = 440.;
pub(super) const PANEL_TOP: f32 = 0.16;
const LIST_HEIGHT: f32 = 320.;

impl Workspace {
    fn palette_admitted(&self, cx: &App) -> bool {
        self.rename.is_none()
            && self.pending_close.is_none()
            && self.pending_template_apply.is_none()
            && !cx.has_active_drag()
    }

    pub(super) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.palette, Some(Palette::Commands { .. })) {
            self.close_palette(window, cx);
            return;
        }
        if !self.palette_admitted(cx) {
            return;
        }
        self.drop_drawer_popover(cx);
        self.profile_picker = None;
        self.tab_context_menu = None;
        self.terminal_menu = None;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a command…"));
        let changes = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                if let Some(Palette::Commands { selected, .. }) = &mut this.palette {
                    *selected = 0;
                    cx.notify();
                }
            }
        });
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.palette = Some(Palette::Commands {
            input,
            selected: 0,
            scroll: ScrollHandle::new(),
            _changes: changes,
        });
        cx.notify();
    }

    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.focus_current(window, cx);
            cx.notify();
        }
    }

    pub(super) fn palette_matches(&self, cx: &App) -> Vec<PaletteEntry> {
        let Some(Palette::Commands { input, .. }) = &self.palette else {
            return Vec::new();
        };
        filter_entries(
            palette_entries(&crate::keys::keymap(cx)),
            input.read(cx).value().as_ref(),
        )
    }

    /// The panel owns the keys that move in it; text goes to its field.
    pub(super) fn palette_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.palette_matches(cx).len();
        let name = key.key.to_ascii_lowercase();
        if name == "escape" {
            cx.stop_propagation();
            self.close_palette(window, cx);
            return;
        }
        let Some(Palette::Commands {
            selected, scroll, ..
        }) = &mut self.palette
        else {
            return;
        };
        match name.as_str() {
            "up" | "down" if count > 0 => {
                let current = (*selected).min(count - 1);
                *selected = if name == "down" {
                    (current + 1) % count
                } else {
                    (current + count - 1) % count
                };
                scroll.scroll_to_item(*selected);
                cx.stop_propagation();
                cx.notify();
            }
            "up" | "down" => cx.stop_propagation(),
            "enter" | "return" => {
                let index = *selected;
                cx.stop_propagation();
                self.run_palette_row(index, window, cx);
            }
            _ => {}
        }
    }

    fn run_palette_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let matches = self.palette_matches(cx);
        let Some(entry) = matches.get(index.min(matches.len().saturating_sub(1))) else {
            return;
        };
        let command = entry.command;
        self.close_palette(window, cx);
        let PaletteCommand::Run(command) = command;
        self.run_command(command, window, cx);
    }

    pub(super) fn render_palette(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let palette = self.palette.as_ref()?;
        let colors = &self.appearance;
        let panel = menu_style::panel(colors)
            .id("palette-panel")
            .when(cfg!(all(test, feature = "ui-test-support")), |panel| {
                panel.debug_selector(|| "palette-panel".into())
            })
            .w(chrome_rems(PANEL_WIDTH))
            .max_w(relative(0.9))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let panel = match palette {
            Palette::Commands {
                input,
                selected,
                scroll,
                ..
            } => {
                let matches = self.palette_matches(cx);
                let active = (*selected).min(matches.len().saturating_sub(1));
                let mut list = div()
                    .id("palette-list")
                    .flex()
                    .flex_col()
                    .max_h(chrome_rems(LIST_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(scroll);
                if matches.is_empty() {
                    list =
                        list.child(menu_style::disabled_item(colors).child("No matching command"));
                }
                for (index, entry) in matches.into_iter().enumerate() {
                    list = list.child(
                        menu_style::item(colors, index == active, false)
                            .id(("palette-row", index))
                            .child(menu_style::label(
                                colors,
                                entry.label,
                                None,
                                index == active,
                            ))
                            .when_some(entry.shortcut, |row, text| {
                                row.child(menu_style::meta().child(menu_style::shortcut(
                                    colors,
                                    text,
                                    index == active,
                                )))
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run_palette_row(index, window, cx)
                            })),
                    );
                }
                panel
                    .child(
                        div()
                            .mx(chrome_rems(4.))
                            .mt(chrome_rems(2.))
                            .mb(chrome_rems(4.))
                            .h(chrome_rems(28.))
                            .px(chrome_rems(8.))
                            .flex()
                            .items_center()
                            .edged(colors, Relief::Sunken)
                            .rounded(chrome_rems(colors.corner(5.)))
                            .bg(rgb(colors.field.unwrap_or(colors.surface)))
                            .line_height(relative(1.3))
                            .child(div().flex_1().min_w_0().child(Input::new(input))),
                    )
                    .child(list)
            }
        };
        Some(
            div()
                .id("palette-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .flex()
                .flex_col()
                .items_center()
                .pt(relative(PANEL_TOP))
                .bg(rgba(0x00000040))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
                )
                .child(panel)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{filter_entries, palette_entries, KeyCommand, PaletteCommand};
    use splitty_client_core::keymap::{Keymap, Overrides, Platform};

    #[test]
    fn palette_filters_by_every_word_and_names_platform_keys() {
        let linux = Keymap::defaults(Platform::Linux);
        let all = palette_entries(&linux);
        assert!(all.len() > 10);
        let split: Vec<_> = filter_entries(palette_entries(&linux), "spl do")
            .into_iter()
            .map(|entry| (entry.label, entry.shortcut))
            .collect();
        assert_eq!(split, [("Split Down".into(), Some("Alt+Shift+D".into()))]);
        assert!(filter_entries(palette_entries(&linux), "nothing here").is_empty());
        // The Hosts manager is listed with its key.
        for platform in [Platform::Linux, Platform::Mac] {
            let keymap = Keymap::defaults(platform);
            let hosts = keymap.label(KeyCommand::Hosts).unwrap();
            assert!(palette_entries(&keymap).iter().any(|entry| {
                entry.label == "Hosts and Ports" && entry.shortcut.as_ref() == Some(&hosts)
            }));
        }
        // A command without a key is listed without one.
        let connect = all
            .iter()
            .find(|entry| entry.command == PaletteCommand::Run(KeyCommand::ConnectHost))
            .unwrap();
        assert_eq!(connect.shortcut, None);
        let mut overrides = Overrides::default();
        overrides.set(KeyCommand::SplitDown, Vec::new()).unwrap();
        let unbound = Keymap::new(Platform::Linux, &overrides);
        assert!(palette_entries(&unbound)
            .iter()
            .any(|entry| entry.label == "Split Down" && entry.shortcut.is_none()));
    }
}
