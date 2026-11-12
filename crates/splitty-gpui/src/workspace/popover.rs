//! Small panels that open beside the Drawer: a Screen's actions, a host's
//! Screens, the hosts to connect to and a host's ports. They share the
//! context menus' look (`menu_style`) and one keyboard model: arrows move,
//! Enter chooses, Escape and Tab close.
use super::drawer_ports::{host_indicator, HostIndicator};
use super::hosts::{admit_withdraw_host_connect, display, HostAction, HostGroupTarget, HostRow};
use super::ports::OpenOrigin;
use super::*;
use gpui_kit::base::DeferredPopover;
use splitty_client_core::hub::HostManagementRequest;
use splitty_client_core::ports::{ForwardRequest, ForwardState, MappingKey};
use splitty_client_core::workspace::HostActionTarget;

/// Host and port panels: wide enough for a target and its status.
const PANEL_WIDTH: f32 = 292.;
/// Lists: label 12px over a 10.5px detail, a 13 px leading slot.
const LEAD_SLOT: f32 = 13.;
const HEAD_TEXT: f32 = 12.5;
const NOTE_TEXT: f32 = 11.;
/// Trailing row buttons: a 22 px square with a 12 px icon.
const ROW_ACTION: f32 = 22.;
const ROW_ACTION_ICON: f32 = 12.;
/// The filter field: 28 px tall.
const FIELD_HEIGHT: f32 = 28.;
/// Ports listed before the list points to the manager.
pub(super) const MAX_MENU_PORTS: usize = 12;
const MAX_MENU_HOSTS: usize = 24;

pub(super) type Run = Rc<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

#[derive(Clone)]
pub(super) enum Lead {
    None,
    Icon(IconName),
    Dot(HostIndicator),
}

/// A small button at the end of a row, beside the row's own action.
#[derive(Clone)]
pub(super) struct RowAction {
    icon: IconName,
    label: SharedString,
    run: Run,
}

#[derive(Clone)]
pub(super) struct MenuEntry {
    /// Identifies the entry while the panel is open; also its test selector.
    key: SharedString,
    label: SharedString,
    detail: Option<SharedString>,
    lead: Lead,
    meta: Option<SharedString>,
    enabled: bool,
    danger: bool,
    separator_before: bool,
    /// The panel stays open: the entry changes what it shows.
    keep_open: bool,
    actions: Vec<RowAction>,
    run: Run,
}

impl MenuEntry {
    pub(super) fn new(
        key: impl Into<SharedString>,
        label: impl Into<SharedString>,
        run: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            detail: None,
            lead: Lead::None,
            meta: None,
            enabled: true,
            danger: false,
            separator_before: false,
            keep_open: false,
            actions: Vec::new(),
            run: Rc::new(run),
        }
    }
    pub(super) fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
    pub(super) fn icon(mut self, icon: IconName) -> Self {
        self.lead = Lead::Icon(icon);
        self
    }
    fn dot(mut self, indicator: HostIndicator) -> Self {
        self.lead = Lead::Dot(indicator);
        self
    }
    pub(super) fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }
    pub(super) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub(super) fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    pub(super) fn separated(mut self) -> Self {
        self.separator_before = true;
        self
    }
    fn separated_if(mut self, separated: bool) -> Self {
        self.separator_before = separated;
        self
    }
    fn keep_open(mut self) -> Self {
        self.keep_open = true;
        self
    }
    fn action(
        mut self,
        icon: IconName,
        label: impl Into<SharedString>,
        run: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    ) -> Self {
        self.actions.push(RowAction {
            icon,
            label: label.into(),
            run: Rc::new(run),
        });
        self
    }
}

pub(super) enum PopoverKind {
    /// A fixed list of commands, as a context menu.
    Actions(Vec<MenuEntry>),
    /// The known hosts and a field for a new SSH target.
    Connect,
    /// One host: its state and its Screens.
    Host(String),
    /// The ports of a host with Screens open here.
    Ports(HostGroupTarget),
    /// The local ports shared with such a host.
    Shares(HostGroupTarget),
}

#[derive(Clone, Copy)]
pub(super) enum PopoverAnchor {
    Pointer(Point<Pixels>),
    /// Beside the Drawer, level with the row that opened it.
    Row(Bounds<Pixels>),
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Muted,
    Warning,
    Danger,
}

#[derive(Default)]
struct PopoverContent {
    /// A host's state, shown before its title.
    indicator: Option<HostIndicator>,
    title: Option<SharedString>,
    subtitle: Option<SharedString>,
    status: Option<(SharedString, Tone)>,
    caption: Option<SharedString>,
    notes: Vec<SharedString>,
    entries: Vec<MenuEntry>,
}

pub(super) struct DrawerPopover {
    id: Uuid,
    owner: Option<SourceToken>,
    kind: PopoverKind,
    anchor: PopoverAnchor,
    /// The Screen whose row opened an action menu; its row keeps its actions.
    screen: Option<ScreenKey>,
    /// Opened from the Drawer's Hosts item, which stays selected.
    from_nav: bool,
    /// Names the control that opened an action menu, which stays selected.
    tag: Option<&'static str>,
    input: Option<Entity<InputState>>,
    /// Index among the enabled entries.
    selected: usize,
    focus_visible: bool,
    error: Option<String>,
    connect_pending: Option<(Uuid, String)>,
    /// Where focus returns when the keyboard opened the panel.
    restore: Option<FocusHandle>,
    scroll: ScrollHandle,
    _input_changes: Option<Subscription>,
    _open: DeferredPopover,
    _keys: Subscription,
}

/// The remote port a typed filter names, if it is one.
/// A typed share: a local port, or `local:host port`.
fn typed_share(query: &str) -> Option<(u16, u16)> {
    match query.split_once(':') {
        Some((local, remote)) => Some((typed_port(local.trim())?, typed_port(remote.trim())?)),
        None => typed_port(query).map(|port| (port, port)),
    }
}

fn typed_port(query: &str) -> Option<u16> {
    (!query.is_empty() && query.len() <= 5 && query.bytes().all(|b| b.is_ascii_digit()))
        .then(|| query.parse::<u16>().ok())
        .flatten()
        .filter(|port| *port > 0)
}

/// A Screen's observation as the lists word it.
fn screen_detail(detail: &str) -> Option<String> {
    let detail = detail
        .strip_prefix("Live Screen · ")
        .unwrap_or(detail)
        .replace("Last observed · ", "Last seen · ");
    let detail = match detail.strip_suffix("1 panes") {
        Some(rest) if !rest.ends_with(|c: char| c.is_ascii_digit()) => format!("{rest}1 pane"),
        _ => detail,
    };
    (!detail.contains("unknown")).then_some(detail)
}

impl Workspace {
    /// The host a panel shows, while it shows one.
    pub(super) fn drawer_popover_host(&self) -> Option<&str> {
        match &self.drawer_popover.as_ref()?.kind {
            PopoverKind::Host(target) => Some(target),
            _ => None,
        }
    }

    pub(super) fn drawer_popover_screen(&self) -> Option<&ScreenKey> {
        self.drawer_popover.as_ref()?.screen.as_ref()
    }

    pub(super) fn tag_drawer_popover(&mut self, tag: &'static str) {
        if let Some(popover) = self.drawer_popover.as_mut() {
            popover.tag = Some(tag);
        }
    }

    pub(super) fn drawer_popover_is(&self, tag: &str) -> bool {
        self.drawer_popover
            .as_ref()
            .is_some_and(|popover| popover.tag == Some(tag))
    }

    pub(super) fn hosts_popover_open(&self) -> bool {
        self.drawer_popover
            .as_ref()
            .is_some_and(|popover| popover.from_nav)
    }

    pub(super) fn ports_popover_open(&self, group: &HostGroupTarget) -> bool {
        self.drawer_popover.as_ref().is_some_and(
            |popover| matches!(&popover.kind, PopoverKind::Ports(open) if open == group),
        )
    }

    /// The window bounds a Drawer row last painted at.
    pub(super) fn drawer_anchor(&self, key: &str) -> Option<Bounds<Pixels>> {
        self.drawer_anchors.borrow().get(key).copied()
    }

    /// Records where its parent paints, for a panel to open beside.
    pub(super) fn anchor_probe(&self, key: impl Into<SharedString>) -> impl IntoElement {
        let key: SharedString = key.into();
        let anchors = self.drawer_anchors.clone();
        canvas(
            move |bounds, _, _| {
                anchors.borrow_mut().insert(key.clone(), bounds);
            },
            |_, (), _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    pub(super) fn open_drawer_popover(
        &mut self,
        kind: PopoverKind,
        anchor: PopoverAnchor,
        screen: Option<ScreenKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.rename.is_some()
            || self
                .pending_close
                .as_ref()
                .is_some_and(|pending| pending.prompting)
            || cx.has_active_drag()
        {
            return;
        }
        let restore = window
            .last_input_was_keyboard()
            .then(|| window.focused(cx))
            .flatten();
        self.drop_drawer_popover(cx);
        self.profile_picker = None;
        self.tab_context_menu = None;
        self.terminal_menu = None;
        let owner = cx.entity().downgrade();
        let menu_window = window.window_handle();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() == menu_window {
                let _ = owner.update(cx, |this, cx| {
                    this.drawer_popover_key(&event.keystroke, window, cx)
                });
            }
        });
        self.drawer_screen_menu = screen.clone();
        if matches!(kind, PopoverKind::Connect | PopoverKind::Host(_)) {
            self.hosts.command_error = None;
        }
        self.drawer_popover = Some(DrawerPopover {
            id: Uuid::new_v4(),
            owner: self.lifecycle_token(),
            kind,
            anchor,
            screen,
            from_nav: false,
            tag: None,
            input: None,
            selected: 0,
            focus_visible: false,
            error: None,
            connect_pending: None,
            restore,
            scroll: ScrollHandle::new(),
            _input_changes: None,
            _open: GlobalState::register_deferred_popover(cx),
            _keys: keys,
        });
        self.arm_drawer_popover(window, cx);
        cx.notify();
    }

