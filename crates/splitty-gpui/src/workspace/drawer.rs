//! Window-local Screen catalog presentation. The service catalog remains authoritative.
use super::*;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    Disableable, Sizable,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WheelTarget {
    Screens,
    Slot { screen: String, slot: String },
}

#[derive(Default)]
pub(super) struct WheelNavigation {
    target: Option<WheelTarget>,
    pixels: f32,
}

impl WheelNavigation {
    pub(super) fn reset(&mut self) {
        self.target = None;
        self.pixels = 0.;
    }

    fn admit(&mut self, target: WheelTarget, event: &ScrollEvent, blocked: bool) -> i32 {
        if blocked {
            self.reset();
            return 0;
        }
        self.steps(target, event)
    }

    /// Returns bounded relative steps. A phase-less pixel stream still advances
    /// as distance accrues; a real boundary discards the prior partial step.
    fn steps(&mut self, target: WheelTarget, event: &ScrollEvent) -> i32 {
        if !Self::vertical(event) {
            self.reset();
            return 0;
        }
        let (x, y) = match event.delta {
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
        };
        if !x.is_finite() || !y.is_finite() || x.abs() >= y.abs() || y == 0. {
            self.reset();
            return 0;
        }
        if self.target.as_ref() != Some(&target) || event.touch_phase == TouchPhase::Started {
            self.pixels = 0.;
        }
        self.target = Some(target);
        if event.touch_phase == TouchPhase::Cancelled {
            self.reset();
            return 0;
        }
        let result = match event.delta {
            // The line value is an OS scroll setting, not a detent count.
            ScrollDelta::Lines(_) => {
                if y > 0. {
                    1
                } else {
                    -1
                }
            }
            ScrollDelta::Pixels(_) => {
                if self.pixels.signum() != y.signum() {
                    self.pixels = 0.;
                }
                self.pixels = (self.pixels + y).clamp(-192., 192.);
                let steps = (self.pixels / 48.).trunc().clamp(-1., 1.) as i32;
                self.pixels -= steps as f32 * 48.;
                steps
            }
        };
        if matches!(event.touch_phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            self.reset();
        }
        result
    }

    fn vertical(event: &ScrollEvent) -> bool {
        if event.modifiers.shift {
            return false;
        }
        let (x, y) = match event.delta {
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
        };
        x.is_finite() && y.is_finite() && y != 0. && y.abs() > x.abs()
    }
}

fn wheel_step(direction: i32) -> TabStep {
    if direction > 0 {
        TabStep::Previous
    } else {
        TabStep::Next
    }
}

