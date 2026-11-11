//! Group-local chrome. Each command carries the pane displayed in its own slot.
use super::popover::{MenuEntry, PopoverAnchor, PopoverKind};
use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::base::{FocusableExt as _, Selectable as _};
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    Disableable, Icon, Sizable,
};

pub(super) fn compact_group_actions(width: Option<Pixels>, window_scale: f32) -> bool {
    width.is_none_or(|width| f32::from(width) < 400. * window_scale)
}

pub(super) fn maximized_chrome_copy(compact: bool) -> (&'static str, &'static str) {
    if compact {
        ("Maximized", "Restore")
    } else {
        ("Group maximized", "Restore layout")
    }
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
    pub(super) fn capture_pane(model: &SourceWorkspace, pane: &str) -> Option<Self> {
        fn slot<'a>(model: &SourceWorkspace, node: &'a LayoutNode, pane: &str) -> Option<&'a str> {
            match node {
                LayoutNode::Slot { id, .. } => {
                    (model.selected_pane(node) == Some(pane)).then_some(id.as_str())
                }
                LayoutNode::Split { children, .. } => {
                    children.iter().find_map(|child| slot(model, child, pane))
                }
            }
        }
        let id = model
            .presentation_root()
            .and_then(|root| slot(model, root, pane))
            .or_else(|| {
                model
                    .presented_floats()
                    .iter()
                    .find_map(|entry| slot(model, &entry.root, pane))
            })?;
        Self::capture(model, id, pane)
    }

    fn capture(model: &SourceWorkspace, slot: &str, pane: &str) -> Option<Self> {
        let node = presented_slot_by_id(model, slot)?;
        if !selected_slot_matches(slot, pane, node, model.selected_pane(node)) {
            return None;
        }
        Some(Self {
            slot: slot.to_owned(),
            pane: model.capture_pane_action(pane).ok()?,
        })
    }

    fn valid_for(&self, model: &SourceWorkspace) -> bool {
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
    use super::{compact_group_actions, maximized_chrome_copy, selected_slot_matches};
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

    #[test]
    fn maximized_copy_keeps_the_action_visible_in_compact_chrome() {
        assert_eq!(maximized_chrome_copy(true), ("Maximized", "Restore"));
        assert_eq!(
            maximized_chrome_copy(false),
            ("Group maximized", "Restore layout")
        );
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) mod view_tests {
    use super::{Command, Destination, GroupTarget, Workspace};
    use gpui_kit::{
        px, size, AppContext, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
        TestAppContext, VisualTestContext,
    };
    use splitty_client_core::{native_workspace::Worker, workspace::WindowWorkspace};
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{atomic::AtomicBool, Arc},
    };
    use tokio::sync::mpsc;
    use uuid::Uuid;

    pub(in crate::workspace) fn with_view(
        run: impl FnOnce(&mut VisualTestContext, Entity<Workspace>),
    ) {
        let mut app = TestAppContext::single();
        app.update(gpui_kit::init);
        let content = Rc::new(RefCell::new(None));
        let (_, window) = app.add_window_view({
            let content = content.clone();
            move |window, cx| {
                let view = cx.new(|cx| {
                    let (workspace, worker) = Workspace::new_with_worker(
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
        let view = content.borrow_mut().take().unwrap();
        run(window, view);
    }

    fn click(window: &mut VisualTestContext, selector: &'static str) {
        let center = window.debug_bounds(selector).expect(selector).center();
        window.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: center,
            click_count: 1,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        window.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: center,
            click_count: 1,
            modifiers: Modifiers::default(),
        });
    }

    #[test]
    fn maximized_group_shows_labeled_restore_and_restores_canonical_slots() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let mut model = WindowWorkspace::new(owner);
            model.ui_test_local_screen_fixture("Work").unwrap();
            let source = model.lifecycle_token();
            model
                .source_mut(source.source_id())
                .unwrap()
                .apply_snapshot(
                    owner,
                    &serde_json::json!({"type":"mux.screen_snapshot","screen":{
                        "id":"work","revision":2,
                        "root":{"type":"split","id":"split","axis":"horizontal",
                            "sizes":[0.5,0.5],"children":[
                                {"type":"slot","id":"left","paneIds":["p0"],"activePaneId":"p0"},
                                {"type":"slot","id":"right","paneIds":["p1"],"activePaneId":"p1"}
                            ]},
                        "panes":{"p0":{"id":"p0","sessionId":"s0"},
                            "p1":{"id":"p1","sessionId":"s1"}},
                        "sessions":{"s0":{"id":"s0","status":"running"},
                            "s1":{"id":"s1","status":"running"}}
                    }}),
                )
                .unwrap();
            let (commands, mut receiver) = mpsc::channel(32);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.model = Some(model);
                    workspace.commands = commands;
                    workspace.destination = Destination::Screens;
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            window.simulate_resize(size(px(560.), px(420.)));
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let model = workspace.model.as_mut().unwrap();
                    assert!(model
                        .source_mut(source.source_id())
                        .unwrap()
                        .set_maximized_slot(Some("left"), Some("p0")));
                    cx.notify();
                });
                window.draw(cx).clear(cx);
                window.draw(cx).clear(cx);
            });
            assert!(window.debug_bounds("group-maximized-state").is_some());
            let restore = window
                .debug_bounds("group-restore-button")
                .expect("visible compact restore control");
            let viewport = window
                .debug_bounds("workspace-viewport")
                .expect("terminal viewport");
            assert!(
                f32::from(viewport.size.width) < 400.,
                "compact viewport width: {}",
                f32::from(viewport.size.width)
            );
            assert!(f32::from(restore.origin.x) >= f32::from(viewport.origin.x) - 1.);
            assert!(
                f32::from(restore.origin.x + restore.size.width)
                    <= f32::from(viewport.origin.x + viewport.size.width) + 1.,
                "Restore remains inside the compact viewport"
            );
            window.update(|_, cx| {
                let workspace = view.read(cx);
                let source = workspace.model.as_ref().unwrap().local_source();
                assert_eq!(source.presentation_root().unwrap().id().as_str(), "left");
                assert_eq!(source.snapshot().unwrap().root.id().as_str(), "split");
                assert!(GroupTarget::capture(source, "left", "p0").is_some());
                assert_eq!(
                    workspace.execution_token(source.source_id()),
                    Some(source.source_token())
                );
            });
            click(window, "group-restore-button");
            let sent = loop {
                let sent = receiver.try_recv().expect("restore sent");
                if matches!(sent.kind(), Command::SetMaximized { .. }) {
                    break sent;
                }
            };
            assert!(matches!(
                sent.kind(),
                Command::SetMaximized {
                    slot: None,
                    pane: None,
                    ..
                }
            ));
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let model = workspace.model.as_mut().unwrap();
                    assert!(model
                        .source_mut(source.source_id())
                        .unwrap()
                        .set_maximized_slot(None, None));
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            assert!(window.debug_bounds("group-maximized-state").is_none());
            window.update(|_, cx| {
                let workspace = view.read(cx);
                let source = workspace.model.as_ref().unwrap().local_source();
                assert_eq!(source.presentation_root().unwrap().id().as_str(), "split");
            });
        });
    }
}

