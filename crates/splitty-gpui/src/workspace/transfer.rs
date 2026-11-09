//! The captured tab's context menu: Close actions over its tab group, its
//! working directory and, for local Screens, moving it to another Screen.
//! Entries, order and keyboard behavior follow the web client's tab menu.
use super::*;
use gpui_kit::base::DeferredPopover;

#[cfg(all(test, feature = "ui-test-support"))]
mod view_tests;

/// A main-menu entry, in the web client's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabMenuAction {
    Close,
    CloseOthers,
    CloseRight,
    CopyDirectory,
    CopySshPath,
    MoveToScreen,
}

impl TabMenuAction {
    /// The suffix of the entry's element id and test selector.
    fn key(self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::CloseOthers => "close-others",
            Self::CloseRight => "close-right",
            Self::CopyDirectory => "copy-cwd",
            Self::CopySshPath => "copy-ssh",
            Self::MoveToScreen => "move",
        }
    }
}

struct TabMenuRow {
    action: TabMenuAction,
    label: &'static str,
    detail: Option<SharedString>,
    enabled: bool,
    separator_before: bool,
}

/// A "Move to Screen" destination.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TransferChoice {
    Existing(String),
    New,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabCloseScope {
    Others,
    Right,
}

/// The open "Move to Screen" submenu.
#[derive(Default)]
struct TransferMenu {
    /// The keyboard focus once it moved into the submenu; the parent row keeps
    /// it otherwise.
    focused: Option<TransferChoice>,
    scroll: ScrollHandle,
}

pub(super) struct TabContextMenu {
    pub(super) source: PaneActionTarget,
    /// The pointer position that opened the menu, in window coordinates.
    position: Point<Pixels>,
    /// The keyboard focus among the main entries. As in the web client it
    /// starts on the first enabled entry and is shown only once the keyboard
    /// moves it.
    selected: Option<TabMenuAction>,
    focus_visible: bool,
    transfer: Option<TransferMenu>,
    scroll: ScrollHandle,
    panel_bounds: menu_style::PlacedBounds,
    move_row_bounds: menu_style::PlacedBounds,
    /// Popup-aware interactions elsewhere step aside while the menu is open.
    _open: DeferredPopover,
    /// Kit's root binds Tab to focus traversal ahead of raw key handlers;
    /// while the menu is open, Tab closes it instead, as in the web client.
    _tab_closes: Subscription,
}

impl TabContextMenu {
    fn keyboard_in_transfer(&self) -> bool {
        self.transfer
            .as_ref()
            .is_some_and(|transfer| transfer.focused.is_some())
    }
}

fn tab_context_matches(
    model: &SourceWorkspace,
    source: SourceToken,
    screen: &ScreenKey,
    pane: &str,
) -> bool {
    model.source_token() == source
        && model.shown_screen_id() == Some(screen.screen_id())
        && model.screen_key(screen.screen_id()) == *screen
        && model.screen_for(pane) == Some(screen.screen_id())
}

/// The tabs of the canonical group holding `pane`, in tab order.
fn tab_group<'a>(model: &'a SourceWorkspace, pane: &str) -> Option<Vec<&'a str>> {
    fn slot_tabs<'a>(node: &'a LayoutNode, pane: &str) -> Option<Vec<&'a str>> {
        match node {
            LayoutNode::Slot { pane_ids, .. } => pane_ids
                .iter()
                .any(|id| id.as_str() == pane)
                .then(|| pane_ids.iter().map(|id| id.as_str()).collect()),
            LayoutNode::Split { children, .. } => {
                children.iter().find_map(|child| slot_tabs(child, pane))
            }
        }
    }
    let screen = model.snapshot()?;
    std::iter::once(&screen.root)
        .chain(screen.floating.iter().map(|entry| &entry.root))
        .find_map(|root| slot_tabs(root, pane))
}

/// The longest working directory the menu copies.
const MAX_CWD_BYTES: usize = 4096;
/// The longest detail text, in characters, as in the web client.
const MAX_DETAIL_CHARS: usize = 120;

