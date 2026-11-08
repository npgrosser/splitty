//! Bounded Screen presentation; selection and terminal routes stay in the model.
use super::*;
use crate::terminal::TerminalPresentation;
use gpui_kit::base::{transition_with_status, MotionStatus};

/// Shared tab geometry and colors; interaction is attached only by the live projection.
pub(super) fn tab_surface(appearance: &NativeAppearance, active: bool) -> Div {
    div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .pl_3()
        .pr_1()
        .bg(rgb(if active {
            appearance.active
        } else {
            appearance.raised
        }))
        .text_color(rgb(if active {
            appearance.text
        } else {
            appearance.secondary
        }))
}

pub(super) fn tab_label(title: String) -> Div {
    div()
        .max_w(chrome_rems(180.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(title)
}

// These are inert drawings of Kit's compact ghost icon-button geometry, not
// disabled Buttons: they create no focus, tooltip, hover or pointer behavior.
fn passive_icon(icon: IconName, compact: bool, disabled: bool, cx: &App) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .line_height(relative(1.25))
        .text_color(if disabled {
            cx.theme().muted_foreground.opacity(0.5)
        } else {
            cx.theme().secondary_foreground
        })
        .when(compact, |view| {
            view.size_5().child(Icon::new(icon).xsmall())
        })
        .when(!compact, |view| {
            view.size_6().child(Icon::new(icon).small())
        })
}

#[derive(Default)]
pub(super) struct ScreenMotion {
    outgoing: Option<ScreenPresentation>,
    target: f32,
    sampled: f32,
}

pub(super) struct ScreenPresentation {
    key: ScreenKey,
    owner: SourceToken,
    model: ScreenModel,
    viewport: Bounds<Pixels>,
    origin: Point<Pixels>,
    panes: HashMap<String, (TerminalToken, TerminalPresentation)>,
    floats: Vec<String>,
    remote: bool,
    ready: bool,
    tab_strips: HashMap<String, (Pixels, Point<Pixels>)>,
    slot_widths: HashMap<String, Pixels>,
    floatable: HashSet<String>,
    anchor: f32,
}

pub(super) enum ScreenProjection<'a> {
    Interactive(&'a SourceWorkspace),
    Presentation(&'a ScreenPresentation),
}

impl ScreenProjection<'_> {
    pub(super) fn key(&self) -> ScreenKey {
        match self {
            Self::Interactive(source) => {
                source.screen_key(source.shown_screen_id().unwrap_or_default())
            }
            Self::Presentation(presentation) => presentation.key.clone(),
        }
    }

    pub(super) fn owner(&self) -> SourceToken {
        match self {
            Self::Interactive(source) => source.source_token(),
            Self::Presentation(presentation) => presentation.owner,
        }
    }

    pub(super) fn is_interactive(&self) -> bool {
        matches!(self, Self::Interactive(_))
    }
}

impl ScreenMotion {
    pub(super) fn clear(&mut self) -> bool {
        let retained = self.outgoing.is_some();
        self.outgoing = None;
        self.target = 0.;
        self.sampled = 0.;
        retained
    }
}

impl ScreenPresentation {
    fn valid(&self, model: &WindowWorkspace, cx: &App) -> bool {
        model.source(self.key.source_id()).is_some_and(|source| {
            source.source_token() == self.owner
                && source
                    .screens()
                    .iter()
                    .any(|screen| screen.id.as_str() == self.key.screen_id())
        }) && self.panes.values().all(|(route, presentation)| {
            model.contains_terminal_binding(route) && presentation.is_valid(cx)
        })
    }
}

impl Workspace {
    pub(super) fn preserve_screen_motion_focus(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(viewport) = self.screen_viewport else {
            return;
        };
        let incoming = Bounds::new(
            viewport.origin + point(px(0.), self.screen_offset()),
            viewport.size,
        );
        if self.screen_motion.outgoing.is_some()
            && viewport.contains(&event.position)
            && !incoming.contains(&event.position)
            && !GlobalState::is_in_deferred_context(cx)
        {
            // The outgoing drawing has no focus node. Without this parent
            // guard, a click there focuses the workspace background and leaves
            // subsequent typing stranded after the current Screen settles.
            window.prevent_default();
            self.focus_current(window, cx);
            cx.stop_propagation();
        }
    }

    fn capture_screen(&self, cx: &mut Context<Self>) -> Option<ScreenPresentation> {
        let key = self.current_screen_key()?;
        let source = self.selected_model()?;
        if source.pending_template().is_some() {
            return None;
        }
        let model = source.shown()?.clone();
        let viewport = self.float_viewport?;
        let panes = model
            .displayed_panes()
            .into_iter()
            .map(|pane| {
                let view = self.panes.get(&source.pane_key(&pane))?;
                let presentation = view
                    .terminal
                    .update(cx, |terminal, cx| terminal.presentation(cx))?;
                Some((pane, (view.route.clone(), presentation)))
            })
            .collect::<Option<HashMap<_, _>>>()?;
        if panes.is_empty() {
            return None;
        }
        Some(ScreenPresentation {
            owner: source.source_token(),
            model,
            viewport,
            origin: self.screen_viewport?.origin + point(px(0.), self.screen_offset()),
            panes,
            floats: self.float_order.get(&key).cloned().unwrap_or_default(),
            remote: self.is_remote_selected(),
            ready: self.execution_token(source.source_id()) == Some(source.source_token()),
            tab_strips: self
                .tab_scrolls
                .iter()
                .filter(|(layout, _)| layout.screen() == &key)
                .map(|(layout, scroll)| {
                    (
                        layout.node_id().to_owned(),
                        (scroll.handle.bounds().size.width, scroll.handle.offset()),
                    )
                })
                .collect(),
            slot_widths: self
                .slot_bounds
                .iter()
                .filter(|(layout, _)| layout.screen() == &key)
                .map(|(layout, bounds)| (layout.node_id().to_owned(), bounds.size.width))
                .collect(),
            floatable: source
                .displayed_panes()
                .into_iter()
                .filter(|pane| {
                    source
                        .capture_pane_action(pane)
                        .is_ok_and(|target| source.can_float_pane(&target))
                })
                .collect(),
            key,
            anchor: self.screen_motion.target,
        })
    }

    pub(super) fn prepare_screen_motion(
        &mut self,
        next: &WindowWorkspace,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.destination != Destination::Screens
            || cx.reduce_motion()
            || !window.is_window_active()
        {
            self.screen_motion.clear();
            return;
        }
        if self
            .screen_motion
            .outgoing
            .as_ref()
            .is_some_and(|outgoing| !outgoing.valid(next, cx))
        {
            self.screen_motion.clear();
        }
        let (Some(old), Some(new)) = (self.current_screen_key(), next.shown_key()) else {
            self.screen_motion.clear();
            return;
        };
        if old == new {
            return;
        }
        let opened = next.opened();
        let Some(old_ix) = opened.iter().position(|entry| entry.key() == &old) else {
            self.screen_motion.clear();
            return;
        };
        let Some(new_ix) = opened.iter().position(|entry| entry.key() == &new) else {
            self.screen_motion.clear();
            return;
        };
        let Some(outgoing) = self
            .capture_screen(cx)
            .filter(|outgoing| outgoing.valid(next, cx))
        else {
            self.screen_motion.clear();
            return;
        };
        let direction = if new_ix > old_ix { 1. } else { -1. };
        // Reversal reuses both anchors and the Kit channel's sampled position.
        // A third Screen replaces, rather than chains, the previous outgoing one.
        self.screen_motion.target = self
            .screen_motion
            .outgoing
            .as_ref()
            .filter(|previous| previous.key == new)
            .map_or(self.screen_motion.sampled + direction, |previous| {
                previous.anchor
            });
        self.screen_motion.outgoing = Some(outgoing);
    }

    pub(super) fn sample_screen_motion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let valid = self.destination == Destination::Screens
            && !cx.reduce_motion()
            && self.model.as_ref().is_some_and(|model| {
                self.screen_motion.outgoing.as_ref().is_none_or(|outgoing| {
                    outgoing.valid(model, cx)
                        && self
                            .screen_viewport
                            .is_some_and(|bounds| bounds.size == outgoing.viewport.size)
                })
            });
        if !valid {
            self.screen_motion.clear();
        }
        let motion = transition_with_status(
            "screen-position",
            self.screen_motion.target,
            Transition::new(if self.screen_motion.outgoing.is_some() {
                Duration::from_millis(200)
            } else {
                Duration::ZERO
            })
            .easing(Easing::EaseOut),
            window,
            cx,
        );
        self.screen_motion.sampled = motion.value;
        if matches!(motion.status, MotionStatus::Finished | MotionStatus::Idle) {
            self.screen_motion.outgoing = None;
        }
    }

    pub(super) fn screen_offset(&self) -> Pixels {
        self.screen_viewport.map_or(px(0.), |viewport| {
            viewport.size.height * (self.screen_motion.target - self.screen_motion.sampled)
        })
    }

    pub(super) fn render_outgoing_screen(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let outgoing = self.screen_motion.outgoing.as_ref()?;
        let projection = ScreenProjection::Presentation(outgoing);
        let root = outgoing.model.presentation_root()?;
        let snapshot = outgoing.model.snapshot()?;
        let surface = if outgoing
            .model
            .maximized_slot_id()
            .is_some_and(|slot| slot_by_id(&snapshot.root, slot).is_none())
        {
            Surface::Floating
        } else {
            Surface::Tiled
        };
        let mut layer = div()
            .id("outgoing-screen")
            .when(cfg!(all(test, feature = "ui-test-support")), |layer| {
                layer.debug_selector(|| "outgoing-screen".into())
            })
            .absolute()
            .left_0()
            .top(outgoing.viewport.size.height * (outgoing.anchor - self.screen_motion.sampled))
            .w(outgoing.viewport.size.width)
            .h(outgoing.viewport.size.height)
            .overflow_hidden()
            .p_1()
            .pt_0()
            .bg(rgb(self.appearance.surface))
            .child(self.render_node(root, surface, &projection, window, cx))
            .children(Self::presented_terminals(root, outgoing));
        let mut floats: Vec<_> = outgoing.model.presented_floats().iter().collect();
        floats.sort_by_key(|entry| {
            outgoing
                .floats
                .iter()
                .position(|id| id == entry.id.as_str())
                .unwrap_or(usize::MAX)
        });
        for entry in floats {
            layer = layer.child(self.render_float(entry, &projection, window, cx));
        }
        Some(layer.into_any_element())
    }

    pub(super) fn render_presented_slot(
        &self,
        node: &LayoutNode,
        surface: Surface,
        projection: &ScreenProjection<'_>,
        cx: &App,
    ) -> Option<AnyElement> {
        let ScreenProjection::Presentation(outgoing) = projection else {
            return None;
        };
        let LayoutNode::Slot { id, pane_ids, .. } = node else {
            return None;
        };
        let selected = outgoing.model.selected_pane(node);
        let focused = outgoing
            .model
            .focused_pane()
            .is_some_and(|pane| pane_ids.iter().any(|id| id.as_str() == pane));
        let (strip_width, offset) = outgoing
            .tab_strips
            .get(id.as_str())
            .copied()
            .unwrap_or_default();
        let mut tabs = div()
            .flex()
            .flex_none()
            .h_full()
            .relative()
            .left(offset.x)
            .gap_1();
        for (ix, pane) in pane_ids.iter().enumerate() {
            let title = outgoing
                .model
                .snapshot()
                .and_then(|screen| screen.panes.get(pane))
                .and_then(|pane| pane.title.clone())
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| format!("Terminal {}", ix + 1));
            tabs = tabs.child(
                tab_surface(&self.appearance, selected == Some(pane.as_str()))
                    .child(Icon::new(IconName::SquareTerminal).xsmall())
                    .child(tab_label(title))
                    .child(passive_icon(IconName::X, true, !outgoing.ready, cx)),
            );
        }
        let mut new_tab = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .px_1()
            .child(passive_icon(IconName::Plus, false, !outgoing.ready, cx));
        if !outgoing.remote {
            new_tab = new_tab.child(passive_icon(
                IconName::ChevronDown,
                false,
                !outgoing.ready,
                cx,
            ));
        }
        let mut actions = div().flex().flex_none().items_center().gap_1().px_1();
        if surface == Surface::Tiled {
            let maximized = outgoing.model.maximized_slot_id() == Some(id.as_str());
            let compact = chrome::compact_group_actions(
                outgoing.slot_widths.get(id.as_str()).copied(),
                self.appearance.zoom.window_scale(),
            );
            if compact && !outgoing.remote {
                actions = actions.child(passive_icon(IconName::Ellipsis, false, false, cx));
            } else {
                for icon in [
                    IconName::SquareSplitHorizontal,
                    IconName::SquareSplitVertical,
                ] {
                    actions = actions.child(passive_icon(icon, false, !outgoing.ready, cx));
                }
                if !outgoing.remote {
                    actions = actions.child(passive_icon(
                        IconName::PanelsTopLeft,
                        false,
                        !selected.is_some_and(|pane| outgoing.floatable.contains(pane)),
                        cx,
                    ));
                    if !maximized {
                        actions = actions.child(passive_icon(
                            IconName::Maximize,
                            false,
                            !outgoing.ready,
                            cx,
                        ));
                    }
                }
            }
            if maximized && !outgoing.remote {
                actions = actions
                    .child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(chrome::maximized_chrome_copy(compact).0),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .h_6()
                            .min_w_6()
                            .px_1p5()
                            .line_height(relative(1.25))
                            .text_sm()
                            .border_1()
                            .border_color(cx.theme().input)
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().input_background())
                            .text_color(cx.theme().button_foreground)
                            .child(Icon::new(IconName::Minimize).small())
                            .child(chrome::maximized_chrome_copy(compact).1),
                    );
            }
        }
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .border_1()
                .border_color(rgb(if focused {
                    self.appearance.border
                } else {
                    self.appearance.subtle_border
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_none()
                        .h(chrome_rems((self.appearance.font_size + 19.).max(32.)))
                        .bg(rgb(self.appearance.raised))
                        .child(
                            div()
                                .flex_initial()
                                .w(strip_width)
                                .min_w_0()
                                .h_full()
                                .overflow_hidden()
                                .child(tabs),
                        )
                        .child(new_tab)
                        .child(div().flex_1().min_w_0())
                        .child(actions),
                )
                .into_any_element(),
        )
    }

    fn presented_terminals(node: &LayoutNode, outgoing: &ScreenPresentation) -> Vec<AnyElement> {
        match node {
            LayoutNode::Slot { .. } => outgoing
                .model
                .selected_pane(node)
                .and_then(|pane| outgoing.panes.get(pane))
                .map(|(_, terminal)| {
                    let bounds = terminal.bounds();
                    vec![div()
                        .absolute()
                        .left(bounds.left() - outgoing.origin.x)
                        .top(bounds.top() - outgoing.origin.y)
                        .child(terminal.clone())
                        .into_any_element()]
                })
                .unwrap_or_default(),
            LayoutNode::Split { children, .. } => children
                .iter()
                .flat_map(|node| Self::presented_terminals(node, outgoing))
                .collect(),
        }
    }

    pub(super) fn render_presented_float(
        &self,
        entry: &splitty_client_core::workspace::FloatingEntry,
        projection: &ScreenProjection<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ScreenProjection::Presentation(outgoing) = projection else {
            return None;
        };
        let bounds = floating::project(
            &entry.bounds,
            outgoing.viewport,
            self.appearance.zoom.window_scale(),
        );
        let group = div()
            .absolute()
            .left(bounds.left() - outgoing.viewport.left())
            .top(bounds.top() - outgoing.viewport.top())
            .w(bounds.size.width)
            .h(bounds.size.height)
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_2()
            .border_color(rgb(self.appearance.accent))
            .bg(rgb(self.appearance.surface))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .h(chrome_rems((self.appearance.font_size + 19.).max(32.)))
                    .px_2()
                    .bg(rgb(self.appearance.raised))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child("Floating group"),
                    )
                    .child(passive_icon(IconName::PanelsTopLeft, false, false, cx))
                    .child(passive_icon(IconName::Maximize, false, false, cx)),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(self.render_node(
                &entry.root,
                Surface::Floating,
                projection,
                window,
                cx,
            )))
            .into_any_element();
        Some(
            div()
                .absolute()
                .inset_0()
                .child(group)
                .children(Self::presented_terminals(&entry.root, outgoing))
                .into_any_element(),
        )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod view_tests {
    use super::{Command, ScreenKey, WindowWorkspace, Worker, WorkerCommand, Workspace};
    use gpui_kit::{point, px, AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{atomic::AtomicBool, Arc},
        time::Duration,
    };
    use tokio::sync::mpsc;
    use uuid::Uuid;

    fn with_view(
        run: impl FnOnce(
            &mut VisualTestContext,
            Entity<Workspace>,
            Vec<ScreenKey>,
            mpsc::Receiver<WorkerCommand>,
        ),
    ) {
        let mut app = TestAppContext::single();
        app.update(gpui_kit::init);
        let content = Rc::new(RefCell::new(None));
        let (_, window) = app.add_window_view({
            let content = content.clone();
            move |window, cx| {
                let view = cx.new(|cx| {
                    let (view, worker) = Workspace::new_with_worker(
                        false,
                        Arc::new(AtomicBool::new(false)),
                        window,
                        cx,
                        |_, _, _| Worker::ui_test_fixture(),
                    );
                    worker.shutdown().unwrap();
                    view
                });
                *content.borrow_mut() = Some(view.clone());
                gpui_kit::base::Root::new(view, window, cx)
            }
        });
        let view = content.borrow_mut().take().unwrap();
        let mut model = WindowWorkspace::new(Uuid::new_v4());
        let keys = ["me@192.0.2.1", "me@192.0.2.2", "me@192.0.2.3"]
            .into_iter()
            .map(|target| model.ui_test_remote_terminal_fixture(target).unwrap().0)
            .collect::<Vec<_>>();
        for key in &keys {
            model.source_mut(key.source_id()).unwrap().focus_pane("p0");
        }
        model.select(model.lifecycle_token(), &keys[0]).unwrap();
        let (commands, mut receiver) = mpsc::channel(512);
        window.update(|window, _| window.activate_window());
        window.run_until_parked();
        window.update(|window, cx| {
            cx.set_reduce_motion(true);
            view.update(cx, |view, cx| {
                view.commands = commands;
                view.reconcile(model, window, cx);
            });
            window.draw(cx).clear(cx);
            cx.set_reduce_motion(false);
            window.draw(cx).clear(cx);
        });
        while receiver.try_recv().is_ok() {}
        run(window, view, keys, receiver);
    }

    fn select(window: &mut VisualTestContext, view: &Entity<Workspace>, key: &ScreenKey) {
        window.update(|window, cx| {
            view.update(cx, |view, cx| {
                let mut model = view.model.clone().unwrap();
                model.select(model.lifecycle_token(), key).unwrap();
                view.reconcile(model, window, cx);
            });
            window.draw(cx).clear(cx);
        });
    }

    fn advance(window: &mut VisualTestContext, millis: u64) {
        window
            .executor()
            .advance_clock(Duration::from_millis(millis));
        window.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[test]
    fn screen_slide_keeps_outgoing_panes_passive_and_routes_only_current_input() {
        with_view(|window, view, keys, mut receiver| {
            let old_terminal = window.update(|_, cx| {
                let view = view.read(cx);
                view.panes
                    .get(
                        &view
                            .model
                            .as_ref()
                            .unwrap()
                            .source(keys[0].source_id())
                            .unwrap()
                            .pane_key("p0"),
                    )
                    .unwrap()
                    .terminal
                    .clone()
            });
            let old_frames = window.update(|_, cx| old_terminal.read(cx).frames);
            select(window, &view, &keys[1]);
            advance(window, 70);
            let outgoing = window
                .debug_bounds("outgoing-screen")
                .expect("outgoing layer");
            let incoming = window
                .debug_bounds("workspace-viewport")
                .expect("current layer");
            assert!(outgoing.top() < incoming.top());
            window.update(|_, cx| {
                let view = view.read(cx);
                assert_eq!(view.screen_motion.outgoing.as_ref().unwrap().key, keys[0]);
                assert_eq!(view.visible_screen.as_ref(), Some(&keys[1]));
                assert!(view
                    .visible
                    .iter()
                    .all(|pane| pane.source_id() == keys[1].source_id()));
                assert_eq!(old_terminal.read(cx).frames, old_frames);
            });
            while let Ok(command) = receiver.try_recv() {
                match command.kind() {
                    Command::Visible { screen, .. } => assert_eq!(screen.as_ref(), Some(&keys[1])),
                    Command::ResizePane { screen, .. } => assert_eq!(screen, &keys[1]),
                    _ => panic!("motion submitted an unrelated command"),
                }
            }
            window.simulate_input("x");
            let command = receiver.try_recv().expect("current terminal input");
            match command.kind() {
                Command::Input { route, .. } => {
                    assert_eq!(route.source_token().source_id(), keys[1].source_id())
                }
                _ => panic!("current terminal input was not routed"),
            }
            // The old Screen is still visible above the incoming one. Parent
            // focus routing keeps typing on the current pane without giving
            // the passive layer any handlers or command authority.
            window.simulate_click(
                point(incoming.left() + px(40.), incoming.top() - px(30.)),
                Modifiers::default(),
            );
            assert!(receiver.try_recv().is_err());
            window.simulate_input("y");
            assert!(
                matches!(receiver.try_recv().unwrap().kind(), Command::Input { route, .. } if route.source_token().source_id() == keys[1].source_id())
            );
            advance(window, 210);
            window.simulate_input("z");
            assert!(
                matches!(receiver.try_recv().unwrap().kind(), Command::Input { route, .. } if route.source_token().source_id() == keys[1].source_id())
            );
            window.update(|_, cx| {
                let view = view.read(cx);
                assert!(view.screen_motion.outgoing.is_none());
                assert_eq!(view.screen_offset(), px(0.));
            });
        });
    }

    #[test]
    fn screen_slide_reverses_from_displayed_positions_and_bounds_retention() {
        with_view(|window, view, keys, mut receiver| {
            select(window, &view, &keys[1]);
            advance(window, 70);
            let a = window.debug_bounds("outgoing-screen").unwrap();
            let b = window.debug_bounds("workspace-viewport").unwrap();
            select(window, &view, &keys[0]);
            let reversed_a = window.debug_bounds("workspace-viewport").unwrap();
            let reversed_b = window.debug_bounds("outgoing-screen").unwrap();
            assert!((f32::from(a.top() - reversed_a.top())).abs() < 0.5);
            assert!((f32::from(b.top() - reversed_b.top())).abs() < 0.5);
            advance(window, 20);
            assert!(window.debug_bounds("workspace-viewport").unwrap().top() > reversed_a.top());
            select(window, &view, &keys[2]);
            window.update(|_, cx| {
                let view = view.read(cx);
                assert_eq!(view.screen_motion.outgoing.as_ref().unwrap().key, keys[0]);
                assert_eq!(view.visible_screen.as_ref(), Some(&keys[2]));
            });
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                window.draw(cx).clear(cx);
                let view = view.read(cx);
                assert!(view.screen_motion.outgoing.is_none());
                assert_eq!(view.screen_offset(), px(0.));
            });
            while let Ok(command) = receiver.try_recv() {
                assert!(matches!(
                    command.kind(),
                    Command::Visible { .. } | Command::ResizePane { .. }
                ));
            }
        });
    }

    #[test]
    fn screen_slide_drops_removed_replaced_and_departed_sources() {
        for invalidation in 0..3 {
            with_view(|window, view, keys, _| {
                select(window, &view, &keys[1]);
                advance(window, 70);
                window.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        assert!(view.screen_motion.outgoing.is_some());
                        if invalidation == 2 {
                            view.show_settings(window, cx);
                        } else {
                            let mut model = view.model.clone().unwrap();
                            if invalidation == 0 {
                                model.detach(model.lifecycle_token(), &keys[0]).unwrap();
                            } else {
                                model
                                    .ui_test_remote_terminal_fixture("me@192.0.2.1")
                                    .unwrap();
                                model.select(model.lifecycle_token(), &keys[1]).unwrap();
                            }
                            view.reconcile(model, window, cx);
                        }
                        assert!(view.screen_motion.outgoing.is_none());
                    });
                    window.draw(cx).clear(cx);
                });
            });
        }
    }
}
