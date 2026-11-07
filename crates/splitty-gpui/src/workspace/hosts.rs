//! Passive, owner-bound host directory presentation. No row is an attach target.

use super::*;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{ActiveTheme as _, Disableable};
use gpui_kit::UniformListScrollHandle;
use splitty_client_core::hub::{HostDirectoryState, HubUpdate};
use splitty_client_core::workspace::{CatalogState, HostActionTarget, SourceStatus};
use std::time::{SystemTime, UNIX_EPOCH};

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
    command_error: Option<String>,
    scroll: UniformListScrollHandle,
}

impl Default for HostsView {
    fn default() -> Self {
        Self {
            owner: None,
            generation: 0,
            state: HostDirectoryState::Unavailable,
            command_error: None,
            scroll: UniformListScrollHandle::new(),
        }
    }
}

impl HostsView {
    pub(super) fn set_owner(&mut self, owner: Uuid) {
        if self.owner == Some(owner) {
            return;
        }
        self.owner = Some(owner);
        self.generation = 0;
        self.state = HostDirectoryState::Unavailable;
        self.command_error = None;
        self.scroll = UniformListScrollHandle::new();
    }

    pub(super) fn accept(&mut self, update: HubUpdate) -> bool {
        if !accepts_update(self.owner, self.generation, &self.state, &update) {
            return false;
        }
        self.generation = update.generation;
        self.state = update.state;
        self.command_error = None;
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

    fn can_refresh(&self) -> bool {
        self.owner.is_some()
            && !matches!(
                self.state,
                HostDirectoryState::Unavailable | HostDirectoryState::Loading { .. }
            )
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
    },
    Screen {
        target: String,
        id: String,
        label: String,
        detail: String,
        route: Option<(SourceToken, ScreenKey)>,
        opened: bool,
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
        self.wheel_navigation.reset();
        self.sync_visible(cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn show_screens(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.destination == Destination::Screens {
            return;
        }
        let browsing = self
            .model
            .as_ref()
            .map(|model| {
                model
                    .hosts()
                    .filter(|host| host.is_browsing())
                    .map(|host| host.action_target().clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for target in browsing {
            let owner = target.local_owner();
            if let Err(error) = self.commands.try_send(WorkerCommand::new(
                owner,
                Command::ReleaseHostBrowse { target },
            )) {
                self.hosts.command_error = Some(format!("Could not leave host browsing: {error}"));
                cx.notify();
                return;
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
        let row_height = chrome_rems((self.appearance.font_size * 3.0 + 16.0).max(64.0));
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
        if let Some(snapshot) = self.hosts.snapshot() {
            for host in &snapshot.directory().hosts {
                let target = host.target.clone();
                let title = display(host.alias.as_deref().unwrap_or(&target));
                let assigned = self
                    .model
                    .as_ref()
                    .and_then(|model| model.source_for_target(&target));
                let presentation = assigned.and_then(|id| self.model.as_ref()?.host(id));
                let action = match presentation {
                    None => self.lifecycle_token().map(|owner| HostAction::Connect {
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
                let state = match presentation.map(|host| host.status()) {
                    None => "Not connected",
                    Some(SourceStatus::Connecting) => "Connecting…",
                    Some(SourceStatus::Reconnecting) => "Reconnecting…",
                    Some(SourceStatus::Ready) => "Connected",
                    Some(SourceStatus::Offline) => "Offline",
                    Some(SourceStatus::Closing) => "Closing…",
                }
                .to_owned();
                let detail = presentation
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
                        host.last_seen
                            .map(|seen| format!("Last observed {}", age(seen, now)))
                    })
                    .unwrap_or_else(|| "Reachability has not been checked".into());
                rows.push(HostRow::Host {
                    target: target.clone(),
                    title,
                    state,
                    detail,
                    action,
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
                            CatalogState::Failed => {
                                "Live catalog failed · retry the catalog".into()
                            }
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
                                label: display(
                                    screen.name.as_deref().unwrap_or(screen.id.as_str()),
                                ),
                                detail: count.map_or_else(
                                    || "Last observed · pane count unknown".into(),
                                    |n| format!("Last observed · {n} panes"),
                                ),
                                route: None,
                                opened: false,
                            });
                        }
                    } else {
                        for screen in &host.screens {
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
                    if host.screens.is_empty() && presentation.is_none() {
                        let detail = match observation(host.last_seen, 0) {
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
            }
        }
        let rows = Arc::new(rows);
        let count = rows.len();
        let list = uniform_list("hosts-list", count, {
            let rows = rows.clone();
            let weak = cx.entity().downgrade();
            move |range, _, cx| {
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
                                        ),
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
            .child(list)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        admit_host_open, age, display, failure_status, observation, HostDirectoryState, HostsView,
        HubUpdate, Observation,
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
