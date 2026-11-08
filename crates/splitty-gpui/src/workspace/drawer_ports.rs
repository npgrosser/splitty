//! Drawer commands project the existing host and forwarding owners.
use super::hosts::{HostGroupTarget, HostSection};
use super::ports::OpenOrigin;
use super::*;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use splitty_client_core::ports::{ForwardRequest, ForwardRow, ForwardState, MappingKey};

const MAX_MENU_PORTS: usize = 12;

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;

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
    fn drawer_host_actions_available(&self) -> bool {
        !self.hosts_navigation_blocked()
            && !self.hosts.has_open_form()
            && !self.ports.has_open_form()
    }

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
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let source = group.source_id();
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
        div()
            .mx_2()
            .mt_2()
            .pt_2()
            .pb_1()
            .flex()
            .items_center()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                Button::new(SharedString::from(format!("drawer-host-{source}")))
                    .debug_selector(move || format!("drawer-host-{source}"))
                    .accessibility_label(format!(
                        "Show host {}",
                        super::hosts::display(group.target())
                    ))
                    .tooltip(format!(
                        "Show host {}",
                        super::hosts::display(group.target())
                    ))
                    .ghost()
                    .compact()
                    .small()
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(chrome_rems(7.))
                            .text_size(chrome_rems(11.5))
                            .font_weight(FontWeight::NORMAL)
                            .child(Icon::new(IconName::Server).size_3())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(title),
                            ),
                    )
                    .disabled(!available)
                    .on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.show_host_group(&caption_group, HostSection::Host, window, cx);
                        });
                    }),
            )
            .child(
                Button::new(SharedString::from(format!("drawer-ports-{source}")))
                    .track_focus(&trigger_focus)
                    .debug_selector(move || format!("drawer-ports-{source}"))
                    .accessibility_label(format!(
                        "Ports for {}: {count} known",
                        super::hosts::display(group.target())
                    ))
                    .tooltip("Open or forward a host port")
                    .label(format!("{count}"))
                    .icon(Icon::new(IconName::Network).size_3())
                    .ghost()
                    .compact()
                    .small()
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
                                this.drawer_port_menu_change(
                                    &group,
                                    *open,
                                    &trigger_focus,
                                    window,
                                    cx,
                                )
                            });
                        }
                    }),
            )
            .into_any_element()
    }
}
