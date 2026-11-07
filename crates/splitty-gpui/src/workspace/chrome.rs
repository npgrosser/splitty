//! Group-local chrome. Each command carries the pane displayed in its own slot.
use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
    Disableable, Icon, Sizable,
};

pub(super) fn compact_group_actions(width: Option<Pixels>, window_scale: f32) -> bool {
    width.is_none_or(|width| f32::from(width) < 400. * window_scale)
}

#[derive(Clone, Debug)]
pub(super) struct GroupTarget {
    slot: String,
    pane: PaneActionTarget,
}

fn selected_slot_matches(
    slot: &str,
    pane: &str,
    node: &LayoutNode,
    selected: Option<&str>,
) -> bool {
    matches!(node, LayoutNode::Slot { .. }) && node.id().as_str() == slot && selected == Some(pane)
}

impl GroupTarget {
    fn capture(model: &LocalWorkspace, slot: &str, pane: &str) -> Option<Self> {
        let node = presented_slot_by_id(model, slot)?;
        if !selected_slot_matches(slot, pane, node, model.selected_pane(node)) {
            return None;
        }
        Some(Self {
            slot: slot.to_owned(),
            pane: model.capture_pane_action(pane).ok()?,
        })
    }

    fn valid_for(&self, model: &LocalWorkspace) -> bool {
        model.validate_pane_action(&self.pane).is_ok()
            && presented_slot_by_id(model, &self.slot).is_some_and(|node| {
                selected_slot_matches(
                    &self.slot,
                    &self.pane.pane_id,
                    node,
                    model.selected_pane(node),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{compact_group_actions, selected_slot_matches};
    use gpui_kit::px;
    use splitty_client_core::workspace::LayoutNode;

    #[test]
    fn group_action_rejects_a_different_slot_or_selected_tab() {
        let slot: LayoutNode = serde_json::from_value(serde_json::json!({
            "type": "slot", "id": "right", "paneIds": ["a", "b"], "activePaneId": "b"
        }))
        .expect("slot");
        assert!(selected_slot_matches("right", "b", &slot, Some("b")));
        assert!(!selected_slot_matches("left", "b", &slot, Some("b")));
        assert!(!selected_slot_matches("right", "a", &slot, Some("b")));
        assert!(!selected_slot_matches("right", "b", &slot, None));
    }

    #[test]
    fn group_actions_leave_room_for_tabs_at_window_zoom() {
        assert!(compact_group_actions(None, 1.));
        assert!(compact_group_actions(Some(px(399.)), 1.));
        assert!(!compact_group_actions(Some(px(400.)), 1.));
        assert!(compact_group_actions(Some(px(500.)), 1.5));
        assert!(!compact_group_actions(Some(px(800.)), 1.5));
    }
}

#[derive(Clone)]
pub(super) enum GroupCommand {
    NewTab,
    Split(Axis),
    Float,
    Maximize,
}

impl Workspace {
    fn cycle_screen_button(&mut self, step: TabStep, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename.is_some()
            || GlobalState::is_in_deferred_context(cx)
            || self.template_dialog.is_some()
            || self.profile_picker.is_some()
            || self.transfer_menu.is_some()
            || self.pending_template_apply.is_some()
            || self.pending_close.is_some()
            || self.drag.is_some()
            || self.float_gesture.is_some()
        {
            return;
        }
        let Some(connection) = self.model.as_ref().map(LocalWorkspace::connection_id) else {
            return;
        };
        self.send(Command::CycleScreen { connection, step }, cx);
        self.focus_current(window, cx);
    }

    pub(super) fn render_screen_navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        let selection = self.model.as_ref().and_then(|model| {
            let shown = model.shown_screen_id()?;
            let index = model
                .screens()
                .iter()
                .position(|screen| screen.id.as_str() == shown)?;
            let name = model.screens()[index]
                .name
                .clone()
                .unwrap_or_else(|| format!("Screen {}", index + 1));
            Some((index, model.screens().len(), name))
        });
        let (index, count, name) = selection.unwrap_or((0, 0, "No Screen".into()));
        let blocked = self.rename.is_some()
            || self.template_dialog.is_some()
            || self.profile_picker.is_some()
            || self.transfer_menu.is_some()
            || self.pending_template_apply.is_some()
            || self.pending_close.is_some();
        let previous = cx.entity().downgrade();
        let next = cx.entity().downgrade();
        div()
            .id("screen-navigation")
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                this.chrome_wheel(WheelTarget::Screens, event, window, cx);
            }))
            .child(
                Button::new("previous-screen")
                    .accessibility_label("Previous Screen")
                    .tooltip("Previous Screen")
                    .icon(Icon::new(IconName::ChevronLeft))
                    .ghost()
                    .compact()
                    .small()
                    .disabled(blocked || count == 0 || index == 0)
                    .on_click(move |_, window, cx| {
                        let _ = previous.update(cx, |this, cx| {
                            this.cycle_screen_button(TabStep::Previous, window, cx)
                        });
                    }),
            )
            .child(
                Button::new("current-screen")
                    .accessibility_label(format!("Current Screen: {name}"))
                    .tooltip(format!(
                        "{name} · Scroll to switch Screens; click to toggle the Screen list"
                    ))
                    .label(if count == 0 {
                        name
                    } else {
                        format!("{name}  ·  {} / {count}", index + 1)
                    })
                    .ghost()
                    .compact()
                    .small()
                    .max_w(chrome_rems(230.))
                    .overflow_hidden()
                    .disabled(self.drawer_pending.is_some())
                    .on_click({
                        let weak = cx.entity().downgrade();
                        move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.toggle_drawer(cx);
                                this.focus_current(window, cx);
                            });
                        }
                    }),
            )
            .child(
                Button::new("next-screen")
                    .accessibility_label("Next Screen")
                    .tooltip("Next Screen")
                    .icon(Icon::new(IconName::ChevronRight))
                    .ghost()
                    .compact()
                    .small()
                    .disabled(blocked || count == 0 || index + 1 >= count)
                    .on_click(move |_, window, cx| {
                        let _ = next.update(cx, |this, cx| {
                            this.cycle_screen_button(TabStep::Next, window, cx)
                        });
                    }),
            )
            .into_any_element()
    }

    fn group_command(
        &mut self,
        target: &GroupTarget,
        action: GroupCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self.model.as_ref() else {
            return;
        };
        if !target.valid_for(model) {
            self.status = "Group changed; action was not sent".into();
            cx.notify();
            return;
        }
        match action {
            GroupCommand::NewTab => {
                self.send(Command::Tab(target.pane.pane_id.clone()), cx);
            }
            GroupCommand::Split(axis) => {
                if !model.is_root_pane(&target.pane.pane_id) {
                    return;
                }
                self.send(
                    Command::Split {
                        pane: target.pane.pane_id.clone(),
                        axis,
                    },
                    cx,
                );
            }
            GroupCommand::Float => {
                if !model.can_float_pane(&target.pane) {
                    return;
                }
                let request_id = Uuid::new_v4();
                self.submit_float_action(
                    request_id,
                    Command::FloatPane {
                        request_id,
                        source: target.pane.clone(),
                    },
                    true,
                    cx,
                );
            }
            GroupCommand::Maximize => {
                let restoring = model.maximized_slot_id() == Some(target.slot.as_str());
                let (slot, pane) = if restoring {
                    (None, None)
                } else {
                    (Some(target.slot.clone()), Some(target.pane.pane_id.clone()))
                };
                match self.commands.try_send(Command::SetMaximized {
                    screen: target.pane.screen_id.clone(),
                    connection: target.pane.connection_id,
                    slot,
                    pane,
                }) {
                    Ok(()) => {}
                    Err(error) => {
                        self.status = format!("Action not sent: {error}");
                        cx.notify();
                    }
                }
            }
        }
        self.focus_current(window, cx);
    }

    fn group_button(
        weak: WeakEntity<Self>,
        id: String,
        label: &'static str,
        icon: IconName,
        disabled: bool,
        target: Option<GroupTarget>,
        action: GroupCommand,
    ) -> AnyElement {
        Button::new(SharedString::from(id))
            .accessibility_label(label)
            .tooltip(label)
            .icon(Icon::new(icon))
            .ghost()
            .compact()
            .small()
            .disabled(disabled)
            .on_click(move |_, window, cx| {
                if let Some(target) = &target {
                    let _ = weak.update(cx, |this, cx| {
                        this.group_command(target, action.clone(), window, cx)
                    });
                }
            })
            .into_any_element()
    }

    pub(super) fn render_group_actions(
        &self,
        slot: &str,
        selected: Option<&str>,
        surface: Surface,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = self
            .model
            .as_ref()
            .and_then(|model| GroupTarget::capture(model, slot, selected?));
        let can_float = target.as_ref().is_some_and(|target| {
            self.model
                .as_ref()
                .is_some_and(|model| model.can_float_pane(&target.pane))
        });
        let maximized = self
            .model
            .as_ref()
            .is_some_and(|model| model.maximized_slot_id() == Some(slot));
        let tiled = surface == Surface::Tiled;
        let compact = compact_group_actions(
            self.slot_bounds.get(slot).map(|bounds| bounds.size.width),
            self.appearance.zoom.window_scale(),
        );
        let weak = cx.entity().downgrade();
        let mut actions = div()
            .id(SharedString::from(format!("group-actions-{slot}")))
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .px_1()
            .child(Self::group_button(
                weak.clone(),
                format!("group-new-tab-{slot}"),
                "New tab in this group",
                IconName::Plus,
                target.is_none(),
                target.clone(),
                GroupCommand::NewTab,
            ));
        let picker_slot = slot.to_owned();
        let picker_target = target.clone();
        actions = actions.child(
            Button::new(SharedString::from(format!("group-profile-{slot}")))
                .accessibility_label("Choose shell profile for this group")
                .tooltip("Choose shell profile")
                .icon(Icon::new(IconName::ChevronDown))
                .ghost()
                .compact()
                .small()
                .disabled(target.is_none())
                .on_click(move |_, window, cx| {
                    if let Some(target) = &picker_target {
                        let _ = weak.update(cx, |this, cx| {
                            if this
                                .model
                                .as_ref()
                                .is_some_and(|model| target.valid_for(model))
                            {
                                this.toggle_profile_picker(
                                    &picker_slot,
                                    &target.pane.pane_id,
                                    window,
                                    cx,
                                );
                            }
                        });
                    }
                }),
        );
        if tiled {
            if compact {
                let menu_target = target.clone();
                let menu_weak = cx.entity().downgrade();
                let focus_weak = cx.entity().downgrade();
                return actions
                    .child(
                        Button::new(SharedString::from(format!("group-more-{slot}")))
                            .accessibility_label("More group actions")
                            .tooltip("Split, float or maximize this group")
                            .icon(Icon::new(IconName::Ellipsis))
                            .ghost()
                            .compact()
                            .small()
                            .dropdown_menu(move |mut menu, _, _| {
                                for (label, icon, disabled, command) in [
                                    (
                                        "Split right",
                                        IconName::SquareSplitHorizontal,
                                        menu_target.is_none(),
                                        GroupCommand::Split(Axis::Horizontal),
                                    ),
                                    (
                                        "Split below",
                                        IconName::SquareSplitVertical,
                                        menu_target.is_none(),
                                        GroupCommand::Split(Axis::Vertical),
                                    ),
                                    (
                                        "Float tab",
                                        IconName::PanelsTopLeft,
                                        !can_float,
                                        GroupCommand::Float,
                                    ),
                                    (
                                        if maximized {
                                            "Restore group"
                                        } else {
                                            "Maximize group"
                                        },
                                        if maximized {
                                            IconName::Minimize
                                        } else {
                                            IconName::Maximize
                                        },
                                        menu_target.is_none(),
                                        GroupCommand::Maximize,
                                    ),
                                ] {
                                    let target = menu_target.clone();
                                    let weak = menu_weak.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(label)
                                            .icon(Icon::new(icon))
                                            .disabled(disabled)
                                            .on_click(move |_, window, cx| {
                                                if let Some(target) = &target {
                                                    let _ = weak.update(cx, |this, cx| {
                                                        this.group_command(
                                                            target,
                                                            command.clone(),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                }
                                            }),
                                    );
                                }
                                menu
                            })
                            .on_open_change(move |open, window, cx| {
                                let _ = focus_weak.update(cx, |this, cx| {
                                    this.wheel_navigation.reset();
                                    if *open {
                                        this.drop_target = None;
                                    } else {
                                        this.focus_current(window, cx);
                                    }
                                    cx.notify();
                                });
                            }),
                    )
                    .into_any_element();
            }
            for (suffix, label, icon, action) in [
                (
                    "split-right",
                    "Split this group right",
                    IconName::SquareSplitHorizontal,
                    GroupCommand::Split(Axis::Horizontal),
                ),
                (
                    "split-below",
                    "Split this group below",
                    IconName::SquareSplitVertical,
                    GroupCommand::Split(Axis::Vertical),
                ),
            ] {
                actions = actions.child(Self::group_button(
                    cx.entity().downgrade(),
                    format!("group-{suffix}-{slot}"),
                    label,
                    icon,
                    target.is_none(),
                    target.clone(),
                    action,
                ));
            }
            actions = actions.child(Self::group_button(
                cx.entity().downgrade(),
                format!("group-float-{slot}"),
                "Float this tab",
                IconName::PanelsTopLeft,
                !can_float,
                target.clone(),
                GroupCommand::Float,
            ));
            actions = actions.child(Self::group_button(
                cx.entity().downgrade(),
                format!("group-maximize-{slot}"),
                if maximized {
                    "Restore this group"
                } else {
                    "Maximize this group"
                },
                if maximized {
                    IconName::Minimize
                } else {
                    IconName::Maximize
                },
                target.is_none(),
                target,
                GroupCommand::Maximize,
            ));
        }
        actions.into_any_element()
    }
}
