//! Owner-bound host directory and retained local host administration.

use super::popover::{MenuEntry, PopoverAnchor, PopoverKind};
use super::*;
use gpui_kit::base::{FocusableExt as _, Selectable as _};
use gpui_kit::component::{ActiveTheme as _, Disableable};
use gpui_kit::{ScrollStrategy, UniformListScrollHandle};
use splitty_client_core::hub::{
    HostDirectoryState, HostManagementAction, HostManagementOutcome, HostManagementResult,
    HubUpdate, RememberedScreens,
};
use splitty_client_core::workspace::{
    CatalogState, HostActionTarget, HostUpdatePlan, HostUpdateState, SourceStatus,
};
use std::{
    collections::{HashMap, HashSet},
    time::{SystemTime, UNIX_EPOCH},
};

mod editor;

/// A rendered Drawer group, independent of remote terminal send readiness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HostGroupTarget {
    owner: SourceToken,
    source: SourceId,
    target: String,
}

impl HostGroupTarget {
    pub(super) fn capture(model: &WindowWorkspace, source: SourceId) -> Option<Self> {
        let target = model.host(source)?.action_target().target().to_owned();
        let captured = Self {
            owner: model.lifecycle_token(),
            source,
            target,
        };
        captured.is_current(model).then_some(captured)
    }

    pub(super) fn is_current(&self, model: &WindowWorkspace) -> bool {
        self.owner == model.lifecycle_token()
            && model
                .host(self.source)
                .is_some_and(|host| host.action_target().target() == self.target.as_str())
            && model
                .opened()
                .iter()
                .any(|entry| entry.key().source_id() == self.source)
    }

    pub(super) fn owner(&self) -> SourceToken {
        self.owner
    }

    pub(super) fn source_id(&self) -> SourceId {
        self.source
    }

