//! The entry points of the keyboard commands that have no control of their
//! own elsewhere: Screen, tab and pane operations on what the window shows,
//! the focused terminal's actions and the window's own commands.
use super::*;
use crate::terminal::{LocalAction, ScrollStep};

impl Workspace {
    /// The terminal of the focused pane of the shown Screen.
    fn focused_terminal(&self) -> Option<Entity<Terminal>> {
        let model = self.selected_model()?;
        let pane = model.focused_pane()?;
        self.panes
            .get(&model.pane_key(pane))
            .map(|view| view.terminal.clone())
    }

    /// Run a terminal action on the focused pane, as its context menu does.
    pub(super) fn focused_terminal_action(
        &mut self,
        action: LocalAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(terminal) = self.focused_terminal() {
            terminal.update(cx, |terminal, cx| terminal.local_action(action, window, cx));
        }
    }

    /// Show the next Screen of the Drawer's order, as the Drawer's wheel does.
    pub(super) fn next_screen(&mut self, cx: &mut Context<Self>) {
        if let Some(owner) = self.lifecycle_token() {
            self.send_captured(
                owner,
                Command::CycleScreen {
                    step: TabStep::Next,
                },
                cx,
            );
        }
    }

    /// Show the previous Screen of the Drawer's order.
    pub(super) fn previous_screen(&mut self, cx: &mut Context<Self>) {
        if let Some(owner) = self.lifecycle_token() {
            self.send_captured(
                owner,
                Command::CycleScreen {
                    step: TabStep::Previous,
                },
                cx,
            );
        }
    }

    /// Close the shown Screen as its Drawer entry does: a local one after
    /// its assessment, a host's one is detached.
    pub(super) fn close_shown_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(screen) = self.current_screen_key() {
            self.request_close_screen(screen, window, cx);
        }
    }

    /// Open the inline rename editor of the shown Screen.
    pub(super) fn rename_shown_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(screen) = self
            .selected_model()
            .and_then(SourceWorkspace::shown_screen_id)
            .map(str::to_owned)
        {
            self.start_rename(&screen, window, cx);
        }
    }

    /// Move the shown Screen one place up or down in its group of the
    /// Drawer, as keyboard placement does.
    pub(super) fn move_shown_screen(&mut self, _up: bool, _cx: &mut Context<Self>) {}

    /// Open the shell profile picker of the focused group, for a new tab.
    pub(super) fn new_tab_with_profile(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Close every tab of the focused group but the selected one, after
    /// their assessment.
    pub(super) fn close_other_tabs(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Move the selected tab one place right or left in its group.
    pub(super) fn move_tab(&mut self, _right: bool, _cx: &mut Context<Self>) {}

    /// Choose another Screen for the selected tab, as the tab menu's Move to
    /// Screen does.
    pub(super) fn move_tab_to_screen(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Float the focused pane, or dock it when it floats, as the group's
    /// float button does.
    pub(super) fn toggle_float(&mut self, _cx: &mut Context<Self>) {}

    /// Move keyboard focus into the Drawer's Screen list.
    pub(super) fn focus_drawer(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Clear the focused terminal's screen and scrollback.
    pub(super) fn clear_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focused_terminal_action(LocalAction::ClearBuffer, window, cx);
    }

    /// Move the focused terminal's viewport through its scrollback.
    pub(super) fn scroll_terminal(
        &mut self,
        step: ScrollStep,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focused_terminal_action(LocalAction::Scroll(step), window, cx);
    }

    /// Copy the focused pane's working directory, as its context menu does.
    pub(super) fn copy_working_directory(&mut self, cx: &mut Context<Self>) {
        let cwd = self.selected_model().and_then(|model| {
            model
                .focused_pane()
                .and_then(|pane| transfer::pane_cwd(model, pane))
        });
        match cwd {
            Some(cwd) => cx.write_to_clipboard(ClipboardItem::new_string(cwd)),
            None => {
                self.status = "The working directory is not known".into();
                cx.notify();
            }
        }
    }

    /// Open the focused terminal's selection in a viewer, as its context
    /// menu does.
    pub(super) fn open_selection_in_viewer(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// Open the Connect panel, centered under the top of the window.
    pub(super) fn connect_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        self.open_drawer_popover(
            popover::PopoverKind::Connect,
            popover::PopoverAnchor::Pointer(point(
                viewport.width / 2. - px(120.),
                viewport.height * palette::PANEL_TOP,
            )),
            None,
            window,
            cx,
        );
    }

    pub(super) fn toggle_fullscreen(&mut self, window: &mut Window) {
        window.toggle_fullscreen();
    }

    /// Close the window as its close button does, after the assessment.
    pub(super) fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_window_close(window, cx);
    }
}
