//! Terminal body context menu routed through captured workspace authority.
use super::chrome::{GroupCommand, GroupTarget};
use super::transfer::{self, EditorChoice, EditorSubmenu, TransferChoice, TransferMenu};
use super::*;
use crate::terminal::{ContextMenuRequest, LocalAction};
use gpui_kit::base::DeferredPopover;
use splitty_client_core::native_workspace::PathKind;

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TerminalMenuAction {
    Copy,
    OpenSelection,
    Paste,
    Find,
    ClearBuffer,
    SplitRight,
    SplitDown,
    Scratchpad,
    CopyDirectory,
    CopySshPath,
    OpenFolderIn,
    MoveToScreen,
    ShowDrawer,
    ClosePane,
}

impl TerminalMenuAction {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::OpenSelection => "open-selection",
            Self::Paste => "paste",
            Self::Find => "find",
            Self::ClearBuffer => "clear-buffer",
            Self::SplitRight => "split-right",
            Self::SplitDown => "split-down",
            Self::Scratchpad => "scratchpad",
            Self::CopyDirectory => "copy-cwd",
            Self::CopySshPath => "copy-ssh",
            Self::OpenFolderIn => "open-folder",
            Self::MoveToScreen => "move",
            Self::ShowDrawer => "show-drawer",
            Self::ClosePane => "close",
        }
    }
}

#[derive(Clone)]
pub(super) struct TerminalMenuRow {
    pub(super) action: TerminalMenuAction,
    pub(super) label: &'static str,
    pub(super) detail: Option<SharedString>,
    pub(super) shortcut: Option<&'static str>,
    pub(super) enabled: bool,
    pub(super) danger: bool,
    pub(super) separator_before: bool,
    pub(super) has_submenu: bool,
}

#[derive(Clone)]
pub(super) struct TerminalMenuTarget {
    route: TerminalToken,
    screen: ScreenKey,
    pane: Option<PaneActionTarget>,
    pub(super) group: Option<GroupTarget>,
    terminal: Entity<Terminal>,
    request: ContextMenuRequest,
    pub(super) authority: Option<SourceToken>,
    has_selection: bool,
    can_find: bool,
}

impl TerminalMenuTarget {
    fn valid_for(&self, workspace: &Workspace, cx: &App) -> bool {
        if workspace.destination != Destination::Screens
            || workspace.pending_template_apply.is_some()
            || workspace.current_screen_key().as_ref() != Some(&self.screen)
        {
            return false;
        }
        let Some(window) = workspace.model.as_ref() else {
            return false;
        };
        let Some(model) = workspace.selected_model() else {
            return false;
        };
        if model.source_token() != self.route.source_token()
            || model.pending_template().is_some()
            || window
                .validate_terminal_focus(&self.screen, &self.route)
                .is_err()
            || workspace.execution_token(self.screen.source_id()) != self.authority
        {
            return false;
        }
        if self.authority.is_some()
            && self
                .pane
                .as_ref()
                .is_none_or(|pane| model.validate_pane_action(pane).is_err())
        {
            return false;
        }
        workspace
            .panes
            .get(&self.route.pane_key())
            .is_some_and(|view| {
                view.route == self.route
                    && view.terminal.entity_id() == self.terminal.entity_id()
                    && view
                        .terminal
                        .read(cx)
                        .context_menu_is_current(&self.request)
            })
    }
}

pub(super) struct TerminalMenu {
    pub(super) target: TerminalMenuTarget,
    position: Point<Pixels>,
    pub(super) rows: Vec<TerminalMenuRow>,
    transfer_choices: Vec<(TransferChoice, String)>,
    editor_choices: Vec<(EditorChoice, Option<String>)>,
    pub(super) selected: Option<TerminalMenuAction>,
    pub(super) focus_visible: bool,
    pub(super) transfer: Option<TransferMenu>,
    pub(super) editor: Option<EditorSubmenu>,
    scroll: ScrollHandle,
    panel_bounds: menu_style::PlacedBounds,
    move_row_bounds: menu_style::PlacedBounds,
    editor_row_bounds: menu_style::PlacedBounds,
    _open: DeferredPopover,
    _tab_closes: Subscription,
}

impl TerminalMenu {
    fn keyboard_in_transfer(&self) -> bool {
        self.transfer
            .as_ref()
            .is_some_and(|transfer| transfer.focused.is_some())
    }

    fn keyboard_in_editor(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor.focused.is_some())
    }

    pub(super) fn enabled_actions(&self) -> Vec<TerminalMenuAction> {
        self.rows
            .iter()
            .filter(|row| row.enabled)
            .map(|row| row.action)
            .collect()
    }
}