    pub(super) fn target(&self) -> &str {
        &self.target
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HostSection {
    Host,
    Ports,
}

struct HostConnectForm {
    owner: SourceToken,
    target: Entity<InputState>,
    alias: Entity<InputState>,
    selected: usize,
    pending: Option<(Uuid, String)>,
    error: Option<String>,
}

struct HostAliasEditor {
    owner: SourceToken,
    target: String,
    input: Entity<InputState>,
    error: Option<String>,
}

struct HostMutation {
    owner: SourceToken,
    request_id: Uuid,
    action: HostManagementAction,
    outcome: HostManagementOutcome,
    reason: Option<String>,
}

pub(super) struct HostUpdatePrompt {
    pub(super) request_id: Uuid,
    pub(super) target: HostActionTarget,
    pub(super) trigger: FocusHandle,
    pub(super) plan: Option<Arc<HostUpdatePlan>>,
    pub(super) error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HostKey {
    Back,
    Refresh,
}

impl HostKey {
    pub(super) fn from_keystroke(key: &Keystroke) -> Option<Self> {
        let mods = key.modifiers;
        if mods.control || mods.alt || mods.platform || mods.function {
            return None;
        }
        match key.key.to_ascii_lowercase().as_str() {
            "escape" => Some(Self::Back),
            "r" => Some(Self::Refresh),
            _ => None,
        }
    }
}

pub(super) struct PendingHostOpen {
    owner: SourceToken,
    target: String,
    /// The Screen the user picked, or the Screens the host had open here.
    screens: Vec<String>,
    restore: bool,
    source: Option<SourceId>,
    observed: bool,
}

impl PendingHostOpen {
    /// What the wait is for, as a panel or the manager says it.
    pub(super) fn label(&self) -> String {
        match (self.restore, self.screens.len()) {
            (false, _) => "Connecting to open the Screen…".into(),
            (true, 1) => "Connecting to reopen 1 Screen…".into(),
            (true, count) => format!("Connecting to reopen {count} Screens…"),
        }
    }
}

pub(super) struct HostsView {
    owner: Option<Uuid>,
    generation: u64,
    state: HostDirectoryState,
    pub(super) command_error: Option<String>,
    scroll: UniformListScrollHandle,
    selected: Option<String>,
    pub(super) section: HostSection,
    unopened_only: bool,
    search: Option<Entity<InputState>>,
    search_subscription: Option<Subscription>,
    pub(super) pending_open: Option<PendingHostOpen>,
    /// The remote Screens the local service remembers as open here.
    pub(super) remembered: Option<Arc<RememberedScreens>>,
    /// The hosts chosen to be reconnected at start, as last asked for.
    pub(super) reconnect: Vec<String>,
    connect: Option<HostConnectForm>,
    connect_subscription: Option<Subscription>,
    alias_editor: Option<HostAliasEditor>,
    alias_subscription: Option<Subscription>,
    mutations: HashMap<String, HostMutation>,
    pub(super) update_prompt: Option<HostUpdatePrompt>,
}

impl Default for HostsView {
    fn default() -> Self {
        Self {
            owner: None,
            generation: 0,
            state: HostDirectoryState::Unavailable,
            command_error: None,
            scroll: UniformListScrollHandle::new(),
            selected: None,
            section: HostSection::Host,
            unopened_only: false,
            search: None,
            search_subscription: None,
            pending_open: None,
            remembered: None,
            reconnect: Vec::new(),
            connect: None,
            connect_subscription: None,
            alias_editor: None,
            alias_subscription: None,
            mutations: HashMap::new(),
            update_prompt: None,
        }
    }
}

impl HostsView {
    const MAX_MUTATIONS: usize = 32;

    pub(super) fn has_open_form(&self) -> bool {
        self.connect.is_some() || self.alias_editor.is_some() || self.update_prompt.is_some()
    }
    pub(super) fn set_owner(&mut self, owner: Uuid) {
        if self.owner == Some(owner) {
            return;
        }
        self.owner = Some(owner);
        self.generation = 0;
        self.state = HostDirectoryState::Unavailable;
        self.command_error = None;
        self.scroll = UniformListScrollHandle::new();
        self.selected = None;
        self.search = None;
        self.search_subscription = None;
        self.pending_open = None;
        self.remembered = None;
        self.reconnect.clear();
        self.unopened_only = false;
        self.section = HostSection::Host;
        self.connect = None;
        self.connect_subscription = None;
        self.alias_editor = None;
        self.alias_subscription = None;
        self.mutations.clear();
        self.update_prompt = None;
    }

    pub(super) fn accept(&mut self, update: HubUpdate) -> bool {
        if !accepts_update(self.owner, self.generation, &self.state, &update) {
            return false;
        }
        self.generation = update.generation;
        let authoritative = matches!(&update.state, HostDirectoryState::Ready(_));
        self.state = update.state;
        self.command_error = None;
        if authoritative {
            self.mutations
                .retain(|_, mutation| mutation.outcome != HostManagementOutcome::Applied);
        }
        true
    }

    fn snapshot(&self) -> Option<&Arc<splitty_client_core::hub::HostDirectorySnapshot>> {
        match &self.state {
            HostDirectoryState::Loading { previous }
            | HostDirectoryState::Failed { previous, .. } => previous.as_ref(),
            HostDirectoryState::Ready(snapshot) => Some(snapshot),
            HostDirectoryState::Unavailable => None,
        }
    }

    /// Whether `target` is in the saved host list.
    pub(super) fn is_saved(&self, target: &str) -> bool {
        self.snapshot().is_some_and(|snapshot| {
            snapshot
                .directory()
                .hosts
                .iter()
                .any(|host| host.target == target)
        })
    }

    pub(super) fn reconnects(&self, target: &str) -> bool {
        self.reconnect.iter().any(|host| host == target)
    }

    pub(super) fn accepted_alias(&self, owner: SourceToken, target: &str) -> Option<&str> {
        if self.owner != Some(owner.connection_id()) {
            return None;
        }
        self.snapshot()
            .filter(|snapshot| snapshot.connection_id() == owner.connection_id())?
            .directory()
            .hosts
            .iter()
            .find(|host| host.target == target)
            .and_then(|host| host.alias.as_deref())
    }

    fn can_refresh(&self) -> bool {
        self.owner.is_some()
            && !matches!(
                self.state,
                HostDirectoryState::Unavailable | HostDirectoryState::Loading { .. }
            )
    }

    fn can_record_mutation(&self, target: &str) -> bool {
        self.mutations.contains_key(target)
            || self.mutations.len() < Self::MAX_MUTATIONS
            || self
                .mutations
                .values()
                .any(|mutation| mutation.outcome != HostManagementOutcome::Pending)
    }

    fn record_mutation(&mut self, target: String, mutation: HostMutation) {
        if !self.mutations.contains_key(&target) && self.mutations.len() >= Self::MAX_MUTATIONS {
            if let Some(oldest_terminal) = self
                .mutations
                .iter()
                .find(|(_, mutation)| mutation.outcome != HostManagementOutcome::Pending)
                .map(|(target, _)| target.clone())
            {
                self.mutations.remove(&oldest_terminal);
            }
        }
        self.mutations.insert(target, mutation);
    }
}

fn accepts_update(
    owner: Option<Uuid>,
    generation: u64,
    current: &HostDirectoryState,
    update: &HubUpdate,
) -> bool {
    if owner != Some(update.connection_id) || update.generation < generation {
        return false;
    }
    if update.generation == generation
        && !matches!(
            current,
            HostDirectoryState::Loading { .. } | HostDirectoryState::Unavailable
        )
        && matches!(update.state, HostDirectoryState::Loading { .. })
    {
        return false;
    }
    let valid_snapshot = |snapshot: &splitty_client_core::hub::HostDirectorySnapshot| {
        snapshot.connection_id() == update.connection_id
            && snapshot.generation() <= update.generation
    };
    match &update.state {
        HostDirectoryState::Loading { previous } | HostDirectoryState::Failed { previous, .. } => {
            previous.as_deref().is_none_or(valid_snapshot)
        }
        HostDirectoryState::Ready(snapshot) => {
            valid_snapshot(snapshot) && snapshot.generation() == update.generation
        }
        HostDirectoryState::Unavailable => update.generation == 0,
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Observation {
    Unknown,
    Empty,
    Cached,
}

fn observation(last_seen: Option<u64>, screens: usize) -> Observation {
    if screens != 0 {
        Observation::Cached
    } else if last_seen.is_some() {
        Observation::Empty
    } else {
        Observation::Unknown
    }
}

fn failure_status(has_previous: bool, reason: &str) -> String {
    if has_previous {
        format!("Refresh failed: {reason} · showing cached observations")
    } else {
        format!("Could not load hosts: {reason}")
    }
}

fn age(timestamp: u64, now: u64) -> String {
    if timestamp > now {
        return "time unavailable".into();
    }
    let seconds = now - timestamp;
    if seconds < 60 {
        "just now".into()
    } else if seconds < 3_600 {
        format!("{} min ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{} h ago", seconds / 3_600)
    } else {
        format!("{} d ago", seconds / 86_400)
    }
}

pub(super) fn display(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_control()
                || matches!(ch, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                '\u{fffd}'
            } else {
                ch
            }
        })
        .collect()
}

#[derive(Clone)]
pub(super) enum HostAction {
    Connect { owner: SourceToken, target: String },
    Cancel(HostActionTarget),
    Retry(HostActionTarget),
    RetryCatalog(HostActionTarget),
    Release(HostActionTarget),
}

#[derive(Clone)]
pub(super) enum HostRow {
    Host {
        target: String,
        title: String,
        state: String,
        detail: String,
        action: Option<HostAction>,
        update: Option<HostActionTarget>,
        update_blocked: bool,
        refresh_backend: bool,
        saved: bool,
        alias: Option<String>,
        feedback: Option<String>,
        mutation_pending: bool,
        owner: Option<SourceToken>,
    },
    Screen {
        target: String,
        id: String,
        label: String,
        detail: String,
        route: Option<(SourceToken, ScreenKey)>,
        opened: bool,
    },
    PortHeader {
        target: String,
        owner: Option<SourceToken>,
    },
    Port {
        row: splitty_client_core::ports::ForwardRow,
        owner: Option<SourceToken>,
        feedback: Option<String>,
        feedback_outcome: Option<splitty_client_core::ports::ForwardOutcome>,
        pending: bool,
    },
    PortPending {
        key: splitty_client_core::ports::MappingKey,
        owner: Option<SourceToken>,
        feedback: String,
    },
}

fn admit_host_open(
    commands: &mpsc::Sender<WorkerCommand>,
    current: Option<SourceToken>,
    captured: SourceToken,
    key: &ScreenKey,
    still_live: bool,
) -> Result<bool, String> {
    if current != Some(captured) || !still_live {
        return Ok(false);
    }
    commands
        .try_send(WorkerCommand::new(
            captured,
            Command::OpenRemoteScreen {
                screen: key.clone(),
            },
        ))
        .map_err(|error| error.to_string())?;
    Ok(true)
}

pub(super) fn admit_withdraw_host_connect(
    commands: &mpsc::Sender<WorkerCommand>,
    owner: SourceToken,
    request_id: Uuid,
) -> Result<(), String> {
    commands
        .try_send(WorkerCommand::new(
            owner,
            Command::WithdrawHostConnect { request_id },
        ))
        .map_err(|error| error.to_string())
}

fn invoke_host_menu_for_owner(
    captured: SourceToken,
    current: Option<SourceToken>,
    action: impl FnOnce(),
) {
    if current == Some(captured) {
        action();
    }
}

impl Workspace {
    pub(super) fn place_host_group(
        &mut self,
        owner: SourceToken,
        source: SourceId,
        before: Option<SourceId>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(model) = self.model.as_ref().filter(|m| m.lifecycle_token() == owner) else {
            return false;
        };
        let mut groups = super::host_drag::opened_host_groups(model);
        let Some(index) = groups.iter().position(|id| *id == source) else {
            return false;
        };
        if before == Some(source) || before.is_some_and(|id| !groups.contains(&id)) {
            return false;
        }
        let Some(target) = model.source_target(source).map(str::to_owned) else {
            return false;
        };
        groups.remove(index);
        let at = before
            .and_then(|id| groups.iter().position(|item| *item == id))
            .unwrap_or(groups.len());
        groups.insert(at, source);
        let mut order: Vec<_> = groups
            .iter()
            .filter_map(|source| model.source_target(*source).map(str::to_owned))
            .collect();
        if let Some(snapshot) = self.hosts.snapshot() {
            for target in &snapshot.directory().order {
                if !order.contains(target) {
                    order.push(target.clone());
                }
            }
        }
        let request =
            match splitty_client_core::hub::HostManagementRequest::set_order(&target, order) {
                Ok(request) => request,
                Err(error) => {
                    self.status = error.to_string();
                    cx.notify();
                    return false;
                }
            };
        if !self.hosts.can_record_mutation(&target) {
            return false;
        }
        let request_id = Uuid::new_v4();
        if let Err(error) = self.commands.try_send(WorkerCommand::new(
            owner,
            Command::ManageHost {
                request_id,
                request,
            },
        )) {
            self.status = format!("Host order was not sent: {error}");
            cx.notify();
            return false;
        }
        self.hosts.record_mutation(
            target,
            HostMutation {
                owner,
                request_id,
                action: HostManagementAction::SetOrder,
                outcome: HostManagementOutcome::Pending,
                reason: None,
            },
        );
        true
    }

    fn ensure_host_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hosts.search.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search hosts…"));
        self.hosts.search_subscription =
            Some(cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        self.hosts.search = Some(input);
    }

    pub(super) fn host_search_focused(&self, window: &Window, cx: &App) -> bool {
        self.hosts
            .search
            .as_ref()
            .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    fn render_host_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let owner = self.lifecycle_token();
        let mut tabs = div()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border);
        for (section, label, icon) in [
            (HostSection::Host, "Screens", IconName::PanelsTopLeft),
            (HostSection::Ports, "Ports", IconName::SlidersHorizontal),
        ] {
            tabs = tabs.child(
                Button::new(label)
                    .focus_ring(focus_visible(cx))
                    .xsmall()
                    .h(chrome_rems(24.))
                    .ghost()
                    .label(label)
                    .icon(Icon::new(icon))
                    .selected(self.hosts.section == section)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.lifecycle_token() == owner {
                            this.hosts.section = section;
                            this.hosts
                                .scroll
                                .scroll_to_item_strict(0, ScrollStrategy::Top);
                            cx.notify();
                        }
                    })),
            );
        }
        if self.hosts.section == HostSection::Host {
            tabs = tabs.child(div().flex_1()).child(
                Button::new("hosts-unopened")
                    .focus_ring(focus_visible(cx))
                    .xsmall()
                    .h(chrome_rems(24.))
                    .ghost()
                    .debug_selector(|| "hosts-unopened".into())
                    .label(if self.hosts.unopened_only {
                        "Not open here ✓"
                    } else {
                        "Not open here"
                    })
                    .selected(self.hosts.unopened_only)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.lifecycle_token() == owner {
                            this.hosts.unopened_only = !this.hosts.unopened_only;
                            this.hosts
                                .scroll
                                .scroll_to_item_strict(0, ScrollStrategy::Top);
                            cx.notify();
                        }
                    })),
            );
        }
        tabs.into_any_element()
    }

    pub(super) fn known_host_screen(&self, target: &str, id: &str) -> bool {
        self.model
            .as_ref()
            .and_then(|m| m.source_for_target(target).and_then(|s| m.source(s)))
            .and_then(SourceWorkspace::live_catalog)
            .is_some_and(|rows| rows.iter().any(|s| s.id.as_str() == id))
            || self.hosts.snapshot().is_some_and(|snapshot| {
                snapshot
                    .directory()
                    .hosts
                    .iter()
                    .any(|h| h.target == target && h.screens.iter().any(|s| s.id == id))
            })
    }

    /// The Screens `target` had open in this window when it was closed or the
    /// app quit. Empty while the host has Screens open here.
    pub(super) fn remembered_host_screens(&self, target: &str) -> Vec<String> {
        let Some(remembered) = &self.hosts.remembered else {
            return Vec::new();
        };
        let open_here = self.model.as_ref().is_some_and(|model| {
            model
                .source_for_target(target)
                .and_then(|source| model.source(source))
                .is_some_and(|source| !source.screens().is_empty())
        });
        if open_here {
            return Vec::new();
        }
        remembered
            .on(target)
            .map(|screen| screen.screen_id.clone())
            .collect()
    }

    pub(super) fn begin_cached_host_open(
        &mut self,
        owner: SourceToken,
        target: String,
        screen: String,
        cx: &mut Context<Self>,
    ) {
        if self.known_host_screen(&target, &screen) {
            self.begin_host_open(owner, target, vec![screen], false, cx);
        }
    }

    /// Connect `target` and reopen the Screens it had open here. False when
    /// nothing is remembered for it.
    pub(super) fn begin_host_restore(
        &mut self,
        owner: SourceToken,
        target: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let screens = self.remembered_host_screens(&target);
        if screens.is_empty() {
            return false;
        }
        self.begin_host_open(owner, target, screens, true, cx);
        self.hosts.pending_open.is_some()
    }

    fn begin_host_open(
        &mut self,
        owner: SourceToken,
        target: String,
        screens: Vec<String>,
        restore: bool,
        cx: &mut Context<Self>,
    ) {
        if self.lifecycle_token() != Some(owner)
            || !(self.destination == Destination::Hosts
                || self.drawer_popover_host() == Some(target.as_str()))
        {
            return;
        }
        let model = self.model.as_ref().expect("checked local owner");
        let source = model.source_for_target(&target);
        let host = source.and_then(|s| model.host(s));
        let command = match host {
            Some(host) if host.can_retry() => Some(Command::RetryHost {
                target: host.action_target().clone(),
            }),
            Some(host) if host.status() == SourceStatus::Closing => {
                self.hosts.command_error =
                    Some("This host is closing. Try again when it is ready.".into());
                cx.notify();
                return;
            }
            Some(_) => None,
            None => Some(Command::ConnectHost {
                target: target.clone(),
            }),
        };
        self.hosts.selected = Some(target.clone());
        self.hosts.pending_open = Some(PendingHostOpen {
            owner,
            target,
            screens,
            restore,
            source,
            observed: false,
        });
        self.hosts.command_error = None;
        if let Some(command) = command {
            if let Err(error) = self.commands.try_send(WorkerCommand::new(owner, command)) {
                self.hosts.pending_open = None;
                self.hosts.command_error = Some(format!("Open could not be sent: {error}"));
            }
        }
        cx.notify();
    }

    pub(super) fn reconcile_host_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut pending) = self.hosts.pending_open.take() else {
            return;
        };
        let Some(model) = self
            .model
            .as_ref()
            .filter(|m| m.lifecycle_token() == pending.owner)
        else {
            return;
        };
        let in_browser = self.destination == Destination::Hosts
            && self.hosts.selected.as_deref() == Some(pending.target.as_str());
        if !in_browser && self.drawer_popover_host() != Some(pending.target.as_str()) {
            return;
        }
        let Some(source) = model.source_for_target(&pending.target) else {
            self.hosts.pending_open = Some(pending);
            return;
        };
        if pending.source.is_some_and(|old| old != source) {
            return;
        }
        pending.source = Some(source);
        let Some(host) = model.host(source) else {
            return;
        };
        if matches!(
            host.status(),
            SourceStatus::Connecting | SourceStatus::Reconnecting | SourceStatus::Ready
        ) {
            pending.observed = true;
        }
        if matches!(host.status(), SourceStatus::Offline | SourceStatus::Closing)
            && pending.observed
        {
            self.hosts.command_error =
                Some("The screen could not be opened. Retry its host connection.".into());
            return;
        }
        if host.status() == SourceStatus::Ready && host.catalog_state() == CatalogState::Failed {
            self.hosts.command_error =
                Some("The screen catalog is unavailable. Retry the catalog to continue.".into());
            return;
        }
        if host.status() != SourceStatus::Ready || host.catalog_state() != CatalogState::Ready {
            self.hosts.pending_open = Some(pending);
            return;
        }
        let Some(source_model) = model.source(source) else {
            return;
        };
        let live = source_model.live_catalog().unwrap_or_default();
        let present: Vec<ScreenKey> = pending
            .screens
            .iter()
            .filter(|id| live.iter().any(|screen| screen.id.as_str() == id.as_str()))
            .map(|id| source_model.screen_key(id))
            .collect();
        if present.is_empty() {
            self.hosts.command_error = Some(
                if pending.restore {
                    "The Screens that were open here have ended on the host."
                } else {
                    "This screen is no longer on the host. The current catalog is shown; no replacement was created."
                }
                .into(),
            );
            return;
        }
        let Some(token) = model.execution_token(source) else {
            return;
        };
        if pending.restore {
            self.open_host_screens(token, present, window, cx);
        } else if let Some(key) = present.into_iter().next() {
            self.open_host_screen(token, key, window, cx);
        }
    }

    /// The live Screens of a connected host that are not open in this
    /// window, in the catalog's order, with the connection they open through.
    /// Nothing while the host's catalog is not the current one.
    pub(super) fn unopened_host_screens(
        &self,
        target: &str,
    ) -> Option<(SourceToken, Vec<ScreenKey>)> {
        let model = self.model.as_ref()?;
        let id = model.source_for_target(target)?;
        let host = model.host(id)?;
        if host.status() != SourceStatus::Ready || host.catalog_state() != CatalogState::Ready {
            return None;
        }
        let source = model.source(id)?;
        let token = model.execution_token(id)?;
        let screens = source
            .live_catalog()?
            .iter()
            .map(|screen| source.screen_key(screen.id.as_str()))
            .filter(|key| !model.opened().iter().any(|entry| entry.key() == key))
            .collect();
        Some((token, screens))
    }

    /// How many Screens "Open all" would open: offered from two on, since
    /// one Screen has its own entry.
    pub(super) fn open_all_host_screens_offer(&self, target: &str) -> Option<(SourceToken, usize)> {
        let (source, screens) = self.unopened_host_screens(target)?;
        (screens.len() >= 2).then_some((source, screens.len()))
    }

    /// Open every Screen of the host that is not open here, in one step,
    /// through the connection the offer was made for. What is opened is read
    /// when the action runs, so a Screen that ended meanwhile is left out.
    pub(super) fn open_all_host_screens(
        &mut self,
        source: SourceToken,
        target: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.hosts.pending_open.is_some() {
            return;
        }
        match self.unopened_host_screens(target) {
            Some((current, screens)) if current == source => {
                self.open_host_screens(source, screens, window, cx)
            }
            _ => {}
        }
    }

    /// Open several Screens of one connected host in one step and show the
    /// first.
    fn open_host_screens(
        &mut self,
        source: SourceToken,
        screens: Vec<ScreenKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(first) = screens.first() else {
            return;
        };
        if self.execution_token(first.source_id()) != Some(source) {
            return;
        }
        match self.commands.try_send(WorkerCommand::new(
            source,
            Command::OpenRemoteScreens { screens },
        )) {
            Ok(()) => {
                self.close_drawer_popover(window, cx);
                self.show_screens(window, cx)
            }
            Err(error) => {
                self.hosts.command_error = Some(format!("Open could not be sent: {error}"));
                cx.notify();
            }
        }
    }

    /// What the Settings view holds for the hosts to reconnect at start.
    pub(super) fn sync_reconnect_hosts(&mut self, cx: &mut Context<Self>) {
        let hosts = self.settings.read(cx).reconnect_hosts();
        if self.hosts.reconnect != hosts {
            self.hosts.reconnect = hosts;
            cx.notify();
        }
    }

    /// Choose whether `target` is reconnected at start when its Screens were
    /// open as the app was closed.
    pub(super) fn toggle_host_reconnect(&mut self, target: &str, cx: &mut Context<Self>) {
        if !self.hosts.is_saved(target) {
            return;
        }
        let mut hosts = self.settings.read(cx).reconnect_hosts();
        match hosts.iter().position(|host| host == target) {
            Some(index) => {
                hosts.remove(index);
            }
            None => hosts.push(target.to_owned()),
        }
        let sent = self
            .settings
            .update(cx, |view, cx| view.set_reconnect_hosts(hosts, cx));
        if !sent {
            self.hosts.command_error =
                Some("The setting is being saved. Try again in a moment.".into());
        }
        self.sync_reconnect_hosts(cx);
        cx.notify();
    }

    /// Take a host out of this window. Its Screens keep running and are
    /// offered again when the host is connected the next time.
    pub(super) fn close_host(&mut self, group: &HostGroupTarget, cx: &mut Context<Self>) {
        let Some(model) = self.model.as_ref().filter(|model| group.is_current(model)) else {
            return;
        };
        let Some(target) = model
            .host(group.source_id())
            .map(|host| host.action_target().clone())
        else {
            return;
        };
        let screens: Vec<ScreenKey> = model
            .opened()
            .iter()
            .map(|entry| entry.key().clone())
            .filter(|key| key.source_id() == group.source_id())
            .collect();
        match self.commands.try_send(WorkerCommand::new(
            group.owner(),
            Command::CloseHost { target },
        )) {
            Ok(()) => {
                self.hosts.pending_open = None;
                for screen in &screens {
                    self.cancel_remote_create_follow_for_detach(screen);
                    self.revoke_screen_links(screen, cx);
                }
            }
            Err(error) => self.status = format!("Close was not sent: {error}"),
        }
        cx.notify();
    }

    pub(super) fn host_action(&mut self, action: HostAction, cx: &mut Context<Self>) {
        // Any explicit host lifecycle command supersedes the queued navigation.
        self.hosts.pending_open = None;
        let (owner, command) = match action {
            HostAction::Connect { owner, target } => {
                if self.lifecycle_token() != Some(owner) {
                    return;
                }
                if self.hosts.snapshot().is_none_or(|snapshot| {
                    !snapshot
                        .directory()
                        .hosts
                        .iter()
                        .any(|host| host.target == target)
                }) {
                    self.hosts.command_error =
                        Some("Host directory changed. Refresh and try again.".into());
                    cx.notify();
                    return;
                }
                (owner, Command::ConnectHost { target })
            }
            HostAction::Cancel(target) => (target.local_owner(), Command::CancelHost { target }),
            HostAction::Retry(target) | HostAction::RetryCatalog(target) => {
                (target.local_owner(), Command::RetryHost { target })
            }
            HostAction::Release(target) => {
                (target.local_owner(), Command::ReleaseHostBrowse { target })
            }
        };
        self.send_captured(owner, command, cx);
    }

    pub(super) fn open_host_screen(
        &mut self,
        source: SourceToken,
        key: ScreenKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let still_live = self
            .model
            .as_ref()
            .and_then(|model| model.host(key.source_id()))
            .is_some_and(|host| {
                host.status() == SourceStatus::Ready && host.catalog_state() == CatalogState::Ready
            })
            && self
                .model_for_source(key.source_id())
                .and_then(SourceWorkspace::live_catalog)
                .is_some_and(|catalog| {
                    catalog
                        .iter()
                        .any(|screen| screen.id.as_str() == key.screen_id())
                });
        match admit_host_open(
            &self.commands,
            self.execution_token(key.source_id()),
            source,
            &key,
            still_live,
        ) {
            Ok(true) => {
                self.close_drawer_popover(window, cx);
                self.show_screens(window, cx)
            }
            Ok(false) => {}
            Err(error) => {
                self.hosts.command_error = Some(format!("Open could not be sent: {error}"));
                cx.notify();
            }
        }
    }

    pub(super) fn hosts_navigation_blocked(&self) -> bool {
        self.rename.is_some()
            || self
                .template_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.pending.is_some())
            || self.profile_picker.is_some()
            || self.tab_context_menu.is_some()
            || self.pending_template_apply.is_some()
            || self.pending_close.is_some()
            || self.drag.is_some()
            || self.float_gesture.is_some()
    }

    pub(super) fn show_hosts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hosts_navigation_blocked() || self.destination == Destination::Hosts {
            return;
        }
        self.ensure_host_search(window, cx);
        self.dismiss_templates(window, cx);
        if !self.drawer_visible {
            self.drawer_overlay_open = true;
        }
        self.destination = Destination::Hosts;
        self.cancel_remote_create_follow();
        self.refresh_ports(cx);
        self.wheel_navigation.reset();
        self.sync_visible(cx);
        self.host_browser_focus.focus(window, cx);
        cx.notify();
    }

    /// Reveal one current host section without making a connection or changing a mapping.
    pub(super) fn show_host_group(
        &mut self,
        target: &HostGroupTarget,
        section: HostSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.hosts_navigation_blocked()
            || self.hosts.has_open_form()
            || self.ports.has_open_form()
            || cx.has_active_drag()
            || !self
                .model
                .as_ref()
                .is_some_and(|model| target.is_current(model))
        {
            return false;
        }
        let Some(_index) = self
            .project_host_rows()
            .iter()
            .position(|row| match (section, row) {
                (HostSection::Host, HostRow::Host { target: row, .. })
                | (HostSection::Ports, HostRow::PortHeader { target: row, .. }) => {
                    row == target.target()
                }
                _ => false,
            })
        else {
            return false;
        };
        if self.destination != Destination::Hosts {
            self.show_hosts(window, cx);
            if self.destination != Destination::Hosts {
                return false;
            }
        }
        self.hosts.selected = Some(target.target().to_owned());
        self.hosts.section = section;
        self.hosts.unopened_only = false;
        self.hosts
            .scroll
            .scroll_to_item_strict(0, ScrollStrategy::Top);
        self.hosts
            .scroll
            .scroll_to_item_strict(0, ScrollStrategy::Top);
        self.host_browser_focus.focus(window, cx);
        cx.notify();
        true
    }

    pub(super) fn show_screens(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.destination == Destination::Screens {
            return;
        }
        if self.destination == Destination::Hosts {
            self.hosts.pending_open = None;
            if let Some(request_id) = self
                .hosts
                .update_prompt
                .as_ref()
                .map(|prompt| prompt.request_id)
            {
                self.dismiss_host_update(request_id, window, cx);
            }
            self.leave_ports(window, cx);
            let Some(owner) = self.lifecycle_token() else {
                return;
            };
            if let Err(error) = self
                .commands
                .try_send(WorkerCommand::new(owner, Command::ReleaseHostBrowsing))
            {
                self.hosts.command_error = Some(format!("Could not leave host browsing: {error}"));
                cx.notify();
                return;
            }
        }
        self.enter_screens_destination(window, cx);
    }

    pub(super) fn enter_screens_after_remote_create(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.destination == Destination::Hosts {
            // The local-owner create command releases browsing itself.
            self.leave_ports(window, cx);
        }
        self.enter_screens_destination(window, cx);
    }

    fn enter_screens_destination(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.destination == Destination::Hosts
            && (self.hosts.connect.take().is_some() || self.hosts.alias_editor.take().is_some())
        {
            self.hosts.connect_subscription = None;
            self.hosts.alias_subscription = None;
            window.close_dialog(cx);
        }
        self.destination = Destination::Screens;
        self.sync_visible(cx);
        self.focus_current(window, cx);
        cx.notify();
    }

    fn show_screens_captured(
        &mut self,
        source: SourceToken,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .local_model()
            .is_some_and(|model| model.source_token() == source)
        {
            self.show_screens(window, cx);
        }
    }

    pub(super) fn refresh_hosts(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.local_model().map(SourceWorkspace::source_token) else {
            return;
        };
        self.refresh_hosts_captured(source, cx);
    }

    fn refresh_hosts_captured(&mut self, source: SourceToken, cx: &mut Context<Self>) {
        self.refresh_ports(cx);
        if !self.hosts.can_refresh() {
            return;
        }
        if !self
            .local_model()
            .is_some_and(|model| model.source_token() == source)
        {
            return;
        }
        let Some(connection) = self.hosts.owner else {
            return;
        };
        if connection != source.connection_id() {
            return;
        }
        if let Err(error) = self.commands.try_send(WorkerCommand::new(
            source,
            Command::RefreshHostDirectory { connection },
        )) {
            self.hosts.command_error = Some(format!("Refresh could not be sent: {error}"));
            cx.notify();
        }
    }

    /// Show the Hosts manager with one host selected.
    pub(super) fn manage_host(
        &mut self,
        target: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_hosts(window, cx);
        if self.destination == Destination::Hosts {
            self.hosts.selected = Some(target.to_owned());
            self.hosts.section = HostSection::Host;
            self.hosts.unopened_only = false;
            cx.notify();
        }
    }

    /// The Drawer's Hosts item opens the hosts beside it; the manager is one
    /// step further, from that list.
    pub(super) fn render_hosts_navigation(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let source = self.local_model().map(SourceWorkspace::source_token);
        let managing = self.destination == Destination::Hosts;
        let active = managing || self.hosts_popover_open();
        let disabled = source.is_none() || (self.hosts_navigation_blocked() && !active);
        // A host that is coming up without a group in the Drawer yet, as one
        // reconnected at start, is announced where it will be found.
        let connecting = self.model.as_ref().is_some_and(|model| {
            model.hosts().any(|host| {
                matches!(
                    host.status(),
                    SourceStatus::Connecting | SourceStatus::Reconnecting
                ) && model
                    .source(host.action_target().source_id())
                    .is_none_or(|source| source.screens().is_empty())
            })
        });
        let item = self
            .drawer_nav_item(
                "open-hosts",
                super::drawer::NavItem {
                    icon: IconName::Server,
                    label: "Hosts",
                    shortcut: Some(if connecting {
                        "Connecting…".into()
                    } else {
                        super::drawer::nav_shortcut(Shortcut::Hosts, cx)
                    }),
                    active,
                },
                window,
                cx,
            )
            .accessibility_label("Connect to a host")
            .selected(active)
            .disabled(disabled)
            .when(disabled, |button| button.cursor_default())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if !source.is_some_and(|source| {
                    this.local_model()
                        .is_some_and(|model| model.source_token() == source)
                }) {
                    return;
                }
                if this.destination == Destination::Hosts {
                    this.show_screens(window, cx);
                } else {
                    this.toggle_hosts_popover(window, cx);
                }
            }));
        div()
            .relative()
            .w_full()
            .child(self.anchor_probe("nav-hosts"))
            .child(item)
            .into_any_element()
    }

    pub(super) fn project_host_rows(&self) -> Vec<HostRow> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut rows = Vec::<HostRow>::new();
        let forward_rows = self.ports.rows(self.model.as_ref());
        let mut entries: Vec<_> = self
            .hosts
            .snapshot()
            .into_iter()
            .flat_map(|snapshot| &snapshot.directory().hosts)
            .map(|host| (host.target.clone(), Some(host.clone())))
            .collect();
        let mut temporary: Vec<String> = self
            .model
            .as_ref()
            .into_iter()
            .flat_map(WindowWorkspace::hosts)
            .map(|host| host.action_target().target().to_owned())
            .filter(|target| !entries.iter().any(|(known, _)| known == target))
            .collect();
        temporary.sort();
        temporary.dedup();
        entries.extend(temporary.into_iter().map(|target| (target, None)));
        let mut known_targets: HashSet<String> =
            entries.iter().map(|(target, _)| target.clone()).collect();
        let mut forward_by_target: HashMap<String, Vec<splitty_client_core::ports::ForwardRow>> =
            HashMap::new();
        for row in &forward_rows {
            if known_targets.insert(row.key().target().to_owned()) {
                entries.push((row.key().target().to_owned(), None));
            }
            forward_by_target
                .entry(row.key().target().to_owned())
                .or_default()
                .push(row.clone());
        }
        for key in self
            .ports
            .feedback
            .iter()
            .filter(|(_, feedback)| {
                feedback.outcome() == splitty_client_core::ports::ForwardOutcome::Pending
            })
            .map(|(key, _)| key)
        {
            if known_targets.insert(key.target().to_owned()) {
                entries.push((key.target().to_owned(), None));
            }
        }
        for (target, host) in entries {
            let saved = host.is_some();
            let alias = host.as_ref().and_then(|host| host.alias.clone());
            let title = display(alias.as_deref().unwrap_or(&target));
            let assigned = self
                .model
                .as_ref()
                .and_then(|model| model.source_for_target(&target));
            let presentation = assigned.and_then(|id| self.model.as_ref()?.host(id));
            let action = match presentation {
                None if saved => self.lifecycle_token().map(|owner| HostAction::Connect {
                    owner,
                    target: target.clone(),
                }),
                Some(host) if host.can_retry() => {
                    Some(HostAction::Retry(host.action_target().clone()))
                }
                Some(host) if host.can_retry_catalog() => {
                    Some(HostAction::RetryCatalog(host.action_target().clone()))
                }
                Some(host) if host.can_cancel() => {
                    Some(HostAction::Cancel(host.action_target().clone()))
                }
                Some(host) if host.status() == SourceStatus::Ready && host.is_browsing() => {
                    Some(HostAction::Release(host.action_target().clone()))
                }
                _ => None,
            };
            let update = presentation
                .filter(|host| host.is_update_available())
                .map(|host| host.action_target().clone());
            let update_blocked = update.as_ref().is_some_and(|_| {
                presentation
                    .and_then(|host| host.update_offer())
                    .is_some_and(|offer| offer.to_version().is_none())
                    && self
                        .model
                        .as_ref()
                        .and_then(WindowWorkspace::backend_version)
                        .is_none()
            }) || self.hosts.update_prompt.is_some();
            let refresh_backend = update_blocked
                && self
                    .model
                    .as_ref()
                    .is_some_and(WindowWorkspace::backend_version_failed);
            let state = match presentation.map(|host| host.update_state()) {
                Some(HostUpdateState::Closing) => "Closing for update…",
                Some(HostUpdateState::Updating) => "Updating…",
                Some(HostUpdateState::Unconfirmed) => "Update unconfirmed · Retry to inspect",
                _ => match presentation.map(|host| host.status()) {
                    None => "Not connected",
                    Some(SourceStatus::Connecting) => "Connecting…",
                    Some(SourceStatus::Reconnecting) => "Reconnecting…",
                    Some(SourceStatus::Ready) => "Connected",
                    Some(SourceStatus::Offline) => "Offline",
                    Some(SourceStatus::Closing) => "Closing…",
                },
            }
            .to_owned();
            let mut detail = presentation
                .and_then(|host| {
                    host.diagnostic()
                        .map(|diagnostic| {
                            let mut message = display(diagnostic.message());
                            if let Some(hint) = diagnostic.hint() {
                                message.push_str(" · ");
                                message.push_str(&display(hint));
                            }
                            message
                        })
                        .or_else(|| host.progress().map(|progress| display(&progress.message)))
                })
                .or_else(|| {
                    presentation
                        .filter(|host| host.status() == SourceStatus::Ready)
                        .map(|_| "Live connection".to_owned())
                })
                .or_else(|| {
                    host.as_ref()
                        .and_then(|host| host.last_seen)
                        .map(|seen| format!("Last observed {}", age(seen, now)))
                })
                .unwrap_or_else(|| {
                    if saved {
                        "Reachability has not been checked".into()
                    } else {
                        "Not saved in the host list".into()
                    }
                });
            if !saved && presentation.is_some() {
                detail = format!("Current connection work · {detail}");
            }
            if presentation
                .filter(|host| host.is_update_available())
                .and_then(|host| host.update_offer())
                .is_some_and(|offer| offer.to_version().is_none())
                && self
                    .model
                    .as_ref()
                    .and_then(WindowWorkspace::backend_version)
                    .is_none()
            {
                detail.push_str(
                    if self
                        .model
                        .as_ref()
                        .is_some_and(WindowWorkspace::backend_version_failed)
                    {
                        " · Backend version unavailable; refresh it to update"
                    } else {
                        " · Reading backend version before update"
                    },
                );
            }
            let feedback =
                self.hosts
                    .mutations
                    .get(&target)
                    .map(|mutation| match mutation.outcome {
                        HostManagementOutcome::Pending => match mutation.action {
                            HostManagementAction::SetAlias => "Saving display name…".to_owned(),
                            HostManagementAction::SetOrder => "Saving host order…".to_owned(),
                            HostManagementAction::Forget => "Removing from list…".to_owned(),
                            HostManagementAction::Connect => "Preparing connection…".to_owned(),
                        },
                        HostManagementOutcome::Applied => {
                            "Change confirmed · refreshing list".to_owned()
                        }
                        HostManagementOutcome::Rejected => format!(
                            "Action rejected: {}",
                            mutation.reason.as_deref().unwrap_or("Unknown error")
                        ),
                        HostManagementOutcome::Uncertain => format!(
                            "Change status unknown: {}",
                            mutation
                                .reason
                                .as_deref()
                                .unwrap_or("Refresh to inspect current values")
                        ),
                    });
            rows.push(HostRow::Host {
                target: target.clone(),
                title,
                state,
                detail,
                action,
                update,
                update_blocked,
                refresh_backend,
                saved,
                alias,
                feedback,
                mutation_pending: self
                    .hosts
                    .mutations
                    .get(&target)
                    .is_some_and(|mutation| mutation.outcome == HostManagementOutcome::Pending),
                owner: self.lifecycle_token(),
            });
            let source = assigned.and_then(|id| self.model.as_ref()?.source(id));
            let live = source
                .filter(|_| {
                    presentation.is_some_and(|host| {
                        host.status() == SourceStatus::Ready
                            && host.catalog_state() == CatalogState::Ready
                    })
                })
                .and_then(SourceWorkspace::live_catalog);
            if let Some(live) = live {
                if live.is_empty() {
                    rows.push(HostRow::Screen {
                        target: target.clone(),
                        id: "empty-live".into(),
                        label: "No live Screens".into(),
                        detail: "This host reported an empty catalog".into(),
                        route: None,
                        opened: false,
                    });
                }
                for screen in live {
                    let Some(source) = source else {
                        continue;
                    };
                    let key = source.screen_key(screen.id.as_str());
                    let opened = self.model.as_ref().is_some_and(|model| {
                        model.opened().iter().any(|entry| entry.key() == &key)
                    });
                    let count = source
                        .screen(screen.id.as_str())
                        .and_then(ScreenModel::snapshot)
                        .map(|screen| screen.panes.len());
                    rows.push(HostRow::Screen {
                        target: target.clone(),
                        id: screen.id.to_string(),
                        label: display(screen.name.as_deref().unwrap_or(screen.id.as_str())),
                        detail: count.map_or_else(
                            || "Live Screen · pane count unknown".into(),
                            |n| format!("Live Screen · {n} panes"),
                        ),
                        route: self
                            .execution_token(key.source_id())
                            .map(|source| (source, key)),
                        opened,
                    });
                }
            } else if let Some(host_state) =
                presentation.filter(|host| host.status() == SourceStatus::Ready)
            {
                let detail = host_state
                    .catalog_error()
                    .map(|error| display(error.message()))
                    .unwrap_or_else(|| match host_state.catalog_state() {
                        CatalogState::Unrequested | CatalogState::Loading => {
                            "Reading live catalog…".into()
                        }
                        CatalogState::Ready => "Live catalog is unavailable".into(),
                        CatalogState::Failed => "Live catalog failed · retry the catalog".into(),
                    });
                rows.push(HostRow::Screen {
                    target: target.clone(),
                    id: "catalog-status".into(),
                    label: "Live Screens unavailable".into(),
                    detail,
                    route: None,
                    opened: false,
                });
            }
            if live.is_none() {
                if let Some(observed) = source.and_then(SourceWorkspace::live_catalog) {
                    for screen in observed {
                        let count = source
                            .and_then(|source| source.screen(screen.id.as_str()))
                            .and_then(ScreenModel::snapshot)
                            .map(|screen| screen.panes.len());
                        rows.push(HostRow::Screen {
                            target: target.clone(),
                            id: screen.id.to_string(),
                            label: display(screen.name.as_deref().unwrap_or(screen.id.as_str())),
                            detail: count.map_or_else(
                                || "Last observed · pane count unknown".into(),
                                |n| format!("Last observed · {n} panes"),
                            ),
                            route: None,
                            opened: false,
                        });
                    }
                } else {
                    for screen in host.as_ref().into_iter().flat_map(|host| &host.screens) {
                        rows.push(HostRow::Screen {
                            target: target.clone(),
                            id: screen.id.clone(),
                            label: display(screen.name.as_deref().unwrap_or(&screen.id)),
                            detail: format!("Last observed · {} panes", screen.panes),
                            route: None,
                            opened: false,
                        });
                    }
                }
                if host.as_ref().is_some_and(|host| host.screens.is_empty())
                    && presentation.is_none()
                {
                    let detail = match observation(host.as_ref().and_then(|host| host.last_seen), 0)
                    {
                        Observation::Empty => "Last observation had no Screens",
                        Observation::Unknown => "No Screen observation yet",
                        Observation::Cached => unreachable!(),
                    };
                    rows.push(HostRow::Screen {
                        target: target.clone(),
                        id: "unknown".into(),
                        label: detail.into(),
                        detail: "Connect to browse live Screens".into(),
                        route: None,
                        opened: false,
                    });
                }
            }
            let port_owner = self
                .ports
                .can_manage()
                .then(|| self.lifecycle_token())
                .flatten();
            rows.push(HostRow::PortHeader {
                target: target.clone(),
                owner: port_owner,
            });
            for row in forward_by_target.remove(&target).unwrap_or_default() {
                let feedback = self.ports.feedback.get(row.key());
                let pending = self.ports.pending(row.key());
                rows.push(HostRow::Port {
                    row,
                    owner: port_owner,
                    feedback: feedback.map(super::ports::feedback_text),
                    feedback_outcome: feedback.map(|value| value.outcome()),
                    pending,
                });
            }
            for (key, feedback) in self.ports.feedback.iter().filter(|(key, feedback)| {
                key.target() == target
                    && feedback.outcome() == splitty_client_core::ports::ForwardOutcome::Pending
                    && !forward_rows.iter().any(|row| row.key() == *key)
            }) {
                rows.push(HostRow::PortPending {
                    key: key.clone(),
                    owner: port_owner,
                    feedback: super::ports::feedback_text(feedback),
                });
            }
        }
        for row in &mut rows {
            if let HostRow::Screen {
                target, id, opened, ..
            } = row
            {
                *opened = self.model.as_ref().is_some_and(|model| {
                    model.opened().iter().any(|entry| {
                        model.source_target(entry.key().source_id()) == Some(target.as_str())
                            && entry.key().screen_id() == id
                    })
                });
            }
        }
        rows
    }

    fn render_host_row(
        &self,
        row: &HostRow,
        create_actions: &HashMap<String, (HostActionTarget, SourceToken)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let warning = cx.theme().warning;
        let weak = cx.entity().downgrade();
        let row_height = match row {
            HostRow::Host { .. } => chrome_rems(72.),
            HostRow::Screen { .. } => chrome_rems(46.),
            HostRow::PortHeader { .. } => chrome_rems(38.),
            _ => chrome_rems(64.),
        };
        match row {
            HostRow::Host {
                target,
                title,
                state,
                detail,
                action,
                update,
                update_blocked,
                refresh_backend,
                saved,
                alias,
                feedback,
                mutation_pending,
                owner,
            } => {
                let entry = div()
                    .id(SharedString::from(format!(
                        "host:{}:{target}",
                        target.len()
                    )))
                    .h(row_height)
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap_1()
                    .px_4()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .max_w_full()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(chrome_rems(13.5))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(title.clone()),
                            )
                            .when(alias.is_some(), |view| {
                                view.child(
                                    div()
                                        .min_w_0()
                                        .text_size(chrome_rems(11.5))
                                        .text_color(muted)
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(display(target)),
                                )
                            }),
                    )
                    .when_some(feedback.clone(), |view, feedback| {
                        view.child(
                            div()
                                .text_size(chrome_rems(12.))
                                .text_color(warning)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(feedback),
                        )
                    });
                let mut actions = div().flex().items_center().gap_2().child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(chrome_rems(11.5))
                        .text_color(muted)
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(format!("{state} · {detail}")),
                );
                let remembered = self.remembered_host_screens(target).len();
                if let Some(owner) = owner.filter(|_| remembered > 0) {
                    let (weak, host) = (weak.clone(), target.clone());
                    actions = actions.child(
                        Button::new(SharedString::from(format!("host-reopen-{target}")))
                            .focus_ring(focus_visible(cx))
                            .debug_selector({
                                let target = target.clone();
                                move || format!("host-reopen-{target}")
                            })
                            .xsmall()
                            .h(chrome_rems(24.))
                            .label(if remembered == 1 {
                                "Reopen 1 Screen".to_owned()
                            } else {
                                format!("Reopen {remembered} Screens")
                            })
                            .tooltip("Connect and open the Screens that were open here")
                            .disabled(self.hosts.pending_open.is_some())
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.begin_host_restore(owner, host.clone(), cx);
                                });
                            }),
                    );
                }
                if let Some((source, count)) = self.open_all_host_screens_offer(target) {
                    let (weak, host) = (weak.clone(), target.clone());
                    actions = actions.child(
                        Button::new(SharedString::from(format!("host-open-all-{target}")))
                            .focus_ring(focus_visible(cx))
                            .debug_selector({
                                let target = target.clone();
                                move || format!("host-open-all-{target}")
                            })
                            .xsmall()
                            .h(chrome_rems(24.))
                            .label(format!("Open all {count} Screens"))
                            .tooltip("Open the Screens of this host that are not open here")
                            .disabled(self.hosts.pending_open.is_some())
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.open_all_host_screens(source, &host, window, cx);
                                });
                            }),
                    );
                }
                if let Some((target, source)) = create_actions.get(target) {
                    let target = target.clone();
                    let source = *source;
                    let weak = weak.clone();
                    actions = actions.child(
                        Button::new(SharedString::from(format!(
                            "host-new-{}",
                            target.source_id()
                        )))
                        .focus_ring(focus_visible(cx))
                        .debug_selector({
                            let source = source.source_id();
                            move || format!("host-new-{source}")
                        })
                        .xsmall()
                        .h(chrome_rems(24.))
                        .label("New Screen")
                        .on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.create_remote_screen(None, &target, source, window, cx);
                            });
                        }),
                    );
                }
                let open_group = self.model.as_ref().and_then(|model| {
                    HostGroupTarget::capture(model, model.source_for_target(target)?)
                });
                if let Some(group) = open_group {
                    let weak = weak.clone();
                    actions = actions.child(
                        Button::new(SharedString::from(format!("host-close-{target}")))
                            .focus_ring(focus_visible(cx))
                            .debug_selector({
                                let target = target.clone();
                                move || format!("host-close-{target}")
                            })
                            .xsmall()
                            .h(chrome_rems(24.))
                            .ghost()
                            .label("Close host")
                            .tooltip("Take the host out of this window; its Screens keep running")
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| this.close_host(&group, cx));
                            }),
                    );
                }
                if let Some(action) = action {
                    let action = action.clone();
                    let label = match &action {
                        HostAction::Connect { .. } => "Connect",
                        HostAction::Cancel(_) => "Cancel",
                        HostAction::Retry(_) => "Retry",
                        HostAction::RetryCatalog(_) => "Retry catalog",
                        HostAction::Release(_) => "Stop browsing",
                    };
                    let target = target.clone();
                    let weak = weak.clone();
                    actions = actions.child(
                        Button::new(SharedString::from(format!(
                            "host-action:{}:{target}",
                            target.len()
                        )))
                        .focus_ring(focus_visible(cx))
                        .xsmall()
                        .h(chrome_rems(24.))
                        .label(label)
                        .accessibility_label(format!("{label} {target}"))
                        .on_click(move |_, _, cx| {
                            let _ =
                                weak.update(cx, |this, cx| this.host_action(action.clone(), cx));
                        }),
                    );
                }
                if let Some(update_target) = update {
                    let trigger = window
                        .use_keyed_state(
                            SharedString::from(format!("host-update-focus-{target}")),
                            cx,
                            |_, cx| cx.focus_handle().tab_stop(true),
                        )
                        .read(cx)
                        .clone();
                    let update_target = update_target.clone();
                    let update_weak = weak.clone();
                    let update_focus = trigger.clone();
                    actions = actions.child(
                        Button::new(SharedString::from(format!("host-update-{target}")))
                            .focus_ring(focus_visible(cx))
                            .debug_selector({
                                let target = target.clone();
                                move || format!("host-update-{target}")
                            })
                            .xsmall()
                            .h(chrome_rems(24.))
                            .label("Update…")
                            .track_focus(&trigger)
                            .disabled(*update_blocked)
                            .on_click(move |_, window, cx| {
                                let _ = update_weak.update(cx, |this, cx| {
                                    this.request_host_update(
                                        update_target.clone(),
                                        update_focus.clone(),
                                        window,
                                        cx,
                                    )
                                });
                            }),
                    );
                }
                if *refresh_backend {
                    let refresh_weak = weak.clone();
                    actions = actions.child(
                        Button::new(SharedString::from(format!("host-backend-refresh-{target}")))
                            .focus_ring(focus_visible(cx))
                            .debug_selector({
                                let target = target.clone();
                                move || format!("host-backend-refresh-{target}")
                            })
                            .xsmall()
                            .h(chrome_rems(24.))
                            .ghost()
                            .label("Refresh version")
                            .on_click(move |_, _, cx| {
                                let _ = refresh_weak
                                    .update(cx, |this, cx| this.refresh_backend_version(cx));
                            }),
                    );
                }
                if *saved {
                    let owner = *owner;
                    let menu_target = target.clone();
                    let menu_alias = alias.clone();
                    let tag_open = self.drawer_popover_is("host-more");
                    actions =
                        actions.child(
                            Button::new(SharedString::from(format!(
                                "host-more:{}:{target}",
                                target.len()
                            )))
                            .accessibility_label(format!("More for host {target}"))
                            .icon(Icon::new(IconName::Ellipsis))
                            .ghost()
                            .compact()
                            .xsmall()
                            .h(chrome_rems(24.))
                            .focus_ring(focus_visible(cx))
                            .selected(tag_open)
                            .disabled(*mutation_pending || owner.is_none())
                            .on_click(cx.listener(
                                move |this, event: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    let Some(owner) = owner
                                        .filter(|owner| this.lifecycle_token() == Some(*owner))
                                    else {
                                        return;
                                    };
                                    let (edit_target, edit_alias) =
                                        (menu_target.clone(), menu_alias.clone());
                                    let forget_target = menu_target.clone();
                                    let reconnect_target = menu_target.clone();
                                    let reconnects = this.hosts.reconnects(&menu_target);
                                    let mut entries = vec![
                                        MenuEntry::new(
                                            "rename-host",
                                            "Change Display Name…",
                                            move |this, window, cx| {
                                                invoke_host_menu_for_owner(
                                                    owner,
                                                    this.lifecycle_token(),
                                                    || {
                                                        this.edit_host_alias(
                                                            &edit_target,
                                                            edit_alias.as_deref(),
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                );
                                            },
                                        ),
                                        MenuEntry::new(
                                            "forget-host",
                                            "Remove from List",
                                            move |this, _, cx| {
                                                invoke_host_menu_for_owner(
                                                    owner,
                                                    this.lifecycle_token(),
                                                    || this.forget_host(&forget_target, cx),
                                                );
                                            },
                                        )
                                        .detail("Keeps its Screens and port forwards running"),
                                    ];
                                    entries.insert(
                                        1,
                                        MenuEntry::new(
                                            "reconnect-host",
                                            "Reconnect at Start",
                                            move |this, _, cx| {
                                                invoke_host_menu_for_owner(
                                                    owner,
                                                    this.lifecycle_token(),
                                                    || {
                                                        this.toggle_host_reconnect(
                                                            &reconnect_target,
                                                            cx,
                                                        )
                                                    },
                                                );
                                            },
                                        )
                                        .detail("Brings back the Screens that were open")
                                        .meta(if reconnects { "On" } else { "Off" }),
                                    );
                                    this.open_drawer_popover(
                                        PopoverKind::Actions(entries),
                                        PopoverAnchor::Pointer(event.position()),
                                        None,
                                        window,
                                        cx,
                                    );
                                    this.tag_drawer_popover("host-more");
                                },
                            )),
                        );
                }
                entry.child(actions).into_any_element()
            }
            HostRow::Screen {
                target,
                id,
                label,
                detail,
                route,
                opened,
            } => {
                let blocked = self.host_screen_blocked(target, id);
                let mut entry = div()
                    .id(SharedString::from(format!(
                        "host-screen:{}:{target}:{}:{id}",
                        target.len(),
                        id.len()
                    )))
                    .h(row_height)
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .pl_3()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        Icon::new(IconName::Terminal)
                            .size(chrome_rems(14.))
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(label.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .text_size(chrome_rems(11.))
                                    .text_color(muted)
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(detail.clone()),
                                    )
                                    .when(blocked, |view| {
                                        view.child(
                                            div()
                                                .flex_none()
                                                .text_color(cx.theme().warning)
                                                .child(super::program_status::WAITING),
                                        )
                                    })
                                    .when(*opened, |view| {
                                        view.child(
                                            div()
                                                .flex_none()
                                                .text_color(cx.theme().primary)
                                                .child("Open in this window"),
                                        )
                                    }),
                            ),
                    );
                if let Some((source, key)) = route {
                    let source = *source;
                    let key = key.clone();
                    let weak = weak.clone();
                    let action = if *opened { "Show" } else { "Open" };
                    entry = entry.child(
                        Button::new(SharedString::from(format!(
                            "host-open:{}:{target}:{}:{id}",
                            target.len(),
                            id.len()
                        )))
                        .focus_ring(focus_visible(cx))
                        .xsmall()
                        .h(chrome_rems(24.))
                        .label(action)
                        .accessibility_label(format!("{action} {label} on {target}"))
                        .on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.open_host_screen(source, key.clone(), window, cx)
                            });
                        }),
                    );
                }
                if route.is_none() && self.known_host_screen(target, id) {
                    let owner = self.lifecycle_token();
                    let target = target.clone();
                    let id = id.clone();
                    let weak = weak.clone();
                    entry = entry.child(
                        Button::new(SharedString::from(format!(
                            "host-open-cached-{target}-{id}"
                        )))
                        .focus_ring(focus_visible(cx))
                        .xsmall()
                        .h(chrome_rems(24.))
                        .label("Open")
                        .disabled(self.hosts.pending_open.is_some())
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                if let Some(owner) = owner {
                                    this.begin_cached_host_open(
                                        owner,
                                        target.clone(),
                                        id.clone(),
                                        cx,
                                    );
                                }
                            });
                        }),
                    );
                }
                if *opened {
                    let key = self
                        .model
                        .as_ref()
                        .and_then(|model| {
                            model
                                .source_for_target(target)
                                .and_then(|source| model.source(source))
                        })
                        .map(|source| source.screen_key(id));
                    if let Some(key) = key {
                        let owner = self.lifecycle_token();
                        let weak = weak.clone();
                        entry = entry.child(
                            Button::new(SharedString::from(format!("host-detach-{target}-{id}")))
                                .focus_ring(focus_visible(cx))
                                .xsmall()
                                .h(chrome_rems(24.))
                                .ghost()
                                .icon(Icon::new(IconName::Unplug))
                                .tooltip("Detach from this window; keep running on the host")
                                .accessibility_label(format!("Detach {label} from this window"))
                                .on_click(move |_, window, cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        if this.lifecycle_token() == owner {
                                            this.request_close_screen(key.clone(), window, cx);
                                        }
                                    });
                                }),
                        );
                    }
                }
                entry.into_any_element()
            }
            HostRow::PortHeader { target, owner } => {
                Self::render_port_header(target, *owner, row_height, weak.clone(), cx)
            }
            HostRow::Port {
                row,
                owner,
                feedback,
                feedback_outcome,
                pending,
            } => Self::render_port_row(
                row,
                *owner,
                feedback.as_deref().zip(*feedback_outcome),
                *pending,
                row_height,
                weak.clone(),
                cx,
            ),
            HostRow::PortPending {
                key,
                owner,
                feedback,
            } => Self::render_pending_port(key, *owner, feedback, row_height, weak.clone(), cx),
        }
    }

    pub(super) fn render_hosts(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let compact = window.viewport_size().width / window.rem_size() < 46.;
        let forward_rows = self.ports.rows(self.model.as_ref());
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let warning = theme.warning;
        let foreground = theme.foreground;
        let border = theme.border;
        let surface = theme.sidebar;
        let (status, failed, loading) = match &self.hosts.state {
            HostDirectoryState::Unavailable => (
                "Host directory unavailable for this local service".to_owned(),
                false,
                false,
            ),
            HostDirectoryState::Loading { previous } => (
                if previous.is_some() {
                    "Refreshing hosts · showing last observations"
                } else {
                    "Loading hosts…"
                }
                .to_owned(),
                false,
                true,
            ),
            HostDirectoryState::Ready(snapshot) => (
                if snapshot.directory().hosts.is_empty() {
                    "No known hosts yet"
                } else {
                    "Connect a host to browse its live Screens"
                }
                .to_owned(),
                false,
                false,
            ),
            HostDirectoryState::Failed { previous, reason } => {
                (failure_status(previous.is_some(), reason), true, false)
            }
        };
        let all_rows = self.project_host_rows();
        let selected = self
            .hosts
            .selected
            .as_deref()
            .filter(|target| {
                all_rows
                    .iter()
                    .any(|row| matches!(row, HostRow::Host { target: t, .. } if t == target))
            })
            .or_else(|| {
                all_rows.iter().find_map(|row| match row {
                    HostRow::Host { target, .. } => Some(target.as_str()),
                    _ => None,
                })
            });
        let query = self
            .hosts
            .search
            .as_ref()
            .map(|input| input.read(cx).value().to_lowercase())
            .unwrap_or_default();
        let owner = self.lifecycle_token();
        let mut directory = div()
            .id("host-browser-directory")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .when(compact, |view| {
                view.flex().overflow_x_scroll().overflow_y_hidden()
            })
            .px_2();
        for row in &all_rows {
            let HostRow::Host {
                target,
                title,
                alias,
                state,
                detail,
                ..
            } = row
            else {
                continue;
            };
            if !format!("{title} {target}").to_lowercase().contains(&query) {
                continue;
            }
            let list_title = if alias.is_none() {
                display(
                    target
                        .rsplit_once('@')
                        .map_or(target.as_str(), |(_, host)| host),
                )
            } else {
                title.clone()
            };
            let target = target.clone();
            let active = selected == Some(target.as_str());
            let weak = cx.entity().downgrade();
            directory = directory.child(
                Button::new(SharedString::from(format!("browse-host-{target}")))
                    .focus_ring(focus_visible(cx))
                    .debug_selector({
                        let target = target.clone();
                        move || format!("browse-host-{target}")
                    })
                    .ghost()
                    .w_full()
                    .when(compact, |view| view.w(chrome_rems(180.)).flex_none())
                    .h(chrome_rems(48.))
                    .when(compact, |view| view.h(chrome_rems(48.)))
                    .px_3()
                    .selected(active)
                    .accessibility_label(format!("Browse {title}, {state}"))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(chrome_rems(12.5))
                                    .child(Icon::new(IconName::Server).size(chrome_rems(13.)))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(list_title),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(chrome_rems(11.))
                                    .text_color(muted)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(state.clone()),
                            ),
                    )
                    .tooltip(format!("{}\n{}", display(&target), detail))
                    .on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            if this.lifecycle_token() != owner {
                                return;
                            }
                            this.hosts.pending_open = None;
                            this.hosts.selected = Some(target.clone());
                            this.hosts.unopened_only = false;
                            this.hosts
                                .scroll
                                .scroll_to_item_strict(0, ScrollStrategy::Top);
                            cx.notify();
                        });
                    }),
            );
        }
        let rows: Vec<_> = all_rows
            .iter()
            .filter(|row| match row {
                HostRow::Host { target, .. } => Some(target.as_str()) == selected,
                HostRow::Screen { target, opened, .. } => {
                    Some(target.as_str()) == selected
                        && self.hosts.section == HostSection::Host
                        && (!self.hosts.unopened_only || !opened)
                }
                HostRow::PortHeader { target, .. } => {
                    Some(target.as_str()) == selected && self.hosts.section == HostSection::Ports
                }
                HostRow::Port { row, .. } => {
                    Some(row.key().target()) == selected && self.hosts.section == HostSection::Ports
                }
                HostRow::PortPending { key, .. } => {
                    Some(key.target()) == selected && self.hosts.section == HostSection::Ports
                }
            })
            .cloned()
            .collect();
        let create_actions: HashMap<_, _> = self
            .model
            .iter()
            .flat_map(|model| {
                model.hosts().filter_map(|host| {
                    let target = host.action_target();
                    let source = model.execution_token(target.source_id())?;
                    (self.drawer_host_actions_available()
                        && !self
                            .remote_create_enqueues
                            .contains_key(&target.source_id())
                        && model
                            .validate_remote_screen_creation(target, source)
                            .is_ok())
                    .then(|| (target.target().to_owned(), (target.clone(), source)))
                })
            })
            .collect();
        let visible_targets: HashSet<&str> = rows
            .iter()
            .filter_map(|row| match row {
                HostRow::Host { target, .. } => Some(target.as_str()),
                HostRow::Screen { .. }
                | HostRow::PortHeader { .. }
                | HostRow::Port { .. }
                | HostRow::PortPending { .. } => None,
            })
            .collect();
        let mut detached_feedback: Vec<_> = self
            .hosts
            .mutations
            .iter()
            .filter(|(target, mutation)| {
                !visible_targets.contains(target.as_str())
                    && matches!(
                        mutation.outcome,
                        HostManagementOutcome::Rejected | HostManagementOutcome::Uncertain
                    )
            })
            .map(|(target, mutation)| {
                format!(
                    "{} · {}: {}",
                    display(target),
                    if mutation.outcome == HostManagementOutcome::Uncertain {
                        "Change status unknown"
                    } else {
                        "Action rejected"
                    },
                    display(
                        mutation
                            .reason
                            .as_deref()
                            .unwrap_or("Refresh to inspect current values")
                    )
                )
            })
            .collect();
        detached_feedback.extend(
            self.ports
                .feedback
                .iter()
                .filter(|(key, feedback)| {
                    !forward_rows.iter().any(|row| row.key() == *key)
                        && matches!(
                            feedback.outcome(),
                            splitty_client_core::ports::ForwardOutcome::Partial
                                | splitty_client_core::ports::ForwardOutcome::Failed
                                | splitty_client_core::ports::ForwardOutcome::Rejected
                                | splitty_client_core::ports::ForwardOutcome::Uncertain
                        )
                })
                .map(|(key, feedback)| {
                    format!(
                        "{} · port {} · {}",
                        display(key.target()),
                        key.remote(),
                        super::ports::feedback_text(feedback)
                    )
                }),
        );
        detached_feedback.sort();
        let mut detached_success: Vec<_> = self
            .ports
            .feedback
            .iter()
            .filter(|(key, feedback)| {
                feedback.outcome() == splitty_client_core::ports::ForwardOutcome::Succeeded
                    && !forward_rows.iter().any(|row| row.key() == *key)
            })
            .map(|(key, feedback)| {
                format!(
                    "{} · port {} · {}",
                    display(key.target()),
                    key.remote(),
                    super::ports::feedback_text(feedback)
                )
            })
            .collect();
        detached_success.sort();
        let ports_status = match self.ports.snapshot.as_deref() {
            None => "Port forwarding state has not loaded".to_owned(),
            Some(snapshot) if !snapshot.is_available() => {
                "Port forwarding is unavailable for this local service".to_owned()
            }
            Some(snapshot) if snapshot.is_loading() => {
                "Refreshing active and saved ports…".to_owned()
            }
            Some(snapshot)
                if snapshot.active_error().is_some() || snapshot.saved_error().is_some() =>
            {
                format!(
                    "Ports refresh incomplete: {}{}",
                    snapshot.active_error().unwrap_or_default(),
                    snapshot
                        .saved_error()
                        .map(|error| format!(" {error}"))
                        .unwrap_or_default()
                )
            }
            Some(_)
                if selected
                    .and_then(|target| {
                        self.model.as_ref().and_then(|model| {
                            model
                                .source_for_target(target)
                                .and_then(|id| model.source(id))
                        })
                    })
                    .is_some_and(|source| source.port_observation().is_none()) =>
            {
                "Port discovery has not been reported. You can add a port manually.".to_owned()
            }
            Some(_)
                if !forward_rows
                    .iter()
                    .any(|row| Some(row.key().target()) == selected) =>
            {
                "No detected or saved ports for this host. Add a port to get started.".to_owned()
            }
            Some(_) => String::new(),
        };
        let mut header = Vec::new();
        let mut content = Vec::new();
        let mut port_header = Vec::new();
        for row in rows {
            match row {
                HostRow::Host { .. } => {
                    header.push(self.render_host_row(&row, &create_actions, window, cx))
                }
                HostRow::PortHeader { .. } => {
                    port_header.push(self.render_host_row(&row, &create_actions, window, cx))
                }
                _ => content.push(row),
            }
        }
        let empty_screens =
            content.is_empty() && selected.is_some() && self.hosts.section == HostSection::Host;
        let count = content.len();
        let weak = cx.entity().downgrade();
        let catalog = uniform_list("host-catalog", count, move |range, window, cx| {
            let Some(view) = weak.upgrade() else {
                return Vec::new();
            };
            view.update(cx, |this, cx| {
                content[range]
                    .iter()
                    .map(|row| this.render_host_row(row, &create_actions, window, cx))
                    .collect()
            })
        })
        .track_scroll(&self.hosts.scroll)
        .w_full()
        .flex_1()
        .min_h_0();
        let list = div().id("hosts-list").debug_selector(|| "hosts-list".into())
            .flex_1().min_h_0().min_w_0().w_full().flex().flex_col().px_4()
            .children(port_header)
            .when(
                self.hosts.section == HostSection::Ports && !ports_status.is_empty(),
                |view| {
                    view.child(
                        div()
                            .py_3()
                            .text_size(chrome_rems(12.))
                            .text_color(muted)
                            .child(ports_status.clone()),
                    )
                },
            )
            .when(empty_screens, |view| view.child(div().py_6().text_size(chrome_rems(12.)).text_color(muted)
                .child(if self.hosts.unopened_only { "All listed screens are already open in this window." }
                    else { "No screens listed. Connect to refresh, or create a screen on this host." })))
            .when(count > 0, |view| view.child(catalog));
        let source = self.local_model().map(SourceWorkspace::source_token);
        let weak_back = cx.entity().downgrade();
        let weak_refresh = cx.entity().downgrade();
        let weak_connect = cx.entity().downgrade();
        div()
            .id("hosts-view")
            .debug_selector(|| "hosts-view".into())
            .size_full()
            .text_size(chrome_rems(12.5))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .bg(surface)
            .text_color(foreground)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(border)
                    .child(div().font_weight(FontWeight::BOLD).child("Hosts"))
                    .child(div().flex_1())
                    .child(
                        Button::new("hosts-connect")
                            .focus_ring(focus_visible(cx))
                            .label("Connect…")
                            .accessibility_label("Connect to host")
                            .track_focus(&self.host_connect_trigger)
                            .xsmall()
                            .h(chrome_rems(24.))
                            .disabled(source.is_none())
                            .on_click(move |_, window, cx| {
                                let _ = weak_connect
                                    .update(cx, |this, cx| this.open_host_connect(window, cx));
                            }),
                    )
                    .child(
                        Button::new("hosts-refresh")
                            .focus_ring(focus_visible(cx))
                            .label(if failed { "Retry" } else { "Refresh" })
                            .accessibility_label(if failed {
                                "Retry host directory refresh"
                            } else {
                                "Refresh host directory"
                            })
                            .disabled(loading || !self.hosts.can_refresh())
                            .xsmall()
                            .h(chrome_rems(24.))
                            .on_click(move |_, _, cx| {
                                if let Some(source) = source {
                                    let _ = weak_refresh.update(cx, |this, cx| {
                                        this.refresh_hosts_captured(source, cx)
                                    });
                                }
                            }),
                    )
                    .child(
                        Button::new("hosts-back")
                            .focus_ring(focus_visible(cx))
                            .label("Close")
                            .accessibility_label("Close Hosts")
                            .ghost()
                            .xsmall()
                            .h(chrome_rems(24.))
                            .on_click(move |_, window, cx| {
                                if let Some(source) = source {
                                    let _ = weak_back.update(cx, |this, cx| {
                                        this.show_screens_captured(source, window, cx)
                                    });
                                }
                            }),
                    ),
            )
            .when(
                failed
                    || loading
                    || selected.is_none()
                    || self.hosts.command_error.is_some()
                    || self.hosts.pending_open.is_some(),
                |view| {
                    view.child(
                        div()
                            .px_4()
                            .py_2()
                            .text_color(if failed { danger } else { muted })
                            .child(match &self.hosts.pending_open {
                                Some(pending) => pending.label(),
                                None => status,
                            })
                            .when(self.hosts.pending_open.is_some(), |view| {
                                view.child(
                                    Button::new("cancel-host-open")
                                        .focus_ring(focus_visible(cx))
                                        .xsmall()
                                        .h(chrome_rems(24.))
                                        .ghost()
                                        .label("Cancel open")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.hosts.pending_open = None;
                                            cx.notify();
                                        })),
                                )
                            })
                            .when_some(self.hosts.command_error.clone(), |view, error| {
                                view.child(div().text_color(cx.theme().danger).child(error))
                            }),
                    )
                },
            )
            .when(
                self.hosts.section == HostSection::Ports && self.ports.error.is_some(),
                |view| {
                    view.child(
                        div().px_4().py_1().text_size(chrome_rems(12.)).when_some(
                            self.ports.error.clone(),
                            |view, error| {
                                view.child(div().text_color(cx.theme().danger).child(error))
                            },
                        ),
                    )
                },
            )
            .when(!detached_feedback.is_empty(), |view| {
                view.child(
                    div()
                        .id("hosts-detached-feedback")
                        .when(cfg!(all(test, feature = "ui-test-support")), |view| {
                            view.debug_selector(|| "hosts-detached-feedback".into())
                        })
                        .px_4()
                        .py_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_size(chrome_rems(12.))
                        .text_color(warning)
                        .children(detached_feedback),
                )
            })
            .when(!detached_success.is_empty(), |view| {
                view.child(
                    div()
                        .id("ports-detached-success")
                        .px_4()
                        .py_1()
                        .flex()
                        .flex_col()
                        .text_size(chrome_rems(12.))
                        .text_color(muted)
                        .children(detached_success),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(compact, |view| view.flex_col())
                    .child(
                        div()
                            .w(chrome_rems(210.))
                            .when(compact, |view| view.w_full().h(chrome_rems(96.)))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(border)
                            .when_some(self.hosts.search.as_ref(), |view, input| {
                                view.child(
                                    div().p_3().child(
                                        gpui_kit::component::input::Input::new(input).small(),
                                    ),
                                )
                            })
                            .child(directory),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .children(header)
                            .child(self.render_host_tabs(cx))
                            .child(list),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        admit_host_open, admit_withdraw_host_connect, age, display, failure_status,
        invoke_host_menu_for_owner, observation, HostDirectoryState, HostsView, HubUpdate,
        Observation,
    };
    use splitty_client_core::{native_workspace::CommandKind, workspace::SourceWorkspace};
    use tokio::sync::mpsc;
    use uuid::Uuid;

    #[cfg(feature = "ui-test-support")]
    fn with_view(
        run: impl FnOnce(&mut gpui_kit::VisualTestContext, gpui_kit::Entity<super::Workspace>),
    ) {
        use gpui_kit::{AppContext, TestAppContext};
        use splitty_client_core::native_workspace::Worker;
        use std::{
            cell::RefCell,
            rc::Rc,
            sync::{atomic::AtomicBool, Arc},
        };

        let mut app = TestAppContext::single();
        app.update(gpui_kit::init);
        let content = Rc::new(RefCell::new(None));
        let (_, window) = app.add_window_view({
            let content = content.clone();
            move |window, cx| {
                let view = cx.new(|cx| {
                    let (workspace, worker) = super::Workspace::new_with_worker(
                        false,
                        Arc::new(AtomicBool::new(false)),
                        window,
                        cx,
                        |_, _, _| Worker::ui_test_fixture(),
                    );
                    worker.shutdown().unwrap();
                    workspace
                });
                *content.borrow_mut() = Some(view.clone());
                gpui_kit::base::Root::new(view, window, cx)
            }
        });
        run(window, content.borrow_mut().take().unwrap());
    }

    #[cfg(feature = "ui-test-support")]
    #[test]
    fn deliberate_cached_open_is_revoked_or_opens_only_the_current_existing_screen() {
        use super::{Destination, HostAction};
        use splitty_client_core::workspace::WindowWorkspace;
        for outcome in ["open", "cancel", "missing", "owner"] {
            with_view(|window, view| {
                let mut model = WindowWorkspace::new(Uuid::new_v4());
                let key = model
                    .ui_test_screen_order_fixture(&[("work", "Work")], Some("build.test"))
                    .unwrap()
                    .remove(0);
                let owner = model.lifecycle_token();
                let (commands, mut receiver) = mpsc::channel(16);
                window.update(|window, cx| {
                    view.update(cx, |workspace, cx| {
                        workspace.commands = commands;
                        workspace.model = Some(model.clone());
                        workspace.destination = Destination::Hosts;
                        workspace.begin_cached_host_open(
                            owner,
                            "build.test".into(),
                            "work".into(),
                            cx,
                        );
                        assert!(
                            receiver.try_recv().is_err(),
                            "the ready catalog needs no new connection"
                        );
                        match outcome {
                            "cancel" => workspace.host_action(
                                HostAction::Release(
                                    model.host(key.source_id()).unwrap().action_target().clone(),
                                ),
                                cx,
                            ),
                            "missing" => {
                                model.disconnect(key.source_id()).unwrap();
                                model.finish_attempt(key.source_id()).unwrap();
                                model.retry(key.source_id()).unwrap();
                                model
                                    .ui_test_screen_order_fixture(&[], Some("build.test"))
                                    .unwrap();
                                workspace.model = Some(model.clone());
                            }
                            "owner" => workspace.model = Some(WindowWorkspace::new(Uuid::new_v4())),
                            _ => {}
                        }
                        workspace.reconcile_host_open(window, cx);
                        assert!(workspace.hosts.pending_open.is_none());
                        if outcome == "missing" {
                            assert!(workspace
                                .hosts
                                .command_error
                                .as_deref()
                                .unwrap()
                                .contains("no longer"));
                        }
                    });
                });
                let mut opens = 0;
                while let Ok(command) = receiver.try_recv() {
                    match command.kind() {
                        CommandKind::OpenRemoteScreen { screen } => {
                            assert_eq!(screen, &key);
                            opens += 1;
                        }
                        CommandKind::ReleaseHostBrowse { .. }
                        | CommandKind::ReleaseHostBrowsing
                        | CommandKind::Visible { .. } => {}
                        _ => panic!("unexpected cached-open effect"),
                    }
                }
                assert_eq!(opens, usize::from(outcome == "open"));
            });
        }
    }

    #[cfg(feature = "ui-test-support")]
    #[test]
    fn hosts_browser_keeps_catalog_visible_above_terminals() {
        use gpui_kit::{px, size};
        use splitty_client_core::workspace::WindowWorkspace;
        with_view(|window, view| {
            window.simulate_resize(size(px(1200.), px(760.)));
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.model = Some(WindowWorkspace::new(Uuid::new_v4()));
                    workspace.drawer_visible = true;
                    workspace.show_hosts(window, cx);
                });
                window.draw(cx).clear(cx);
            });
            let panel = window.debug_bounds("hosts-view").unwrap();
            let list = window.debug_bounds("hosts-list").unwrap();
            assert!(
                panel.size.height > px(550.),
                "Hosts browser needs a usable catalog: {panel:?}"
            );
            assert!(
                list.size.height > px(350.),
                "Host rows need a visible viewport: {list:?}"
            );
            assert!(list.origin.y + list.size.height <= panel.origin.y + panel.size.height);
        });
    }

    #[cfg(feature = "ui-test-support")]
    #[test]
    fn hosts_can_create_before_any_remote_screen_is_opened() {
        use super::{Destination, HostGroupTarget};
        use gpui_kit::{px, size, Modifiers};
        use splitty_client_core::workspace::WindowWorkspace;
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let (key, _) = model.ui_test_remote_terminal_fixture("empty.test").unwrap();
            model.detach(model.lifecycle_token(), &key).unwrap();
            assert!(model.opened().is_empty());
            assert!(HostGroupTarget::capture(&model, key.source_id()).is_none());
            let (commands, mut receiver) = mpsc::channel(8);
            window.simulate_resize(size(px(1200.), px(760.)));
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(model.clone());
                    workspace.drawer_visible = true;
                    workspace.show_hosts(window, cx);
                });
                window.draw(cx).clear(cx);
            });
            while receiver.try_recv().is_ok() {}
            let button = window
                .debug_bounds(Box::leak(
                    format!("host-new-{}", key.source_id()).into_boxed_str(),
                ))
                .unwrap();
            window.simulate_click(button.center(), Modifiers::default());
            let request = loop {
                let command = receiver
                    .try_recv()
                    .expect("New Screen must enqueue creation");
                if let CommandKind::CreateRemoteScreen { request } = command.kind() {
                    break request.clone();
                }
            };
            assert_eq!(request.target().source_id(), key.source_id());
            window.update(|_, cx| {
                view.update(cx, |workspace, _| {
                    assert!(workspace.destination == Destination::Screens);
                    workspace.reconcile_remote_create(&model);
                })
            });
            assert!(
                request.is_follow_requested(),
                "the pre-admission snapshot must not cancel the first Screen"
            );
        });
    }

    #[cfg(feature = "ui-test-support")]
    #[test]
    fn host_group_target_requires_exact_live_group_but_not_remote_readiness() {
        use super::HostGroupTarget;
        use splitty_client_core::workspace::WindowWorkspace;

        let owner = Uuid::new_v4();
        let mut model = WindowWorkspace::new(owner);
        let keys = model
            .ui_test_screen_order_fixture(&[("one", "One")], Some("one.test"))
            .unwrap();
        let source = keys[0].source_id();
        let target = HostGroupTarget::capture(&model, source).unwrap();
        assert_eq!(target.owner(), model.lifecycle_token());
        assert_eq!(target.source_id(), source);
        assert_eq!(target.target(), "one.test");

        model.disconnect(source).unwrap();
        assert!(
            target.is_current(&model),
            "an offline opened group remains inspectable"
        );
        model.finish_attempt(source).unwrap();
        model.retry(source).unwrap();
        assert!(
            target.is_current(&model),
            "reconnect does not replace the group"
        );
        let mut changed_target = target.clone();
        changed_target.target = "two.test".into();
        assert!(!changed_target.is_current(&model));
        let replacement_keys = model
            .ui_test_screen_order_fixture(&[("other", "Other")], Some("two.test"))
            .unwrap();
        let mut changed_source = target.clone();
        changed_source.source = replacement_keys[0].source_id();
        assert!(!changed_source.is_current(&model));
        model.detach(model.lifecycle_token(), &keys[0]).unwrap();
        assert!(!target.is_current(&model));
        assert!(HostGroupTarget::capture(&model, source).is_none());

        let mut replacement = WindowWorkspace::new(Uuid::new_v4());
        replacement
            .ui_test_screen_order_fixture(&[("one", "One")], Some("one.test"))
            .unwrap();
        assert!(
            !target.is_current(&replacement),
            "another local owner cannot reuse the target"
        );
    }

    #[cfg(feature = "ui-test-support")]
    #[test]
    fn host_group_navigation_uses_current_rows_and_never_submits_an_action() {
        use super::{Destination, HostGroupTarget, HostRow, HostSection};
        use splitty_client_core::{hub::HostDirectorySnapshot, workspace::WindowWorkspace};
        use std::sync::Arc;

        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let mut model = WindowWorkspace::new(owner);
            let first = model
                .ui_test_screen_order_fixture(&[("a", "A")], Some("one.test"))
                .unwrap();
            model
                .ui_test_screen_order_fixture(&[("b", "B")], Some("two.test"))
                .unwrap();
            let one = HostGroupTarget::capture(&model, first[0].source_id()).unwrap();
            let two =
                HostGroupTarget::capture(&model, model.source_for_target("two.test").unwrap())
                    .unwrap();
            let (commands, mut receiver) = mpsc::channel(32);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(model);
                    workspace.hosts.set_owner(owner);
                    assert!(workspace.hosts.accept(HubUpdate {
                        connection_id: owner,
                        generation: 1,
                        state: HostDirectoryState::Ready(Arc::new(
                            HostDirectorySnapshot::ui_test_fixture_with_aliases(
                                owner,
                                1,
                                &[("two.test", Some("Same")), ("one.test", Some("Same"))],
                            )
                            .unwrap(),
                        )),
                    }));
                    let host_index = workspace
                        .project_host_rows()
                        .iter()
                        .position(|row| {
                            matches!(row,
                        HostRow::Host { target, .. } if target == one.target())
                        })
                        .unwrap();
                    assert!(workspace.show_host_group(&one, HostSection::Host, window, cx));
                    assert!(workspace.destination == Destination::Hosts);
                    assert_eq!(
                        workspace
                            .hosts
                            .scroll
                            .0
                            .borrow()
                            .deferred_scroll_to_item
                            .unwrap()
                            .item_index,
                        0
                    );

                    let ports_index = workspace
                        .project_host_rows()
                        .iter()
                        .position(|row| {
                            matches!(row,
                        HostRow::PortHeader { target, .. } if target == two.target())
                        })
                        .unwrap();
                    assert!(
                        workspace.show_host_group(&two, HostSection::Ports, window, cx),
                        "already-open Hosts must still scroll"
                    );
                    assert_eq!(
                        workspace
                            .hosts
                            .scroll
                            .0
                            .borrow()
                            .deferred_scroll_to_item
                            .unwrap()
                            .item_index,
                        0
                    );
                    assert_ne!(
                        host_index, ports_index,
                        "colliding aliases do not merge canonical rows"
                    );

                    workspace.open_host_connect(window, cx);
                    assert!(!workspace.show_host_group(&one, HostSection::Host, window, cx));
                    assert_eq!(
                        workspace
                            .hosts
                            .scroll
                            .0
                            .borrow()
                            .deferred_scroll_to_item
                            .unwrap()
                            .item_index,
                        0
                    );
                    workspace.close_host_connect(window, cx);
                    assert!(workspace.hosts.accept(HubUpdate {
                        connection_id: owner,
                        generation: 2,
                        state: HostDirectoryState::Ready(Arc::new(
                            HostDirectorySnapshot::ui_test_fixture_with_aliases(
                                owner,
                                2,
                                &[("one.test", Some("Same")), ("two.test", Some("Same"))],
                            )
                            .unwrap(),
                        )),
                    }));
                    let reordered_index = workspace
                        .project_host_rows()
                        .iter()
                        .position(|row| matches!(row, HostRow::Host { target, .. } if target == one.target()))
                        .unwrap();
                    assert_ne!(reordered_index, host_index);
                    assert!(workspace.show_host_group(&one, HostSection::Host, window, cx));
                    assert_eq!(workspace.hosts.scroll.0.borrow().deferred_scroll_to_item.unwrap().item_index, 0);

                    workspace
                        .model
                        .as_mut()
                        .unwrap()
                        .detach(one.owner(), &first[0])
                        .unwrap();
                    assert!(!workspace.show_host_group(&one, HostSection::Host, window, cx));
                    workspace.ports_owner_changed(one.owner(), window, cx);
                    workspace.open_port_form(None, window, cx);
                    assert!(workspace.ports.has_open_form());
                    assert!(!workspace.show_host_group(&two, HostSection::Ports, window, cx));
                    assert_eq!(workspace.hosts.scroll.0.borrow().deferred_scroll_to_item.unwrap().item_index, 0);
                });
                window.draw(cx).clear(cx);
            });
            while let Ok(command) = receiver.try_recv() {
                assert!(
                    matches!(
                        command.kind(),
                        CommandKind::RefreshForwards | CommandKind::Visible { .. }
                    ),
                    "passive navigation queued command kind {:?}",
                    std::mem::discriminant(command.kind())
                );
            }
        });
    }

    #[test]
    fn host_open_keeps_rendered_attempt_and_requires_queue_admission() {
        let mut source = SourceWorkspace::new(Uuid::new_v4());
        let captured = source.source_token();
        let screen = source.screen_key("screen-a");
        source.reset_connection(Uuid::new_v4());
        let current = source.source_token();
        let (commands, mut receiver) = mpsc::channel(1);

        assert!(!admit_host_open(&commands, Some(current), captured, &screen, true).unwrap());
        assert!(!admit_host_open(&commands, Some(current), current, &screen, false).unwrap());
        assert!(receiver.try_recv().is_err());

        assert!(admit_host_open(&commands, Some(current), current, &screen, true).unwrap());
        assert!(admit_host_open(&commands, Some(current), current, &screen, true).is_err());
        let queued = receiver.try_recv().unwrap();
        assert_eq!(queued.source_token(), Some(current));
        assert!(matches!(
            queued.kind(),
            CommandKind::OpenRemoteScreen { screen: queued_screen } if queued_screen == &screen
        ));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn retained_host_menu_and_cancel_do_not_cross_owner_or_full_queue() {
        let mut source = SourceWorkspace::new(Uuid::new_v4());
        let captured = source.source_token();
        source.reset_connection(Uuid::new_v4());
        let current = source.source_token();
        let mut invoked = false;
        invoke_host_menu_for_owner(captured, Some(current), || invoked = true);
        assert!(!invoked);
        invoke_host_menu_for_owner(captured, Some(captured), || invoked = true);
        assert!(invoked);

        let (commands, mut receiver) = mpsc::channel(1);
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        admit_withdraw_host_connect(&commands, captured, first).unwrap();
        assert!(admit_withdraw_host_connect(&commands, captured, second).is_err());
        let queued = receiver.try_recv().unwrap();
        assert_eq!(queued.source_token(), Some(captured));
        assert!(matches!(
            queued.kind(),
            CommandKind::WithdrawHostConnect { request_id } if *request_id == first
        ));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn directory_updates_follow_the_current_owner_and_allow_completion_of_loading() {
        let owner = Uuid::new_v4();
        let other = Uuid::new_v4();
        let mut view = HostsView::default();
        view.set_owner(owner);
        let loading = HubUpdate {
            connection_id: owner,
            generation: 4,
            state: HostDirectoryState::Loading { previous: None },
        };
        assert!(view.accept(loading));
        assert!(view.accept(HubUpdate {
            connection_id: owner,
            generation: 4,
            state: HostDirectoryState::Failed {
                previous: None,
                reason: "read failed".into(),
            },
        }));
        assert!(!view.accept(HubUpdate {
            connection_id: owner,
            generation: 4,
            state: HostDirectoryState::Loading { previous: None },
        }));
        assert!(!view.accept(HubUpdate {
            connection_id: owner,
            generation: 3,
            state: HostDirectoryState::Failed {
                previous: None,
                reason: "old result".into(),
            },
        }));
        assert!(!view.accept(HubUpdate {
            connection_id: other,
            generation: 5,
            state: HostDirectoryState::Loading { previous: None },
        }));
        view.set_owner(other);
        assert_eq!(view.generation, 0);
        assert!(view.snapshot().is_none());
        assert!(!view.accept(HubUpdate {
            connection_id: owner,
            generation: 6,
            state: HostDirectoryState::Loading { previous: None },
        }));
    }

    #[cfg(feature = "ui-test-support")]
    #[test]
    fn authoritative_ready_settles_applied_even_when_another_writer_changes_alias() {
        use super::{HostManagementAction, HostManagementOutcome, HostMutation};
        use splitty_client_core::hub::HostDirectorySnapshot;
        use std::sync::Arc;

        let owner = Uuid::new_v4();
        let token = SourceWorkspace::new(owner).source_token();
        let mut view = HostsView::default();
        view.set_owner(owner);
        let previous =
            Arc::new(HostDirectorySnapshot::ui_test_fixture(owner, 1, &["beta.example"]).unwrap());
        assert!(view.accept(HubUpdate {
            connection_id: owner,
            generation: 1,
            state: HostDirectoryState::Ready(previous.clone()),
        }));
        view.record_mutation(
            "beta.example".into(),
            HostMutation {
                owner: token,
                request_id: Uuid::new_v4(),
                action: HostManagementAction::SetAlias,
                outcome: HostManagementOutcome::Applied,
                reason: None,
            },
        );
        view.record_mutation(
            "other.example".into(),
            HostMutation {
                owner: token,
                request_id: Uuid::new_v4(),
                action: HostManagementAction::Forget,
                outcome: HostManagementOutcome::Uncertain,
                reason: Some("reply lost".into()),
            },
        );
        assert!(view.accept(HubUpdate {
            connection_id: owner,
            generation: 2,
            state: HostDirectoryState::Loading {
                previous: Some(previous),
            },
        }));
        assert!(view.mutations.contains_key("beta.example"));
        assert!(view.accept(HubUpdate {
            connection_id: owner,
            generation: 2,
            state: HostDirectoryState::Ready(Arc::new(
                HostDirectorySnapshot::ui_test_fixture(owner, 2, &["beta.example"]).unwrap(),
            )),
        }));
        assert!(!view.mutations.contains_key("beta.example"));
        assert_eq!(
            view.mutations["other.example"].outcome,
            HostManagementOutcome::Uncertain
        );
    }

    #[cfg(feature = "ui-test-support")]
    #[test]
    fn accepted_alias_is_owner_bound_and_clears_with_authoritative_directory() {
        use splitty_client_core::hub::HostDirectorySnapshot;
        use std::sync::Arc;

        let owner = Uuid::new_v4();
        let token = SourceWorkspace::new(owner).source_token();
        let stale = SourceWorkspace::new(Uuid::new_v4()).source_token();
        let mut view = HostsView::default();
        view.set_owner(owner);
        assert!(view.accept(HubUpdate {
            connection_id: owner,
            generation: 1,
            state: HostDirectoryState::Ready(Arc::new(
                HostDirectorySnapshot::ui_test_fixture_with_aliases(
                    owner,
                    1,
                    &[("beta.example", Some("Beta work"))],
                )
                .unwrap(),
            )),
        }));
        assert_eq!(
            view.accepted_alias(token, "beta.example"),
            Some("Beta work")
        );
        assert_eq!(view.accepted_alias(stale, "beta.example"), None);
        assert!(view.accept(HubUpdate {
            connection_id: owner,
            generation: 2,
            state: HostDirectoryState::Ready(Arc::new(
                HostDirectorySnapshot::ui_test_fixture(owner, 2, &["beta.example"]).unwrap(),
            )),
        }));
        assert_eq!(view.accepted_alias(token, "beta.example"), None);
    }

    #[test]
    fn unknown_and_observed_empty_remain_distinct() {
        assert_eq!(observation(None, 0), Observation::Unknown);
        assert_eq!(observation(Some(0), 0), Observation::Empty);
        assert_eq!(observation(None, 1), Observation::Cached);
        assert_eq!(age(120, 179), "just now");
        assert_eq!(age(120, 180), "1 min ago");
        assert_eq!(age(181, 180), "time unavailable");
        assert_eq!(
            failure_status(false, "read failed"),
            "Could not load hosts: read failed"
        );
        assert_eq!(
            failure_status(true, "read failed"),
            "Refresh failed: read failed · showing cached observations"
        );
    }

    #[test]
    fn display_removes_direction_overrides_without_changing_host_identity() {
        let raw = "alpha\u{202e}beta\n";
        assert_eq!(display(raw), "alpha\u{fffd}beta\u{fffd}");
        assert_eq!(raw, "alpha\u{202e}beta\n");
    }
}
