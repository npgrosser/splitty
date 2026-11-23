//! The entry points of the keyboard commands that have no control of their
//! own elsewhere: Screen, tab and pane operations on what the window shows,
//! the focused terminal's actions and the window's own commands.
use super::*;
use crate::terminal::{LocalAction, ScrollStep};

impl Workspace {
    /// The terminal of the focused pane of the shown Screen.
    pub(super) fn focused_terminal(&self) -> Option<Entity<Terminal>> {
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

    /// The Screen after (or before) the shown one in the Drawer's order,
    /// local Screens first and then each host's, wrapping around.
    fn adjacent_screen(&self, forward: bool) -> Option<ScreenKey> {
        let model = self.model.as_ref()?;
        let shown = model.shown_key()?;
        let opened: Vec<ScreenKey> = model
            .opened()
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        let index = opened.iter().position(|key| *key == shown)?;
        let count = opened.len();
        let next = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        (next != index).then(|| opened[next].clone())
    }

    /// Show the next Screen of the Drawer's order, the first after the last.
    pub(super) fn next_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.adjacent_screen(true) {
            self.switch_screen_key(key, window, cx);
        }
    }

    /// Show the previous Screen of the Drawer's order, the last before the
    /// first.
    pub(super) fn previous_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.adjacent_screen(false) {
            self.switch_screen_key(key, window, cx);
        }
    }

    /// Close the shown Screen as its Drawer entry does: a local one after
    /// its assessment, a host's one is detached.
    pub(super) fn close_shown_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(screen) = self.current_screen_key() {
            self.request_close_screen(screen, window, cx);
        }
    }

    /// Open the inline rename editor of the shown Screen in the Drawer, as
    /// its menu's Rename does, revealing a hidden Drawer while it is open.
    pub(super) fn rename_shown_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let owner = model.lifecycle_token();
        let Some(screen) = model.shown_key() else {
            return;
        };
        let Some(source) = model
            .source(screen.source_id())
            .filter(|source| source.validate_screen_rename(screen.screen_id()).is_ok())
            .map(SourceWorkspace::source_token)
        else {
            return;
        };
        let revealed = self.reveal_drawer_for_keyboard(cx);
        self.start_captured_rename(owner, source, &screen, window, cx);
        if revealed && self.rename.is_none() {
            self.end_keyboard_reveal(cx);
        }
    }

    /// Move the shown Screen one place up or down within its group of the
    /// Drawer, as its menu's Move Up and Move Down do.
    pub(super) fn move_shown_screen(&mut self, up: bool, cx: &mut Context<Self>) {
        let (Some(owner), Some(screen)) = (self.lifecycle_token(), self.current_screen_key())
        else {
            return;
        };
        if let Some(before) = self.screen_menu_before(&screen, !up) {
            self.place_screen(owner, screen, before, cx);
        }
    }

    /// The focused pane of the shown Screen while its source takes actions.
    fn focused_pane_action(&self) -> Option<PaneActionTarget> {
        let model = self.selected_model()?;
        let target = model.capture_pane_action(model.focused_pane()?).ok()?;
        (self.execution_token(target.source_token().source_id()) == Some(target.source_token()))
            .then_some(target)
    }

    /// Open the shell profile picker of the focused group, as its arrow
    /// button does; a host's group has none.
    pub(super) fn new_tab_with_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_remote_selected() {
            return;
        }
        let Some(model) = self.selected_model() else {
            return;
        };
        let (Some(pane), Some(slot)) = (model.focused_pane(), model.focused_slot_id()) else {
            return;
        };
        let (pane, slot) = (pane.to_owned(), slot.to_owned());
        if self.focused_pane_action().is_none() {
            return;
        }
        self.toggle_profile_picker(&slot, &pane, window, cx);
        if let Some(picker) = self.profile_picker.as_mut() {
            picker.from_key = true;
        }
    }

    /// Close every tab of the focused group but the selected one, as the
    /// tab menu's Close Other Tabs does.
    pub(super) fn close_other_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.focused_pane_action() else {
            return;
        };
        let alone = self.selected_model().is_none_or(|model| {
            model
                .focused_slot_id()
                .and_then(|slot| presented_slot_by_id(model, slot))
                .is_none_or(|slot| match slot {
                    LayoutNode::Slot { pane_ids, .. } => pane_ids.len() < 2,
                    LayoutNode::Split { .. } => true,
                })
        });
        if !alone {
            self.close_other_tabs_captured(target, window, cx);
        }
    }

    /// Move the selected tab one place right or left in its group, as a drag
    /// onto its tab bar does; nothing at the end. The tab keeps the focus.
    pub(super) fn move_tab(&mut self, right: bool, cx: &mut Context<Self>) {
        if self.focused_pane_action().is_none() {
            return;
        }
        let Some(model) = self.selected_model() else {
            return;
        };
        let (Some(pane), Some(slot)) = (model.focused_pane(), model.focused_slot_id()) else {
            return;
        };
        let Some(LayoutNode::Slot { pane_ids, .. }) = presented_slot_by_id(model, slot) else {
            return;
        };
        let Some(index) = pane_ids.iter().position(|id| id.as_str() == pane) else {
            return;
        };
        let before = if right {
            if index + 1 >= pane_ids.len() {
                return;
            }
            pane_ids.get(index + 2).map(|id| id.to_string())
        } else {
            let Some(previous) = index.checked_sub(1) else {
                return;
            };
            Some(pane_ids[previous].to_string())
        };
        let Some(drag) = model.capture_drag(pane) else {
            return;
        };
        let slot = slot.to_owned();
        match model.check_place_tab(&drag, &slot, before.as_deref()) {
            Ok(true) => self.send_captured(
                drag.source_token(),
                Command::PlaceTab {
                    drag,
                    target: slot,
                    before,
                },
                cx,
            ),
            Ok(false) => {}
            Err(error) => {
                self.status = error.to_string();
                cx.notify();
            }
        }
    }

    /// Open the tab menu's Move to Screen chooser for the selected tab, at
    /// its group, with the keyboard focus on its first Screen.
    pub(super) fn move_tab_to_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.focused_pane_action() else {
            return;
        };
        let Some(screen) = self.current_screen_key() else {
            return;
        };
        let position = self
            .selected_model()
            .and_then(SourceWorkspace::focused_slot_id)
            .and_then(|slot| self.current_layout_key(slot))
            .and_then(|key| self.slot_bounds.get(&key))
            .map(|bounds| bounds.origin + point(px(16.), px(28.)))
            .unwrap_or_else(|| {
                let viewport = window.viewport_size();
                point(viewport.width / 3., viewport.height / 4.)
            });
        self.open_tab_transfer_menu(
            target.source_token(),
            &screen,
            &target.pane_id,
            position,
            window,
            cx,
        );
    }

    /// Float the focused tab, or dock its group when it floats, as the
    /// group's Float button and the floating group's Dock button do.
    pub(super) fn toggle_float(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.focused_pane_action() else {
            return;
        };
        let Some(model) = self.selected_model() else {
            return;
        };
        if let Some(id) = model
            .floating_for_pane(&target.pane_id)
            .map(|entry| entry.id.to_string())
        {
            self.dock_float(&id, cx);
            return;
        }
        if !model.can_float_pane(&target) {
            return;
        }
        let request_id = Uuid::new_v4();
        self.submit_float_action(
            target.source_token(),
            request_id,
            Command::FloatPane {
                request_id,
                source: target,
            },
            true,
            cx,
        );
    }

    /// Reveal a hidden Drawer as an overlay for a keyboard command; whether
    /// it was revealed.
    fn reveal_drawer_for_keyboard(&mut self, cx: &mut Context<Self>) -> bool {
        if self.drawer_visible || self.drawer_overlay_open {
            return false;
        }
        self.open_drawer_overlay(cx);
        self.drawer_keyboard_reveal = self.drawer_overlay_open;
        self.drawer_keyboard_reveal
    }

    /// Close the overlay Drawer that a keyboard command revealed.
    pub(super) fn end_keyboard_reveal(&mut self, cx: &mut Context<Self>) {
        if self.drawer_keyboard_reveal {
            self.close_drawer_overlay(cx);
            self.drawer_keyboard_reveal = false;
        }
    }

    /// Move keyboard focus to the shown Screen's row in the Drawer, revealing
    /// a hidden Drawer; from there, the same command returns it to the
    /// terminal.
    pub(super) fn focus_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drawer_focus.contains_focused(window, cx) {
            self.leave_drawer_focus(window, cx);
            return;
        }
        if self.current_screen_key().is_none() {
            return;
        }
        self.drawer_focus_row = None;
        self.reveal_drawer_for_keyboard(cx);
        self.drawer_focus.focus(window, cx);
        set_focus_visible(true, cx);
        cx.notify();
    }

    /// Return the keyboard focus from the Drawer to the work, closing the
    /// overlay a keyboard command revealed.
    pub(super) fn leave_drawer_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drawer_focus_row = None;
        self.end_keyboard_reveal(cx);
        self.focus_current(window, cx);
        cx.notify();
    }

    /// A key on a Screen row of the Drawer: the arrows move the keyboard
    /// focus to the row above or below, Escape returns it to the work.
    /// Whether the key was taken.
    pub(super) fn drawer_row_key(
        &mut self,
        row: &ScreenKey,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match key {
            "escape" => {
                self.leave_drawer_focus(window, cx);
                true
            }
            "up" | "down" => {
                let Some(model) = self.model.as_ref() else {
                    return false;
                };
                let opened: Vec<ScreenKey> = model
                    .opened()
                    .iter()
                    .map(|entry| entry.key().clone())
                    .collect();
                let Some(index) = opened.iter().position(|key| key == row) else {
                    return false;
                };
                let next = if key == "up" {
                    index.checked_sub(1)
                } else {
                    Some(index + 1).filter(|next| *next < opened.len())
                };
                if let Some(next) = next {
                    self.drawer_focus_row = Some(opened[next].clone());
                    self.drawer_focus.focus(window, cx);
                    cx.notify();
                }
                true
            }
            _ => false,
        }
    }

    /// Clear the focused terminal's screen and scrollback.
    pub(super) fn clear_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focused_pane_action().is_some() {
            self.focused_terminal_action(LocalAction::ClearBuffer, window, cx);
        }
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
    pub(super) fn open_selection_in_viewer(&mut self, cx: &mut Context<Self>) {
        if !self
            .selected_model()
            .is_some_and(SourceWorkspace::viewers_supported)
        {
            return;
        }
        let (Some(target), Some(screen), Some(terminal)) = (
            self.focused_pane_action(),
            self.current_screen_key(),
            self.focused_terminal(),
        ) else {
            return;
        };
        self.open_terminal_selection(
            target.source_token(),
            &screen,
            &target.pane_id,
            &terminal,
            cx,
        );
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

    /// Enter or leave the window's full screen state.
    pub(super) fn toggle_fullscreen(&mut self, window: &mut Window) {
        window.toggle_fullscreen();
    }

    /// Close the application for all its windows, after the assessment.
    pub(super) fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close(CloseTarget::Window, window, cx);
    }

    /// Open another window of the application with a new local Screen.
    pub(super) fn new_window(&mut self, cx: &mut Context<Self>) {
        self.open_window(
            splitty_client_core::native_workspace::WindowStart::NewScreen,
            cx,
        );
    }

    /// Hand a Screen this window holds to a new window.
    pub(super) fn move_screen_to_new_window(&mut self, screen: ScreenKey, cx: &mut Context<Self>) {
        if self.holds_screen(&screen) {
            self.open_window(
                splitty_client_core::native_workspace::WindowStart::Screen(screen),
                cx,
            );
        }
    }

    /// Hand a Screen this window holds to another open window.
    pub(super) fn move_screen_to_window(
        &mut self,
        screen: ScreenKey,
        target: WindowId,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self
            .lifecycle_token()
            .filter(|_| self.holds_screen(&screen))
        else {
            return;
        };
        self.send_captured(owner, Command::MoveScreenToWindow { screen, target }, cx);
    }

    /// The shown Screen goes to a new window.
    pub(super) fn move_shown_screen_to_new_window(&mut self, cx: &mut Context<Self>) {
        if let Some(screen) = self.current_screen_key() {
            self.move_screen_to_new_window(screen, cx);
        }
    }

    /// The shown Screen goes to the window opened after this one, or to the
    /// first one from the last.
    pub(super) fn move_shown_screen_to_next_window(&mut self, cx: &mut Context<Self>) {
        let Some(screen) = self.current_screen_key() else {
            return;
        };
        match self.next_window(cx) {
            Some(target) => self.move_screen_to_window(screen, target, cx),
            None => {
                self.status = "There is no other window to move the Screen to".into();
                cx.notify();
            }
        }
    }

    /// The window opened after this one, or the first one from the last.
    pub(super) fn next_window(&self, cx: &App) -> Option<WindowId> {
        let mut others: Vec<WindowId> = crate::windows::Windows::others(self.window_id, cx)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        others.sort();
        others
            .iter()
            .find(|id| **id > self.window_id)
            .or(others.first())
            .copied()
    }

    /// The Screens of one group this window holds: the local ones, or those
    /// of one host, in the order the Drawer lists them.
    fn group_screens(&self, source: SourceId) -> Vec<ScreenKey> {
        self.model.as_ref().map_or_else(Vec::new, |model| {
            model
                .opened()
                .iter()
                .map(|entry| entry.key().clone())
                .filter(|key| key.source_id() == source)
                .collect()
        })
    }

    /// Hand every Screen of a group to a new window, in one step.
    pub(super) fn move_group_to_new_window(&mut self, source: SourceId, cx: &mut Context<Self>) {
        let screens = self.group_screens(source);
        if !screens.is_empty() {
            self.open_window(
                splitty_client_core::native_workspace::WindowStart::Screens(screens),
                cx,
            );
        }
    }

    /// Hand every Screen of a group to another open window, in one step.
    pub(super) fn move_group_to_window(
        &mut self,
        source: SourceId,
        target: WindowId,
        cx: &mut Context<Self>,
    ) {
        let screens = self.group_screens(source);
        let Some(owner) = self.lifecycle_token().filter(|_| !screens.is_empty()) else {
            return;
        };
        self.send_captured(owner, Command::MoveScreensToWindow { screens, target }, cx);
    }

    pub(super) fn holds_screen(&self, screen: &ScreenKey) -> bool {
        self.model
            .as_ref()
            .is_some_and(|model| model.opened().iter().any(|entry| entry.key() == screen))
    }

    /// The platform window opens first, so that it reads from the start what
    /// the worker then says it holds.
    fn open_window(
        &mut self,
        start: splitty_client_core::native_workspace::WindowStart,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self.lifecycle_token() else {
            return;
        };
        let commands = self.commands.clone();
        cx.defer(move |cx| {
            let Some((id, presence)) = crate::windows::Windows::open(cx) else {
                return;
            };
            let opened = commands.try_send(WorkerCommand::new(
                owner,
                Command::OpenWindow {
                    window: id,
                    presence,
                    start,
                },
            ));
            if opened.is_err() {
                // The worker never heard of the window: it has nothing to show.
                crate::windows::Windows::abandon(id, cx);
            }
        });
    }
}
