//! The command palette and the list of keyboard shortcuts: one centered panel
//! over the window, opened by a shortcut. The palette filters the window's
//! commands as the web client's does; choosing one runs it as its own control
//! or shortcut would.
use super::*;

/// What a palette row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaletteCommand {
    Run(Shortcut),
    ConnectHost,
    Hosts,
    Shortcuts,
    Zoom(ZoomAction),
}

pub(super) struct PaletteEntry {
    pub(super) label: &'static str,
    pub(super) shortcut: Option<String>,
    pub(super) command: PaletteCommand,
}

/// The modifiers as the platform writes them, as in the web client.
struct KeyNames {
    primary: &'static str,
    shift: &'static str,
    control: &'static str,
}

fn key_names(is_mac: bool) -> KeyNames {
    if is_mac {
        KeyNames {
            primary: "⌘",
            shift: "⇧",
            control: "⌃",
        }
    } else {
        KeyNames {
            primary: "Alt+",
            shift: "Shift+",
            control: "Ctrl+",
        }
    }
}

/// The key combination of a shortcut, as shown beside its command.
pub(super) fn shortcut_label(shortcut: Shortcut, is_mac: bool) -> String {
    let KeyNames {
        primary,
        shift,
        control,
    } = key_names(is_mac);
    match shortcut {
        Shortcut::ToggleDrawer => format!("{control}B"),
        Shortcut::NewScreen => format!("{control}{shift}Space"),
        Shortcut::Settings => format!("{control},"),
        Shortcut::Templates => format!("{primary}P"),
        Shortcut::CommandPalette => format!("{primary}{shift}P"),
        Shortcut::NewTab => format!("{primary}T"),
        Shortcut::ToggleMaximize => format!("{primary}M"),
        Shortcut::Scratchpad => format!("{primary}J"),
        Shortcut::SplitRight => format!("{primary}D"),
        Shortcut::SplitDown => format!("{primary}{shift}D"),
        Shortcut::ClosePane => format!("{primary}W"),
        Shortcut::PreviousTab => format!("{primary}PageUp"),
        Shortcut::NextTab => format!("{primary}PageDown"),
        Shortcut::Focus(direction) => format!(
            "{primary}{shift}{}",
            match direction {
                Direction::Left => "←",
                Direction::Right => "→",
                Direction::Up => "↑",
                Direction::Down => "↓",
            }
        ),
        Shortcut::ScreenIndex(index) => format!("{primary}{shift}{}", index + 1),
    }
}

/// Every command of the window, in the order the palette lists them.
pub(super) fn palette_entries(is_mac: bool) -> Vec<PaletteEntry> {
    let run = |label, shortcut: Shortcut| PaletteEntry {
        label,
        shortcut: Some(shortcut_label(shortcut, is_mac)),
        command: PaletteCommand::Run(shortcut),
    };
    let plain = |label, command| PaletteEntry {
        label,
        shortcut: None,
        command,
    };
    let control = key_names(is_mac).control;
    let zoom = |label, key: &str, action| PaletteEntry {
        label,
        shortcut: Some(format!("{}{key}", if is_mac { "⌘" } else { control })),
        command: PaletteCommand::Zoom(action),
    };
    vec![
        run("New Screen", Shortcut::NewScreen),
        plain("Connect to Host…", PaletteCommand::ConnectHost),
        run("New Tab", Shortcut::NewTab),
        run("Split Right", Shortcut::SplitRight),
        run("Split Down", Shortcut::SplitDown),
        run("Close Tab", Shortcut::ClosePane),
        run("Maximize Pane", Shortcut::ToggleMaximize),
        run("Toggle Scratchpad", Shortcut::Scratchpad),
        run("Templates…", Shortcut::Templates),
        run("Toggle Drawer", Shortcut::ToggleDrawer),
        run("Settings", Shortcut::Settings),
        plain("Hosts and Ports", PaletteCommand::Hosts),
        zoom("Zoom In", "+", ZoomAction::In),
        zoom("Zoom Out", "-", ZoomAction::Out),
        zoom("Reset Zoom", "0", ZoomAction::Reset),
        plain("Keyboard Shortcuts", PaletteCommand::Shortcuts),
    ]
}