impl Workspace {
    pub(super) fn render_drawer_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let selected = self.drawer_visible;
        Button::new("drawer-toggle")
            .accessibility_label(if selected {
                "Hide Screens"
            } else {
                "Show Screens"
            })
            .disabled(self.drawer_pending.is_some())
            .selected(selected)
            .ghost()
            .compact()
            .small()
            .px_3()
            .tooltip(if selected {
                "Hide screens"
            } else {
                "Show screens"
            })
            .label("☰  Screens")
            .on_click(move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.toggle_drawer(cx);
                    this.focus_current(window, cx);
                });
            })
            .into_any_element()
    }

    pub(super) fn render_drawer(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div()
            .id("drawer-screen-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.drawer_scroll)
            .px_2()
            .py_2();
        if let Some(model) = self.model.as_ref() {
            for (index, info) in model.screens().iter().enumerate() {
                let id = info.id.to_string();
                let active = model.shown_screen_id() == Some(id.as_str());
                let mut label = info
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Screen {}", index + 1));
                if model
                    .pending_template()
                    .is_some_and(|pending| pending.screen_id() == id)
                {
                    label.push_str(" · Applying…");
                }
                let count = model
                    .screen(&id)
                    .and_then(ScreenModel::snapshot)
                    .map(|screen| screen.panes.len());
                let select = id.clone();
                let rename = id.clone();
                let close = id.clone();
                let mut row = div()
                    .id(SharedString::from(format!("drawer-screen-{id}")))
                    .flex()
                    .items_center()
                    .min_w_0()
                    .h(chrome_rems(38.))
                    .my_1()
                    .rounded_md()
                    .bg(rgb(if active {
                        self.appearance.active
                    } else {
                        self.appearance.raised
                    }))
                    .child(
                        div()
                            .w(chrome_rems(25.))
                            .flex_none()
                            .text_color(rgb(if active {
                                self.appearance.accent
                            } else {
                                self.appearance.muted
                            }))
                            .child(format!("{}", index + 1)),
                    );
                if self
                    .rename
                    .as_ref()
                    .is_some_and(|editor| editor.screen == id)
                {
                    if let Some(editor) = self.rename.as_ref() {
                        row = row.child(div().flex_1().min_w_0().child(Input::new(&editor.input)));
                    }
                } else {
                    let weak = cx.entity().downgrade();
                    row = row.child(
                        Button::new(SharedString::from(format!("drawer-select-{id}")))
                            .accessibility_label(format!("Select {label}"))
                            .selected(active)
                            .ghost()
                            .compact()
                            .small()
                            .flex_1()
                            .min_w_0()
                            .justify_start()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .label(label.clone())
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.switch_screen(&select, window, cx);
                                    this.focus_current(window, cx);
                                });
                            }),
                    );
                }
                if let Some(count) = count {
                    row = row.child(
                        div()
                            .flex_none()
                            .text_color(rgb(self.appearance.muted))
                            .child(count.to_string()),
                    );
                }
                if self
                    .rename
                    .as_ref()
                    .is_none_or(|editor| editor.screen != id)
                {
                    let weak = cx.entity().downgrade();
                    row = row.child(
                        Button::new(SharedString::from(format!("drawer-rename-{id}")))
                            .accessibility_label(format!("Rename {label}"))
                            .ghost()
                            .compact()
                            .xsmall()
                            .w(chrome_rems(27.))
                            .tooltip(format!("Rename {label}"))
                            .label("✎")
                            .on_click(move |_, window, cx| {
                                let _ = weak
                                    .update(cx, |this, cx| this.start_rename(&rename, window, cx));
                            }),
                    );
                }
                let weak = cx.entity().downgrade();
                row = row.child(
                    Button::new(SharedString::from(format!("drawer-close-{id}")))
                        .accessibility_label(format!("Close {label}"))
                        .ghost()
                        .compact()
                        .xsmall()
                        .w(chrome_rems(27.))
                        .tooltip(format!("Close {label}"))
                        .label("×")
                        .on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.request_close_screen(&close, window, cx)
                            });
                        }),
                );
                list = list.child(row);
            }
            if let Some(pending) = model.pending_template().filter(|pending| {
                !model
                    .screens()
                    .iter()
                    .any(|info| info.id.as_str() == pending.screen_id())
            }) {
                list = list.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_color(rgb(self.appearance.secondary))
                        .child(format!("{} · Applying…", pending.display_name())),
                );
            }
            if model.screens().is_empty() && model.pending_template().is_none() {
                list = list.child(
                    div()
                        .px_2()
                        .py_3()
                        .text_color(rgb(self.appearance.muted))
                        .child("No Screens yet"),
                );
            }
        } else {
            list = list.child(
                div()
                    .px_2()
                    .py_3()
                    .text_color(rgb(self.appearance.muted))
                    .child("Connecting…"),
            );
        }
        let weak = cx.entity().downgrade();
        let create = Button::new("drawer-create")
            .accessibility_label("Create Screen")
            .small()
            .w_full()
            .label("+  New screen")
            .on_click(move |_, _, cx| {
                let _ = weak.update(cx, |this, cx| this.create_screen(cx));
            });
        let weak = cx.entity().downgrade();
        let templates = Button::new("drawer-templates")
            .accessibility_label("Open Templates")
            .ghost()
            .small()
            .w_full()
            .label("▦  Templates")
            .on_click(move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| this.toggle_templates(window, cx));
            });
        div()
            .id("screen-drawer")
            .flex_none()
            .flex()
            .flex_col()
            .w(chrome_rems(self.drawer_width as f32))
            .min_h_0()
            .bg(rgb(self.appearance.raised))
            .border_r_1()
            .border_color(rgb(self.appearance.subtle_border))
            .child(
                div()
                    .id("drawer-header")
                    .h(chrome_rems(44.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_color(rgb(self.appearance.muted))
                    .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                        this.chrome_wheel(WheelTarget::Screens, event, window, cx);
                    }))
                    .child("SCREENS")
                    .child(
                        self.model
                            .as_ref()
                            .map_or(0, |model| model.screens().len())
                            .to_string(),
                    ),
            )
            .child(list)
            .child(div().px_2().py_2().child(create))
            .child(div().px_2().pb_2().child(templates))
            .into_any_element()
    }

    pub(super) fn chrome_wheel(
        &mut self,
        target: WheelTarget,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let blocked = self.rename.is_some()
            || self.template_dialog.is_some()
            || self.profile_picker.is_some()
            || self.transfer_menu.is_some()
            || self.pending_template_apply.is_some()
            || self
                .search_owner
                .as_ref()
                .is_some_and(|terminal| terminal.read(cx).search_input_focused(window, cx))
            || self
                .pending_close
                .as_ref()
                .is_some_and(|close| close.prompting)
            || self.drag.is_some()
            || self.float_gesture.is_some()
            || cx.has_active_drag();
        if blocked {
            self.wheel_navigation.admit(target, event, true);
            return;
        }
        let vertical = WheelNavigation::vertical(event);
        let steps = self.wheel_navigation.admit(target.clone(), event, false);
        if vertical {
            cx.stop_propagation();
        }
        if steps == 0 {
            return;
        }
        let Some(connection) = self.model.as_ref().map(LocalWorkspace::connection_id) else {
            return;
        };
        for _ in 0..steps.unsigned_abs() {
            match &target {
                WheelTarget::Screens => self.send(
                    Command::CycleScreen {
                        connection,
                        step: wheel_step(steps),
                    },
                    cx,
                ),
                WheelTarget::Slot { screen, slot } => self.send(
                    Command::CycleTabInSlot {
                        connection,
                        screen: screen.clone(),
                        slot: slot.clone(),
                        step: wheel_step(steps),
                    },
                    cx,
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{wheel_step, WheelNavigation, WheelTarget};
    use gpui_kit::{point, px, ScrollDelta, ScrollEvent, TouchPhase};
    use splitty_client_core::workspace::TabStep;

    fn event(delta: ScrollDelta, phase: TouchPhase) -> ScrollEvent {
        ScrollEvent {
            delta,
            touch_phase: phase,
            ..Default::default()
        }
    }
    #[test]
    fn line_steps_are_discrete_and_bounded() {
        assert_eq!(wheel_step(1), TabStep::Previous);
        assert_eq!(wheel_step(-1), TabStep::Next);
        let mut wheel = WheelNavigation::default();
        let down = event(ScrollDelta::Lines(point(0., 1.)), TouchPhase::Moved);
        assert_eq!(wheel.steps(WheelTarget::Screens, &down), 1);
        assert_eq!(wheel.steps(WheelTarget::Screens, &down), 1);
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(ScrollDelta::Lines(point(0., -8.)), TouchPhase::Moved)
            ),
            -1
        );
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(ScrollDelta::Lines(point(0., 3.)), TouchPhase::Moved)
            ),
            1
        );
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(ScrollDelta::Lines(point(0., -0.5)), TouchPhase::Moved)
            ),
            -1
        );
    }
    #[test]
    fn pixels_accumulate_and_reset_on_target_or_phase() {
        let mut wheel = WheelNavigation::default();
        let part = event(
            ScrollDelta::Pixels(point(px(0.), px(25.))),
            TouchPhase::Moved,
        );
        assert_eq!(wheel.steps(WheelTarget::Screens, &part), 0);
        assert_eq!(wheel.steps(WheelTarget::Screens, &part), 1);
        assert_eq!(
            wheel.steps(
                WheelTarget::Slot {
                    screen: "s".into(),
                    slot: "g".into()
                },
                &part
            ),
            0
        );
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(
                    ScrollDelta::Pixels(point(px(0.), px(25.))),
                    TouchPhase::Started
                )
            ),
            0
        );
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(
                    ScrollDelta::Pixels(point(px(0.), px(25.))),
                    TouchPhase::Cancelled
                )
            ),
            0
        );
        assert_eq!(wheel.steps(WheelTarget::Screens, &part), 0);
    }
    #[test]
    fn horizontal_has_priority() {
        let mut wheel = WheelNavigation::default();
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(
                    ScrollDelta::Pixels(point(px(60.), px(20.))),
                    TouchPhase::Moved
                )
            ),
            0
        );
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(
                    ScrollDelta::Pixels(point(px(0.), px(48.))),
                    TouchPhase::Ended
                )
            ),
            1
        );
    }

    #[test]
    fn blocked_navigation_clears_partial_distance() {
        let mut wheel = WheelNavigation::default();
        let part = event(
            ScrollDelta::Pixels(point(px(0.), px(30.))),
            TouchPhase::Moved,
        );
        assert_eq!(wheel.admit(WheelTarget::Screens, &part, false), 0);
        assert_eq!(wheel.admit(WheelTarget::Screens, &part, true), 0);
        assert_eq!(wheel.admit(WheelTarget::Screens, &part, false), 0);
    }
}
