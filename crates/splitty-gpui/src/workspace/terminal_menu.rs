//! Body context actions retain the clicked terminal and use existing command owners.
use super::*;
use crate::terminal::{ContextMenuRequest, LocalAction};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;

#[derive(Clone)]
struct Target {
    id: Uuid,
    screen: ScreenKey,
    authority: Option<PaneActionTarget>,
    route: TerminalToken,
    terminal: Entity<Terminal>,
    request: ContextMenuRequest,
    group: Option<chrome::GroupTarget>,
}

pub(super) struct TerminalMenu {
    target: Target,
    popup: Entity<PopupMenu>,
    _dismiss: Subscription,
}

#[derive(Clone)]
enum Action {
    Local(LocalAction),
    Split(Axis),
    ShowDrawer,
    Close,
}

impl Workspace {
    fn terminal_menu_target_is_current(&self, target: &Target, cx: &App) -> bool {
        self.destination == Destination::Screens
            && self.model.as_ref().is_some_and(|model| {
                model
                    .validate_terminal_focus(&target.screen, &target.route)
                    .is_ok()
            })
            && self
                .panes
                .get(&target.route.pane_key())
                .is_some_and(|view| view.route == target.route && view.terminal == target.terminal)
            && target
                .terminal
                .read(cx)
                .context_menu_is_current(&target.request)
    }

    pub(super) fn open_terminal_menu(
        &mut self,
        route: TerminalToken,
        terminal: Entity<Terminal>,
        request: ContextMenuRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if GlobalState::is_in_deferred_context(cx)
            || self.rename.is_some()
            || self.pending_close.is_some()
            || self.template_dialog.is_some()
            || self.profile_picker.is_some()
            || self.tab_context_menu.is_some()
            || self.drag.is_some()
            || self.float_gesture.is_some()
            || cx.has_active_drag()
        {
            return;
        }
        let Some(model) = self.selected_model() else {
            return;
        };
        let Some(screen) = model.shown_screen_id().map(|id| model.screen_key(id)) else {
            return;
        };
        let authority = model.capture_pane_action(route.pane_id()).ok();
        let mut target = Target {
            id: Uuid::new_v4(),
            screen,
            authority,
            route,
            terminal,
            request,
            group: None,
        };
        if !self.terminal_menu_target_is_current(&target, cx) {
            return;
        }
        self.focus_captured(
            target.route.source_token(),
            &target.screen,
            target.route.pane_id(),
            cx,
        );
        let model = self.selected_model().expect("captured current source");
        if model.focused_pane() != Some(target.route.pane_id()) {
            self.focus_current(window, cx);
            return;
        }
        target.group = chrome::GroupTarget::capture_pane(model, target.route.pane_id());
        let live = self.terminal_menu_authority_is_current(&target);
        let split = live && model.is_root_pane(target.route.pane_id()) && target.group.is_some();
        let copy = target
            .terminal
            .update(cx, |terminal, _| terminal.has_selection());
        let paste = live
            && cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .is_some();
        let show_drawer = !self.drawer_visible && !self.drawer_overlay_open;
        let weak = cx.entity().downgrade();
        let captured = target.clone();
        let popup = PopupMenu::build(window, cx, move |mut menu, _, _| {
            let entries = [
                ("Copy", Action::Local(LocalAction::Copy), copy),
                ("Paste", Action::Local(LocalAction::Paste), paste),
                ("Find", Action::Local(LocalAction::Find), true),
                ("Split Right", Action::Split(Axis::Horizontal), split),
                ("Split Below", Action::Split(Axis::Vertical), split),
            ];
            for (ix, (label, action, enabled)) in entries.into_iter().enumerate() {
                if ix == 3 {
                    menu = menu.separator();
                }
                menu = menu.item(Self::terminal_menu_item(
                    &weak, &captured, label, action, enabled,
                ));
            }
            if show_drawer {
                menu = menu.separator().item(Self::terminal_menu_item(
                    &weak,
                    &captured,
                    "Show Drawer",
                    Action::ShowDrawer,
                    true,
                ));
            }
            menu.separator().item(Self::terminal_menu_item(
                &weak,
                &captured,
                "Close Pane",
                Action::Close,
                live,
            ))
        });
        let id = target.id;
        let dismiss = cx.subscribe_in(
            &popup,
            window,
            move |this, _, _: &DismissEvent, window, cx| {
                this.dismiss_terminal_menu(id, window, cx);
            },
        );
        popup.focus_handle(cx).focus(window, cx);
        self.terminal_menu = Some(TerminalMenu {
            target,
            popup,
            _dismiss: dismiss,
        });
        cx.notify();
    }

