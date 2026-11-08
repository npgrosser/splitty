//! Owner-bound host directory and retained local host administration.

use super::*;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    menu::{DropdownMenu, PopupMenuItem},
    ActiveTheme as _, Disableable,
};
use gpui_kit::UniformListScrollHandle;
use splitty_client_core::hub::{
    HostDirectoryState, HostManagementAction, HostManagementOutcome, HostManagementResult,
    HubUpdate,
};
use splitty_client_core::workspace::{
    CatalogState, HostActionTarget, HostUpdatePlan, HostUpdateState, SourceStatus,
};
use std::{
    collections::{HashMap, HashSet},
    time::{SystemTime, UNIX_EPOCH},
};

mod editor;

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

pub(super) struct HostsView {
    owner: Option<Uuid>,
    generation: u64,
    state: HostDirectoryState,
    pub(super) command_error: Option<String>,
    scroll: UniformListScrollHandle,
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
enum HostAction {
    Connect { owner: SourceToken, target: String },
    Cancel(HostActionTarget),
    Retry(HostActionTarget),
    RetryCatalog(HostActionTarget),
    Release(HostActionTarget),
}

#[derive(Clone)]
enum HostRow {
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

fn admit_withdraw_host_connect(
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
    fn host_action(&mut self, action: HostAction, cx: &mut Context<Self>) {
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

    fn open_host_screen(
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
            Ok(true) => self.show_screens(window, cx),
            Ok(false) => {}
            Err(error) => {
                self.hosts.command_error = Some(format!("Open could not be sent: {error}"));
                cx.notify();
            }
        }
    }

    fn hosts_navigation_blocked(&self) -> bool {
        self.rename.is_some()
            || self.template_dialog.is_some()
            || self.profile_picker.is_some()
            || self.transfer_menu.is_some()
            || self.pending_template_apply.is_some()
            || self.pending_close.is_some()
            || self.drag.is_some()
            || self.float_gesture.is_some()
    }

    fn show_hosts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hosts_navigation_blocked() || self.destination == Destination::Hosts {
            return;
        }
        self.destination = Destination::Hosts;
        self.refresh_ports(cx);
        self.wheel_navigation.reset();
        self.sync_visible(cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn show_screens(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.destination == Destination::Screens {
            return;
        }
        if self.destination == Destination::Hosts {
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
            if self.hosts.connect.take().is_some() || self.hosts.alias_editor.take().is_some() {
                self.hosts.connect_subscription = None;
                self.hosts.alias_subscription = None;
                window.close_dialog(cx);
            }
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

    pub(super) fn render_hosts_navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let source = self.local_model().map(SourceWorkspace::source_token);
        Button::new("open-hosts")
            .accessibility_label("Show Hosts")
            .tooltip("Inspect cached Hosts")
            .selected(self.destination == Destination::Hosts)
            .disabled(
                source.is_none()
                    || (self.hosts_navigation_blocked() && self.destination != Destination::Hosts),
            )
            .ghost()
            .compact()
            .small()
            .w_full()
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(chrome_rems(10.))
                    .text_color(rgb(self.appearance.secondary))
                    .child(
                        div()
                            .w(chrome_rems(15.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .child(Icon::new(IconName::Server).size_3()),
                    )
                    .child("Hosts"),
            )
            .on_click(move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if !source.is_some_and(|source| {
                        this.local_model()
                            .is_some_and(|model| model.source_token() == source)
                    }) {
                        return;
                    }
                    if this.destination == Destination::Hosts {
                        this.show_screens(window, cx);
                    } else {
                        this.show_hosts(window, cx);
                    }
                });
            })
            .into_any_element()
    }

    pub(super) fn render_hosts(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let foreground = theme.foreground;
        let border = theme.border;
        let surface = theme.background;
        let row_height = chrome_rems((self.appearance.font_size * 4.0 + 16.0).max(82.0));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
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
        let rows = Arc::new(rows);
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
                if self.model.as_ref().is_some_and(|model| {
                    model.hosts().any(|host| {
                        model
                            .source(host.action_target().source_id())
                            .is_some_and(|source| source.port_observation().is_none())
                    })
                }) =>
            {
                "Some hosts have not reported port discovery. Add a port manually.".to_owned()
            }
            Some(_) if forward_rows.is_empty() => {
                "No detected or saved ports. Add a port manually.".to_owned()
            }
            Some(_) => {
                "Ports are shown under each host. Forwarding starts only when requested.".to_owned()
            }
        };
        let count = rows.len();
        let list = uniform_list("hosts-list", count, {
            let rows = rows.clone();
            let weak = cx.entity().downgrade();
            move |range, window, cx| {
                let muted = cx.theme().muted_foreground;
                let border = cx.theme().border;
                let mut views = Vec::with_capacity(range.len());
                for row in rows[range].iter() {
                    let view = match row {
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
                            let mut entry = div()
                                .id(SharedString::from(format!(
                                    "host:{}:{target}",
                                    target.len()
                                )))
                                .h(row_height)
                                .w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_3()
                                .border_b_1()
                                .border_color(border)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(title.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(muted)
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(display(target)),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(muted)
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(detail.clone()),
                                        )
                                        .when_some(feedback.clone(), |view, feedback| {
                                            view.child(
                                                div()
                                                    .text_sm()
                                                    .text_color(cx.theme().warning)
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .child(feedback),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_sm()
                                        .text_color(muted)
                                        .child(state.clone()),
                                );
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
                                entry = entry.child(
                                    Button::new(SharedString::from(format!(
                                        "host-action:{}:{target}",
                                        target.len()
                                    )))
                                    .small()
                                    .label(label)
                                    .accessibility_label(format!("{label} {target}"))
                                    .on_click(
                                        move |_, _, cx| {
                                            let _ = weak.update(cx, |this, cx| {
                                                this.host_action(action.clone(), cx)
                                            });
                                        },
                                    ),
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
                                entry = entry.child(
                                    Button::new(SharedString::from(format!(
                                        "host-update-{target}"
                                    )))
                                    .debug_selector({
                                        let target = target.clone();
                                        move || format!("host-update-{target}")
                                    })
                                    .small()
                                    .label("Update…")
                                    .track_focus(&trigger)
                                    .disabled(*update_blocked)
                                    .on_click(
                                        move |_, window, cx| {
                                            let _ = update_weak.update(cx, |this, cx| {
                                                this.request_host_update(
                                                    update_target.clone(),
                                                    update_focus.clone(),
                                                    window,
                                                    cx,
                                                )
                                            });
                                        },
                                    ),
                                );
                            }
                            if *refresh_backend {
                                let refresh_weak = weak.clone();
                                entry = entry.child(
                                    Button::new(SharedString::from(format!(
                                        "host-backend-refresh-{target}"
                                    )))
                                    .debug_selector({
                                        let target = target.clone();
                                        move || format!("host-backend-refresh-{target}")
                                    })
                                    .small()
                                    .ghost()
                                    .label("Refresh version")
                                    .on_click(
                                        move |_, _, cx| {
                                            let _ = refresh_weak.update(cx, |this, cx| {
                                                this.refresh_backend_version(cx)
                                            });
                                        },
                                    ),
                                );
                            }
                            if *saved {
                                let owner = *owner;
                                let edit_target = target.clone();
                                let edit_alias = alias.clone();
                                let forget_target = target.clone();
                                let edit_weak = weak.clone();
                                let forget_weak = weak.clone();
                                entry = entry.child(
                                    Button::new(SharedString::from(format!(
                                        "host-more:{}:{target}",
                                        target.len()
                                    )))
                                    .accessibility_label(format!("More for host {target}"))
                                    .icon(Icon::new(IconName::Ellipsis))
                                    .ghost()
                                    .compact()
                                    .small()
                                    .disabled(*mutation_pending || owner.is_none())
                                    .dropdown_menu(
                                        move |menu, _, _| {
                                            let edit_target = edit_target.clone();
                                            let forget_target = forget_target.clone();
                                            let edit_alias = edit_alias.clone();
                                            let edit_weak = edit_weak.clone();
                                            let forget_weak = forget_weak.clone();
                                            let Some(owner) = owner else {
                                                return menu;
                                            };
                                            menu.item(
                                                PopupMenuItem::new("Change display name…")
                                                    .on_click(move |_, window, cx| {
                                                        let _ = edit_weak.update(cx, |this, cx| {
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
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new("Remove from list").on_click(
                                                    move |_, _, cx| {
                                                        let _ =
                                                            forget_weak.update(cx, |this, cx| {
                                                                invoke_host_menu_for_owner(
                                                                    owner,
                                                                    this.lifecycle_token(),
                                                                    || {
                                                                        this.forget_host(
                                                                            &forget_target,
                                                                            cx,
                                                                        )
                                                                    },
                                                                );
                                                            });
                                                    },
                                                ),
                                            )
                                        },
                                    ),
                                );
                            }
                            entry.into_any_element()
                        }
                        HostRow::Screen {
                            target,
                            id,
                            label,
                            detail,
                            route,
                            opened,
                        } => {
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
                                .pl_6()
                                .border_b_1()
                                .border_color(border)
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
                                                .text_sm()
                                                .text_color(muted)
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(detail.clone()),
                                        ),
                                );
                            if let Some((source, key)) = route {
                                let source = *source;
                                let key = key.clone();
                                let weak = weak.clone();
                                let action = if *opened { "View" } else { "Open" };
                                entry = entry.child(
                                    Button::new(SharedString::from(format!(
                                        "host-open:{}:{target}:{}:{id}",
                                        target.len(),
                                        id.len()
                                    )))
                                    .small()
                                    .label(action)
                                    .accessibility_label(format!("{action} {label} on {target}"))
                                    .on_click(
                                        move |_, window, cx| {
                                            let _ = weak.update(cx, |this, cx| {
                                                this.open_host_screen(
                                                    source,
                                                    key.clone(),
                                                    window,
                                                    cx,
                                                )
                                            });
                                        },
                                    ),
                                );
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
                        } => Self::render_pending_port(
                            key,
                            *owner,
                            feedback,
                            row_height,
                            weak.clone(),
                            cx,
                        ),
                    };
                    views.push(view);
                }
                views
            }
        })
        .flex_1()
        .min_h_0()
        .min_w_0()
        .w_full()
        .px_4()
        .track_scroll(&self.hosts.scroll);
        let source = self.local_model().map(SourceWorkspace::source_token);
        let weak_back = cx.entity().downgrade();
        let weak_refresh = cx.entity().downgrade();
        let weak_connect = cx.entity().downgrade();
        let weak_port = cx.entity().downgrade();
        div()
            .id("hosts-view")
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
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        Button::new("hosts-back")
                            .icon(Icon::new(IconName::ArrowLeft))
                            .label("Screens")
                            .accessibility_label("Back to Screens")
                            .ghost()
                            .small()
                            .on_click(move |_, window, cx| {
                                if let Some(source) = source {
                                    let _ = weak_back.update(cx, |this, cx| {
                                        this.show_screens_captured(source, window, cx)
                                    });
                                }
                            }),
                    )
                    .child(div().font_weight(FontWeight::BOLD).child("Hosts"))
                    .child(div().flex_1())
                    .child(
                        Button::new("hosts-add-port")
                            .label("Add port…")
                            .accessibility_label("Add port")
                            .track_focus(&self.port_trigger)
                            .small()
                            .disabled(!self.ports.can_manage())
                            .on_click(move |_, window, cx| {
                                let _ = weak_port
                                    .update(cx, |this, cx| this.open_port_form(None, window, cx));
                            }),
                    )
                    .child(
                        Button::new("hosts-connect")
                            .label("Connect…")
                            .accessibility_label("Connect to host")
                            .track_focus(&self.host_connect_trigger)
                            .small()
                            .disabled(source.is_none())
                            .on_click(move |_, window, cx| {
                                let _ = weak_connect
                                    .update(cx, |this, cx| this.open_host_connect(window, cx));
                            }),
                    )
                    .child(
                        Button::new("hosts-refresh")
                            .label(if failed { "Retry" } else { "Refresh" })
                            .accessibility_label(if failed {
                                "Retry host directory refresh"
                            } else {
                                "Refresh host directory"
                            })
                            .disabled(loading || !self.hosts.can_refresh())
                            .small()
                            .on_click(move |_, _, cx| {
                                if let Some(source) = source {
                                    let _ = weak_refresh.update(cx, |this, cx| {
                                        this.refresh_hosts_captured(source, cx)
                                    });
                                }
                            }),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .text_color(if failed { theme.danger } else { muted })
                    .child(status)
                    .when_some(self.hosts.command_error.clone(), |view, error| {
                        view.child(div().text_color(cx.theme().danger).child(error))
                    }),
            )
            .child(
                div()
                    .px_4()
                    .py_1()
                    .text_sm()
                    .text_color(muted)
                    .child(ports_status)
                    .when_some(self.ports.error.clone(), |view, error| {
                        view.child(div().text_color(cx.theme().danger).child(error))
                    }),
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
                        .text_sm()
                        .text_color(theme.warning)
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
                        .text_sm()
                        .text_color(muted)
                        .children(detached_success),
                )
            })
            .child(list)
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