    /// Give the panel the field its kind filters with, and the focus.
    fn arm_drawer_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popover) = self.drawer_popover.as_mut() else {
            return;
        };
        let placeholder = match &popover.kind {
            PopoverKind::Connect => Some("Find a host or enter user@host[:port]"),
            PopoverKind::Ports(_) => Some("Port to open in the browser"),
            PopoverKind::Shares(_) => Some("Local port to share, e.g. 11434 or 11434:8080"),
            PopoverKind::Actions(_) | PopoverKind::Host(_) => None,
        };
        popover.selected = 0;
        popover.error = None;
        popover.focus_visible = placeholder.is_some();
        popover.scroll = ScrollHandle::new();
        let Some(placeholder) = placeholder else {
            popover.input = None;
            popover._input_changes = None;
            self.focus.focus(window, cx);
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let id = popover.id;
        popover._input_changes = Some(cx.subscribe(
            &input,
            move |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Some(popover) = this.drawer_popover.as_mut().filter(|p| p.id == id) {
                        popover.selected = 0;
                        popover.error = None;
                        cx.notify();
                    }
                }
            },
        ));
        input.read(cx).focus_handle(cx).focus(window, cx);
        popover.input = Some(input);
    }

    /// Forget the panel without moving focus or the Drawer.
    pub(super) fn drop_drawer_popover(&mut self, cx: &mut Context<Self>) -> Option<DrawerPopover> {
        let popover = self.drawer_popover.take()?;
        self.drawer_screen_menu = None;
        if let (Some((request_id, _)), Some(owner)) = (&popover.connect_pending, popover.owner) {
            let _ = admit_withdraw_host_connect(&self.commands, owner, *request_id);
        }
        if matches!(popover.kind, PopoverKind::Connect | PopoverKind::Host(_))
            && self.destination != Destination::Hosts
        {
            // What the panel connected only to look at is released with it.
            self.hosts.pending_open = None;
            if let Some(owner) = popover
                .owner
                .filter(|owner| self.lifecycle_token() == Some(*owner))
            {
                let _ = self
                    .commands
                    .try_send(WorkerCommand::new(owner, Command::ReleaseHostBrowsing));
            }
        }
        cx.notify();
        Some(popover)
    }

    pub(super) fn close_drawer_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popover) = self.drop_drawer_popover(cx) else {
            return;
        };
        // The Drawer stayed revealed for its panel; hide it again once the
        // pointer has left it.
        if self.drawer_overlay_open
            && !self.has_drawer_panel()
            && window.mouse_position().x
                > chrome_rems(self.drawer_width as f32).to_pixels(window.rem_size())
        {
            self.close_drawer_overlay(cx);
        }
        match popover.restore {
            Some(handle) if self.drawer_visible || self.drawer_overlay_open => {
                handle.focus(window, cx)
            }
            _ if self.destination == Destination::Hosts => {
                self.host_browser_focus.focus(window, cx)
            }
            _ => self.focus_current(window, cx),
        }
    }

    /// Drop a panel whose subject left the window.
    pub(super) fn reconcile_drawer_popover(
        &mut self,
        model: &WindowWorkspace,
        cx: &mut Context<Self>,
    ) {
        let Some(popover) = self.drawer_popover.as_ref() else {
            return;
        };
        let current = popover.owner == Some(model.lifecycle_token())
            && match &popover.kind {
                PopoverKind::Ports(group) | PopoverKind::Shares(group) => group.is_current(model),
                PopoverKind::Actions(_) => popover
                    .screen
                    .as_ref()
                    .is_none_or(|screen| model.opened().iter().any(|entry| entry.key() == screen)),
                PopoverKind::Connect | PopoverKind::Host(_) => true,
            };
        if !current {
            // The owner may be gone: nothing is left to withdraw from it.
            if let Some(popover) = self.drawer_popover.as_mut() {
                popover.connect_pending = None;
                popover.owner = None;
            }
            self.drop_drawer_popover(cx);
        }
    }

    /// The last Screen of a host group is closing: its ports panel goes too.
    pub(super) fn revoke_ports_popover_for_screen(
        &mut self,
        screen: &ScreenKey,
        cx: &mut Context<Self>,
    ) {
        let last = self.drawer_popover.as_ref().is_some_and(|popover| {
            matches!(&popover.kind, PopoverKind::Ports(group) | PopoverKind::Shares(group)
            if group.source_id() == screen.source_id()
                && self.model.as_ref().is_none_or(|model| {
                    !model.opened().iter().any(|entry| {
                        entry.key().source_id() == screen.source_id() && entry.key() != screen
                    })
                }))
        });
        if last {
            self.drop_drawer_popover(cx);
        }
    }

    pub(super) fn toggle_hosts_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hosts_popover_open() {
            self.close_drawer_popover(window, cx);
            return;
        }
        let Some(anchor) = self.drawer_anchor("nav-hosts") else {
            return;
        };
        self.refresh_hosts(cx);
        self.open_drawer_popover(
            PopoverKind::Connect,
            PopoverAnchor::Row(anchor),
            None,
            window,
            cx,
        );
        if let Some(popover) = self.drawer_popover.as_mut() {
            popover.from_nav = true;
        }
    }

    /// Show one host in the panel; `connect` also starts or retries it.
    pub(super) fn popover_show_host(
        &mut self,
        target: &str,
        connect: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(popover) = self.drawer_popover.as_mut() else {
            return;
        };
        popover.kind = PopoverKind::Host(target.to_owned());
        popover.connect_pending = None;
        self.hosts.command_error = None;
        self.arm_drawer_popover(window, cx);
        if connect {
            // A host that was closed here comes back with its Screens.
            if let Some(owner) = self.lifecycle_token() {
                if self.begin_host_restore(owner, target.to_owned(), cx) {
                    return;
                }
            }
            let action = self
                .project_host_rows()
                .into_iter()
                .find_map(|row| match row {
                    HostRow::Host {
                        target: row,
                        action,
                        ..
                    } if row == target => action,
                    _ => None,
                });
            if let Some(action @ (HostAction::Connect { .. } | HostAction::Retry(_))) = action {
                self.host_action(action, cx);
            }
        }
        cx.notify();
    }

    fn popover_show_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popover) = self.drawer_popover.as_mut() else {
            return;
        };
        popover.kind = PopoverKind::Connect;
        self.hosts.pending_open = None;
        self.arm_drawer_popover(window, cx);
        cx.notify();
    }

    /// Ask the local service to admit a target the user typed.
    fn popover_connect_new(&mut self, target: &str, cx: &mut Context<Self>) {
        let Some(owner) = self.local_model().map(SourceWorkspace::source_token) else {
            return;
        };
        let request = match HostManagementRequest::connect(target, None) {
            Ok(request) => request,
            Err(error) => {
                if let Some(popover) = self.drawer_popover.as_mut() {
                    popover.error = Some(error.to_string());
                }
                cx.notify();
                return;
            }
        };
        let canonical = request.target().to_owned();
        let request_id = Uuid::new_v4();
        let sent = self.commands.try_send(WorkerCommand::new(
            owner,
            Command::ManageHost {
                request_id,
                request,
            },
        ));
        if let Some(popover) = self.drawer_popover.as_mut() {
            match sent {
                Ok(()) => {
                    popover.connect_pending = Some((request_id, canonical));
                    popover.error = None;
                }
                Err(error) => popover.error = Some(format!("Connect was not sent: {error}")),
            }
        }
        cx.notify();
    }

    /// The answer to `popover_connect_new`; true when the panel asked for it.
    pub(super) fn popover_connect_result(
        &mut self,
        request_id: Uuid,
        target: &str,
        applied: Option<bool>,
        reason: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(popover) = self.drawer_popover.as_mut().filter(|popover| {
            popover
                .connect_pending
                .as_ref()
                .is_some_and(|(id, pending)| *id == request_id && pending == target)
        }) else {
            return false;
        };
        match applied {
            None => {}
            Some(true) => self.popover_show_host(target, false, window, cx),
            Some(false) => {
                popover.connect_pending = None;
                popover.error = Some(format!(
                    "Could not connect: {}",
                    reason.unwrap_or("The local service did not confirm this request")
                ));
                cx.notify();
            }
        }
        true
    }

    fn popover_create_target(&self, target: &str) -> Option<(HostActionTarget, SourceToken)> {
        let model = self.model.as_ref()?;
        let action = model
            .hosts()
            .map(|host| host.action_target())
            .find(|action| action.target() == target)?;
        let source = model.execution_token(action.source_id())?;
        (self.drawer_host_actions_available()
            && !self
                .remote_create_enqueues
                .contains_key(&action.source_id())
            && model
                .validate_remote_screen_creation(action, source)
                .is_ok())
        .then(|| (action.clone(), source))
    }

    fn popover_query(&self, popover: &DrawerPopover, cx: &App) -> String {
        popover
            .input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_owned())
            .unwrap_or_default()
    }

    fn connect_content(&self, popover: &DrawerPopover, cx: &App) -> PopoverContent {
        let query = self.popover_query(popover, cx);
        let needle = query.to_lowercase();
        let mut content = PopoverContent::default();
        let mut known = false;
        let mut hidden = 0;
        for row in self.project_host_rows() {
            let HostRow::Host {
                target,
                title,
                alias,
                state,
                ..
            } = row
            else {
                continue;
            };
            known |= target == query;
            if !format!("{title} {target}").to_lowercase().contains(&needle) {
                continue;
            }
            if content.entries.len() >= MAX_MENU_HOSTS {
                hidden += 1;
                continue;
            }
            let indicator = host_indicator(self.model.as_ref().and_then(|model| {
                model
                    .source_for_target(&target)
                    .and_then(|id| model.host(id))
            }));
            let pick = target.clone();
            let mut entry =
                MenuEntry::new(format!("host-{target}"), title, move |this, window, cx| {
                    this.popover_show_host(&pick, true, window, cx)
                })
                .dot(indicator)
                .keep_open()
                .enabled(popover.connect_pending.is_none());
            if alias.is_some() {
                entry = entry.detail(display(&target));
            }
            let remembered = self.remembered_host_screens(&target).len();
            if indicator != HostIndicator::Idle {
                entry = entry.meta(state);
            } else if remembered == 1 {
                entry = entry.meta("1 Screen");
            } else if remembered > 1 {
                entry = entry.meta(format!("{remembered} Screens"));
            }
            content.entries.push(entry);
        }
        if !known && HostManagementRequest::connect(&query, None).is_ok() {
            let typed = query.clone();
            content.entries.insert(
                0,
                MenuEntry::new("connect-new", format!("Connect to {}", display(&query)), {
                    move |this, _, cx| this.popover_connect_new(&typed, cx)
                })
                .icon(IconName::Plus)
                .detail("New SSH host")
                .keep_open()
                .enabled(popover.connect_pending.is_none()),
            );
        }
        if let Some((_, target)) = &popover.connect_pending {
            content.status = Some((
                format!("Preparing {}…", display(target)).into(),
                Tone::Muted,
            ));
        }
        if content.entries.is_empty() {
            content.notes.push(
                if query.is_empty() {
                    "No hosts yet. Enter an SSH target such as user@host."
                } else {
                    "No host matches. Enter a full SSH target to connect."
                }
                .into(),
            );
        }
        if hidden > 0 {
            content
                .notes
                .push(format!("{hidden} more hosts; type to narrow the list.").into());
        }
        content.entries.push(
            MenuEntry::new("manage-hosts", "Manage hosts…", |this, window, cx| {
                this.show_hosts(window, cx)
            })
            .icon(IconName::SlidersHorizontal)
            .separated(),
        );
        content
    }

    fn host_content(&self, target: &str) -> PopoverContent {
        let mut content = PopoverContent::default();
        let owner = self.lifecycle_token();
        let opening = self
            .hosts
            .pending_open
            .as_ref()
            .map(|pending| pending.label());
        let waiting = opening.clone();
        let opening = opening.is_some();
        let mut lifecycle = None;
        for row in self.project_host_rows() {
            match row {
                HostRow::Host {
                    target: row,
                    title,
                    alias,
                    state,
                    daemon_version,
                    detail,
                    action,
                    ..
                } if row == target => {
                    let indicator = host_indicator(self.model.as_ref().and_then(|model| {
                        model
                            .source_for_target(target)
                            .and_then(|id| model.host(id))
                    }));
                    content.indicator = Some(indicator);
                    content.title = Some(title.into());
                    content.subtitle = alias.map(|_| display(target).into());
                    // What the manager spells out, the panel says in one line.
                    let detail = detail
                        .strip_prefix("Current connection work · ")
                        .unwrap_or(&detail)
                        .replace("Last observed", "last seen");
                    content.status = Some((
                        if let Some(waiting) = waiting.clone() {
                            waiting.into()
                        } else {
                            match indicator {
                                HostIndicator::Connected => super::hosts::state_with_version(
                                    &state,
                                    daemon_version.as_deref(),
                                )
                                .into(),
                                HostIndicator::Connecting | HostIndicator::Reconnecting => {
                                    detail.into()
                                }
                                _ => format!("{state} · {detail}").into(),
                            }
                        },
                        match indicator {
                            HostIndicator::Failed => Tone::Danger,
                            HostIndicator::Offline => Tone::Warning,
                            _ => Tone::Muted,
                        },
                    ));
                    lifecycle = action;
                }
                HostRow::Screen {
                    target: row,
                    id,
                    label,
                    detail,
                    route,
                    opened,
                } if row == target => {
                    if matches!(id.as_str(), "empty-live" | "catalog-status" | "unknown") {
                        content.notes.push(format!("{label}. {detail}.").into());
                        continue;
                    }
                    let key = SharedString::from(format!("screen-{id}"));
                    let mut entry = if opened {
                        let screen = self.model.as_ref().and_then(|model| {
                            model
                                .source_for_target(target)
                                .and_then(|source| model.source(source))
                                .map(|source| source.screen_key(&id))
                        });
                        let detach = screen.clone();
                        MenuEntry::new(key, label, move |this, window, cx| {
                            if let Some(screen) = screen.clone() {
                                this.switch_screen_key(screen, window, cx);
                            }
                        })
                        .meta("Open here")
                        .action(
                            IconName::Unplug,
                            "Detach: keeps running on the host",
                            move |this, window, cx| {
                                if let Some(screen) = detach.clone() {
                                    this.request_close_screen(screen, window, cx);
                                }
                            },
                        )
                        .enabled(!opening)
                    } else if let Some((source, screen)) = route {
                        MenuEntry::new(key, label, move |this, window, cx| {
                            this.open_host_screen(source, screen.clone(), window, cx)
                        })
                        .keep_open()
                        .enabled(!opening)
                    } else {
                        let known = self.known_host_screen(target, &id);
                        let (host, screen) = (target.to_owned(), id.clone());
                        MenuEntry::new(key, label, move |this, _, cx| {
                            if let Some(owner) = owner {
                                this.begin_cached_host_open(
                                    owner,
                                    host.clone(),
                                    screen.clone(),
                                    cx,
                                );
                            }
                        })
                        .keep_open()
                        .enabled(known && !opening)
                    };
                    entry = entry.icon(IconName::Terminal);
                    let detail = screen_detail(&detail);
                    let detail = if self.host_screen_blocked(target, &id) {
                        let waiting = super::program_status::WAITING;
                        Some(detail.map_or_else(
                            || waiting.to_owned(),
                            |detail| format!("{waiting} · {detail}"),
                        ))
                    } else {
                        detail
                    };
                    if let Some(detail) = detail {
                        entry = entry.detail(detail);
                    }
                    content.entries.push(entry);
                }
                _ => {}
            }
        }
        if content.title.is_none() {
            content.title = Some(display(target).into());
            content.status = Some(("Not in the host list".into(), Tone::Muted));
        }
        if !content.entries.is_empty() {
            content.caption = Some("Screens".into());
        }
        let mut first = true;
        let mut separated = |entry: MenuEntry| {
            if std::mem::take(&mut first) {
                entry.separated()
            } else {
                entry
            }
        };
        if let Some((source, count)) = self.open_all_host_screens_offer(target) {
            let host = target.to_owned();
            content.entries.push(separated(
                MenuEntry::new(
                    "open-all-screens",
                    format!("Open all {count} Screens"),
                    move |this, window, cx| {
                        this.open_all_host_screens(source, &host, window, cx);
                    },
                )
                .icon(IconName::PanelsTopLeft)
                .detail("The Screens that are not open here")
                .keep_open()
                .enabled(!opening),
            ));
        }
        let remembered = self.remembered_host_screens(target).len();
        if let Some(owner) = owner.filter(|_| remembered > 0 && !opening) {
            let host = target.to_owned();
            content.entries.push(separated(
                MenuEntry::new(
                    "reopen-screens",
                    if remembered == 1 {
                        "Reopen 1 Screen".to_owned()
                    } else {
                        format!("Reopen {remembered} Screens")
                    },
                    move |this, _, cx| {
                        this.begin_host_restore(owner, host.clone(), cx);
                    },
                )
                .icon(IconName::RotateCcw)
                .detail("The Screens that were open here")
                .keep_open(),
            ));
        }
        if let Some((action_target, source)) = self.popover_create_target(target) {
            let host = target.to_owned();
            content.entries.push(separated(
                MenuEntry::new("new-screen", "New Screen", move |this, window, cx| {
                    // Creation is admitted for the host the panel shows.
                    if this.drawer_popover_host() == Some(host.as_str()) {
                        this.create_remote_screen(None, &action_target, source, window, cx);
                        this.close_drawer_popover(window, cx);
                    }
                })
                .icon(IconName::Plus)
                .keep_open()
                .enabled(!opening),
            ));
        }
        if opening {
            content.entries.push(separated(
                MenuEntry::new("cancel-open", "Cancel", |this, _, cx| {
                    this.hosts.pending_open = None;
                    cx.notify();
                })
                .icon(IconName::X)
                .keep_open(),
            ));
        } else if let Some(action) = lifecycle {
            let label = match &action {
                HostAction::Connect { .. } => Some("Connect"),
                HostAction::Cancel(_) => Some("Cancel connecting"),
                HostAction::Retry(_) => Some("Retry connection"),
                HostAction::RetryCatalog(_) => Some("Retry Screen list"),
                HostAction::Release(_) => None,
            };
            if let Some(label) = label {
                let lifecycle_icon = action.clone();
                content.entries.push(separated(
                    MenuEntry::new("host-lifecycle", label, move |this, _, cx| {
                        this.host_action(action.clone(), cx)
                    })
                    .icon(match &lifecycle_icon {
                        HostAction::Cancel(_) => IconName::X,
                        HostAction::Connect { .. } => IconName::Plug,
                        _ => IconName::RefreshCw,
                    })
                    .keep_open(),
                ));
            }
        }
        let open_group = self
            .model
            .as_ref()
            .and_then(|model| HostGroupTarget::capture(model, model.source_for_target(target)?));
        if let Some(group) = open_group {
            content.entries.push(separated(
                MenuEntry::new("close-host", "Close host", move |this, _, cx| {
                    this.close_host(&group, cx)
                })
                .icon(IconName::Unplug)
                .detail("Its Screens keep running")
                .enabled(!opening),
            ));
        }
        let host = target.to_owned();
        if self.hosts.is_saved(target) {
            let on = self.hosts.reconnects(target);
            let reconnect = host.clone();
            content.entries.push(
                MenuEntry::new(
                    "host-reconnect",
                    "Reconnect at start",
                    move |this, _, cx| this.toggle_host_reconnect(&reconnect, cx),
                )
                .icon(IconName::RotateCcw)
                .detail("Brings back the Screens that were open")
                .meta(if on { "On" } else { "Off" })
                .keep_open()
                .separated(),
            );
        }
        content.entries.push(
            MenuEntry::new(
                "host-settings",
                "Host settings…",
                move |this, window, cx| this.manage_host(&host, window, cx),
            )
            .icon(IconName::SlidersHorizontal)
            .separated_if(!self.hosts.is_saved(target)),
        );
        content.entries.push(
            MenuEntry::new("all-hosts", "Other hosts…", |this, window, cx| {
                this.popover_show_connect(window, cx)
            })
            .icon(IconName::Server)
            .keep_open(),
        );
        content
    }

    fn ports_content(
        &self,
        popover: &DrawerPopover,
        group: &HostGroupTarget,
        cx: &App,
    ) -> PopoverContent {
        let query = self.popover_query(popover, cx);
        let needle = query.to_lowercase();
        let snapshot = self.ports.snapshot.as_deref();
        let stale = snapshot.is_some_and(|read| read.active_error().is_some());
        let rows: Vec<_> = self
            .ports
            .rows(self.model.as_ref())
            .into_iter()
            .filter(|row| row.key().target() == group.target())
            .collect();
        let mut content = PopoverContent {
            title: Some("Ports".into()),
            subtitle: Some(display(group.target()).into()),
            ..PopoverContent::default()
        };
        let note = match snapshot {
            None => Some("Loading ports…"),
            Some(read) if !read.is_available() => Some("Port forwarding is unavailable"),
            Some(read) if read.is_loading() => Some("Refreshing ports…"),
            Some(read) if read.active_error().is_some() || read.saved_error().is_some() => {
                Some("Some port information is unavailable")
            }
            Some(_)
                if self
                    .model
                    .as_ref()
                    .and_then(|model| model.source(group.source_id()))
                    .and_then(SourceWorkspace::port_observation)
                    .is_none() =>
            {
                Some("This host has not reported its ports yet")
            }
            Some(_) if rows.is_empty() => Some("No ports found on this host"),
            Some(_) => None,
        };
        content.notes.extend(note.map(SharedString::from));
        let can_manage = self.ports.can_manage();
        let listed = rows
            .iter()
            .any(|row| Some(row.key().remote()) == typed_port(&query));
        if let Some(port) = typed_port(&query).filter(|_| !listed) {
            if let Ok(key) = MappingKey::new(group.target(), port) {
                let host = group.clone();
                content.entries.push(
                    MenuEntry::new(
                        "port-typed",
                        format!("Open port {port}"),
                        move |this, _, cx| this.popover_open_port(&host, &key, None, cx),
                    )
                    .icon(IconName::ExternalLink)
                    .detail("Forward and open in the browser")
                    .enabled(can_manage),
                );
            }
        }
        let mut hidden = 0;
        for row in &rows {
            let remote = row.key().remote();
            let name = row
                .label()
                .filter(|label| !label.is_empty())
                .or(row.process())
                .map(display);
            let matches = needle.is_empty()
                || remote.to_string().starts_with(&needle)
                || name
                    .as_ref()
                    .is_some_and(|name| name.to_lowercase().contains(&needle));
            if !matches {
                continue;
            }
            if content.entries.len() >= MAX_MENU_PORTS {
                hidden += 1;
                continue;
            }
            let pending = self.ports.pending(row.key());
            let up = row.state() == ForwardState::Up;
            let detail = if pending {
                "Working…".to_owned()
            } else {
                match (row.state(), row.local()) {
                    (ForwardState::Up, Some(local)) if stale => {
                        format!("Last known at localhost:{local}")
                    }
                    (ForwardState::Up, Some(local)) => format!("Forwarded to localhost:{local}"),
                    (ForwardState::Up, None) => "Forwarded".into(),
                    (ForwardState::Rebuilding, _) => "Reconnecting…".into(),
                    (ForwardState::Failed, _) => "Failed · opens a new forward".into(),
                    (ForwardState::Paused, _) => "Paused · forwards and opens".into(),
                    (ForwardState::Saved | ForwardState::Detected, _) => {
                        "Forwards and opens in the browser".into()
                    }
                }
            };
            let (host, key, local) = (group.clone(), row.key().clone(), row.local());
            let mut entry = MenuEntry::new(
                format!("port-{remote}"),
                match name {
                    Some(name) => format!("{remote} · {name}"),
                    None => remote.to_string(),
                },
                move |this, _, cx| this.popover_open_port(&host, &key, local, cx),
            )
            .icon(IconName::ExternalLink)
            .detail(detail)
            .enabled(can_manage && !pending);
            if let (true, Some(local)) = (up && can_manage && !pending, row.local()) {
                let key = row.key().clone();
                entry = entry
                    .action(
                        IconName::Copy,
                        format!("Copy localhost:{local}"),
                        move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                "localhost:{local}"
                            )));
                        },
                    )
                    .action(IconName::X, "Stop forwarding", move |this, _, cx| {
                        this.submit_forward(
                            ForwardRequest::stop(key.clone()),
                            OpenOrigin::Hosts,
                            cx,
                        )
                    });
            }
            content.entries.push(entry);
        }
        if !content.entries.is_empty() {
            content.caption = Some("Open in the browser".into());
        }
        if hidden > 0 {
            content
                .notes
                .push(format!("{hidden} more ports; type to narrow the list.").into());
        }
        let host = group.clone();
        content.entries.push(
            MenuEntry::new(
                "share-ports",
                "Share a local port…",
                move |this, window, cx| this.show_shares(&host, window, cx),
            )
            .icon(IconName::ArrowUp)
            .detail("Let programs on this host reach a service here")
            .separated()
            .keep_open(),
        );
        let host = group.clone();
        content.entries.push(
            MenuEntry::new(
                "manage-ports",
                "Manage ports…",
                move |this, window, cx| {
                    this.show_host_group(&host, super::hosts::HostSection::Ports, window, cx);
                },
            )
            .icon(IconName::SlidersHorizontal),
        );
        content
    }

    /// Forward `key` if it is not, then open it in the browser.
    fn popover_open_port(
        &mut self,
        group: &HostGroupTarget,
        key: &MappingKey,
        local: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        if cx.has_active_drag() || self.ports.pending(key) {
            return;
        }
        match ForwardRequest::open(key.clone(), local, false, None) {
            Ok(request) => self.submit_forward(request, OpenOrigin::Drawer(group.clone()), cx),
            Err(error) => {
                self.ports.link_notice = Some(error.to_string());
                cx.notify();
            }
        }
    }

    fn popover_content(&self, popover: &DrawerPopover, cx: &App) -> PopoverContent {
        match &popover.kind {
            PopoverKind::Actions(entries) => PopoverContent {
                entries: entries.clone(),
                ..PopoverContent::default()
            },
            PopoverKind::Connect => self.connect_content(popover, cx),
            PopoverKind::Host(target) => self.host_content(target),
            PopoverKind::Ports(group) => self.ports_content(popover, group, cx),
            PopoverKind::Shares(group) => self.shares_content(popover, group, cx),
        }
    }

    /// The local ports shared with one host, and the field to share another.
    fn shares_content(
        &self,
        popover: &DrawerPopover,
        group: &HostGroupTarget,
        cx: &App,
    ) -> PopoverContent {
        use splitty_client_core::hub::{is_administrative_port, ShareRequest, ShareState};
        let query = self.popover_query(popover, cx);
        let host = display(group.target());
        let mut content = PopoverContent {
            title: Some("Shared local ports".into()),
            subtitle: Some(host.clone().into()),
            ..PopoverContent::default()
        };
        let snapshot = self.shares.as_deref();
        match snapshot {
            None => content.notes.push("Loading shares…".into()),
            Some(read) if !read.is_loaded() => content.notes.push("Loading shares…".into()),
            Some(read) => {
                if let Some(error) = read.read_error() {
                    content
                        .notes
                        .push(format!("Shares could not be read: {}", display(error)).into());
                }
            }
        }
        let target = group.target().to_owned();
        let typed = typed_share(&query);
        if let Some((local, remote)) = typed {
            let listed =
                snapshot.is_some_and(|read| read.on(&target).any(|share| share.remote == remote));
            if !listed {
                let request = ShareRequest::Share {
                    target: target.clone(),
                    remote,
                    local,
                    remember: true,
                };
                let pending = snapshot.is_some_and(|read| read.pending(&target, remote));
                let failure = snapshot.and_then(|read| read.failure(&target, remote));
                // The consequence is stated in full, above the choice.
                content.notes.push(
                    if is_administrative_port(local) {
                        format!(
                            "Warning: port {local} is an administrative service. Every program running as you on {host} could use it."
                        )
                    } else {
                        format!(
                            "Programs running as you on {host} will be able to use localhost:{local} of this computer."
                        )
                    }
                    .into(),
                );
                content.entries.push(
                    MenuEntry::new(
                        "share-typed",
                        if local == remote {
                            format!("Share local port {local}")
                        } else {
                            format!("Share local port {local} as {remote}")
                        },
                        move |this, _, cx| this.submit_share(request.clone(), cx),
                    )
                    .icon(IconName::ArrowUp)
                    .detail(if pending {
                        "Working…"
                    } else if failure.is_some() {
                        "Failed · choose to try again"
                    } else {
                        "Shared again whenever this host connects"
                    })
                    .enabled(!pending)
                    .keep_open(),
                );
            }
        } else if !query.is_empty() {
            content
                .notes
                .push("Enter a local port, or local:host port.".into());
        }
        for share in snapshot.into_iter().flat_map(|read| read.on(&target)) {
            let remote = share.remote;
            let pending = snapshot.is_some_and(|read| read.pending(&target, remote));
            let failure = snapshot
                .and_then(|read| read.failure(&target, remote))
                .or(share.error.as_deref());
            let detail = if pending {
                "Working…".to_owned()
            } else if let Some(failure) = failure {
                format!("Failed · {}", display(failure))
            } else {
                match share.state {
                    ShareState::Up => {
                        format!("Host port {remote} reaches localhost:{}", share.local)
                    }
                    ShareState::Rebuilding => "Reconnecting…".into(),
                    ShareState::Saved => "Starts when the host connects".into(),
                    ShareState::Paused => "Stopped".into(),
                }
            };
            let active = share.is_active();
            let toggle = if active {
                ShareRequest::Stop {
                    target: target.clone(),
                    remote,
                }
            } else {
                ShareRequest::Share {
                    target: target.clone(),
                    remote,
                    local: share.local,
                    remember: false,
                }
            };
            let mut entry = MenuEntry::new(
                format!("share-{remote}"),
                if share.local == remote {
                    format!("{remote}")
                } else {
                    format!("{} → {remote}", share.local)
                },
                move |this, _, cx| this.submit_share(toggle.clone(), cx),
            )
            .icon(if active {
                IconName::CircleCheck
            } else {
                IconName::Circle
            })
            .detail(detail)
            .meta(if active { "Stop" } else { "Resume" })
            .enabled(!pending)
            .keep_open();
            if !pending {
                if share.remembered {
                    let forget = ShareRequest::Forget {
                        target: target.clone(),
                        remote,
                    };
                    entry = entry.action(IconName::X, "Stop and forget", move |this, _, cx| {
                        this.submit_share(forget.clone(), cx)
                    });
                } else if active {
                    let remember = ShareRequest::Remember {
                        target: target.clone(),
                        remote,
                        local: share.local,
                    };
                    entry = entry.action(
                        IconName::Star,
                        "Share again when this host connects",
                        move |this, _, cx| this.submit_share(remember.clone(), cx),
                    );
                }
            }
            content.entries.push(entry);
        }
        // A refusal is shown in the service's own words, in full.
        let failed = snapshot.and_then(|read| {
            typed
                .and_then(|(_, remote)| read.failure(&target, remote))
                .or_else(|| {
                    read.on(&target)
                        .find_map(|share| read.failure(&target, share.remote))
                })
        });
        if let Some(reason) = failed {
            content.status = Some((display(reason).into(), Tone::Danger));
        }
        if content.entries.is_empty()
            && snapshot.is_some_and(|read| read.is_loaded())
            && query.is_empty()
        {
            content
                .notes
                .push("Nothing is shared with this host. Type a local port to share it.".into());
        }
        content
    }

    fn submit_share(
        &mut self,
        request: splitty_client_core::hub::ShareRequest,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self.lifecycle_token() else {
            return;
        };
        if let Err(error) = self
            .commands
            .try_send(WorkerCommand::new(owner, Command::ManageShare { request }))
        {
            self.status = format!("Share was not sent: {error}");
            cx.notify();
        }
    }

    /// Show the shares of the host whose ports panel is open, in its place.
    fn show_shares(
        &mut self,
        group: &HostGroupTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((anchor, screen)) = self
            .drawer_popover
            .as_ref()
            .map(|popover| (popover.anchor, popover.screen.clone()))
        else {
            return;
        };
        if let Some(owner) = self.lifecycle_token() {
            let _ = self
                .commands
                .try_send(WorkerCommand::new(owner, Command::RefreshShares));
        }
        self.open_drawer_popover(
            PopoverKind::Shares(group.clone()),
            anchor,
            screen,
            window,
            cx,
        );
    }

    fn run_popover_entry(
        &mut self,
        id: Uuid,
        key: &str,
        action: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(popover) = self
            .drawer_popover
            .as_ref()
            .filter(|popover| popover.id == id)
        else {
            return;
        };
        let Some(entry) = self
            .popover_content(popover, cx)
            .entries
            .into_iter()
            .find(|entry| entry.key.as_ref() == key && entry.enabled)
        else {
            return;
        };
        let (run, keep_open) = match action {
            Some(index) => match entry.actions.get(index) {
                Some(action) => (action.run.clone(), false),
                None => return,
            },
            None => (entry.run.clone(), entry.keep_open),
        };
        if !keep_open {
            self.close_drawer_popover(window, cx);
        }
        run(self, window, cx);
    }

    /// Keys while a panel is open, ahead of every binding. The field keeps
    /// what it edits with; the panel takes what moves and chooses.
    fn drawer_popover_key(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popover) = self.drawer_popover.as_ref() else {
            return;
        };
        if key.modifiers.control || key.modifiers.alt || key.modifiers.platform {
            return;
        }
        let has_field = popover.input.is_some();
        let name = key.key.to_ascii_lowercase();
        match name.as_str() {
            "escape" | "tab" => {
                cx.stop_propagation();
                self.close_drawer_popover(window, cx);
            }
            "up" | "down" => self.move_popover_choice(&name, cx),
            "home" | "end" if !has_field => self.move_popover_choice(&name, cx),
            "enter" | "return" => self.confirm_popover_choice(window, cx),
            "space" if !has_field => self.confirm_popover_choice(window, cx),
            _ if !has_field => cx.stop_propagation(),
            _ => {}
        }
    }

    fn move_popover_choice(&mut self, key: &str, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let Some(popover) = self.drawer_popover.as_ref() else {
            return;
        };
        let entries = self.popover_content(popover, cx).entries;
        let enabled: Vec<_> = (0..entries.len())
            .filter(|index| entries[*index].enabled)
            .collect();
        let Some(last) = enabled.len().checked_sub(1) else {
            return;
        };
        let popover = self.drawer_popover.as_mut().expect("open panel");
        let current = popover.selected.min(last);
        popover.selected = match key {
            "home" => 0,
            "end" => last,
            // The first arrow shows the choice the panel opened on.
            "down" if !popover.focus_visible => current,
            "up" if !popover.focus_visible => last,
            "down" => (current + 1) % (last + 1),
            _ => current.checked_sub(1).unwrap_or(last),
        };
        popover.focus_visible = true;
        let entry = enabled[popover.selected];
        let separators = entries[..=entry]
            .iter()
            .filter(|entry| entry.separator_before)
            .count();
        popover.scroll.scroll_to_item(entry + separators);
        cx.notify();
    }

    fn confirm_popover_choice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let Some(popover) = self.drawer_popover.as_ref() else {
            return;
        };
        if !popover.focus_visible {
            return;
        }
        let id = popover.id;
        let key = self
            .popover_content(popover, cx)
            .entries
            .into_iter()
            .filter(|entry| entry.enabled)
            .nth(popover.selected)
            .map(|entry| entry.key);
        if let Some(key) = key {
            self.run_popover_entry(id, &key, None, window, cx);
        }
    }

    pub(super) fn host_dot(&self, indicator: HostIndicator) -> Div {
        let muted: Hsla = rgb(self.appearance.muted).into();
        let dot = div().flex_none().size(chrome_rems(7.)).rounded_full();
        match indicator {
            HostIndicator::Connected => dot.bg(rgb(self.appearance.success)),
            HostIndicator::Connecting | HostIndicator::Reconnecting => {
                dot.bg(rgb(self.appearance.warning))
            }
            HostIndicator::Failed => dot.bg(rgb(self.appearance.danger)),
            HostIndicator::Offline => dot.bg(muted).opacity(0.6),
            HostIndicator::Idle => dot.border(chrome_rems(1.5)).border_color(muted),
        }
    }

    pub(super) fn render_drawer_popover(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let popover = self.drawer_popover.as_ref()?;
        let content = self.popover_content(popover, cx);
        let colors = &self.appearance;
        let id = popover.id;
        let muted: Hsla = rgb(colors.muted).into();
        let secondary: Hsla = rgb(colors.secondary).into();
        let wide = !matches!(popover.kind, PopoverKind::Actions(_));
        let selected = content
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.enabled)
            .nth(popover.selected)
            .map(|(index, _)| index)
            .filter(|_| popover.focus_visible);
        let mut list = div()
            .id("drawer-popover-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&popover.scroll);
        let mut enabled_index = 0;
        for (index, entry) in content.entries.into_iter().enumerate() {
            if entry.separator_before && index > 0 {
                list = list.child(menu_style::separator(colors));
            }
            let lead = match entry.lead {
                Lead::None => None,
                Lead::Icon(icon) => Some(
                    Icon::new(icon)
                        .size(chrome_rems(LEAD_SLOT))
                        .into_any_element(),
                ),
                Lead::Dot(indicator) => Some(self.host_dot(indicator).into_any_element()),
            };
            let label = div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(chrome_rems(8.))
                .when_some(lead, |row, lead| {
                    row.child(
                        div()
                            .w(chrome_rems(LEAD_SLOT))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .text_color(if entry.danger {
                                rgb(colors.danger).into()
                            } else {
                                muted
                            })
                            .child(lead),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap(chrome_rems(2.))
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(entry.label.clone()),
                        )
                        .when_some(entry.detail.clone(), |label, detail| {
                            let selector = format!("popover-{}-detail-{detail}", entry.key);
                            label.child(
                                menu_style::detail(colors, detail)
                                    .when(cfg!(all(test, feature = "ui-test-support")), |detail| {
                                        detail.debug_selector(move || selector.clone())
                                    }),
                            )
                        }),
                );
            let key = entry.key.clone();
            let selector = format!("popover-{key}");
            let mut meta = menu_style::meta().items_center();
            if let Some(text) = entry.meta.clone() {
                meta = meta.child(menu_style::shortcut(colors, text));
            }
            for (action_index, action) in entry.actions.iter().enumerate() {
                let action_key = key.clone();
                let tooltip = action.label.clone();
                let hover: Hsla = rgba(colors.hover_overlay).into();
                let text: Hsla = rgb(colors.text).into();
                meta = meta.child(
                    div()
                        .id(SharedString::from(format!(
                            "popover-action-{key}-{action_index}"
                        )))
                        .debug_selector({
                            let selector = format!("popover-{key}-action-{action_index}");
                            move || selector.clone()
                        })
                        .flex_none()
                        .size(chrome_rems(ROW_ACTION))
                        .rounded(chrome_rems(4.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(muted)
                        .hover(move |button| button.bg(hover).text_color(text))
                        .child(Icon::new(action.icon).size(chrome_rems(ROW_ACTION_ICON)))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tooltip.clone())
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            this.run_popover_entry(id, &action_key, Some(action_index), window, cx);
                        })),
                );
            }
            let row = if entry.enabled {
                let position = enabled_index;
                enabled_index += 1;
                menu_style::item(colors, selected == Some(index), entry.danger)
                    .id(SharedString::from(format!("popover-entry-{key}")))
                    .debug_selector(move || selector.clone())
                    .when(entry.danger, |row| row.text_color(rgb(colors.danger)))
                    .child(label)
                    .child(meta)
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if let Some(popover) = this
                            .drawer_popover
                            .as_mut()
                            .filter(|popover| *hovered && popover.id == id)
                        {
                            // One highlight: the pointer takes the keyboard's.
                            if popover.input.is_some() {
                                popover.selected = position;
                            } else {
                                popover.focus_visible = false;
                            }
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.run_popover_entry(id, &key, None, window, cx);
                    }))
                    .into_any_element()
            } else {
                menu_style::disabled_item(colors)
                    .id(SharedString::from(format!("popover-entry-{key}")))
                    .debug_selector(move || selector.clone())
                    .child(label)
                    .child(meta)
                    .into_any_element()
            };
            if index == 0 {
                if let Some(caption) = content.caption.clone() {
                    list = list.child(menu_style::caption(colors, caption));
                }
            }
            list = list.child(row);
        }
        let note = |text: SharedString, color: Hsla| {
            div()
                .px(chrome_rems(10.))
                .py(chrome_rems(3.))
                .text_size(chrome_rems(NOTE_TEXT))
                .font_weight(FontWeight::NORMAL)
                .text_color(color)
                .child(text)
        };
        let tone = |tone: Tone| -> Hsla {
            match tone {
                Tone::Muted => muted,
                Tone::Warning => rgb(colors.warning).into(),
                Tone::Danger => rgb(colors.danger).into(),
            }
        };
        let error = popover
            .error
            .clone()
            .or_else(|| self.hosts.command_error.clone().filter(|_| wide));
        let panel = menu_style::panel(colors)
            .id("drawer-popover")
            .debug_selector(|| "drawer-popover".into())
            .when(wide, |panel| panel.w(chrome_rems(PANEL_WIDTH)))
            .max_w(if wide {
                chrome_rems(PANEL_WIDTH)
            } else {
                chrome_rems(300.)
            })
            .max_h(menu_style::panel_max_height(window))
            .when_some(content.title, |panel, title| {
                panel.child(
                    div()
                        .px(chrome_rems(10.))
                        .pt(chrome_rems(6.))
                        .pb(chrome_rems(2.))
                        .flex()
                        .items_center()
                        .gap(chrome_rems(8.))
                        .when_some(content.indicator, |head, indicator| {
                            head.child(self.host_dot(indicator))
                        })
                        .child(
                            div()
                                .flex_none()
                                .max_w_full()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(chrome_rems(HEAD_TEXT))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .when_some(content.subtitle, |head, subtitle| {
                            head.child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(chrome_rems(NOTE_TEXT))
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(secondary)
                                    .child(subtitle),
                            )
                        }),
                )
            })
            .when_some(content.status, |panel, (status, kind)| {
                panel.child(note(status, tone(kind)).pb(chrome_rems(5.)))
            })
            .when_some(popover.input.as_ref(), |panel, input| {
                panel.child(
                    div()
                        .mx(chrome_rems(4.))
                        .mt(chrome_rems(2.))
                        .mb(chrome_rems(4.))
                        .h(chrome_rems(FIELD_HEIGHT))
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
            })
            .when_some(error, |panel, error| {
                panel.child(note(display(&error).into(), tone(Tone::Danger)))
            })
            .children(content.notes.into_iter().map(|text| note(text, muted)))
            .child(list);
        let viewport = window.viewport_size();
        let placement = match popover.anchor {
            PopoverAnchor::Pointer(position) => menu_style::Placement::Pointer(position),
            PopoverAnchor::Row(row) => {
                let drawer = Bounds::new(
                    point(px(0.), px(0.)),
                    size(
                        chrome_rems(self.drawer_width as f32).to_pixels(window.rem_size()),
                        viewport.height,
                    ),
                );
                menu_style::Placement::Beside {
                    parent: Rc::new(Cell::new(Some(drawer))),
                    row: Rc::new(Cell::new(Some(row))),
                }
            }
        };
        Some(
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("drawer-popover-backdrop")
                        .w(viewport.width)
                        .h(viewport.height)
                        .occlude()
                        .on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.close_drawer_popover(window, cx);
                        }))
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .child(menu_style::placed(
                            placement,
                            None,
                            panel
                                .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                                .on_scroll(|_, _, cx| cx.stop_propagation()),
                        )),
                ),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{screen_detail, typed_port};

    #[test]
    fn typed_ports_are_whole_decimal_ports() {
        assert_eq!(typed_port("5173"), Some(5173));
        assert_eq!(typed_port("0"), None);
        assert_eq!(typed_port("65536"), None);
        assert_eq!(typed_port("51 73"), None);
        assert_eq!(typed_port("vite"), None);
        assert_eq!(typed_port(""), None);
    }

    #[test]
    fn screen_details_drop_what_the_list_already_says() {
        assert_eq!(
            screen_detail("Live Screen · 2 panes").as_deref(),
            Some("2 panes")
        );
        assert_eq!(
            screen_detail("Last observed · 1 panes").as_deref(),
            Some("Last seen · 1 pane")
        );
        assert_eq!(
            screen_detail("Live Screen · 11 panes").as_deref(),
            Some("11 panes")
        );
        assert_eq!(screen_detail("Live Screen · pane count unknown"), None);
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod view_tests {
    use crate::workspace::chrome::view_tests::with_view;
    use crate::workspace::{focus_visible, Command, Destination};
    use gpui_kit::{px, size, Modifiers, MouseButton, VisualTestContext};
    use splitty_client_core::hub::{
        HostDirectorySnapshot, HostDirectoryState, HostManagementAction, HostManagementOutcome,
        HostManagementResult, HubUpdate, RememberedScreen, RememberedScreens,
    };
    use splitty_client_core::workspace::WindowWorkspace;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    fn draw(window: &mut VisualTestContext) {
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn click(window: &mut VisualTestContext, selector: &'static str) {
        draw(window);
        let center = window.debug_bounds(selector).expect(selector).center();
        window.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        window.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
        draw(window);
    }

    #[test]
    fn hosts_item_connects_beside_the_drawer_and_the_manager_stays_one_step_away() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let (commands, mut receiver) = mpsc::channel(32);
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                window.resize(size(px(900.), px(620.)));
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(WindowWorkspace::new(owner));
                    workspace.hosts.set_owner(owner);
                    assert!(workspace.hosts.accept(HubUpdate {
                        connection_id: owner,
                        generation: 1,
                        state: HostDirectoryState::Ready(Arc::new(
                            HostDirectorySnapshot::ui_test_fixture(owner, 1, &["beta.example"])
                                .unwrap(),
                        )),
                    }));
                    workspace.drawer_visible = true;
                    cx.notify();
                });
            });
            draw(window);
            while receiver.try_recv().is_ok() {}

            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.toggle_hosts_popover(window, cx)
                })
            });
            draw(window);
            window.update(|_, cx| {
                let workspace = view.read(cx);
                assert!(workspace.hosts_popover_open());
                assert!(
                    workspace.destination == Destination::Screens,
                    "connecting does not need the manager"
                );
            });
            assert!(window.debug_bounds("popover-host-beta.example").is_some());
            while let Ok(command) = receiver.try_recv() {
                assert!(
                    matches!(
                        command.kind(),
                        Command::RefreshForwards | Command::RefreshHostDirectory { .. }
                    ),
                    "listing hosts connects nothing"
                );
            }

            // A typed target is offered first and Enter asks for it.
            window.simulate_input("new.example");
            draw(window);
            assert!(window.debug_bounds("popover-connect-new").is_some());
            window.simulate_keystrokes("enter");
            let sent = receiver.try_recv().expect("Enter connects the typed host");
            let Command::ManageHost {
                request_id,
                request,
            } = sent.kind()
            else {
                panic!("expected a host connect request");
            };
            assert_eq!(request.action(), HostManagementAction::Connect);
            assert_eq!(request.target(), "new.example");
            let (token, request_id) = (sent.source_token().unwrap(), *request_id);
            assert!(receiver.try_recv().is_err());
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.host_management_result(
                        HostManagementResult::ui_test_fixture(
                            token,
                            request_id,
                            "new.example".into(),
                            HostManagementAction::Connect,
                            HostManagementOutcome::Applied,
                            None,
                        ),
                        window,
                        cx,
                    );
                    assert_eq!(
                        workspace.drawer_popover_host(),
                        Some("new.example"),
                        "the panel follows the host it connected"
                    );
                })
            });

            // Escape closes the panel and gives up what it only looked at.
            draw(window);
            window.simulate_keystrokes("escape");
            window.update(|_, cx| assert!(view.read(cx).drawer_popover.is_none()));
            let released: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();
            assert!(released
                .iter()
                .any(|command| matches!(command.kind(), Command::ReleaseHostBrowsing)));
            assert!(released
                .iter()
                .all(|command| !matches!(command.kind(), Command::ManageHost { .. })));

            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.toggle_hosts_popover(window, cx)
                })
            });
            click(window, "popover-manage-hosts");
            window.update(|_, cx| {
                let workspace = view.read(cx);
                assert!(workspace.drawer_popover.is_none());
                assert!(workspace.destination == Destination::Hosts);
            });
        });
    }
    #[test]
    fn the_footer_keys_reach_each_destination_from_the_others_and_leave_their_own() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let (commands, _receiver) = mpsc::channel(64);
            let is_mac = window.update(|window, cx| {
                cx.set_reduce_motion(true);
                window.resize(size(px(900.), px(620.)));
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(WindowWorkspace::new(owner));
                    workspace.hosts.set_owner(owner);
                    workspace.drawer_visible = true;
                    workspace.focus.focus(window, cx);
                    cx.notify();
                });
                cx.platform_info().is_mac()
            });
            draw(window);
            let (hosts, templates) = if is_mac {
                ("cmd-shift-h", "cmd-p")
            } else {
                ("alt-h", "alt-p")
            };
            let settings = "ctrl-,";
            let mut press = |key: &str| {
                window.simulate_keystrokes(key);
                draw(window);
                window.update(|_, cx| {
                    view.update(cx, |workspace, _| {
                        // No worker answers the panel's list request here.
                        if let Some(dialog) = workspace.template_dialog.as_mut() {
                            dialog.pending = None;
                        }
                        (workspace.destination, workspace.template_dialog.is_some())
                    })
                })
            };
            // Each key opens its destination and, pressed again, leaves it.
            assert!(press(hosts) == (Destination::Hosts, false));
            assert!(press(hosts) == (Destination::Screens, false));
            assert!(press(settings) == (Destination::Settings, false));
            assert!(press(settings) == (Destination::Screens, false));
            assert!(press(templates) == (Destination::Screens, true));
            assert!(press(templates) == (Destination::Screens, false));
            // Each destination is reached from the two others.
            assert!(press(settings) == (Destination::Settings, false));
            assert!(press(hosts) == (Destination::Hosts, false));
            assert!(press(settings) == (Destination::Settings, false));
            assert!(press(templates) == (Destination::Screens, true));
            assert!(press(hosts) == (Destination::Hosts, false));
            assert!(press(templates) == (Destination::Screens, true));
            assert!(press(settings) == (Destination::Settings, false));
        });
    }

    #[test]
    fn a_closed_host_keeps_running_and_reopens_its_screens_from_the_hosts_item() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let owner = model.lifecycle_token();
            model
                .ui_test_screen_order_fixture(&[("local", "Local")], None)
                .unwrap();
            let open = model
                .ui_test_screen_order_fixture(
                    &[("api", "api"), ("infra", "infra"), ("logs", "logs")],
                    Some("beta.example"),
                )
                .unwrap();
            let source = open[0].source_id();
            let (commands, mut receiver) = mpsc::channel(32);
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                window.resize(size(px(900.), px(620.)));
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.reconcile(model.clone(), window, cx);
                    workspace.hosts.set_owner(owner.connection_id());
                    assert!(workspace.hosts.accept(HubUpdate {
                        connection_id: owner.connection_id(),
                        generation: 1,
                        state: HostDirectoryState::Ready(Arc::new(
                            HostDirectorySnapshot::ui_test_fixture(
                                owner.connection_id(),
                                1,
                                &["beta.example"],
                            )
                            .unwrap(),
                        )),
                    }));
                    workspace.hosts.remembered =
                        Some(Arc::new(RememberedScreens::ui_test_fixture(
                            ["api", "infra", "logs"]
                                .map(|id| RememberedScreen::new("beta.example", id, Some(id)))
                                .to_vec(),
                        )));
                    workspace.drawer_visible = true;
                    assert!(
                        workspace.remembered_host_screens("beta.example").is_empty(),
                        "an open host has nothing to reopen"
                    );
                    cx.notify();
                });
            });
            draw(window);
            while receiver.try_recv().is_ok() {}

            // The caption offers Close while the pointer is on it.
            let close: &'static str =
                Box::leak(format!("drawer-host-close-{source}").into_boxed_str());
            let caption: &'static str = Box::leak(format!("drawer-host-{source}").into_boxed_str());
            assert!(window.debug_bounds(close).is_none());
            let point = window.debug_bounds(caption).expect(caption).center();
            window.simulate_mouse_move(point, None, Modifiers::default());
            click(window, close);
            let sent = receiver.try_recv().expect("Close host is one command");
            let Command::CloseHost { target } = sent.kind() else {
                panic!("expected the host to be closed as a whole");
            };
            assert_eq!(target.target(), "beta.example");
            assert_eq!(sent.source_token(), Some(owner));
            assert!(
                receiver.try_recv().is_err(),
                "no Screen is ended or detached singly"
            );

            // The worker takes the host out of the window; one Screen ended meanwhile.
            model.detach_all(owner, source).unwrap();
            model.begin_close(source).unwrap();
            model.finish_close(source).unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.reconcile(model.clone(), window, cx);
                    assert_eq!(workspace.remembered_host_screens("beta.example").len(), 3);
                    workspace.toggle_hosts_popover(window, cx);
                })
            });
            draw(window);
            while receiver.try_recv().is_ok() {}
            click(window, "popover-host-beta.example");
            let sent = receiver.try_recv().expect("the host is connected");
            assert!(
                matches!(sent.kind(), Command::ConnectHost { target } if target == "beta.example")
            );
            window.update(|_, cx| {
                let workspace = view.read(cx);
                assert_eq!(workspace.drawer_popover_host(), Some("beta.example"));
                assert!(workspace.hosts.pending_open.is_some());
            });

            let live = model
                .ui_test_screen_order_fixture(
                    &[("api", "api"), ("infra", "infra")],
                    Some("beta.example"),
                )
                .unwrap();
            model.detach_all(owner, live[0].source_id()).unwrap();
            let execution = model.execution_token(live[0].source_id()).unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.reconcile(model.clone(), window, cx)
                })
            });
            draw(window);
            let reopened = std::iter::from_fn(|| receiver.try_recv().ok())
                .find_map(|command| match command.kind() {
                    Command::OpenRemoteScreens { screens } => {
                        Some((command.source_token(), screens.clone()))
                    }
                    _ => None,
                })
                .expect("the remembered Screens open in one step");
            assert_eq!(reopened.0, Some(execution));
            assert_eq!(reopened.1, live, "an ended Screen is left out");
            window.update(|_, cx| {
                let workspace = view.read(cx);
                assert!(workspace.drawer_popover.is_none());
                assert!(workspace.hosts.pending_open.is_none());
            });
        });
    }

    #[test]
    fn a_host_opens_all_its_unopened_screens_in_one_step() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let owner = model.lifecycle_token();
            model
                .ui_test_screen_order_fixture(&[("local", "Local")], None)
                .unwrap();
            let keys = model
                .ui_test_screen_order_fixture(
                    &[
                        ("api", "api"),
                        ("infra", "infra"),
                        ("logs", "logs"),
                        ("web", "web"),
                    ],
                    Some("beta.example"),
                )
                .unwrap();
            let source = keys[0].source_id();
            let execution = model.execution_token(source).unwrap();
            // One Screen that is not open here has its own entry only.
            model.detach(owner, &keys[0]).unwrap();
            let (commands, mut receiver) = mpsc::channel(32);
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                window.resize(size(px(900.), px(620.)));
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.reconcile(model.clone(), window, cx);
                    workspace.hosts.set_owner(owner.connection_id());
                    assert!(workspace.hosts.accept(HubUpdate {
                        connection_id: owner.connection_id(),
                        generation: 1,
                        state: HostDirectoryState::Ready(Arc::new(
                            HostDirectorySnapshot::ui_test_fixture(
                                owner.connection_id(),
                                1,
                                &["beta.example"],
                            )
                            .unwrap(),
                        )),
                    }));
                    workspace.drawer_visible = true;
                    assert_eq!(
                        workspace.unopened_host_screens("beta.example"),
                        Some((execution, vec![keys[0].clone()]))
                    );
                    assert_eq!(workspace.open_all_host_screens_offer("beta.example"), None);
                    workspace.toggle_hosts_popover(window, cx);
                    workspace.popover_show_host("beta.example", false, window, cx);
                });
            });
            draw(window);
            assert!(window.debug_bounds("popover-screen-api").is_some());
            assert!(window.debug_bounds("popover-open-all-screens").is_none());

            // From two on, one entry opens them all: the unopened ones only,
            // in the catalog's order.
            model.detach(owner, &keys[2]).unwrap();
            model.detach(owner, &keys[3]).unwrap();
            let unopened = vec![keys[0].clone(), keys[2].clone(), keys[3].clone()];
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    // A catalog that is not the current one offers nothing.
                    let mut stale = model.clone();
                    let action = stale.host(source).unwrap().action_target().clone();
                    stale.ui_test_host_update_catalog_failed(&action).unwrap();
                    workspace.reconcile(stale, window, cx);
                    assert_eq!(workspace.open_all_host_screens_offer("beta.example"), None);
                    workspace.reconcile(model.clone(), window, cx);
                    assert_eq!(
                        workspace.open_all_host_screens_offer("beta.example"),
                        Some((execution, 3))
                    );
                })
            });
            draw(window);
            while receiver.try_recv().is_ok() {}
            click(window, "popover-open-all-screens");
            let mut sent = std::iter::from_fn(|| receiver.try_recv().ok())
                .filter(|command| !matches!(command.kind(), Command::ReleaseHostBrowsing));
            let command = sent.next().expect("the Screens open with one command");
            let Command::OpenRemoteScreens { screens } = command.kind() else {
                panic!("expected one batch open");
            };
            assert_eq!(screens, &unopened);
            assert_eq!(command.source_token(), Some(execution));
            assert!(sent.next().is_none(), "no Screen is opened singly");
            window.update(|_, cx| {
                let workspace = view.read(cx);
                assert!(workspace.drawer_popover.is_none());
                assert!(workspace.destination == Destination::Screens);
            });

            // The Hosts manager offers the same for the host's group.
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.manage_host("beta.example", window, cx)
                })
            });
            draw(window);
            while receiver.try_recv().is_ok() {}
            click(window, "host-open-all-beta.example");
            let batches: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok())
                .filter_map(|command| match command.kind() {
                    Command::OpenRemoteScreens { screens } => Some(screens.clone()),
                    Command::OpenRemoteScreen { .. } => panic!("no Screen is opened singly"),
                    _ => None,
                })
                .collect();
            assert_eq!(batches, [unopened]);
            window.update(|_, cx| {
                assert!(view.read(cx).destination == Destination::Screens);
            });
        });
    }

    #[test]
    fn a_host_is_chosen_for_reconnecting_at_start_in_its_panel() {
        use splitty_client_core::appearance::test_support::{settings_snapshot, Availability};
        use splitty_client_core::settings::{SettingsField, SettingsValue};
        with_view(|window, view| {
            let model = WindowWorkspace::new(Uuid::new_v4());
            let owner = model.lifecycle_token().connection_id();
            let (commands, mut receiver) = mpsc::channel(32);
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                window.resize(size(px(900.), px(620.)));
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.reconcile(model.clone(), window, cx);
                    workspace.hosts.set_owner(owner);
                    assert!(workspace.hosts.accept(HubUpdate {
                        connection_id: owner,
                        generation: 1,
                        state: HostDirectoryState::Ready(Arc::new(
                            HostDirectorySnapshot::ui_test_fixture(owner, 1, &["beta.example"])
                                .unwrap(),
                        )),
                    }));
                    workspace.apply_appearance(
                        &settings_snapshot(owner, 1, Availability::Full),
                        window,
                        cx,
                    );
                    workspace.drawer_visible = true;
                    assert!(!workspace.hosts.reconnects("beta.example"));
                    // A host that is not in the list cannot be chosen.
                    workspace.toggle_host_reconnect("other.example", cx);
                    assert!(workspace.hosts.reconnect.is_empty());
                    workspace.toggle_hosts_popover(window, cx);
                    workspace.popover_show_host("beta.example", false, window, cx);
                });
            });
            draw(window);
            while receiver.try_recv().is_ok() {}
            click(window, "popover-host-reconnect");
            let sent = std::iter::from_fn(|| receiver.try_recv().ok())
                .find_map(|command| match command.kind() {
                    Command::UpdateSettings { edit, .. } => {
                        Some((edit.field(), edit.value().clone()))
                    }
                    _ => None,
                })
                .expect("the choice is saved as a setting");
            assert_eq!(sent.0, SettingsField::ReconnectHosts);
            assert_eq!(sent.1, SettingsValue::List(vec!["beta.example".into()]));
            window.update(|_, cx| {
                let workspace = view.read(cx);
                assert!(workspace.hosts.reconnects("beta.example"));
                assert_eq!(
                    workspace.drawer_popover_host(),
                    Some("beta.example"),
                    "the panel stays to show the choice"
                );
            });
        });
    }

    #[test]
    fn a_focus_ring_follows_keyboard_traversal_and_a_click_leaves_none() {
        with_view(|window, view| {
            window.update(|window, cx| {
                window.resize(size(px(900.), px(620.)));
                view.update(cx, |workspace, cx| {
                    workspace.model = Some(WindowWorkspace::new(Uuid::new_v4()));
                    workspace.drawer_visible = true;
                    cx.notify();
                });
            });
            draw(window);
            window.update(|_, cx| assert!(!focus_visible(cx)));
            window.simulate_keystrokes("tab");
            window.update(|_, cx| assert!(focus_visible(cx), "Tab moves focus visibly"));
            click(window, "drawer-create");
            window.update(|_, cx| {
                assert!(
                    !focus_visible(cx),
                    "a clicked control keeps no ring after its work is done"
                )
            });
        });
    }

    // Level: in-process (the workspace view with a fixture model).
    #[test]
    fn the_host_panel_names_the_daemon_version_of_a_connected_host() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let (versioned, _) = model
                .ui_test_remote_terminal_fixture("versioned.example.test")
                .unwrap();
            model
                .ui_test_remote_terminal_fixture("plain.example.test")
                .unwrap();
            let action = model
                .host(versioned.source_id())
                .unwrap()
                .action_target()
                .clone();
            model
                .ui_test_host_daemon_version(
                    &action,
                    &serde_json::json!({"daemon":{"version":"0.3.2","protocol":1,"started":false}}),
                )
                .unwrap();
            window.update(|_, cx| {
                view.update(cx, |workspace, _| {
                    workspace.model = Some(model);
                    let status = |target: &str| {
                        workspace
                            .host_content(target)
                            .status
                            .map(|(text, _)| text.to_string())
                    };
                    assert_eq!(
                        status("versioned.example.test").as_deref(),
                        Some("Connected · v0.3.2")
                    );
                    assert_eq!(status("plain.example.test").as_deref(), Some("Connected"));
                });
            });
        });
    }
}