#[derive(Clone)]
pub(super) enum GroupCommand {
    NewTab,
    Scratchpad,
    Split(Axis),
    Float,
    Maximize,
}

impl Workspace {
    pub(super) fn group_command(
        &mut self,
        target: &GroupTarget,
        action: GroupCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self.selected_model() else {
            return;
        };
        if !target.valid_for(model) {
            self.status = "Group changed; action was not sent".into();
            cx.notify();
            return;
        }
        match action {
            GroupCommand::NewTab => {
                self.send_captured(
                    target.pane.source_token(),
                    Command::Tab {
                        screen: model.screen_key(&target.pane.screen_id),
                        pane: target.pane.pane_id.clone(),
                    },
                    cx,
                );
            }
            GroupCommand::Scratchpad => {
                self.toggle_scratchpad(&target.pane.pane_id, window, cx);
                return;
            }
            GroupCommand::Split(axis) => {
                if !model.is_root_pane(&target.pane.pane_id) {
                    return;
                }
                self.send_captured(
                    target.pane.source_token(),
                    Command::Split {
                        screen: model.screen_key(&target.pane.screen_id),
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
                    target.pane.source_token(),
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
                match self.commands.try_send(WorkerCommand::new(
                    target.pane.source_token(),
                    Command::SetMaximized {
                        screen: target.pane.screen_id.clone(),
                        connection: target.pane.connection_id,
                        slot,
                        pane,
                    },
                )) {
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
        (id, label, icon): (String, &'static str, IconName),
        disabled: bool,
        target: Option<GroupTarget>,
        action: GroupCommand,
        focus_ring: bool,
    ) -> AnyElement {
        let selector = id.clone();
        Button::new(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .accessibility_label(label)
            .tooltip(label)
            .icon(Icon::new(icon))
            .ghost()
            .compact()
            .xsmall()
            .focus_ring(focus_ring)
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

    pub(super) fn render_group_new_tab_actions(
        &self,
        slot: &str,
        selected: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let remote = self.is_remote_selected();
        let target = self
            .selected_model()
            .and_then(|model| GroupTarget::capture(model, slot, selected?))
            .filter(|target| {
                self.execution_token(target.pane.source_token().source_id())
                    == Some(target.pane.source_token())
            });
        let weak = cx.entity().downgrade();
        let mut actions = div()
            .id(SharedString::from(format!("group-new-tab-actions-{slot}")))
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .px_1()
            .child(Self::group_button(
                weak.clone(),
                (
                    format!("group-new-tab-{slot}"),
                    "New tab in this group",
                    IconName::Plus,
                ),
                target.is_none(),
                target.clone(),
                GroupCommand::NewTab,
                focus_visible(cx),
            ));
        if !remote {
            let trigger_selector = format!("shell-profile-trigger-{slot}");
            let trigger_focus = window
                .use_keyed_state(
                    SharedString::from(format!("group-profile-trigger-focus-{slot}")),
                    cx,
                    |_, cx| cx.focus_handle().tab_stop(true),
                )
                .read(cx)
                .clone();
            let button = Button::new(SharedString::from(format!("group-profile-{slot}")))
                .focus_ring(focus_visible(cx))
                .debug_selector(move || trigger_selector.clone())
                .track_focus(&trigger_focus)
                .accessibility_label("Choose shell profile for this group")
                .tooltip("Choose shell profile")
                .icon(Icon::new(IconName::ChevronDown))
                .ghost()
                .compact()
                .xsmall()
                .disabled(target.is_none());
            if let Some(picker_target) = target {
                let picker_slot = slot.to_owned();
                let close_focus = trigger_focus.clone();
                let open = self.profile_picker_open_for(slot);
                let content = self.profile_picker_content(slot, window, cx);
                actions = actions.child(
                    Popover::new(SharedString::from(format!("group-profile-popover-{slot}")))
                        .anchor(Anchor::TopLeft)
                        .open(open)
                        .trigger(button)
                        .on_open_change(move |open, window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                if *open {
                                    if this
                                        .selected_model()
                                        .is_some_and(|model| picker_target.valid_for(model))
                                    {
                                        this.toggle_profile_picker(
                                            &picker_slot,
                                            &picker_target.pane.pane_id,
                                            window,
                                            cx,
                                        );
                                    }
                                } else {
                                    if this.dismiss_profile_picker(
                                        &picker_slot,
                                        &picker_target.pane,
                                        cx,
                                    ) {
                                        // Only the keyboard returns to the
                                        // trigger; a click goes back to work.
                                        if window.last_input_was_keyboard() {
                                            close_focus.focus(window, cx);
                                        } else {
                                            this.focus_current(window, cx);
                                        }
                                    }
                                }
                            });
                        })
                        .content(move |_, _, _| {
                            content.unwrap_or_else(|| div().into_any_element())
                        }),
                );
            } else {
                actions = actions.child(button);
            }
        }
        actions.into_any_element()
    }

    pub(super) fn render_group_actions(
        &self,
        slot: &str,
        selected: Option<&str>,
        surface: Surface,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = self
            .selected_model()
            .and_then(|model| GroupTarget::capture(model, slot, selected?))
            .filter(|target| {
                self.execution_token(target.pane.source_token().source_id())
                    == Some(target.pane.source_token())
            });
        let can_float = target.as_ref().is_some_and(|target| {
            self.selected_model()
                .is_some_and(|model| model.can_float_pane(&target.pane))
        });
        let maximized = self
            .selected_model()
            .is_some_and(|model| model.maximized_slot_id() == Some(slot));
        let tiled = surface == Surface::Tiled;
        let compact = compact_group_actions(
            self.current_layout_key(slot)
                .and_then(|key| self.slot_bounds.get(&key))
                .map(|bounds| bounds.size.width),
            self.appearance.zoom.window_scale(),
        );
        // Like the web client, group actions show while the pointer is over
        // their tab bar; a maximized group keeps its restore control visible.
        let mut actions = div()
            .id(SharedString::from(format!("group-actions-{slot}")))
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .px_1()
            .when(!maximized, |actions| {
                actions
                    .opacity(0.)
                    .group_hover(screen_motion::TAB_BAR_GROUP, |style| style.opacity(1.))
            });
        if tiled {
            if compact {
                let menu_target = target.clone();
                actions = actions.child(
                    Button::new(SharedString::from(format!("group-more-{slot}")))
                        .accessibility_label("More group actions")
                        .tooltip(if maximized {
                            "Split or float this group"
                        } else {
                            "Split, float or maximize this group"
                        })
                        .icon(Icon::new(IconName::Ellipsis))
                        .ghost()
                        .compact()
                        .xsmall()
                        .focus_ring(focus_visible(cx))
                        .selected(self.drawer_popover_is("group-more"))
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            let Some(target) = menu_target.clone() else {
                                return;
                            };
                            let mut entries = Vec::new();
                            for (key, label, icon, enabled, command) in [
                                (
                                    "scratchpad",
                                    "Scratchpad",
                                    IconName::FileText,
                                    true,
                                    GroupCommand::Scratchpad,
                                ),
                                (
                                    "split-right",
                                    "Split Right",
                                    IconName::SquareSplitHorizontal,
                                    true,
                                    GroupCommand::Split(Axis::Horizontal),
                                ),
                                (
                                    "split-below",
                                    "Split Below",
                                    IconName::SquareSplitVertical,
                                    true,
                                    GroupCommand::Split(Axis::Vertical),
                                ),
                                (
                                    "float",
                                    "Float Tab",
                                    IconName::PanelsTopLeft,
                                    can_float,
                                    GroupCommand::Float,
                                ),
                                (
                                    "maximize",
                                    "Maximize Group",
                                    IconName::Maximize,
                                    true,
                                    GroupCommand::Maximize,
                                ),
                            ] {
                                let shown = match command {
                                    GroupCommand::Split(_) => this.tab_prefs.show_split,
                                    GroupCommand::Float => this.tab_prefs.show_float,
                                    GroupCommand::Maximize => {
                                        !maximized && this.tab_prefs.show_maximize
                                    }
                                    _ => true,
                                };
                                if !shown {
                                    continue;
                                }
                                let target = target.clone();
                                entries.push(
                                    MenuEntry::new(key, label, move |this, window, cx| {
                                        this.group_command(&target, command.clone(), window, cx)
                                    })
                                    .icon(icon)
                                    .enabled(enabled),
                                );
                            }
                            this.wheel_navigation.reset();
                            this.drop_target = None;
                            this.open_drawer_popover(
                                PopoverKind::Actions(entries),
                                PopoverAnchor::Pointer(event.position()),
                                None,
                                window,
                                cx,
                            );
                            this.tag_drawer_popover("group-more");
                        })),
                );
            } else {
                for (suffix, label, icon, action) in [
                    (
                        "scratchpad",
                        "Toggle scratchpad",
                        IconName::FileText,
                        GroupCommand::Scratchpad,
                    ),
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
                    if matches!(action, GroupCommand::Split(_)) && !self.tab_prefs.show_split {
                        continue;
                    }
                    actions = actions.child(Self::group_button(
                        cx.entity().downgrade(),
                        (format!("group-{suffix}-{slot}"), label, icon),
                        target.is_none(),
                        target.clone(),
                        action,
                        focus_visible(cx),
                    ));
                }
                if self.tab_prefs.show_float {
                    actions = actions.child(Self::group_button(
                        cx.entity().downgrade(),
                        (
                            format!("group-float-{slot}"),
                            "Float this tab",
                            IconName::PanelsTopLeft,
                        ),
                        !can_float,
                        target.clone(),
                        GroupCommand::Float,
                        focus_visible(cx),
                    ));
                }
                if !maximized && self.tab_prefs.show_maximize {
                    actions = actions.child(Self::group_button(
                        cx.entity().downgrade(),
                        (
                            format!("group-maximize-{slot}"),
                            "Maximize this group",
                            IconName::Maximize,
                        ),
                        target.is_none(),
                        target.clone(),
                        GroupCommand::Maximize,
                        focus_visible(cx),
                    ));
                }
            }
            if maximized {
                let restore_target = target.clone();
                let weak = cx.entity().downgrade();
                let (state_text, restore_label) = maximized_chrome_copy(compact);
                actions = actions
                    .child(
                        div()
                            .id(SharedString::from(format!("group-maximized-state-{slot}")))
                            .debug_selector(|| "group-maximized-state".into())
                            .flex_none()
                            .whitespace_nowrap()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(state_text),
                    )
                    .child(
                        Button::new(SharedString::from(format!("group-restore-{slot}")))
                            .debug_selector(|| "group-restore-button".into())
                            .label(restore_label)
                            .accessibility_label("Restore layout")
                            .tooltip("Restore layout")
                            .icon(Icon::new(IconName::Minimize))
                            .outline()
                            .compact()
                            .xsmall()
                            .disabled(restore_target.is_none())
                            .on_click(move |_, window, cx| {
                                if let Some(target) = &restore_target {
                                    let _ = weak.update(cx, |this, cx| {
                                        this.group_command(
                                            target,
                                            GroupCommand::Maximize,
                                            window,
                                            cx,
                                        )
                                    });
                                }
                            }),
                    );
            }
        }
        actions.into_any_element()
    }
}
