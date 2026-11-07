//! Window-local Screen catalog presentation. The service catalog remains authoritative.
use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
    Disableable, Icon, Sizable,
};
use splitty_client_core::workspace::ScreenState;

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

const EDGE_WIDTH: f32 = 6.;
const EDGE_DWELL: Duration = Duration::from_millis(200);
const EDGE_INTENT_SPEED: f32 = 800.;

fn edge_motion(
    previous: Option<(Point<Pixels>, Instant)>,
    now: Instant,
    position: Point<Pixels>,
) -> bool {
    if f32::from(position.x) > EDGE_WIDTH {
        return false;
    }
    previous.is_none_or(|(point, then)| {
        let elapsed = now.saturating_duration_since(then).as_secs_f32();
        elapsed == 0. || {
            let dx = f32::from(position.x - point.x);
            let dy = f32::from(position.y - point.y);
            dx.hypot(dy) / elapsed < EDGE_INTENT_SPEED
        }
    })
}

fn collect_pane_groups<'a>(node: &'a LayoutNode, out: &mut Vec<Vec<&'a str>>) {
    match node {
        LayoutNode::Slot { pane_ids, .. } => {
            out.push(pane_ids.iter().map(|id| id.as_str()).collect())
        }
        LayoutNode::Split { children, .. } => {
            for child in children {
                collect_pane_groups(child, out);
            }
        }
    }
}

fn drawer_pane_groups(screen: &ScreenState) -> Vec<Vec<&str>> {
    let mut groups = Vec::new();
    collect_pane_groups(&screen.root, &mut groups);
    for floating in &screen.floating {
        collect_pane_groups(&floating.root, &mut groups);
    }
    groups
}

fn split_title_icon(title: &str) -> (Option<char>, &str) {
    let mut chars = title.chars();
    let Some(first) = chars.next() else {
        return (None, title);
    };
    if !first.is_ascii() && !first.is_alphanumeric() && chars.next() == Some(' ') {
        (Some(first), &title[first.len_utf8() + 1..])
    } else {
        (None, title)
    }
}

impl Workspace {
    fn set_drawer_overlay(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.drawer_visible || self.drawer_overlay_open == open {
            return;
        }
        self.drawer_overlay_open = open;
        self.drawer_slide_generation = self.drawer_slide_generation.wrapping_add(1);
        self.drawer_slide_task = None;
        let generation = self.drawer_slide_generation;
        let start = self.drawer_slide;
        let end = if open { 1. } else { 0. };
        self.wheel_navigation.reset();
        cx.notify();
        self.drawer_slide_task = Some(cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let duration = Duration::from_secs_f32(0.22 * (end - start).abs().max(0.05));
            loop {
                let elapsed = started.elapsed().as_secs_f32() / duration.as_secs_f32();
                let fraction = elapsed.clamp(0., 1.);
                let eased = 1. - (1. - fraction).powi(3);
                let done = this
                    .update(cx, |this, cx| {
                        if this.drawer_slide_generation != generation {
                            return true;
                        }
                        this.drawer_slide = start + (end - start) * eased;
                        cx.notify();
                        fraction >= 1.
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
            }
        }));
    }

    pub(super) fn toggle_drawer_overlay(&mut self, cx: &mut Context<Self>) {
        if self.drawer_visible {
            return;
        }
        self.drawer_edge_task = None;
        self.set_drawer_overlay(!self.drawer_overlay_open, cx);
    }

    pub(super) fn close_drawer_overlay(&mut self, cx: &mut Context<Self>) {
        self.drawer_edge_task = None;
        self.set_drawer_overlay(false, cx);
    }