/// The rows of the shortcut list: every command with a key, then navigation.
fn shortcut_rows(is_mac: bool) -> Vec<(&'static str, String)> {
    let mut rows: Vec<_> = palette_entries(is_mac)
        .into_iter()
        .filter_map(|entry| Some((entry.label, entry.shortcut?)))
        .collect();
    let KeyNames { primary, shift, .. } = key_names(is_mac);
    rows.extend([
        (
            "Command Palette",
            shortcut_label(Shortcut::CommandPalette, is_mac),
        ),
        (
            "Previous Tab",
            shortcut_label(Shortcut::PreviousTab, is_mac),
        ),
        ("Next Tab", shortcut_label(Shortcut::NextTab, is_mac)),
        ("Focus Neighbor", format!("{primary}{shift}Arrow")),
        ("Switch Screen", format!("{primary}{shift}1–9")),
    ]);
    rows
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
    Shortcuts,
}

const PANEL_WIDTH: f32 = 440.;
const PANEL_TOP: f32 = 0.16;
const LIST_HEIGHT: f32 = 320.;
const SHORTCUTS_HEIGHT: f32 = 460.;

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

    pub(super) fn show_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.palette_admitted(cx) {
            return;
        }
        self.palette = Some(Palette::Shortcuts);
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.focus_current(window, cx);
            cx.notify();
        }
    }

    fn palette_matches(&self, cx: &App) -> Vec<PaletteEntry> {
        let Some(Palette::Commands { input, .. }) = &self.palette else {
            return Vec::new();
        };
        filter_entries(
            palette_entries(cx.platform_info().is_mac()),
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
            // The shortcut list takes no other key.
            cx.stop_propagation();
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
        match command {
            PaletteCommand::Run(shortcut) => self.run_shortcut(shortcut, window, cx),
            PaletteCommand::ConnectHost => {
                let viewport = window.viewport_size();
                self.open_drawer_popover(
                    popover::PopoverKind::Connect,
                    popover::PopoverAnchor::Pointer(point(
                        viewport.width / 2. - px(120.),
                        viewport.height * PANEL_TOP,
                    )),
                    None,
                    window,
                    cx,
                );
            }
            PaletteCommand::Hosts => self.show_hosts(window, cx),
            PaletteCommand::Shortcuts => self.show_shortcuts(window, cx),
            PaletteCommand::Zoom(action) => self.submit_zoom(action, cx),
        }
    }

    pub(super) fn render_palette(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let palette = self.palette.as_ref()?;
        let colors = &self.appearance;
        let is_mac = cx.platform_info().is_mac();
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
                            .child(menu_style::label(colors, entry.label, None))
                            .when_some(entry.shortcut, |row, text| {
                                row.child(
                                    menu_style::meta().child(menu_style::shortcut(colors, text)),
                                )
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
                            .border_1()
                            .border_color(rgb(colors.border))
                            .rounded(chrome_rems(5.))
                            .bg(rgb(colors.surface))
                            .line_height(relative(1.3))
                            .child(div().flex_1().min_w_0().child(Input::new(input))),
                    )
                    .child(list)
            }
            Palette::Shortcuts => {
                let mut list = div()
                    .id("shortcut-list")
                    .flex()
                    .flex_col()
                    .max_h(chrome_rems(SHORTCUTS_HEIGHT))
                    .overflow_y_scroll();
                for (label, key) in shortcut_rows(is_mac) {
                    list = list.child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_between()
                            .gap(chrome_rems(20.))
                            .px(chrome_rems(10.))
                            .py(chrome_rems(4.))
                            .child(label)
                            .child(menu_style::shortcut(colors, key)),
                    );
                }
                panel
                    .child(menu_style::caption(colors, "Keyboard Shortcuts"))
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
    use super::{filter_entries, palette_entries, shortcut_label, shortcut_rows, Shortcut};

    #[test]
    fn palette_filters_by_every_word_and_names_platform_keys() {
        let all = palette_entries(false);
        assert!(all.len() > 10);
        let split: Vec<_> = filter_entries(palette_entries(false), "spl do")
            .into_iter()
            .map(|entry| (entry.label, entry.shortcut))
            .collect();
        assert_eq!(split, [("Split Down", Some("Alt+Shift+D".into()))]);
        assert!(filter_entries(palette_entries(false), "nothing here").is_empty());
        assert_eq!(shortcut_label(Shortcut::SplitDown, true), "⌘⇧D");
        assert_eq!(shortcut_label(Shortcut::Settings, false), "Ctrl+,");
        // Every key the list shows belongs to one row only.
        let rows = shortcut_rows(false);
        let mut keys: Vec<_> = rows.iter().map(|(_, key)| key.clone()).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), rows.len());
    }
}
