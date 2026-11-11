//! Drawer commands project the existing host and forwarding owners.
use super::hosts::HostGroupTarget;
use super::popover::{PopoverAnchor, PopoverKind};
use super::*;
use gpui_kit::base::Selectable as _;
use splitty_client_core::workspace::{HostPresentation, SourceStatus};

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
const HOST_NAME_TEXT: f32 = 12.5;
/// `.drawer-host-ports`: a 22 px pill, `padding: 0 6px`, radius 11, 10.5px,
/// with a 12 px icon and `gap: 4px`.
const PORTS_HEIGHT: f32 = 22.;
const PORTS_PADDING_X: f32 = 6.;
const PORTS_RADIUS: f32 = 11.;
const PORTS_TEXT: f32 = 10.5;
const PORTS_ICON: f32 = 12.;
const PORTS_GAP: f32 = 4.;

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

impl Workspace {
    /// A host group's caption: its name opens the host's Screens, the pill
    /// its ports, the plus a new Screen. All three open beside the Drawer.
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
        let available = self.drawer_host_actions_available();
        let anchor: SharedString = format!("host-{source}").into();
        let create_action = self.render_remote_create_action(&group, window, cx);
        let host_open = self.drawer_popover_host() == Some(group.target());
        let ports_open = self.ports_popover_open(&group);
        // Offered while the pointer is on the caption; the host's panel has
        // the same action for the keyboard.
        let close = (available && self.drawer_host_hovered == Some(source)).then(|| {
            let close_group = group.clone();
            super::drawer::drawer_control(
                SharedString::from(format!("drawer-host-close-{source}")),
                muted,
                super::drawer::ICON_BUTTON_RADIUS,
                &self.appearance,
                window,
                cx,
            )
            .debug_selector(move || format!("drawer-host-close-{source}"))
            .accessibility_label(format!(
                "Close {}; its Screens keep running",
                super::hosts::display(group.target())
            ))
            .tooltip("Close host: its Screens keep running")
            .child(Icon::new(IconName::Unplug).size(chrome_rems(PORTS_ICON)))
            .xsmall()
            .flex_none()
            .w(chrome_rems(PORTS_HEIGHT))
            .h(chrome_rems(PORTS_HEIGHT))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                if this.drawer_host_actions_available() {
                    this.close_host(&close_group, cx);
                }
            }))
        });
        let caption_group = group.clone();
        let caption_anchor = anchor.clone();
        let caption = self.host_drag_caption(group.clone(), cx).child(
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
                "Screens on {}, {}",
                super::hosts::display(group.target()),
                indicator.label()
            ))
            .small()
            .flex_1()
            .min_w_0()
            .h(chrome_rems(HOST_NAME_HEIGHT))
            .px(chrome_rems(HOST_NAME_PADDING_X))
            .when(!available, |button| button.cursor_default())
            .selected(host_open)
            .when(host_open, |button| {
                button.bg(rgba(self.appearance.hover_overlay))
            })
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(chrome_rems(HOST_NAME_GAP))
                    .text_size(chrome_rems(HOST_NAME_TEXT))
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.host_dot(indicator))
                    .child(
                        div()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(title),
                    )
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
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                cx.stop_propagation();
                if !this.drawer_host_actions_available()
                    || !this
                        .model
                        .as_ref()
                        .is_some_and(|model| caption_group.is_current(model))
                {
                    return;
                }
                let anchor = this
                    .drawer_anchor(&caption_anchor)
                    .map_or(PopoverAnchor::Pointer(event.position()), PopoverAnchor::Row);
                this.open_drawer_popover(
                    PopoverKind::Host(caption_group.target().to_owned()),
                    anchor,
                    None,
                    window,
                    cx,
                );
            })),
        );
        let ports_group = group.clone();
        let ports_anchor = anchor.clone();
        // Local forwarding status is independent of the Screen connection.
        let ports = super::drawer::drawer_control(
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
        .debug_selector(move || format!("drawer-ports-{source}"))
        .accessibility_label(match (count, forwarded) {
            (count, 0) => format!(
                "Ports on {}: {count} found",
                super::hosts::display(group.target())
            ),
            (count, forwarded) => format!(
                "Ports on {}: {count} found, {forwarded} forwarded",
                super::hosts::display(group.target())
            ),
        })
        .tooltip("Open a port in the browser")
        .child(
            div()
                .flex()
                .items_center()
                .gap(chrome_rems(PORTS_GAP))
                .text_size(chrome_rems(PORTS_TEXT))
                .font_weight(FontWeight::MEDIUM)
                .child(Icon::new(IconName::ArrowLeftRight).size(chrome_rems(PORTS_ICON)))
                .when(count > 0, |pill| {
                    pill.child((if forwarded > 0 { forwarded } else { count }).to_string())
                }),
        )
        .xsmall()
        .flex_none()
        .h(chrome_rems(PORTS_HEIGHT))
        .px(chrome_rems(PORTS_PADDING_X))
        .when(!available, |button| button.cursor_default())
        .selected(ports_open)
        .when(ports_open, |button| {
            button.bg(rgba(self.appearance.hover_overlay))
        })
        .disabled(!available)
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            cx.stop_propagation();
            if !this.drawer_host_actions_available()
                || !this
                    .model
                    .as_ref()
                    .is_some_and(|model| ports_group.is_current(model))
            {
                return;
            }
            let anchor = this
                .drawer_anchor(&ports_anchor)
                .map_or(PopoverAnchor::Pointer(event.position()), PopoverAnchor::Row);
            this.refresh_ports(cx);
            this.open_drawer_popover(
                PopoverKind::Ports(ports_group.clone()),
                anchor,
                None,
                window,
                cx,
            );
        }));
        div()
            .mx(chrome_rems(super::drawer::CARD_INSET))
            .mt(chrome_rems(HOST_MARGIN_TOP))
            .pt(chrome_rems(HOST_CAPTION_PADDING_TOP))
            .border_t_1()
            .border_color(rgb(self.appearance.subtle_border))
            .child(
                div()
                    .id(SharedString::from(format!("drawer-host-caption-{source}")))
                    .relative()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(chrome_rems(HOST_CAPTION_GAP))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.drawer_host_hovered = Some(source);
                        } else if this.drawer_host_hovered == Some(source) {
                            this.drawer_host_hovered = None;
                        }
                        cx.notify();
                    }))
                    .child(self.anchor_probe(anchor))
                    .child(caption)
                    .children(close)
                    .child(ports)
                    .child(create_action),
            )
            .into_any_element()
    }
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