impl Workspace {
    pub(super) fn open_terminal_menu(
        &mut self,
        route: TerminalToken,
        terminal: Entity<Terminal>,
        request: ContextMenuRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.destination != Destination::Screens
            || self.rename.is_some()
            || self.template_dialog.is_some()
            || self.pending_template_apply.is_some()
            || self
                .pending_close
                .as_ref()
                .is_some_and(|pending| pending.prompting)
        {
            return;
        }
        let Some(screen) = self.current_screen_key() else {
            return;
        };
        if self
            .model
            .as_ref()
            .is_none_or(|model| model.validate_terminal_focus(&screen, &route).is_err())
        {
            return;
        }
        self.focus_captured(route.source_token(), &screen, route.pane_id(), cx);
        let Some(model) = self.selected_model() else {
            return;
        };
        if model.focused_pane() != Some(route.pane_id()) {
            self.focus_current(window, cx);
            return;
        }
        if model.source_token() != route.source_token() || model.pending_template().is_some() {
            return;
        }
        let authority = self
            .execution_token(screen.source_id())
            .filter(|token| *token == route.source_token());
        let pane = authority
            .is_some()
            .then(|| model.capture_pane_action(route.pane_id()).ok())
            .flatten();
        if authority.is_some() && pane.is_none() {
            return;
        }
        let group = authority.is_some().then(|| {
            model
                .is_root_pane(route.pane_id())
                .then(|| GroupTarget::capture_pane(model, route.pane_id()))
                .flatten()
        });
        let (has_selection, can_find) = terminal.update(cx, |terminal, _| {
            (terminal.has_selection(), terminal.engine_ready())
        });
        let position = request.position();
        let target = TerminalMenuTarget {
            route: route.clone(),
            screen,
            pane,
            group: group.flatten(),
            terminal,
            request,
            authority,
            has_selection,
            can_find,
        };
        if !target.valid_for(self, cx) {
            return;
        }
        self.wheel_navigation.reset();
        self.float_gesture = None;
        self.drop_target = None;
        self.tab_context_menu = None;
        self.terminal_menu = None;
        self.close_profile_picker(window, cx);
        self.sync_visible(cx);
        if target.authority.is_some() {
            self.focus_current(window, cx);
        } else {
            self.focus.focus(window, cx);
        }

        let is_mac = cx.platform_info().is_mac();
        let rows = self.build_terminal_menu_rows(&target, is_mac);
        let transfer_choices = self.build_terminal_transfer_choices(&target);
        let editor_choices = self.build_terminal_editor_choices(&target);
        let owner = cx.entity().downgrade();
        let menu_window = window.window_handle();
        let tab_closes = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() == menu_window && event.keystroke.key == "tab" {
                cx.stop_propagation();
                let _ = owner.update(cx, |this, cx| {
                    this.cancel_terminal_menu(window, cx);
                });
            }
        });
        self.terminal_menu = Some(TerminalMenu {
            target,
            position,
            rows,
            transfer_choices,
            editor_choices,
            selected: None,
            focus_visible: false,
            transfer: None,
            editor: None,
            scroll: ScrollHandle::new(),
            panel_bounds: Rc::default(),
            move_row_bounds: Rc::default(),
            editor_row_bounds: Rc::default(),
            _open: GlobalState::register_deferred_popover(cx),
            _tab_closes: tab_closes,
        });
        cx.notify();
    }

    fn build_terminal_menu_rows(
        &self,
        target: &TerminalMenuTarget,
        is_mac: bool,
    ) -> Vec<TerminalMenuRow> {
        let live = target.authority.is_some();
        let can_split = live && target.group.is_some();
        let pane_id = target.route.pane_id();
        let scratchpad_open = self.scratchpad_state_for_pane(pane_id);
        let cwd = self
            .selected_model()
            .and_then(|model| transfer::pane_cwd(model, pane_id));
        let remote = self.is_remote_selected();
        let remote_target = self
            .model
            .as_ref()
            .filter(|_| remote)
            .and_then(|model| model.source_target(target.route.source_token().source_id()))
            .map(str::to_owned);
        let ssh = remote_target
            .as_deref()
            .zip(cwd.as_deref())
            .and_then(|(host, cwd)| transfer::ssh_path(host, cwd));
        let can_transfer = live
            && !remote
            && target.pane.as_ref().is_some_and(|pane| {
                self.selected_model()
                    .is_some_and(|model| model.can_transfer_pane(pane))
            });
        let show_drawer = !self.drawer_visible && !self.drawer_overlay_open;

        let mut rows = vec![TerminalMenuRow {
            action: TerminalMenuAction::Copy,
            label: "Copy",
            detail: None,
            shortcut: None,
            enabled: target.has_selection,
            danger: false,
            separator_before: false,
            has_submenu: false,
        }];
        // Offered only while there is a selection and its host opens viewers.
        if target.has_selection
            && live
            && self
                .selected_model()
                .is_some_and(SourceWorkspace::viewers_supported)
        {
            rows.push(TerminalMenuRow {
                action: TerminalMenuAction::OpenSelection,
                label: "Open Selection in Viewer",
                detail: None,
                shortcut: None,
                enabled: true,
                danger: false,
                separator_before: false,
                has_submenu: false,
            });
        }
        rows.extend([
            TerminalMenuRow {
                action: TerminalMenuAction::Paste,
                label: "Paste",
                detail: None,
                shortcut: None,
                enabled: live,
                danger: false,
                separator_before: false,
                has_submenu: false,
            },
            TerminalMenuRow {
                action: TerminalMenuAction::Find,
                label: "Find",
                detail: None,
                shortcut: None,
                enabled: target.can_find,
                danger: false,
                separator_before: false,
                has_submenu: false,
            },
            TerminalMenuRow {
                action: TerminalMenuAction::ClearBuffer,
                label: "Clear Buffer",
                detail: None,
                shortcut: None,
                enabled: live,
                danger: false,
                separator_before: true,
                has_submenu: false,
            },
            TerminalMenuRow {
                action: TerminalMenuAction::SplitRight,
                label: "Split Right",
                detail: None,
                shortcut: Some(if is_mac { "⌘D" } else { "Alt+D" }),
                enabled: can_split,
                danger: false,
                separator_before: true,
                has_submenu: false,
            },
            TerminalMenuRow {
                action: TerminalMenuAction::SplitDown,
                label: "Split Down",
                detail: None,
                shortcut: Some(if is_mac { "⌘⇧D" } else { "Alt+Shift+D" }),
                enabled: can_split,
                danger: false,
                separator_before: false,
                has_submenu: false,
            },
            TerminalMenuRow {
                action: TerminalMenuAction::Scratchpad,
                label: if scratchpad_open == Some(true) {
                    "Hide Scratchpad"
                } else {
                    "Show Scratchpad"
                },
                detail: None,
                shortcut: Some(if is_mac { "⌘J" } else { "Alt+J" }),
                enabled: live && scratchpad_open.is_some(),
                danger: false,
                separator_before: false,
                has_submenu: false,
            },
        ]);

        if let Some(cwd_path) = cwd.as_deref() {
            rows.push(TerminalMenuRow {
                action: TerminalMenuAction::CopyDirectory,
                label: "Copy Working Directory",
                detail: None,
                shortcut: None,
                enabled: live,
                danger: false,
                separator_before: true,
                has_submenu: false,
            });
            if let Some(ssh_text) = ssh.as_deref() {
                rows.push(TerminalMenuRow {
                    action: TerminalMenuAction::CopySshPath,
                    label: "Copy SSH Path",
                    detail: Some(transfer::detail_line(ssh_text)),
                    shortcut: None,
                    enabled: live,
                    danger: false,
                    separator_before: false,
                    has_submenu: false,
                });
            }
            rows.push(TerminalMenuRow {
                action: TerminalMenuAction::OpenFolderIn,
                label: "Open Folder in",
                detail: Some(transfer::detail_line(cwd_path)),
                shortcut: None,
                enabled: live,
                danger: false,
                separator_before: false,
                has_submenu: true,
            });
        }

        if !remote {
            rows.push(TerminalMenuRow {
                action: TerminalMenuAction::MoveToScreen,
                label: "Move to Screen",
                detail: None,
                shortcut: None,
                enabled: can_transfer,
                danger: false,
                separator_before: true,
                has_submenu: true,
            });
        }

        if show_drawer {
            rows.push(TerminalMenuRow {
                action: TerminalMenuAction::ShowDrawer,
                label: "Show Drawer",
                detail: None,
                shortcut: Some(if is_mac { "⌃B" } else { "Ctrl+B" }),
                enabled: live,
                danger: false,
                separator_before: true,
                has_submenu: false,
            });
        }

        rows.push(TerminalMenuRow {
            action: TerminalMenuAction::ClosePane,
            label: "Close Pane",
            detail: None,
            shortcut: Some(if is_mac { "⌘W" } else { "Alt+W" }),
            enabled: live,
            danger: true,
            separator_before: true,
            has_submenu: false,
        });

        rows
    }

    fn build_terminal_transfer_choices(
        &self,
        target: &TerminalMenuTarget,
    ) -> Vec<(TransferChoice, String)> {
        let Some(model) = self.selected_model() else {
            return Vec::new();
        };
        let current_screen = target.screen.screen_id();
        let mut choices: Vec<_> = model
            .screens()
            .iter()
            .filter(|screen| screen.id.as_str() != current_screen)
            .map(|screen| {
                let label = screen
                    .name
                    .as_deref()
                    .filter(|name| !name.is_empty())
                    .map(hosts::display)
                    .unwrap_or_else(|| "Unnamed Screen".to_owned());
                (TransferChoice::Existing(screen.id.to_string()), label)
            })
            .collect();
        choices.push((TransferChoice::New, "New Screen".to_owned()));
        choices
    }

    fn build_terminal_editor_choices(
        &self,
        target: &TerminalMenuTarget,
    ) -> Vec<(EditorChoice, Option<String>)> {
        let pane_id = target.route.pane_id();
        let Some(cwd) = self
            .selected_model()
            .and_then(|model| transfer::pane_cwd(model, pane_id))
        else {
            return Vec::new();
        };
        let remote = self.is_remote_selected();
        let remote_target = self
            .model
            .as_ref()
            .filter(|_| remote)
            .and_then(|model| model.source_target(target.route.source_token().source_id()));
        let editors: &[EditorChoice] = if remote {
            &[EditorChoice::VsCode, EditorChoice::Zed, EditorChoice::Sftp]
        } else {
            &[EditorChoice::VsCode, EditorChoice::Zed]
        };
        editors
            .iter()
            .map(|&choice| {
                let uri = transfer::editor_uri(choice, remote_target, &cwd, PathKind::Folder);
                (choice, uri.ok())
            })
            .collect()
    }

    pub(super) fn reconcile_terminal_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .terminal_menu
            .as_ref()
            .is_some_and(|menu| !menu.target.valid_for(self, cx))
        {
            self.cancel_terminal_menu(window, cx);
        }
    }

    pub(super) fn cancel_terminal_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_menu.take().is_some() {
            self.focus_current(window, cx);
            cx.notify();
        }
    }

    pub(super) fn terminal_menu_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let Some(menu) = self.terminal_menu.as_ref() else {
            return;
        };
        let in_transfer = menu.keyboard_in_transfer();
        let in_editor = menu.keyboard_in_editor();
        match keystroke.key.as_str() {
            "escape" => {
                if in_transfer || in_editor {
                    if let Some(menu) = self.terminal_menu.as_mut() {
                        menu.transfer = None;
                        menu.editor = None;
                        menu.focus_visible = true;
                        cx.notify();
                    }
                } else {
                    self.cancel_terminal_menu(window, cx);
                }
            }
            "tab" => self.cancel_terminal_menu(window, cx),
            "left" if in_transfer || in_editor => {
                if let Some(menu) = self.terminal_menu.as_mut() {
                    menu.transfer = None;
                    menu.editor = None;
                    menu.focus_visible = true;
                    cx.notify();
                }
            }
            "right" if !in_transfer && !in_editor => {
                let selected = menu.selected;
                let enabled = menu.enabled_actions();
                if selected == Some(TerminalMenuAction::MoveToScreen)
                    && enabled.contains(&TerminalMenuAction::MoveToScreen)
                {
                    if let Some(menu) = self.terminal_menu.as_mut() {
                        let first = menu
                            .transfer_choices
                            .first()
                            .map(|(choice, _)| choice.clone());
                        menu.editor = None;
                        menu.transfer
                            .get_or_insert_with(TransferMenu::default)
                            .focused = first;
                        menu.focus_visible = true;
                        cx.notify();
                    }
                } else if selected == Some(TerminalMenuAction::OpenFolderIn)
                    && enabled.contains(&TerminalMenuAction::OpenFolderIn)
                {
                    if let Some(menu) = self.terminal_menu.as_mut() {
                        let first = menu
                            .editor_choices
                            .iter()
                            .find_map(|(choice, uri)| uri.is_some().then_some(*choice));
                        menu.transfer = None;
                        menu.editor
                            .get_or_insert_with(EditorSubmenu::default)
                            .focused = first;
                        menu.focus_visible = true;
                        cx.notify();
                    }
                }
            }
            "up" | "down" | "home" | "end" => {
                if in_transfer {
                    if let Some(menu) = self.terminal_menu.as_mut() {
                        let items: Vec<_> = menu
                            .transfer_choices
                            .iter()
                            .map(|(choice, _)| choice.clone())
                            .collect();
                        if let Some(transfer) = menu.transfer.as_mut() {
                            transfer.focused = transfer::step(
                                &items,
                                transfer.focused.as_ref(),
                                keystroke.key.as_str(),
                            );
                        }
                        menu.focus_visible = true;
                    }
                } else if in_editor {
                    if let Some(menu) = self.terminal_menu.as_mut() {
                        let items: Vec<_> = menu
                            .editor_choices
                            .iter()
                            .filter_map(|(choice, uri)| uri.is_some().then_some(*choice))
                            .collect();
                        if let Some(editor) = menu.editor.as_mut() {
                            editor.focused = transfer::step(
                                &items,
                                editor.focused.as_ref(),
                                keystroke.key.as_str(),
                            );
                        }
                        menu.focus_visible = true;
                    }
                } else if let Some(menu) = self.terminal_menu.as_mut() {
                    let items = menu.enabled_actions();
                    let next =
                        transfer::step(&items, menu.selected.as_ref(), keystroke.key.as_str());
                    menu.selected = next;
                    menu.focus_visible = true;
                    if next != Some(TerminalMenuAction::MoveToScreen) {
                        menu.transfer = None;
                    }
                    if next != Some(TerminalMenuAction::OpenFolderIn) {
                        menu.editor = None;
                    }
                    if let Some(index) =
                        next.and_then(|action| menu.rows.iter().position(|r| r.action == action))
                    {
                        menu.scroll.scroll_to_item(index);
                    }
                }
                cx.notify();
            }
            "enter" | "return" | "space" => {
                if in_transfer {
                    let choice = menu
                        .transfer
                        .as_ref()
                        .and_then(|transfer| transfer.focused.clone());
                    if let Some(choice) = choice {
                        self.choose_terminal_transfer(choice, window, cx);
                    }
                } else if in_editor {
                    let choice = menu.editor.as_ref().and_then(|editor| editor.focused);
                    if let Some(choice) = choice {
                        self.choose_terminal_editor(choice, window, cx);
                    }
                } else if let Some(action) = menu
                    .selected
                    .filter(|action| menu.enabled_actions().contains(action))
                {
                    self.choose_terminal_menu_action(action, window, cx);
                }
            }
            _ => {}
        }
    }

    fn terminal_menu_hover(&mut self, action: Option<TerminalMenuAction>, cx: &mut Context<Self>) {
        let Some(menu) = self.terminal_menu.as_mut() else {
            return;
        };
        if action == Some(TerminalMenuAction::MoveToScreen)
            && menu
                .enabled_actions()
                .contains(&TerminalMenuAction::MoveToScreen)
        {
            let closed_editor = menu.editor.take().is_some();
            let opened_transfer = menu.transfer.is_none();
            if opened_transfer {
                menu.transfer = Some(TransferMenu::default());
            }
            if closed_editor || opened_transfer {
                cx.notify();
            }
        } else if action == Some(TerminalMenuAction::OpenFolderIn)
            && menu
                .enabled_actions()
                .contains(&TerminalMenuAction::OpenFolderIn)
        {
            let closed_transfer = menu.transfer.take().is_some();
            let opened_editor = menu.editor.is_none();
            if opened_editor {
                menu.editor = Some(EditorSubmenu::default());
            }
            if closed_transfer || opened_editor {
                cx.notify();
            }
        } else {
            let closed_transfer = menu.transfer.take().is_some();
            let closed_editor = menu.editor.take().is_some();
            if closed_transfer || closed_editor {
                cx.notify();
            }
        }
    }

    pub(super) fn choose_terminal_menu_action(
        &mut self,
        action: TerminalMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            TerminalMenuAction::MoveToScreen => {
                if let Some(menu) = self.terminal_menu.as_mut() {
                    if menu
                        .enabled_actions()
                        .contains(&TerminalMenuAction::MoveToScreen)
                    {
                        menu.editor = None;
                        menu.transfer.get_or_insert_with(TransferMenu::default);
                        cx.notify();
                    }
                }
                return;
            }
            TerminalMenuAction::OpenFolderIn => {
                if let Some(menu) = self.terminal_menu.as_mut() {
                    if menu
                        .enabled_actions()
                        .contains(&TerminalMenuAction::OpenFolderIn)
                    {
                        menu.transfer = None;
                        menu.editor.get_or_insert_with(EditorSubmenu::default);
                        cx.notify();
                    }
                }
                return;
            }
            _ => {}
        }
        let Some(menu) = self.terminal_menu.take() else {
            return;
        };
        let target = menu.target;
        if !target.valid_for(self, cx) {
            self.focus_current(window, cx);
            cx.notify();
            return;
        }
        match action {
            TerminalMenuAction::Copy => {
                self.focus_current(window, cx);
                target.terminal.update(cx, |terminal, cx| {
                    terminal.local_action(LocalAction::Copy, window, cx)
                });
            }
            TerminalMenuAction::OpenSelection => {
                self.focus_current(window, cx);
                let Some(source) = target.authority else {
                    return;
                };
                let selected = target
                    .terminal
                    .update(cx, |terminal, _| terminal.selection());
                // Trailing blanks of a line are the terminal's, not the text's.
                let text = selected
                    .unwrap_or_default()
                    .lines()
                    .map(str::trim_end)
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = text.trim_end_matches('\n').to_owned();
                if text.is_empty() {
                    return;
                }
                if text.len() > 262_144 {
                    self.status = "The selection is too large for a viewer".into();
                    cx.notify();
                    return;
                }
                let pane = target.route.pane_id().to_owned();
                let Some(model) = self.selected_model() else {
                    return;
                };
                let title = model
                    .snapshot()
                    .and_then(|screen| {
                        screen
                            .panes
                            .values()
                            .find(|entry| entry.id.as_str() == pane)
                    })
                    .and_then(|entry| entry.title.clone())
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| "Selection".to_owned());
                let command = Command::OpenTextViewer {
                    screen: target.screen.screen_id().to_owned(),
                    cwd: transfer::pane_cwd(model, &pane),
                    pane,
                    title,
                    text,
                };
                self.send_captured(source, command, cx);
            }
            TerminalMenuAction::Paste => {
                if target.authority.is_none() {
                    return;
                }
                self.focus_current(window, cx);
                target.terminal.update(cx, |terminal, cx| {
                    terminal.local_action(LocalAction::Paste, window, cx)
                });
            }
            TerminalMenuAction::Find => {
                target.terminal.update(cx, |terminal, cx| {
                    terminal.local_action(LocalAction::Find, window, cx)
                });
            }
            TerminalMenuAction::ClearBuffer => {
                if target.authority.is_none() {
                    return;
                }
                self.focus_current(window, cx);
                target.terminal.update(cx, |terminal, cx| {
                    terminal.local_action(LocalAction::ClearBuffer, window, cx)
                });
            }
            TerminalMenuAction::SplitRight => {
                if let Some(group) = &target.group {
                    self.group_command(group, GroupCommand::Split(Axis::Horizontal), window, cx);
                }
            }
            TerminalMenuAction::SplitDown => {
                if let Some(group) = &target.group {
                    self.group_command(group, GroupCommand::Split(Axis::Vertical), window, cx);
                }
            }
            TerminalMenuAction::Scratchpad => {
                if target.authority.is_some() {
                    self.toggle_scratchpad(target.route.pane_id(), window, cx);
                }
            }
            TerminalMenuAction::CopyDirectory | TerminalMenuAction::CopySshPath => {
                if target.authority.is_none() {
                    return;
                }
                self.focus_current(window, cx);
                let pane_id = target.route.pane_id();
                let cwd = self
                    .selected_model()
                    .and_then(|model| transfer::pane_cwd(model, pane_id));
                let text = if action == TerminalMenuAction::CopyDirectory {
                    cwd
                } else {
                    self.model
                        .as_ref()
                        .filter(|_| self.is_remote_selected())
                        .and_then(|model| {
                            model.source_target(target.route.source_token().source_id())
                        })
                        .zip(cwd)
                        .and_then(|(host, cwd)| transfer::ssh_path(host, &cwd))
                };
                match text {
                    Some(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                    None => self.status = "The working directory is no longer known".into(),
                }
            }
            TerminalMenuAction::ShowDrawer => {
                if target.authority.is_some() {
                    self.open_drawer_overlay(cx);
                    self.focus_current(window, cx);
                }
            }
            TerminalMenuAction::ClosePane => {
                if let Some(source) = target.authority {
                    self.request_close_captured(
                        source,
                        CloseTarget::Pane {
                            screen: target.screen.screen_id().to_owned(),
                            pane: target.route.pane_id().to_owned(),
                        },
                        window,
                        cx,
                    );
                }
            }
            TerminalMenuAction::OpenFolderIn | TerminalMenuAction::MoveToScreen => {}
        }
        cx.notify();
    }

    fn choose_terminal_transfer(
        &mut self,
        choice: TransferChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.terminal_menu.take() else {
            return;
        };
        self.focus_current(window, cx);
        if !menu.target.valid_for(self, cx) {
            cx.notify();
            return;
        }
        let Some(pane) = menu.target.pane else {
            cx.notify();
            return;
        };
        let existing = match &choice {
            TransferChoice::Existing(screen) => Some(screen.as_str()),
            TransferChoice::New => None,
        };
        self.choose_transfer_captured(pane, existing, cx);
    }

    fn choose_terminal_editor(
        &mut self,
        choice: EditorChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.terminal_menu.take() else {
            return;
        };
        self.focus_current(window, cx);
        if !menu.target.valid_for(self, cx) {
            cx.notify();
            return;
        }
        let pane_id = menu.target.route.pane_id();
        let Some(cwd) = self
            .selected_model()
            .and_then(|model| transfer::pane_cwd(model, pane_id))
        else {
            self.status = "The working directory is no longer known".into();
            cx.notify();
            return;
        };
        let remote = self.is_remote_selected();
        let remote_target =
            self.model.as_ref().filter(|_| remote).and_then(|model| {
                model.source_target(menu.target.route.source_token().source_id())
            });
        match transfer::editor_uri(choice, remote_target, &cwd, PathKind::Folder) {
            Ok(uri) => Self::open_editor(choice, &uri, &cwd, remote, cx),
            Err(reason) => {
                self.status = format!("Cannot open folder in {}: {reason}", choice.label())
            }
        }
        cx.notify();
    }

    pub(super) fn open_editor(
        choice: EditorChoice,
        uri: &str,
        cwd: &str,
        remote: bool,
        cx: &mut Context<Self>,
    ) {
        if matches!(choice, EditorChoice::Zed) {
            let mut candidates = Vec::new();
            if let Some(home) = std::env::var_os("HOME") {
                let home = std::path::PathBuf::from(home);
                candidates.push(home.join(".local/bin/zed"));
                candidates.push(home.join(".local/zed.app/bin/zed"));
            }
            candidates.push(std::path::PathBuf::from("/usr/local/bin/zed"));
            candidates.push(std::path::PathBuf::from("/usr/bin/zed"));
            candidates.push(std::path::PathBuf::from("zed"));

            for path in candidates {
                let mut cmd = std::process::Command::new(&path);
                if remote {
                    cmd.arg(uri);
                } else {
                    cmd.arg(cwd);
                }
                if cmd.spawn().is_ok() {
                    return;
                }
            }
        }
        cx.open_url(uri);
    }

    pub(super) fn render_terminal_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.terminal_menu.as_ref()?;
        let viewport = window.viewport_size();
        let max_height = menu_style::panel_max_height(window);
        let colors = &self.appearance;
        let in_transfer = menu.keyboard_in_transfer();
        let in_editor = menu.keyboard_in_editor();
        let mut panel = menu_style::panel(colors)
            .id("terminal-context-menu")
            .when(cfg!(all(test, feature = "ui-test-support")), |panel| {
                panel.debug_selector(|| "terminal-context-menu".into())
            })
            .max_w(chrome_rems(300.))
            .max_h(max_height)
            .overflow_y_scroll()
            .track_scroll(&menu.scroll)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .on_scroll(cx.listener(|_, _, _, cx| cx.stop_propagation()));

        for entry in &menu.rows {
            if entry.separator_before {
                panel = panel.child(menu_style::separator(colors));
            }
            let action = entry.action;
            let submenu_open = (action == TerminalMenuAction::MoveToScreen
                && menu.transfer.is_some())
                || (action == TerminalMenuAction::OpenFolderIn && menu.editor.is_some());
            let selected =
                (menu.focus_visible && !in_transfer && !in_editor && menu.selected == Some(action))
                    || submenu_open;
            let selector = format!("terminal-context-{}", action.key());
            let id = SharedString::from(selector.clone());
            let label = menu_style::label(colors, entry.label, entry.detail.clone());
            let meta = (entry.shortcut.is_some() || entry.has_submenu).then(|| {
                let mut meta = menu_style::meta();
                if let Some(shortcut) = entry.shortcut {
                    meta = meta.child(menu_style::shortcut(colors, shortcut));
                }
                if entry.has_submenu {
                    meta = meta.child(menu_style::chevron());
                }
                meta
            });

            let element = if entry.enabled {
                menu_style::item(colors, selected, entry.danger)
                    .id(id)
                    .when(cfg!(all(test, feature = "ui-test-support")), |row| {
                        row.debug_selector(move || selector.clone())
                    })
                    .child(label)
                    .when_some(meta, |row, meta| row.child(meta))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.terminal_menu_hover(Some(action), cx);
                        }
                    }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.choose_terminal_menu_action(action, window, cx);
                    }))
                    .when(action == TerminalMenuAction::MoveToScreen, |row| {
                        let bounds = menu.move_row_bounds.clone();
                        row.child(
                            canvas(move |row, _, _| bounds.set(Some(row)), |_, _, _, _| {})
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full(),
                        )
                    })
                    .when(action == TerminalMenuAction::OpenFolderIn, |row| {
                        let bounds = menu.editor_row_bounds.clone();
                        row.child(
                            canvas(move |row, _, _| bounds.set(Some(row)), |_, _, _, _| {})
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full(),
                        )
                    })
                    .into_any_element()
            } else {
                menu_style::disabled_item(colors)
                    .id(id)
                    .when(cfg!(all(test, feature = "ui-test-support")), |row| {
                        row.debug_selector(move || selector.clone())
                    })
                    .child(label)
                    .when_some(meta, |row, meta| row.child(meta))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.terminal_menu_hover(None, cx);
                        }
                    }))
                    .into_any_element()
            };
            panel = panel.child(element);
        }

        let panel = menu_style::placed(
            menu_style::Placement::Pointer(menu.position),
            Some(menu.panel_bounds.clone()),
            panel,
        );

        let transfer_submenu = menu.transfer.as_ref().map(|transfer| {
            let mut sub = menu_style::submenu_panel(colors)
                .id("terminal-context-move-menu")
                .when(cfg!(all(test, feature = "ui-test-support")), |sub| {
                    sub.debug_selector(|| "terminal-context-move-menu".into())
                })
                .max_w(chrome_rems(300.))
                .max_h(max_height)
                .overflow_y_scroll()
                .track_scroll(&transfer.scroll)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
                .on_scroll(cx.listener(|_, _, _, cx| cx.stop_propagation()));

            let has_existing = menu
                .transfer_choices
                .iter()
                .any(|(choice, _)| matches!(choice, TransferChoice::Existing(_)));
            for (choice, label) in &menu.transfer_choices {
                if matches!(choice, TransferChoice::New) && has_existing {
                    sub = sub.child(menu_style::separator(colors));
                }
                let selected = menu.focus_visible && transfer.focused.as_ref() == Some(choice);
                let id = match choice {
                    TransferChoice::Existing(screen) => {
                        format!("terminal-context-transfer-{screen}")
                    }
                    TransferChoice::New => "terminal-context-transfer-new".to_owned(),
                };
                let selector = id.clone();
                let picked = choice.clone();
                sub = sub.child(
                    menu_style::item(colors, selected, false)
                        .id(SharedString::from(id))
                        .when(cfg!(all(test, feature = "ui-test-support")), |row| {
                            row.debug_selector(move || selector.clone())
                        })
                        .child(menu_style::label(colors, label.clone(), None))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            this.choose_terminal_transfer(picked.clone(), window, cx);
                        })),
                );
            }
            menu_style::placed(
                menu_style::Placement::Beside {
                    parent: menu.panel_bounds.clone(),
                    row: menu.move_row_bounds.clone(),
                },
                None,
                sub,
            )
        });

        let editor_submenu = menu.editor.as_ref().map(|editor| {
            let mut sub = menu_style::submenu_panel(colors)
                .id("terminal-context-editor-menu")
                .when(cfg!(all(test, feature = "ui-test-support")), |sub| {
                    sub.debug_selector(|| "terminal-context-editor-menu".into())
                })
                .max_w(chrome_rems(300.))
                .max_h(max_height)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                );

            for (choice, uri) in &menu.editor_choices {
                let enabled = uri.is_some();
                let selected = menu.focus_visible && editor.focused == Some(*choice);
                let id = format!("terminal-context-editor-{}", choice.key());
                let selector = id.clone();
                let picked = *choice;
                let element = if enabled {
                    menu_style::item(colors, selected, false)
                        .id(SharedString::from(id))
                        .when(cfg!(all(test, feature = "ui-test-support")), |row| {
                            row.debug_selector(move || selector.clone())
                        })
                        .child(menu_style::label(colors, choice.label(), None))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            this.choose_terminal_editor(picked, window, cx);
                        }))
                        .into_any_element()
                } else {
                    menu_style::disabled_item(colors)
                        .id(SharedString::from(id))
                        .when(cfg!(all(test, feature = "ui-test-support")), |row| {
                            row.debug_selector(move || selector.clone())
                        })
                        .child(menu_style::label(colors, choice.label(), None))
                        .into_any_element()
                };
                sub = sub.child(element);
            }
            menu_style::placed(
                menu_style::Placement::Beside {
                    parent: menu.panel_bounds.clone(),
                    row: menu.editor_row_bounds.clone(),
                },
                None,
                sub,
            )
        });

        Some(
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("terminal-context-backdrop")
                        .when(cfg!(all(test, feature = "ui-test-support")), |backdrop| {
                            backdrop.debug_selector(|| "terminal-context-backdrop".into())
                        })
                        .w(viewport.width)
                        .h(viewport.height)
                        .occlude()
                        .on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.cancel_terminal_menu(window, cx);
                        }))
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .child(panel)
                        .children(transfer_submenu)
                        .children(editor_submenu),
                ),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }
}