/// The working directory the service last reported for the pane's session
/// (or, before that, its pane), when it is plain text that is safe to paste.
fn pane_cwd(model: &SourceWorkspace, pane: &str) -> Option<String> {
    let screen = model.snapshot()?;
    let (_, state) = screen.panes.iter().find(|(id, _)| id.as_str() == pane)?;
    state
        .session_id
        .as_ref()
        .and_then(|session| screen.sessions.get(session))
        .and_then(|session| session.cwd.as_deref())
        .filter(|cwd| !cwd.is_empty())
        .or(state.cwd.as_deref())
        .filter(|cwd| {
            !cwd.is_empty() && cwd.len() <= MAX_CWD_BYTES && !cwd.chars().any(char::is_control)
        })
        .map(str::to_owned)
}

/// A remote folder as scp, rsync and sshfs take it: `target:/path`, or
/// `ssh://target/path` when the target carries a port or an IPv6 literal.
fn ssh_path(target: &str, cwd: &str) -> Option<String> {
    if target.is_empty()
        || !cwd.starts_with('/')
        || target.chars().chain(cwd.chars()).any(char::is_control)
    {
        return None;
    }
    let port = target
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()));
    Some(if port || target.contains('[') {
        format!("ssh://{target}{cwd}")
    } else {
        format!("{target}:{cwd}")
    })
}

/// Untrusted text as one bounded display line.
fn detail_line(text: &str) -> SharedString {
    hosts::display(&text.chars().take(MAX_DETAIL_CHARS).collect::<String>()).into()
}

/// The tab's label as its tab bar shows it.
fn tab_title(model: &SourceWorkspace, pane: &str, index: usize) -> String {
    model
        .snapshot()
        .and_then(|screen| screen.panes.iter().find(|(id, _)| id.as_str() == pane))
        .and_then(|(_, pane)| pane.title.as_deref())
        .filter(|title| !title.is_empty())
        .map(hosts::display)
        .unwrap_or_else(|| format!("Terminal {}", index + 1))
}

/// The next keyboard focus among `items`: arrows wrap, Home and End jump.
fn step<T: Clone + PartialEq>(items: &[T], current: Option<&T>, key: &str) -> Option<T> {
    let last = items.len().checked_sub(1)?;
    let index = current.and_then(|current| items.iter().position(|item| item == current));
    let next = match key {
        "home" => 0,
        "end" => last,
        "down" => index.map_or(0, |index| if index == last { 0 } else { index + 1 }),
        "up" => index.map_or(last, |index| index.checked_sub(1).unwrap_or(last)),
        _ => return current.cloned(),
    };
    items.get(next).cloned()
}

impl Workspace {
    pub(super) fn open_tab_context_menu_captured(
        &mut self,
        source: SourceToken,
        screen: &ScreenKey,
        pane: &str,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .selected_model()
            .is_some_and(|model| tab_context_matches(model, source, screen, pane))
        {
            return;
        }
        if self.rename.is_some()
            || self
                .pending_close
                .as_ref()
                .is_some_and(|pending| pending.prompting)
        {
            return;
        }
        let Some(model) = self.selected_model() else {
            return;
        };
        match model.capture_pane_action(pane) {
            Ok(source) => {
                self.profile_picker = None;
                let owner = cx.entity().downgrade();
                let menu_window = window.window_handle();
                let tab_closes = cx.intercept_keystrokes(move |event, window, cx| {
                    if window.window_handle() == menu_window && event.keystroke.key == "tab" {
                        cx.stop_propagation();
                        let _ = owner.update(cx, |this, cx| {
                            this.dismiss_tab_context_menu(window, cx);
                        });
                    }
                });
                let mut menu = TabContextMenu {
                    source,
                    position,
                    selected: None,
                    focus_visible: false,
                    transfer: None,
                    scroll: ScrollHandle::new(),
                    panel_bounds: Rc::default(),
                    move_row_bounds: Rc::default(),
                    _open: GlobalState::register_deferred_popover(cx),
                    _tab_closes: tab_closes,
                };
                menu.selected = self
                    .tab_menu_rows(&menu)
                    .iter()
                    .find(|row| row.enabled)
                    .map(|row| row.action);
                self.tab_context_menu = Some(menu);
                // Keys reach the workspace's handler while the terminal is
                // not focused; closing returns focus to the terminal.
                self.focus.focus(window, cx);
            }
            Err(error) => self.status = format!("Tab action unavailable: {error}"),
        }
        cx.notify();
    }