    fn terminal_menu_item(
        weak: &WeakEntity<Self>,
        target: &Target,
        label: &'static str,
        action: Action,
        enabled: bool,
    ) -> PopupMenuItem {
        let weak = weak.clone();
        let target = target.clone();
        PopupMenuItem::new(label)
            .disabled(!enabled)
            .on_click(move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.choose_terminal_menu(&target, action.clone(), window, cx);
                });
            })
    }

    fn focus_terminal_menu_target(
        &mut self,
        target: &Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.focus_captured(
            target.route.source_token(),
            &target.screen,
            target.route.pane_id(),
            cx,
        );
        if !self
            .selected_model()
            .is_some_and(|model| model.focused_pane() == Some(target.route.pane_id()))
        {
            return false;
        }
        target
            .terminal
            .update(cx, |terminal, cx| terminal.focus_pty(window, cx));
        true
    }

    fn terminal_menu_authority_is_current(&self, target: &Target) -> bool {
        target.authority.as_ref().is_some_and(|authority| {
            self.execution_token(authority.source_token().source_id())
                == Some(authority.source_token())
                && self
                    .selected_model()
                    .is_some_and(|model| model.validate_pane_action(authority).is_ok())
        })
    }

    fn choose_terminal_menu(
        &mut self,
        target: &Target,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .terminal_menu
            .as_ref()
            .is_some_and(|menu| menu.target.id == target.id)
        {
            return;
        }
        self.terminal_menu = None;
        if !self.terminal_menu_target_is_current(target, cx) {
            cx.notify();
            return;
        }
        let needs_live = matches!(
            action,
            Action::Local(LocalAction::Paste) | Action::Split(_) | Action::Close
        );
        if (needs_live && !self.terminal_menu_authority_is_current(target))
            || !self.focus_terminal_menu_target(target, window, cx)
        {
            cx.notify();
            return;
        }
        match action {
            Action::Local(action) => target.terminal.update(cx, |terminal, cx| {
                terminal.local_action(action, window, cx);
            }),
            Action::Split(axis) => {
                if let Some(group) = &target.group {
                    self.group_command(group, chrome::GroupCommand::Split(axis), window, cx);
                }
            }
            Action::ShowDrawer => {
                if !self.drawer_visible && !self.drawer_overlay_open {
                    self.toggle_drawer_overlay(cx);
                }
            }
            Action::Close => self.request_close_captured(
                target.route.source_token(),
                CloseTarget::Pane {
                    screen: target.screen.screen_id().to_owned(),
                    pane: target.route.pane_id().to_owned(),
                },
                window,
                cx,
            ),
        }
        cx.notify();
    }

    fn dismiss_terminal_menu(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .terminal_menu
            .as_ref()
            .is_some_and(|menu| menu.target.id == id)
        {
            return;
        }
        let menu = self.terminal_menu.take().expect("matched menu");
        if menu.popup.focus_handle(cx).contains_focused(window, cx)
            && self.terminal_menu_target_is_current(&menu.target, cx)
        {
            self.focus_terminal_menu_target(&menu.target, window, cx);
        }
        cx.notify();
    }

    pub(super) fn reconcile_terminal_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .terminal_menu
            .as_ref()
            .is_some_and(|menu| !self.terminal_menu_target_is_current(&menu.target, cx))
        {
            let focused = self
                .terminal_menu
                .as_ref()
                .is_some_and(|menu| menu.popup.focus_handle(cx).contains_focused(window, cx));
            self.terminal_menu = None;
            if focused {
                window.blur(cx);
            }
        }
    }

    pub(super) fn render_terminal_menu(&self, window: &Window) -> Option<AnyElement> {
        let menu = self.terminal_menu.as_ref()?;
        Some(
            deferred(
                anchored().child(
                    div()
                        .w(window.viewport_size().width)
                        .h(window.viewport_size().height)
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .child(
                            anchored()
                                .position(menu.target.request.position())
                                .snap_to_window_with_margin(rems(0.5).to_pixels(window.rem_size()))
                                .anchor(Anchor::TopLeft)
                                .child(menu.popup.clone()),
                        ),
                ),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }

    pub(super) fn cancel_terminal_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &self.terminal_menu {
            self.dismiss_terminal_menu(menu.target.id, window, cx);
        }
    }
}
