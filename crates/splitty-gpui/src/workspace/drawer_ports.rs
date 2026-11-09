//! Drawer commands project the existing host and forwarding owners.
use super::hosts::{HostGroupTarget, HostSection};
use super::ports::OpenOrigin;
use super::*;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use splitty_client_core::ports::{ForwardRequest, ForwardRow, ForwardState, MappingKey};
use splitty_client_core::workspace::{HostPresentation, SourceStatus};

const MAX_MENU_PORTS: usize = 12;

// Host caption metrics from the web client's `Drawer.svelte`, in CSS pixels
// at zoom 1; colors come from the theme.

/// `.drawer-host`: `margin-top: 8px`; its caption: `padding-top: 8px` under
/// a `border-subtle` line, `gap: 2px`.
const HOST_MARGIN_TOP: f32 = 8.;
const HOST_CAPTION_PADDING_TOP: f32 = 8.;
const HOST_CAPTION_GAP: f32 = 2.;
/// `.drawer-host-name`: 26 px tall, `padding: 0 8px`, `gap: 7px`, 11.5px,
/// with a 13 px server icon.
const HOST_NAME_HEIGHT: f32 = 26.;
const HOST_NAME_PADDING_X: f32 = 8.;
const HOST_NAME_GAP: f32 = 7.;
const HOST_NAME_TEXT: f32 = 11.5;
const HOST_ICON: f32 = 13.;
/// `.drawer-host-dot`: 7 px; offline at opacity .6; never connected as a
/// 1.5 px ring.
const HOST_DOT: f32 = 7.;
const HOST_DOT_OFFLINE_OPACITY: f32 = 0.6;
const HOST_DOT_RING: f32 = 1.5;
/// `.drawer-host-ports`: a 22 px pill, `padding: 0 6px`, radius 11, 10.5px.
const PORTS_HEIGHT: f32 = 22.;
const PORTS_PADDING_X: f32 = 6.;
const PORTS_RADIUS: f32 = 11.;
const PORTS_TEXT: f32 = 10.5;

/// A host's connection as its caption dot shows it (the web client's
/// `HostIndicator`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HostIndicator {
    Connected,
    Connecting,
    Reconnecting,
    /// Offline with an error that a retry does not resolve.
    Failed,
    Offline,
    /// No connection attempt.
    Idle,
}

impl HostIndicator {
    fn from_status(status: Option<SourceStatus>, needs_user: bool) -> Self {
        match status {
            None => Self::Idle,
            Some(SourceStatus::Ready) => Self::Connected,
            Some(SourceStatus::Connecting) => Self::Connecting,
            Some(SourceStatus::Reconnecting) => Self::Reconnecting,
            Some(SourceStatus::Offline) if needs_user => Self::Failed,
            Some(SourceStatus::Offline | SourceStatus::Closing) => Self::Offline,
        }
    }

    /// Screens of a host the window cannot reach are dimmed.
    pub(super) fn unreachable(self) -> bool {
        matches!(self, Self::Failed | Self::Offline)
    }

    /// What the caption says besides its dot; nothing while all is well.
    fn note(self) -> Option<&'static str> {
        match self {
            Self::Connecting => Some("connecting…"),
            Self::Reconnecting => Some("reconnecting…"),
            Self::Failed => Some("failed"),
            Self::Offline => Some("offline"),
            Self::Connected | Self::Idle => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Connecting => "connecting",
            Self::Reconnecting => "reconnecting",
            Self::Failed => "failed",
            Self::Offline => "offline",
            Self::Idle => "not connected",
        }
    }
}