    fn dismiss_tab_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_context_menu.take().is_some() {
            self.focus_current(window, cx);
            cx.notify();
        }
    }

    fn tab_menu_can_transfer(&self, menu: &TabContextMenu) -> bool {
        !self.is_remote_selected()
            && self
                .selected_model()
                .is_some_and(|model| model.can_transfer_pane(&menu.source))
    }

    /// The SSH target of the captured tab's remote source.
    fn tab_menu_remote_target(&self, menu: &TabContextMenu) -> Option<&str> {
        self.model
            .as_ref()?
            .source_target(menu.source.source_token().source_id())
    }

    fn tab_menu_rows(&self, menu: &TabContextMenu) -> Vec<TabMenuRow> {
        let Some(model) = self.selected_model() else {
            return Vec::new();
        };
        let pane = menu.source.pane_id.as_str();
        let group = tab_group(model, pane).unwrap_or_default();
        let index = group.iter().position(|tab| *tab == pane);
        let row = |action, label, enabled| TabMenuRow {
            action,
            label,
            detail: None,
            enabled,
            separator_before: false,
        };
        let mut rows = vec![
            row(TabMenuAction::Close, "Close Tab", true),
            row(
                TabMenuAction::CloseOthers,
                "Close Other Tabs",
                group.len() > 1,
            ),
            row(
                TabMenuAction::CloseRight,
                "Close Tabs to the Right",
                index.is_some_and(|index| index + 1 < group.len()),
            ),
        ];
        if let Some(cwd) = pane_cwd(model, pane) {
            rows.push(TabMenuRow {
                separator_before: true,
                ..row(TabMenuAction::CopyDirectory, "Copy Working Directory", true)
            });
            if let Some(path) = self
                .tab_menu_remote_target(menu)
                .and_then(|target| ssh_path(target, &cwd))
            {
                rows.push(TabMenuRow {
                    detail: Some(detail_line(&path)),
                    ..row(TabMenuAction::CopySshPath, "Copy SSH Path", true)
                });
            }
        }
        if self.tab_menu_can_transfer(menu) {
            rows.push(TabMenuRow {
                separator_before: true,
                ..row(TabMenuAction::MoveToScreen, "Move to Screen", true)
            });
        }
        rows
    }

    fn tab_menu_enabled(&self, menu: &TabContextMenu) -> Vec<TabMenuAction> {
        self.tab_menu_rows(menu)
            .into_iter()
            .filter(|row| row.enabled)
            .map(|row| row.action)
            .collect()
    }

    /// The other Screens of the tab's source by name, then a new one.
    fn tab_menu_transfer_choices(&self, menu: &TabContextMenu) -> Vec<(TransferChoice, String)> {
        let mut choices: Vec<_> = self
            .selected_model()
            .map(|model| {
                model
                    .screens()
                    .iter()
                    .filter(|screen| screen.id.as_str() != menu.source.screen_id)
                    .map(|screen| {
                        (
                            TransferChoice::Existing(screen.id.to_string()),
                            screen.name.clone().unwrap_or_else(|| "Screen".to_owned()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let busy = self.pending_transfer.is_some()
            || self
                .selected_model()
                .is_some_and(|model| model.transfer_pending() || model.routes_reconciling());
        choices.push((
            TransferChoice::New,
            if busy {
                "Move in progress"
            } else {
                "New Screen"
            }
            .to_owned(),
        ));
        choices
    }

    /// Scrolls a long panel to keep the keyboard focus in view; separators are
    /// children too.
    fn reveal_tab_menu_focus(&self) {
        let Some(menu) = self.tab_context_menu.as_ref() else {
            return;
        };
        if let Some(transfer) = &menu.transfer {
            if let Some(focused) = &transfer.focused {
                let choices = self.tab_menu_transfer_choices(menu);
                if let Some(index) = choices.iter().position(|(choice, _)| choice == focused) {
                    let separator =
                        usize::from(*focused == TransferChoice::New && choices.len() > 1);
                    transfer.scroll.scroll_to_item(index + separator);
                }
                return;
            }
        }
        let rows = self.tab_menu_rows(menu);
        if let Some(index) = rows
            .iter()
            .position(|row| Some(row.action) == menu.selected)
        {
            let separators = rows[..=index]
                .iter()
                .filter(|row| row.separator_before)
                .count();
            menu.scroll.scroll_to_item(index + separators);
        }
    }

    pub(super) fn tab_context_menu_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let Some(menu) = self.tab_context_menu.as_ref() else {
            return;
        };
        let in_transfer = menu.keyboard_in_transfer();
        let key = key.key.to_ascii_lowercase();
        match key.as_str() {
            "tab" => self.dismiss_tab_context_menu(window, cx),
            "escape" | "left" if in_transfer => self.leave_transfer_menu(cx),
            "escape" => self.dismiss_tab_context_menu(window, cx),
            "right"
                if !in_transfer
                    && menu.selected == Some(TabMenuAction::MoveToScreen)
                    && self
                        .tab_menu_enabled(menu)
                        .contains(&TabMenuAction::MoveToScreen) =>
            {
                self.enter_transfer_menu(cx)
            }
            "up" | "down" | "home" | "end" => {
                if in_transfer {
                    let choices: Vec<_> = self
                        .tab_menu_transfer_choices(menu)
                        .into_iter()
                        .map(|(choice, _)| choice)
                        .collect();
                    let menu = self.tab_context_menu.as_mut().expect("open menu");
                    let transfer = menu.transfer.as_mut().expect("open submenu");
                    transfer.focused = step(&choices, transfer.focused.as_ref(), &key);
                    menu.focus_visible = true;
                } else {
                    let enabled = self.tab_menu_enabled(menu);
                    let menu = self.tab_context_menu.as_mut().expect("open menu");
                    menu.selected = step(&enabled, menu.selected.as_ref(), &key);
                    menu.focus_visible = true;
                    if menu.selected != Some(TabMenuAction::MoveToScreen) {
                        menu.transfer = None;
                    }
                }
                self.reveal_tab_menu_focus();
                cx.notify();
            }
            "enter" | "return" | "space" => {
                let pane = menu.source.pane_id.clone();
                if in_transfer {
                    let choice = menu
                        .transfer
                        .as_ref()
                        .and_then(|transfer| transfer.focused.clone());
                    if let Some(choice) = choice.filter(|choice| {
                        self.tab_menu_transfer_choices(menu)
                            .iter()
                            .any(|(current, _)| current == choice)
                    }) {
                        let existing = match choice {
                            TransferChoice::Existing(screen) => Some(screen),
                            TransferChoice::New => None,
                        };
                        self.choose_transfer(&pane, existing.as_deref(), window, cx);
                    }
                } else if let Some(action) = menu
                    .selected
                    .filter(|action| self.tab_menu_enabled(menu).contains(action))
                {
                    self.choose_tab_menu_action(&pane, action, window, cx);
                }
            }
            _ => {}
        }
    }

    /// Opens the submenu with the keyboard focus on its first entry.
    fn enter_transfer_menu(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.tab_context_menu.as_ref() else {
            return;
        };
        let first = self
            .tab_menu_transfer_choices(menu)
            .into_iter()
            .next()
            .map(|(choice, _)| choice);
        let menu = self.tab_context_menu.as_mut().expect("open menu");
        menu.transfer
            .get_or_insert_with(TransferMenu::default)
            .focused = first;
        menu.focus_visible = true;
        self.reveal_tab_menu_focus();
        cx.notify();
    }

    /// Closes the submenu and returns the keyboard focus to its row.
    fn leave_transfer_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.tab_context_menu.as_mut() {
            menu.transfer = None;
            menu.selected = Some(TabMenuAction::MoveToScreen);
            menu.focus_visible = true;
            cx.notify();
        }
    }

    /// Pointer movement onto a main row opens the submenu of a row that has
    /// one and closes it on any other row; the keyboard focus stays put.
    fn tab_menu_hover(&mut self, action: Option<TabMenuAction>, cx: &mut Context<Self>) {
        let Some(menu) = self.tab_context_menu.as_mut() else {
            return;
        };
        if action == Some(TabMenuAction::MoveToScreen) {
            if menu.transfer.is_none() {
                menu.transfer = Some(TransferMenu::default());
                cx.notify();
            }
        } else if menu.transfer.take().is_some() {
            cx.notify();
        }
    }

    fn tab_menu_click(
        &mut self,
        action: TabMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.tab_context_menu.as_mut() else {
            return;
        };
        menu.selected = Some(action);
        menu.focus_visible = false;
        let pane = menu.source.pane_id.clone();
        let enabled = self
            .tab_context_menu
            .as_ref()
            .is_some_and(|menu| self.tab_menu_enabled(menu).contains(&action));
        if enabled {
            self.choose_tab_menu_action(&pane, action, window, cx);
        }
    }

    fn choose_tab_menu_action(
        &mut self,
        pane: &str,
        action: TabMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            TabMenuAction::Close => self.choose_close_tab(pane, window, cx),
            TabMenuAction::CloseOthers => {
                self.choose_close_tabs(pane, TabCloseScope::Others, window, cx)
            }
            TabMenuAction::CloseRight => {
                self.choose_close_tabs(pane, TabCloseScope::Right, window, cx)
            }
            TabMenuAction::CopyDirectory | TabMenuAction::CopySshPath => {
                self.choose_copy(pane, action, window, cx)
            }
            TabMenuAction::MoveToScreen => {
                if let Some(menu) = self.tab_context_menu.as_mut() {
                    menu.transfer.get_or_insert_with(TransferMenu::default);
                    cx.notify();
                }
            }
        }
    }

    /// Takes the open menu when it belongs to `pane` and returns focus to the
    /// terminal; the chosen action continues from the captured target.
    fn take_tab_menu(
        &mut self,
        pane: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<TabContextMenu> {
        let menu = self.tab_context_menu.take()?;
        if menu.source.pane_id != pane {
            self.tab_context_menu = Some(menu);
            return None;
        }
        self.focus_current(window, cx);
        cx.notify();
        Some(menu)
    }

    fn choose_close_tab(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.take_tab_menu(pane, window, cx) else {
            return;
        };
        let Some(model) = self.selected_model() else {
            return;
        };
        if let Err(error) = model.validate_pane_action(&menu.source) {
            self.status = format!("Tab changed: {error}");
            cx.notify();
            return;
        }
        self.request_close_captured(
            menu.source.source_token(),
            CloseTarget::Pane {
                screen: menu.source.screen_id,
                pane: menu.source.pane_id,
            },
            window,
            cx,
        );
    }

    /// Closes the other tabs of the captured tab's group, or those right of
    /// it, through one assessed and, where needed, confirmed close.
    fn choose_close_tabs(
        &mut self,
        pane: &str,
        scope: TabCloseScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.take_tab_menu(pane, window, cx) else {
            return;
        };
        let Some(model) = self.selected_model() else {
            return;
        };
        if let Err(error) = model.validate_pane_action(&menu.source) {
            self.status = format!("Tab changed: {error}");
            cx.notify();
            return;
        }
        let group = tab_group(model, pane).unwrap_or_default();
        let Some(index) = group.iter().position(|tab| *tab == pane) else {
            return;
        };
        let closing: Vec<String> = match scope {
            TabCloseScope::Others => group
                .iter()
                .filter(|tab| **tab != pane)
                .map(|tab| (*tab).to_owned())
                .collect(),
            TabCloseScope::Right => group[index + 1..]
                .iter()
                .map(|tab| (*tab).to_owned())
                .collect(),
        };
        let titles = closing
            .iter()
            .map(|tab| {
                let index = group.iter().position(|current| current == tab).unwrap_or(0);
                (tab.clone(), tab_title(model, tab, index))
            })
            .collect::<HashMap<_, _>>();
        // Right to left, as the web client closes them.
        let closing: Vec<_> = closing.into_iter().rev().collect();
        self.request_close_tabs_captured(
            menu.source.source_token(),
            &menu.source.screen_id,
            closing,
            &titles,
            window,
            cx,
        );
    }

    fn choose_copy(
        &mut self,
        pane: &str,
        action: TabMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.take_tab_menu(pane, window, cx) else {
            return;
        };
        let Some(model) = self.selected_model() else {
            return;
        };
        if let Err(error) = model.validate_pane_action(&menu.source) {
            self.status = format!("Tab changed: {error}");
            cx.notify();
            return;
        }
        let cwd = pane_cwd(model, pane);
        let text = match action {
            TabMenuAction::CopyDirectory => cwd,
            TabMenuAction::CopySshPath => self
                .tab_menu_remote_target(&menu)
                .zip(cwd)
                .and_then(|(target, cwd)| ssh_path(target, &cwd)),
            _ => None,
        };
        match text {
            Some(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            None => self.status = "The working directory is no longer known".into(),
        }
        cx.notify();
    }

    fn choose_transfer(
        &mut self,
        pane: &str,
        existing: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.take_tab_menu(pane, window, cx) else {
            return;
        };
        let Some(model) = self.selected_model() else {
            return;
        };
        if let Err(error) = model.validate_pane_action(&menu.source) {
            self.status = format!("Move target changed: {error}");
            cx.notify();
            return;
        }
        if self.is_remote_selected() || !model.can_transfer_pane(&menu.source) {
            self.status = "Move unavailable for this terminal".into();
            cx.notify();
            return;
        }
        if model.transfer_pending() || model.routes_reconciling() || self.pending_transfer.is_some()
        {
            self.status = "Screen move is still being verified".into();
            cx.notify();
            return;
        }
        let target = if let Some(screen_id) = existing {
            if screen_id == menu.source.screen_id
                || !model
                    .screens()
                    .iter()
                    .any(|screen| screen.id.as_str() == screen_id)
            {
                self.status = "Move destination changed; open the menu again".into();
                cx.notify();
                return;
            }
            ScreenTransferTarget::Existing {
                screen: model.screen_key(screen_id),
            }
        } else {
            ScreenTransferTarget::New {
                screen_id: Uuid::new_v4().to_string(),
            }
        };
        let request_id = Uuid::new_v4();
        let source_token = menu.source.source_token();
        match self.commands.try_send(WorkerCommand::new(
            source_token,
            Command::TransferPane {
                request_id,
                source: menu.source,
                target,
            },
        )) {
            Ok(()) => {
                self.pending_transfer = Some(PendingTransfer {
                    source: source_token,
                    request_id,
                });
            }
            Err(error) => self.status = format!("Move not sent: {error}"),
        }
        cx.notify();
    }

    /// The open menu as a window overlay: a backdrop that closes it on any
    /// outside press, the panel at the pointer and the open submenu.
    pub(super) fn render_tab_context_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.tab_context_menu.as_ref()?;
        if !self
            .selected_model()
            .is_some_and(|model| model.validate_pane_action(&menu.source).is_ok())
        {
            return None;
        }
        let colors = &self.appearance;
        let pane = menu.source.pane_id.clone();
        let viewport = window.viewport_size();
        let max_height = menu_style::panel_max_height(window);
        let keyboard_in_transfer = menu.keyboard_in_transfer();
        let mut panel = menu_style::panel(colors)
            .id(SharedString::from(format!("tab-context-items-{pane}")))
            .debug_selector(|| "tab-context-menu".into())
            .max_w(chrome_rems(300.))
            .max_h(max_height)
            .overflow_y_scroll()
            .track_scroll(&menu.scroll);
        for row in self.tab_menu_rows(menu) {
            if row.separator_before {
                panel = panel.child(menu_style::separator(colors));
            }
            let action = row.action;
            let id = SharedString::from(format!("tab-context-{}-{pane}", action.key()));
            let selector = format!("tab-context-{}", action.key());
            let label = menu_style::label(colors, row.label, row.detail);
            let element = if row.enabled {
                let expanded = action == TabMenuAction::MoveToScreen && menu.transfer.is_some();
                let focused =
                    menu.focus_visible && !keyboard_in_transfer && menu.selected == Some(action);
                menu_style::item(colors, focused || expanded, action == TabMenuAction::Close)
                    .id(id)
                    .debug_selector(move || selector)
                    .child(label)
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.tab_menu_hover(Some(action), cx);
                        }
                    }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.tab_menu_click(action, window, cx);
                    }))
                    .when(action == TabMenuAction::MoveToScreen, |row| {
                        let bounds = menu.move_row_bounds.clone();
                        row.child(menu_style::meta().child(menu_style::chevron()))
                            .child(
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
                    .debug_selector(move || selector)
                    .child(label)
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.tab_menu_hover(None, cx);
                        }
                    }))
                    .into_any_element()
            };
            panel = panel.child(element);
        }
        let transfer = menu.transfer.as_ref().map(|transfer| {
            let choices = self.tab_menu_transfer_choices(menu);
            let separated = choices.len() > 1;
            let mut submenu = menu_style::submenu_panel(colors)
                .id(SharedString::from(format!("tab-context-move-menu-{pane}")))
                .debug_selector(|| "tab-context-move-menu".into())
                .max_w(chrome_rems(300.))
                .max_h(max_height)
                .overflow_y_scroll()
                .track_scroll(&transfer.scroll);
            for (choice, label) in choices {
                if choice == TransferChoice::New && separated {
                    submenu = submenu.child(menu_style::separator(colors));
                }
                let focused = menu.focus_visible && transfer.focused.as_ref() == Some(&choice);
                let (id, selector, existing) = match choice {
                    TransferChoice::Existing(screen) => (
                        format!("transfer-existing-{pane}-{screen}"),
                        format!("tab-context-transfer-{screen}"),
                        Some(screen),
                    ),
                    TransferChoice::New => (
                        format!("transfer-new-{pane}"),
                        "tab-context-transfer-new".to_owned(),
                        None,
                    ),
                };
                let pane = pane.clone();
                submenu = submenu.child(
                    menu_style::item(colors, focused, false)
                        .id(SharedString::from(id))
                        .debug_selector(move || selector)
                        .child(menu_style::label(colors, label, None))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            this.choose_transfer(&pane, existing.as_deref(), window, cx);
                        })),
                );
            }
            menu_style::placed(
                menu_style::Placement::Beside {
                    parent: menu.panel_bounds.clone(),
                    row: menu.move_row_bounds.clone(),
                },
                None,
                submenu,
            )
        });
        Some(
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("tab-context-backdrop")
                        .w(viewport.width)
                        .h(viewport.height)
                        .occlude()
                        .on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.dismiss_tab_context_menu(window, cx);
                        }))
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .child(menu_style::placed(
                            menu_style::Placement::Pointer(menu.position),
                            Some(menu.panel_bounds.clone()),
                            panel,
                        ))
                        .children(transfer),
                ),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod menu_tests {
    use super::{ssh_path, step};

    #[test]
    fn ssh_paths_follow_the_web_client_forms() {
        assert_eq!(
            ssh_path("demo@192.0.2.9", "/srv/app").as_deref(),
            Some("demo@192.0.2.9:/srv/app")
        );
        assert_eq!(
            ssh_path("devbox:2222", "/srv").as_deref(),
            Some("ssh://devbox:2222/srv")
        );
        assert_eq!(
            ssh_path("demo@[2001:db8::1]", "/srv").as_deref(),
            Some("ssh://demo@[2001:db8::1]/srv")
        );
        assert_eq!(ssh_path("devbox", "relative"), None);
        assert_eq!(ssh_path("", "/srv"), None);
        assert_eq!(ssh_path("devbox", "/srv\n; rm -rf /"), None);
    }

    #[test]
    fn keyboard_steps_wrap_and_jump() {
        let items = ["a", "b", "c"];
        assert_eq!(step(&items, Some(&"c"), "down"), Some("a"));
        assert_eq!(step(&items, Some(&"a"), "up"), Some("c"));
        assert_eq!(step(&items, Some(&"b"), "home"), Some("a"));
        assert_eq!(step(&items, Some(&"a"), "end"), Some("c"));
        assert_eq!(step(&items, None, "down"), Some("a"));
        assert_eq!(step(&items, Some(&"gone"), "up"), Some("c"));
        assert_eq!(step::<&str>(&[], None, "down"), None);
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::tab_context_matches;
    use splitty_client_core::{workspace::SourceWorkspace, EmbeddedService};

    #[tokio::test]
    async fn nonselected_tab_context_stays_valid_only_on_its_captured_source_and_screen() {
        let service = EmbeddedService::start();
        let mut client = service.connect().await.unwrap();
        let connection = client.connection_id();
        let mut model = SourceWorkspace::new(connection);
        model.set_catalog_supported(true);
        model.mark_catalog_dirty();
        model.flush_catalog(&client).unwrap();
        loop {
            let event = client.next_event().await.unwrap().unwrap();
            if event
                .json()
                .is_some_and(|json| json["type"] == "mux.workspace_snapshot")
            {
                model
                    .apply_catalog(connection, event.json().unwrap())
                    .unwrap();
                break;
            }
        }
        let ids = model.create_screen(&client, "Source").unwrap();
        model.flush_catalog(&client).unwrap();
        loop {
            let event = client.next_event().await.unwrap().unwrap();
            if event
                .json()
                .is_some_and(|json| json["type"] == "mux.workspace_snapshot")
            {
                model
                    .apply_catalog(connection, event.json().unwrap())
                    .unwrap();
                break;
            }
        }
        let inactive = uuid::Uuid::new_v4().to_string();
        let snapshot = serde_json::json!({
            "type":"mux.screen_snapshot",
            "screen":{"id":ids.screen_id,"revision":1,
                "root":{"type":"slot","id":"root","paneIds":[ids.pane_id,inactive],"activePaneId":ids.pane_id},
                "panes":{ids.pane_id.clone():{"id":ids.pane_id},inactive.clone():{"id":inactive}},
                "sessions":{}}
        });
        model.apply_snapshot(connection, &snapshot).unwrap();
        let screen = model.screen_key(&ids.screen_id);
        let source = model.source_token();
        assert!(
            !model.is_displayed_pane(&inactive),
            "the context target is deliberately not the selected terminal"
        );
        assert!(tab_context_matches(&model, source, &screen, &inactive));
        assert!(!tab_context_matches(
            &model,
            source,
            &model.screen_key("other"),
            &inactive
        ));
        assert!(!tab_context_matches(
            &model,
            SourceWorkspace::new(uuid::Uuid::new_v4()).source_token(),
            &screen,
            &inactive
        ));
        assert!(!tab_context_matches(&model, source, &screen, "missing"));
        client.finish().await.unwrap();
        service.close().await.unwrap();
    }
}