    pub(super) fn drawer_edge_hover(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let now = Instant::now();
        let eligible = !self.drawer_visible
            && !self.drawer_overlay_open
            && self.drawer_edge_reveal
            && !event.dragging()
            && !GlobalState::is_in_deferred_context(cx)
            && edge_motion(self.drawer_edge_last, now, event.position);
        self.drawer_edge_last = Some((event.position, now));
        if !eligible {
            self.drawer_edge_task = None;
            return;
        }
        if self.drawer_edge_task.is_some() {
            return;
        }
        let owner = self.lifecycle_token();
        self.drawer_edge_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(EDGE_DWELL).await;
            let _ = this.update(cx, |this, cx| {
                if this.lifecycle_token() == owner
                    && !this.drawer_visible
                    && !this.drawer_overlay_open
                    && this.drawer_edge_reveal
                    && this
                        .drawer_edge_last
                        .is_some_and(|(point, _)| f32::from(point.x) <= EDGE_WIDTH)
                    && !GlobalState::is_in_deferred_context(cx)
                {
                    this.set_drawer_overlay(true, cx);
                }
                this.drawer_edge_task = None;
            });
        }));
    }

    pub(super) fn render_drawer(&self, cx: &mut Context<Self>) -> AnyElement {
        let owner = self.lifecycle_token();
        let muted = cx.theme().muted_foreground;
        let mut list = div()
            .id("drawer-screen-list")
            .flex_shrink_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.drawer_scroll)
            .py_1();
        if let Some(window_model) = &self.model {
            let shown = window_model.shown_key();
            let opened = window_model.opened();
            let mut group = None;
            for (index, entry) in opened.iter().enumerate() {
                let key = entry.key().clone();
                let remote = key.source_id() != window_model.local_source().source_id();
                if group != Some(key.source_id()) {
                    group = Some(key.source_id());
                    if remote {
                        let title = {
                            window_model
                                .source_target(key.source_id())
                                .map(super::hosts::display)
                                .unwrap_or_else(|| "Host".into())
                        };
                        list = list.child(
                            div()
                                .mx_2()
                                .mt_2()
                                .px_2()
                                .pt_2()
                                .pb_1()
                                .border_t_1()
                                .border_color(rgb(self.appearance.subtle_border))
                                .text_size(chrome_rems(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(muted)
                                .child(Icon::new(IconName::Server).size_3())
                                .child(format!("  {title}")),
                        );
                    }
                }
                let active = shown.as_ref() == Some(&key);
                let actions_visible = self.drawer_hovered.as_ref() == Some(&key);
                let label = entry
                    .name()
                    .map(super::hosts::display)
                    .unwrap_or_else(|| format!("Screen {}", index + 1));
                let state = match entry.state() {
                    splitty_client_core::workspace::OpenedScreenState::Ready => None,
                    splitty_client_core::workspace::OpenedScreenState::Loading => Some("Loading"),
                    splitty_client_core::workspace::OpenedScreenState::Missing => Some("Missing"),
                    splitty_client_core::workspace::OpenedScreenState::Offline => Some("Offline"),
                };
                let screen_snapshot = window_model
                    .source(key.source_id())
                    .and_then(|source| source.screen(key.screen_id()))
                    .and_then(ScreenModel::snapshot);
                let mut row = div()
                    .id(SharedString::from(format!("drawer-screen-{key}")))
                    .flex()
                    .items_center()
                    .min_w_0()
                    .h(chrome_rems(30.))
                    .px_2()
                    .child(
                        div()
                            .w(chrome_rems(14.))
                            .flex_none()
                            .text_size(chrome_rems(10.5))
                            .font(font(self.mono_family.clone()))
                            .text_align(TextAlign::Right)
                            .text_color(if active {
                                rgb(self.appearance.accent)
                            } else {
                                muted.into()
                            })
                            .child(format!("{}", index + 1)),
                    );
                if !remote
                    && self
                        .rename
                        .as_ref()
                        .is_some_and(|editor| editor.screen == key)
                {
                    if let Some(editor) = &self.rename {
                        row = row.child(div().flex_1().min_w_0().child(Input::new(&editor.input)));
                    }
                } else {
                    let select = key.clone();
                    let weak = cx.entity().downgrade();
                    row = row.child(
                        Button::new(SharedString::from(format!("drawer-select-{key}")))
                            .accessibility_label(format!("Select {label}"))
                            .ghost()
                            .compact()
                            .small()
                            .flex_1()
                            .min_w_0()
                            .justify_start()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(chrome_rems(12.5))
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .text_color(rgb(if active {
                                        self.appearance.text
                                    } else {
                                        self.appearance.secondary
                                    }))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(label.clone()),
                            )
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.lifecycle_token() == owner
                                        && !GlobalState::is_in_deferred_context(cx)
                                    {
                                        this.switch_screen_key(select.clone(), window, cx);
                                        this.focus_current(window, cx);
                                    }
                                });
                            }),
                    );
                }
                if let Some(state) = state {
                    row = row.child(div().flex_none().text_sm().text_color(muted).child(state));
                }
                if actions_visible
                    && screen_snapshot.is_some_and(|screen| {
                        drawer_pane_groups(screen)
                            .iter()
                            .map(Vec::len)
                            .sum::<usize>()
                            > 1
                    })
                {
                    let collapsed = self.drawer_collapsed.contains(&key);
                    let collapse_key = key.clone();
                    let weak = cx.entity().downgrade();
                    row = row.child(
                        Button::new(SharedString::from(format!("drawer-collapse-{key}")))
                            .accessibility_label(if collapsed {
                                format!("Show panes in {label}")
                            } else {
                                format!("Hide panes in {label}")
                            })
                            .ghost()
                            .compact()
                            .xsmall()
                            .w(chrome_rems(24.))
                            .icon(Icon::new(if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            }))
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.lifecycle_token() != owner {
                                        return;
                                    }
                                    if !this.drawer_collapsed.insert(collapse_key.clone()) {
                                        this.drawer_collapsed.remove(&collapse_key);
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                if actions_visible
                    && !remote
                    && self
                        .rename
                        .as_ref()
                        .is_none_or(|editor| editor.screen != key)
                {
                    let rename = key.clone();
                    let weak = cx.entity().downgrade();
                    row = row.child(
                        Button::new(SharedString::from(format!("drawer-rename-{key}")))
                            .accessibility_label(format!("Rename {label}"))
                            .ghost()
                            .compact()
                            .xsmall()
                            .w(chrome_rems(27.))
                            .tooltip(format!("Rename {label}"))
                            .icon(Icon::new(IconName::Pencil))
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.lifecycle_token() == owner
                                        && this.model.as_ref().is_some_and(|model| {
                                            model
                                                .local_source()
                                                .screen(rename.screen_id())
                                                .is_some()
                                        })
                                    {
                                        this.start_rename(rename.screen_id(), window, cx);
                                    }
                                });
                            }),
                    );
                }
                if actions_visible && remote {
                    let terminate = key.clone();
                    let weak = cx.entity().downgrade();
                    let token = window_model
                        .execution_token(key.source_id())
                        .filter(|_| state.is_none());
                    row = row.child(
                        Button::new(SharedString::from(format!("drawer-more-{key}")))
                            .accessibility_label(format!("More actions for {label}"))
                            .tooltip("More Screen actions")
                            .icon(Icon::new(IconName::Ellipsis))
                            .ghost()
                            .compact()
                            .xsmall()
                            .w(chrome_rems(27.))
                            .dropdown_menu(move |menu, _, _| {
                                let weak = weak.clone();
                                let terminate = terminate.clone();
                                menu.item(
                                    PopupMenuItem::new("Terminate Screen…")
                                        .disabled(token.is_none())
                                        .on_click(move |_, window, cx| {
                                            if let Some(token) = token {
                                                let _ = weak.update(cx, |this, cx| {
                                                    this.request_terminate_screen(
                                                        token,
                                                        terminate.clone(),
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            }
                                        }),
                                )
                            }),
                    );
                }
                if actions_visible {
                    let close = key.clone();
                    let weak = cx.entity().downgrade();
                    let close_tooltip = if remote {
                        "Detach screen: keeps running on the host"
                    } else {
                        "Close screen"
                    };
                    row = row.child(
                        Button::new(SharedString::from(format!("drawer-close-{key}")))
                            .accessibility_label(if remote {
                                format!("Detach {label}: keeps running on the host")
                            } else {
                                format!("Close {label}")
                            })
                            .ghost()
                            .compact()
                            .xsmall()
                            .w(chrome_rems(27.))
                            .tooltip(close_tooltip)
                            .icon(Icon::new(if remote {
                                IconName::Unplug
                            } else {
                                IconName::X
                            }))
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.lifecycle_token() == owner
                                        && this.model.as_ref().is_some_and(|model| {
                                            model.opened().iter().any(|entry| entry.key() == &close)
                                        })
                                    {
                                        this.request_close_screen(close.clone(), window, cx);
                                    }
                                });
                            }),
                    );
                }
                let mut block = div()
                    .id(SharedString::from(format!("drawer-block-{key}")))
                    .mx_2()
                    .my_0p5()
                    .rounded_md()
                    .bg(rgb(if active {
                        self.appearance.active
                    } else {
                        self.appearance.raised
                    }))
                    .on_hover({
                        let hover_key = key.clone();
                        cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.drawer_hovered = Some(hover_key.clone());
                                cx.notify();
                            } else if this.drawer_hovered.as_ref() == Some(&hover_key) {
                                this.drawer_hovered = None;
                                cx.notify();
                            }
                        })
                    })
                    .child(row);
                if let Some(screen) =
                    screen_snapshot.filter(|_| !self.drawer_collapsed.contains(&key))
                {
                    let groups = drawer_pane_groups(screen);
                    let multiple_groups = groups.len() > 1;
                    let pane_source = window_model.execution_token(key.source_id());
                    for group in groups {
                        let stacked = multiple_groups && group.len() > 1;
                        let mut group_view = div().flex().flex_col();
                        if stacked {
                            group_view = group_view
                                .ml(chrome_rems(30.))
                                .pl_2()
                                .border_l_1()
                                .border_color(rgb(self.appearance.border));
                        }
                        for pane_id in group {
                            let Some((_, pane)) =
                                screen.panes.iter().find(|(id, _)| id.as_str() == pane_id)
                            else {
                                continue;
                            };
                            let pane_key = pane_id.to_owned();
                            let select_key = key.clone();
                            let weak = cx.entity().downgrade();
                            let title = pane.title.as_deref().unwrap_or("Terminal");
                            let title = super::hosts::display(title);
                            let (leading_icon, pane_label) = split_title_icon(&title);
                            let is_viewer = pane.viewer.is_some();
                            let busy = pane
                                .session_id
                                .as_ref()
                                .and_then(|id| screen.sessions.get(id))
                                .is_some_and(|session| session.busy);
                            let glyph: AnyElement = if is_viewer {
                                Icon::new(IconName::FileText).size_3().into_any_element()
                            } else if let Some(icon) = leading_icon {
                                div()
                                    .text_size(chrome_rems(10.))
                                    .child(icon.to_string())
                                    .into_any_element()
                            } else {
                                div()
                                    .size(chrome_rems(5.))
                                    .rounded_full()
                                    .bg(if busy {
                                        rgb(self.appearance.text)
                                    } else {
                                        muted.into()
                                    })
                                    .into_any_element()
                            };
                            group_view = group_view.child(
                                Button::new(SharedString::from(format!(
                                    "drawer-pane-{key}-{pane_id}"
                                )))
                                .accessibility_label(format!("Open {title} in {label}"))
                                .disabled(pane_source.is_none())
                                .ghost()
                                .compact()
                                .xsmall()
                                .w_full()
                                .h(chrome_rems(21.))
                                .pl(chrome_rems(if stacked { 8. } else { 30. }))
                                .justify_start()
                                .font(font(self.mono_family.clone()))
                                .text_size(chrome_rems(11.))
                                .child(
                                    div()
                                        .w_full()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap(chrome_rems(7.))
                                        .text_color(if busy {
                                            rgb(self.appearance.text)
                                        } else {
                                            muted.into()
                                        })
                                        .child(
                                            div()
                                                .w(chrome_rems(12.))
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(glyph),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(pane_label.to_owned()),
                                        ),
                                )
                                .on_click(
                                    move |_, window, cx| {
                                        let _ = weak.update(cx, |this, cx| {
                                            if this.lifecycle_token() != owner
                                                || !pane_source.is_some_and(|token| {
                                                    this.model.as_ref().is_some_and(|model| {
                                                        model
                                                            .execution_token(select_key.source_id())
                                                            == Some(token)
                                                            && model
                                                                .source(select_key.source_id())
                                                                .and_then(|source| {
                                                                    source.screen(
                                                                        select_key.screen_id(),
                                                                    )
                                                                })
                                                                .and_then(ScreenModel::snapshot)
                                                                .is_some_and(|screen| {
                                                                    screen.panes.keys().any(|id| {
                                                                        id.as_str() == pane_key
                                                                    })
                                                                })
                                                    })
                                                })
                                            {
                                                return;
                                            }
                                            let token = pane_source
                                                .expect("checked captured pane authority");
                                            this.switch_screen_key(select_key.clone(), window, cx);
                                            this.send_captured(
                                                token,
                                                Command::Select {
                                                    screen: select_key.clone(),
                                                    pane: pane_key.clone(),
                                                },
                                                cx,
                                            );
                                        });
                                    },
                                ),
                            );
                        }
                        block = block.child(group_view);
                    }
                }
                list = list.child(block);
            }
            if let Some(pending) =
                window_model
                    .local_source()
                    .pending_template()
                    .filter(|pending| {
                        !window_model
                            .local_source()
                            .screens()
                            .iter()
                            .any(|info| info.id.as_str() == pending.screen_id())
                    })
            {
                list = list.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_color(rgb(self.appearance.secondary))
                        .child(format!("{} · Applying…", pending.display_name())),
                );
            }
            if opened.is_empty() && window_model.local_source().pending_template().is_none() {
                list = list.child(
                    div()
                        .px_2()
                        .py_3()
                        .text_color(muted)
                        .child("No Screens yet"),
                );
            }
        } else {
            list = list.child(div().px_2().py_3().text_color(muted).child("Connecting…"));
        }
        let weak = cx.entity().downgrade();
        let create = Button::new("drawer-create")
            .accessibility_label("Create local Screen")
            .small()
            .w_full()
            .h(chrome_rems(32.))
            .ghost()
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(chrome_rems(8.))
                    .text_color(rgb(self.appearance.secondary))
                    .child(
                        div()
                            .w(chrome_rems(14.))
                            .flex_none()
                            .flex()
                            .justify_end()
                            .child(Icon::new(IconName::Plus).size_3()),
                    )
                    .child("New screen"),
            )
            .on_click(move |_, _, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if this.lifecycle_token() == owner {
                        this.create_screen(cx);
                    }
                });
            });
        let weak = cx.entity().downgrade();
        let templates = Button::new("drawer-templates")
            .accessibility_label("Open local Templates")
            .ghost()
            .small()
            .w_full()
            .h(chrome_rems(30.))
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
                            .child(Icon::new(IconName::LayoutTemplate).size_3()),
                    )
                    .child("Templates"),
            )
            .on_click(move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if this.lifecycle_token() == owner {
                        this.toggle_templates(window, cx);
                    }
                });
            });
        div()
            .id("screen-drawer")
            .flex_none()
            .flex()
            .flex_col()
            .w(chrome_rems(self.drawer_width as f32))
            .min_h_0()
            .h_full()
            .bg(rgb(self.appearance.raised))
            .occlude()
            .border_r_1()
            .border_color(rgb(self.appearance.subtle_border))
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered
                    && this.drawer_overlay_open
                    && this.rename.is_none()
                    && this.transfer_menu.is_none()
                    && this.profile_picker.is_none()
                    && this.template_dialog.is_none()
                    && this
                        .pending_close
                        .as_ref()
                        .is_none_or(|close| !close.prompting)
                    && this.drag.is_none()
                    && this.float_gesture.is_none()
                    && !GlobalState::is_in_deferred_context(cx)
                    && !cx.has_active_drag()
                {
                    this.close_drawer_overlay(cx);
                }
            }))
            .on_scroll(cx.listener(move |this, event: &ScrollEvent, window, cx| {
                if let Some(source) = owner {
                    this.chrome_wheel(source, WheelTarget::Screens, event, window, cx);
                } else if WheelNavigation::vertical(event) {
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .id("drawer-header")
                    .h(chrome_rems(32.))
                    .pl_4()
                    .pr_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_color(muted)
                    .child(
                        div()
                            .text_size(chrome_rems(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("SCREENS"),
                    )
                    .child(
                        Button::new("drawer-pin")
                            .accessibility_label(if self.drawer_visible {
                                "Unpin drawer"
                            } else {
                                "Pin drawer"
                            })
                            .tooltip(if self.drawer_visible {
                                "Unpin drawer"
                            } else {
                                "Pin drawer"
                            })
                            .selected(self.drawer_visible)
                            .disabled(self.drawer_pending.is_some() || owner.is_none())
                            .ghost()
                            .compact()
                            .xsmall()
                            .icon(Icon::new(if self.drawer_visible {
                                IconName::PinOff
                            } else {
                                IconName::Pin
                            }))
                            .on_click({
                                let weak = cx.entity().downgrade();
                                move |_, _, cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        if this.lifecycle_token() == owner {
                                            this.toggle_drawer(cx);
                                        }
                                    });
                                }
                            }),
                    ),
            )
            .child(list)
            .child(div().px_2().py_1().child(create))
            .child(div().flex_1().min_h(chrome_rems(8.)))
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .pb_2()
                    .border_t_1()
                    .border_color(rgb(self.appearance.subtle_border))
                    .child(self.render_hosts_navigation(cx))
                    .child(templates),
            )
            .into_any_element()
    }
    pub(super) fn chrome_wheel(
        &mut self,
        source: SourceToken,
        target: WheelTarget,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if match &target {
            WheelTarget::Screens => self.lifecycle_token() != Some(source),
            WheelTarget::Slot { .. } => !self
                .selected_model()
                .is_some_and(|model| model.source_token() == source),
        } {
            if WheelNavigation::vertical(event) {
                cx.stop_propagation();
            }
            return;
        }
        let blocked = self.rename.is_some()
            || self.template_dialog.is_some()
            || self.profile_picker.is_some()
            || self.transfer_menu.is_some()
            || self.pending_template_apply.is_some()
            || self
                .search_owner
                .as_ref()
                .is_some_and(|(_, terminal)| terminal.read(cx).search_input_focused(window, cx))
            || self
                .pending_close
                .as_ref()
                .is_some_and(|close| close.prompting)
            || self.drag.is_some()
            || self.float_gesture.is_some()
            || GlobalState::is_in_deferred_context(cx)
            || cx.has_active_drag();
        if blocked {
            self.wheel_navigation.admit(target, event, true);
            if WheelNavigation::vertical(event) {
                cx.stop_propagation();
            }
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
        for _ in 0..steps.unsigned_abs() {
            match &target {
                WheelTarget::Screens => self.send_captured(
                    source,
                    Command::CycleScreen {
                        step: wheel_step(steps),
                    },
                    cx,
                ),
                WheelTarget::Slot { screen, slot } => {
                    let connection = source.connection_id();
                    self.send_captured(
                        source,
                        Command::CycleTabInSlot {
                            connection,
                            screen: screen.clone(),
                            slot: slot.clone(),
                            step: wheel_step(steps),
                        },
                        cx,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{edge_motion, split_title_icon, wheel_step, WheelNavigation, WheelTarget};
    use gpui_kit::{point, px, ScrollDelta, ScrollEvent, TouchPhase};
    use splitty_client_core::workspace::TabStep;
    use std::time::{Duration, Instant};

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

    #[test]
    fn edge_reveal_requires_slow_unpressed_motion_inside_six_pixels() {
        let now = Instant::now();
        assert!(edge_motion(None, now, point(px(3.), px(20.))));
        assert!(!edge_motion(None, now, point(px(7.), px(20.))));
        assert!(!edge_motion(
            Some((point(px(1.), px(0.)), now)),
            now + Duration::from_millis(10),
            point(px(3.), px(20.)),
        ));
        assert!(edge_motion(
            Some((point(px(1.), px(0.)), now)),
            now + Duration::from_millis(200),
            point(px(3.), px(20.)),
        ));
    }

    #[test]
    fn leading_pane_icon_requires_a_separate_non_ascii_symbol() {
        assert_eq!(split_title_icon("⚡ Build"), (Some('⚡'), "Build"));
        assert_eq!(split_title_icon("Terminal"), (None, "Terminal"));
        assert_eq!(split_title_icon("Änderung"), (None, "Änderung"));
        assert_eq!(split_title_icon("⚡Build"), (None, "⚡Build"));
    }
}