/// The indicator for a host's current presentation. An offline host whose
/// diagnostic is not retryable counts as failed (provisional mapping: the
/// native model has no separate failed phase).
pub(super) fn host_indicator(host: Option<&HostPresentation>) -> HostIndicator {
    HostIndicator::from_status(
        host.map(HostPresentation::status),
        host.and_then(HostPresentation::diagnostic)
            .is_some_and(|diagnostic| !diagnostic.retryable()),
    )
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;

#[cfg(test)]
mod indicator_tests {
    use super::{HostIndicator, SourceStatus};

    #[test]
    fn host_states_map_to_the_web_caption_indicators() {
        for (status, needs_user, indicator, note, unreachable) in [
            (None, false, HostIndicator::Idle, None, false),
            (
                Some(SourceStatus::Ready),
                false,
                HostIndicator::Connected,
                None,
                false,
            ),
            (
                Some(SourceStatus::Connecting),
                false,
                HostIndicator::Connecting,
                Some("connecting…"),
                false,
            ),
            (
                Some(SourceStatus::Reconnecting),
                true,
                HostIndicator::Reconnecting,
                Some("reconnecting…"),
                false,
            ),
            (
                Some(SourceStatus::Offline),
                false,
                HostIndicator::Offline,
                Some("offline"),
                true,
            ),
            (
                Some(SourceStatus::Offline),
                true,
                HostIndicator::Failed,
                Some("failed"),
                true,
            ),
            (
                Some(SourceStatus::Closing),
                false,
                HostIndicator::Offline,
                Some("offline"),
                true,
            ),
        ] {
            let actual = HostIndicator::from_status(status, needs_user);
            assert_eq!(actual, indicator, "{status:?}");
            assert_eq!(actual.note(), note, "{status:?}");
            assert_eq!(actual.unreachable(), unreachable, "{status:?}");
        }
    }
}

#[derive(Clone)]
pub(super) struct DrawerPortMenu {
    id: Uuid,
    group: HostGroupTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PortChoice {
    key: MappingKey,
    local: Option<u16>,
    up: bool,
}

impl PortChoice {
    fn capture(row: &ForwardRow) -> Self {
        Self {
            key: row.key().clone(),
            local: row.local(),
            up: row.state() == ForwardState::Up,
        }
    }
}

struct PortEntry {
    choice: PortChoice,
    label: String,
    disabled: bool,
}

struct PortMenuPresentation {
    total: usize,
    entries: Vec<PortEntry>,
    status: Option<&'static str>,
}

impl Workspace {
    fn drawer_port_presentation(&self, group: &HostGroupTarget) -> PortMenuPresentation {
        let rows: Vec<_> = self
            .ports
            .rows(self.model.as_ref())
            .into_iter()
            .filter(|row| row.key().target() == group.target())
            .collect();
        let snapshot = self.ports.snapshot.as_deref();
        let active_read_failed = snapshot.is_some_and(|read| read.active_error().is_some());
        let status = match snapshot {
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
                Some("Host has not reported port discovery")
            }
            Some(_) if rows.is_empty() => Some("No ports reported"),
            Some(_) => None,
        };
        let entries = rows
            .iter()
            .take(MAX_MENU_PORTS)
            .map(|row| {
                let pending = self.ports.pending(row.key());
                let remote = row.key().remote();
                let mut label = if pending {
                    format!("Port {remote} · action pending…")
                } else if row.state() == ForwardState::Up {
                    match row.local() {
                        Some(local) if active_read_failed => {
                            format!("Open {remote} · last known local {local}")
                        }
                        Some(local) => format!("Open {remote} · localhost:{local}"),
                        None => format!("Open {remote}"),
                    }
                } else {
                    let state = match row.state() {
                        ForwardState::Detected => "detected",
                        ForwardState::Saved => "saved",
                        ForwardState::Paused => "paused",
                        ForwardState::Failed => "failed",
                        ForwardState::Rebuilding => "reconnecting",
                        ForwardState::Up => unreachable!(),
                    };
                    match row.local() {
                        Some(local) if row.is_saved() => {
                            format!("Forward and open {remote} · {state}, preferred local {local}")
                        }
                        Some(local) => {
                            format!("Forward and open {remote} · {state}, local {local}")
                        }
                        None => format!("Forward and open {remote} · {state}"),
                    }
                };
                if let Some(detail) = row
                    .label()
                    .filter(|label| !label.is_empty())
                    .or(row.process())
                {
                    label.push_str(" · ");
                    label.extend(super::hosts::display(detail).chars().take(80));
                }
                PortEntry {
                    choice: PortChoice::capture(row),
                    label,
                    disabled: pending || !self.ports.can_manage(),
                }
            })
            .collect();
        PortMenuPresentation {
            total: rows.len(),
            entries,
            status,
        }
    }

    fn drawer_port_menu_change(
        &mut self,
        group: &HostGroupTarget,
        open: bool,
        trigger: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cancelled = !open
            && self
                .drawer_port_menu
                .as_ref()
                .is_some_and(|menu| &menu.group == group);
        if open {
            self.drawer_port_menu = self
                .model
                .as_ref()
                .filter(|model| group.is_current(model))
                .filter(|_| self.drawer_host_actions_available())
                .map(|_| DrawerPortMenu {
                    id: Uuid::new_v4(),
                    group: group.clone(),
                });
        } else if self
            .drawer_port_menu
            .as_ref()
            .is_some_and(|menu| &menu.group == group)
        {
            self.drawer_port_menu = None;
        }
        // Leaving the Drawer for its popup suppresses the normal hover-close.
        // On dismissal, resume autohide if the pointer has already left it.
        if !open
            && self.drawer_port_menu.is_none()
            && self.drawer_overlay_open
            && window.mouse_position().x
                > chrome_rems(self.drawer_width as f32).to_pixels(window.rem_size())
        {
            self.close_drawer_overlay(cx);
            if self.destination == Destination::Screens {
                self.focus_current(window, cx);
            } else {
                self.focus.focus(window, cx);
            }
        } else if cancelled
            && (self.drawer_visible || self.drawer_overlay_open)
            && self
                .model
                .as_ref()
                .is_some_and(|model| group.is_current(model))
        {
            // Kit restores its dropdown wrapper, which has no Button key context.
            // Return to the actual trigger so Enter/Space can open it again.
            trigger.focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn reconcile_drawer_port_menu(&mut self, model: &WindowWorkspace) {
        if self
            .drawer_port_menu
            .as_ref()
            .is_some_and(|menu| !menu.group.is_current(model))
        {
            self.drawer_port_menu = None;
        }
    }

    pub(super) fn revoke_drawer_port_menu_for_screen(&mut self, screen: &ScreenKey) {
        if self.drawer_port_menu.as_ref().is_some_and(|menu| {
            menu.group.source_id() == screen.source_id()
                && self.model.as_ref().is_none_or(|model| {
                    !model.opened().iter().any(|entry| {
                        entry.key().source_id() == screen.source_id() && entry.key() != screen
                    })
                })
        }) {
            self.drawer_port_menu = None;
        }
    }

    fn take_drawer_port_menu(&mut self, capture: &DrawerPortMenu) -> bool {
        if self
            .drawer_port_menu
            .as_ref()
            .is_none_or(|menu| menu.id != capture.id)
            || !self
                .model
                .as_ref()
                .is_some_and(|model| capture.group.is_current(model))
            || !self.drawer_host_actions_available()
        {
            return false;
        }
        self.drawer_port_menu = None;
        true
    }

    fn choose_drawer_port(
        &mut self,
        capture: &DrawerPortMenu,
        choice: &PortChoice,
        cx: &mut Context<Self>,
    ) {
        if cx.has_active_drag() || !self.take_drawer_port_menu(capture) {
            return;
        }
        let current = self
            .ports
            .rows(self.model.as_ref())
            .into_iter()
            .find(|row| row.key() == &choice.key && row.key().target() == capture.group.target());
        if current
            .as_ref()
            .is_none_or(|row| PortChoice::capture(row) != *choice)
            || self.ports.pending(&choice.key)
        {
            self.ports.link_notice =
                Some("Port changed; open the ports menu and choose again".into());
            cx.notify();
            return;
        }
        match ForwardRequest::open(choice.key.clone(), choice.local, false, None) {
            Ok(request) => {
                self.submit_forward(request, OpenOrigin::Drawer(capture.group.clone()), cx)
            }
            Err(error) => {
                self.ports.link_notice = Some(error.to_string());
                cx.notify();
            }
        }
    }

    fn drawer_ports_menu(
        &self,
        menu: PopupMenu,
        group: &HostGroupTarget,
        window: &mut Window,
        weak: WeakEntity<Self>,
        trigger: FocusHandle,
    ) -> PopupMenu {
        let Some(capture) = self
            .drawer_port_menu
            .as_ref()
            .filter(|menu| &menu.group == group)
            .cloned()
        else {
            return menu
                .item(PopupMenuItem::new("Host group is no longer available").disabled(true));
        };
        let presentation = self.drawer_port_presentation(group);
        let mut menu = menu
            .max_w((window.rem_size() * 27.).min(window.viewport_size().width - window.rem_size()))
            .max_h(window.viewport_size().height * 0.65)
            .scrollable(true);
        if let Some(status) = presentation.status {
            menu = menu.item(PopupMenuItem::new(status).disabled(true));
        }
        for entry in presentation.entries {
            let weak = weak.clone();
            let capture = capture.clone();
            let trigger = trigger.clone();
            menu = menu.item(
                PopupMenuItem::new(entry.label)
                    .disabled(entry.disabled)
                    .on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            let current = this
                                .drawer_port_menu
                                .as_ref()
                                .is_some_and(|menu| menu.id == capture.id);
                            this.choose_drawer_port(&capture, &entry.choice, cx);
                            if current
                                && (this.drawer_visible || this.drawer_overlay_open)
                                && this
                                    .model
                                    .as_ref()
                                    .is_some_and(|model| capture.group.is_current(model))
                            {
                                trigger.focus(window, cx);
                            }
                        });
                    }),
            );
        }
        if presentation.total > MAX_MENU_PORTS {
            menu = menu.item(
                PopupMenuItem::new(format!(
                    "{} more ports in Hosts",
                    presentation.total - MAX_MENU_PORTS
                ))
                .disabled(true),
            );
        }
        menu.separator().item(
            PopupMenuItem::new("Manage ports…").on_click(move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if this.take_drawer_port_menu(&capture) {
                        this.show_host_group(&capture.group, HostSection::Ports, window, cx);
                    }
                });
            }),
        )
    }

    pub(super) fn render_drawer_host(
        &self,
        group: HostGroupTarget,
        title: String,
        (count, forwarded): (usize, usize),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let source = group.source_id();
        let indicator = host_indicator(self.model.as_ref().and_then(|model| model.host(source)));
        let muted: Hsla = rgb(self.appearance.muted).into();
        let dot = div().flex_none().size(chrome_rems(HOST_DOT)).rounded_full();
        let dot = match indicator {
            HostIndicator::Connected => dot.bg(rgb(self.appearance.success)),
            HostIndicator::Connecting | HostIndicator::Reconnecting => {
                dot.bg(rgb(self.appearance.warning))
            }
            HostIndicator::Failed => dot.bg(rgb(self.appearance.danger)),
            HostIndicator::Offline => dot.bg(muted).opacity(HOST_DOT_OFFLINE_OPACITY),
            HostIndicator::Idle => dot.border(chrome_rems(HOST_DOT_RING)).border_color(muted),
        };
        let trigger_focus = window
            .use_keyed_state(
                SharedString::from(format!("drawer-ports-focus-{source}")),
                cx,
                |_, cx| cx.focus_handle().tab_stop(true),
            )
            .read(cx)
            .clone();
        let builder_focus = trigger_focus.clone();
        let caption_group = group.clone();
        let builder_group = group.clone();
        let weak = cx.entity().downgrade();
        let menu_weak = weak.clone();
        let available = self.drawer_host_actions_available();
        let menu_open = self
            .drawer_port_menu
            .as_ref()
            .is_some_and(|menu| menu.group == group);
        let create_action = self.render_remote_create_action(&group, window, cx);
        div()
            .mx(chrome_rems(super::drawer::CARD_INSET))
            .mt(chrome_rems(HOST_MARGIN_TOP))
            .pt(chrome_rems(HOST_CAPTION_PADDING_TOP))
            .flex()
            .items_center()
            .gap(chrome_rems(HOST_CAPTION_GAP))
            .border_t_1()
            .border_color(rgb(self.appearance.subtle_border))
            .child(
                super::drawer::drawer_control(
                    SharedString::from(format!("drawer-host-{source}")),
                    rgb(self.appearance.secondary).into(),
                    super::drawer::ICON_BUTTON_RADIUS,
                    &self.appearance,
                    window,
                    cx,
                )
                .debug_selector(move || format!("drawer-host-{source}"))
                .accessibility_label(format!(
                    "Show host {}, {}",
                    super::hosts::display(group.target()),
                    indicator.label()
                ))
                .tooltip(format!(
                    "Show host {}",
                    super::hosts::display(group.target())
                ))
                .small()
                .flex_1()
                .min_w_0()
                .h(chrome_rems(HOST_NAME_HEIGHT))
                .px(chrome_rems(HOST_NAME_PADDING_X))
                .when(!available, |button| button.cursor_default())
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(chrome_rems(HOST_NAME_GAP))
                        .text_size(chrome_rems(HOST_NAME_TEXT))
                        .font_weight(FontWeight::MEDIUM)
                        .child(Icon::new(IconName::Server).size(chrome_rems(HOST_ICON)))
                        .child(
                            div()
                                .min_w_0()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(title),
                        )
                        .child(dot)
                        .when_some(indicator.note(), |caption, note| {
                            caption.child(
                                div()
                                    .flex_none()
                                    .whitespace_nowrap()
                                    .text_size(chrome_rems(super::drawer::NOTE_TEXT))
                                    .child(note),
                            )
                        }),
                )
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.show_host_group(&caption_group, HostSection::Host, window, cx);
                    });
                }),
            )
            .child(
                // The web pill counts forwarded ports, in the accent color,
                // once any is forwarded, and the known ports before.
                super::drawer::drawer_control(
                    SharedString::from(format!("drawer-ports-{source}")),
                    if forwarded > 0 {
                        rgb(self.appearance.accent).into()
                    } else {
                        muted
                    },
                    PORTS_RADIUS,
                    &self.appearance,
                    window,
                    cx,
                )
                .track_focus(&trigger_focus)
                .debug_selector(move || format!("drawer-ports-{source}"))
                .accessibility_label(if forwarded > 0 {
                    format!(
                        "Ports for {}: {count} known, {forwarded} forwarded",
                        super::hosts::display(group.target())
                    )
                } else {
                    format!(
                        "Ports for {}: {count} known",
                        super::hosts::display(group.target())
                    )
                })
                .tooltip("Open or forward a host port")
                .icon(Icon::new(IconName::Network))
                .child(
                    div()
                        .text_size(chrome_rems(PORTS_TEXT))
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("{}", if forwarded > 0 { forwarded } else { count })),
                )
                .xsmall()
                .flex_none()
                .h(chrome_rems(PORTS_HEIGHT))
                .px(chrome_rems(PORTS_PADDING_X))
                .when(!available, |button| button.cursor_default())
                .selected(menu_open)
                .disabled(!available)
                .dropdown_menu(move |menu, window, cx| {
                    if let Some(view) = menu_weak.upgrade() {
                        view.read(cx).drawer_ports_menu(
                            menu,
                            &builder_group,
                            window,
                            menu_weak.clone(),
                            builder_focus.clone(),
                        )
                    } else {
                        menu
                    }
                })
                .on_open_change({
                    let weak = cx.entity().downgrade();
                    move |open, window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.drawer_port_menu_change(&group, *open, &trigger_focus, window, cx)
                        });
                    }
                }),
            )
            .child(create_action)
            .into_any_element()
    }
}
