//! Passive, owner-bound host directory presentation. No row is an attach target.

use super::*;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{tooltip::Tooltip, ActiveTheme as _, Disableable};
use gpui_kit::UniformListScrollHandle;
use splitty_client_core::hub::{HostDirectoryState, HubUpdate};
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
    pub(super) open: bool,
    owner: Option<Uuid>,
    generation: u64,
    state: HostDirectoryState,
    command_error: Option<String>,
    scroll: UniformListScrollHandle,
}

impl Default for HostsView {
    fn default() -> Self {
        Self {
            open: false,
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

    fn row_count(&self) -> usize {
        self.snapshot().map_or(0, |snapshot| {
            snapshot
                .directory()
                .hosts
                .iter()
                .map(|host| 1 + host.screens.len() + usize::from(host.screens.is_empty()))
                .sum()
        })
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

fn display(text: &str) -> String {
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

impl Workspace {
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
        if self.hosts_navigation_blocked() || self.hosts.open {
            return;
        }
        self.hosts.open = true;
        self.wheel_navigation.reset();
        self.sync_visible(cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn show_screens(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.hosts.open {
            return;
        }
        self.hosts.open = false;
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
            .model
            .as_ref()
            .is_some_and(|model| model.source_token() == source)
        {
            self.show_screens(window, cx);
        }
    }

    pub(super) fn refresh_hosts(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.model.as_ref().map(LocalWorkspace::source_token) else {
            return;
        };
        self.refresh_hosts_captured(source, cx);
    }

    fn refresh_hosts_captured(&mut self, source: SourceToken, cx: &mut Context<Self>) {
        if !self.hosts.can_refresh() {
            return;
        }
        if !self
            .model
            .as_ref()
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
        let source = self.model.as_ref().map(LocalWorkspace::source_token);
        Button::new("open-hosts")
            .label("Hosts")
            .accessibility_label("Show Hosts")
            .tooltip("Inspect cached Hosts")
            .icon(Icon::new(IconName::Server))
            .selected(self.hosts.open)
            .disabled(source.is_none() || (self.hosts_navigation_blocked() && !self.hosts.open))
            .ghost()
            .compact()
            .small()
            .on_click(move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if !source.is_some_and(|source| {
                        this.model
                            .as_ref()
                            .is_some_and(|model| model.source_token() == source)
                    }) {
                        return;
                    }
                    if this.hosts.open {
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
        let total = self.hosts.row_count();
        let snapshot = self.hosts.snapshot().cloned();
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
                    "Refreshing hosts · showing cached observations"
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
                    "Cached observations · host reachability is not checked"
                }
                .to_owned(),
                false,
                false,
            ),
            HostDirectoryState::Failed { previous, reason } => {
                (failure_status(previous.is_some(), reason), true, false)
            }
        };
        let weak_back = cx.entity().downgrade();
        let weak_refresh = cx.entity().downgrade();
        let source = self.model.as_ref().map(LocalWorkspace::source_token);
        let list = if let Some(snapshot) = snapshot {
            uniform_list("hosts-list", total, move |range, _, cx| {
                let muted = cx.theme().muted_foreground;
                let border = cx.theme().border;
                let hosts = &snapshot.directory().hosts;
                let mut rows = Vec::with_capacity(range.len());
                for (host_index, row) in hosts
                    .iter()
                    .enumerate()
                    .flat_map(|(index, host)| {
                        (0..1 + host.screens.len() + usize::from(host.screens.is_empty()))
                            .map(move |row| (index, row))
                    })
                    .skip(range.start)
                    .take(range.len())
                {
                    let host = &hosts[host_index];
                    let target = display(&host.target);
                    if row == 0 {
                        let title = display(host.alias.as_deref().unwrap_or(&host.target));
                        let timestamp = host
                            .last_seen
                            .map(|seen| format!("Last observed {}", age(seen, now)));
                        rows.push(
                            div()
                                .id(SharedString::from(format!("host:{}", host.target)))
                                .h(row_height)
                                .w_full()
                                .max_w_full()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .justify_center()
                                .px_3()
                                .border_b_1()
                                .border_color(border)
                                .tooltip({
                                    let target = target.clone();
                                    move |window, cx| Tooltip::new(target.clone()).build(window, cx)
                                })
                                .child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_sm()
                                        .text_color(muted)
                                        .child(target),
                                )
                                .when_some(timestamp, |row, timestamp| {
                                    row.child(div().text_sm().text_color(muted).child(timestamp))
                                })
                                .into_any_element(),
                        );
                    } else if let Some(screen) = host.screens.get(row - 1) {
                        let label = display(screen.name.as_deref().unwrap_or(&screen.id));
                        let metadata = format!("{target} · {}", display(&screen.id));
                        let count = if screen.panes == 1 {
                            "1 pane".to_owned()
                        } else {
                            format!("{} panes", screen.panes)
                        };
                        rows.push(
                            div()
                                .id(SharedString::from(format!(
                                    "host-screen:{}:{}:{}",
                                    host.target.len(),
                                    host.target,
                                    screen.id
                                )))
                                .h(row_height)
                                .w_full()
                                .max_w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .px_3()
                                .border_b_1()
                                .border_color(border)
                                .tooltip({
                                    let metadata = metadata.clone();
                                    move |window, cx| {
                                        Tooltip::new(metadata.clone()).build(window, cx)
                                    }
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(label),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .text_sm()
                                                .text_color(muted)
                                                .child(metadata),
                                        ),
                                )
                                .child(div().flex_none().text_color(muted).child(count))
                                .into_any_element(),
                        );
                    } else {
                        let message = match observation(host.last_seen, host.screens.len()) {
                            Observation::Empty => "Last observation had no Screens",
                            Observation::Unknown => "No Screen observation yet",
                            Observation::Cached => unreachable!("cached hosts have Screen rows"),
                        };
                        rows.push(
                            div()
                                .id(SharedString::from(format!(
                                    "host-observation:{}",
                                    host.target
                                )))
                                .h(row_height)
                                .w_full()
                                .max_w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .px_3()
                                .text_color(muted)
                                .child(message)
                                .into_any_element(),
                        );
                    }
                }
                rows
            })
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .max_w_full()
            .px_4()
            .track_scroll(&self.hosts.scroll)
            .into_any_element()
        } else {
            div().flex_1().into_any_element()
        };
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
        age, display, failure_status, observation, HostDirectoryState, HostsView, HubUpdate,
        Observation,
    };
    use uuid::Uuid;

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
