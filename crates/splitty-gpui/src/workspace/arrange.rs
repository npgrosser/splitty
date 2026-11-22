//! Arranging the panes of the shown Screen from the keyboard: arrows move a
//! split edge of the focused group, Shift with an arrow moves its pane into
//! the neighboring group.
use super::*;

/// The share of a split's extent one arrow press moves its edge by.
const STEP_SHARE: f64 = 0.05;
/// The fewest terminal cells one arrow press moves an edge by.
const STEP_MIN_CELLS: f32 = 2.;
/// A terminal cell's width and height in units of its font size.
const CELL_EM: (f32, f32) = (0.6, 1.3);

/// The line the window shows while the mode is on.
const HINT: &str = "Arrange: arrows resize · Shift+arrows move · Esc to finish";

/// The pane the mode acts on, on the Screen it was started on.
pub(super) struct ArrangeMode {
    source: SourceToken,
    screen: ScreenKey,
    pane: String,
    /// The last resize sent: the split, its sizes in the model then, and the
    /// sizes sent. Presses before the model has them build on the sent ones.
    sent: Option<(LayoutKey, Vec<f64>, Vec<f64>)>,
}

/// The split edge an arrow press moves: the split's ID and the index of the
/// child left of (or above) the edge.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct SplitEdge {
    pub node: String,
    pub index: usize,
}

/// The edge of the group `slot` that an arrow toward `direction` moves:
/// the nearest edge on that side, or, where the group has no neighbor on
/// that side, the nearest one on the opposite side.
pub(super) fn arrange_edge(
    root: &LayoutNode,
    slot: &str,
    direction: Direction,
) -> Option<SplitEdge> {
    fn path<'a>(node: &'a LayoutNode, slot: &str, out: &mut Vec<(&'a LayoutNode, usize)>) -> bool {
        match node {
            LayoutNode::Slot { id, .. } => id.as_str() == slot,
            LayoutNode::Split { children, .. } => {
                children.iter().enumerate().any(|(index, child)| {
                    out.push((node, index));
                    path(child, slot, out) || {
                        out.pop();
                        false
                    }
                })
            }
        }
    }
    let mut ancestors = Vec::new();
    if !path(root, slot, &mut ancestors) {
        return None;
    }
    let (axis, forward) = match direction {
        Direction::Left => (Axis::Horizontal, false),
        Direction::Right => (Axis::Horizontal, true),
        Direction::Up => (Axis::Vertical, false),
        Direction::Down => (Axis::Vertical, true),
    };
    let along = ancestors
        .iter()
        .rev()
        .filter_map(|(node, index)| match node {
            LayoutNode::Split {
                id,
                axis: split_axis,
                children,
                ..
            } if *split_axis == axis => Some((id.as_str(), *index, children.len())),
            _ => None,
        });
    let toward = |(id, index, count): (&str, usize, usize), forward: bool| {
        if forward {
            (index + 1 < count).then(|| SplitEdge {
                node: id.to_owned(),
                index,
            })
        } else {
            (index > 0).then(|| SplitEdge {
                node: id.to_owned(),
                index: index - 1,
            })
        }
    };
    along
        .clone()
        .find_map(|entry| toward(entry, forward))
        .or_else(|| along.clone().find_map(|entry| toward(entry, !forward)))
}

/// The share of a split's extent one press moves an edge by: `STEP_SHARE`,
/// and never less than `STEP_MIN_CELLS` cells of `cell` pixels.
pub(super) fn arrange_step(extent: f32, cell: f32) -> f64 {
    if !(extent.is_finite() && extent > 0. && cell.is_finite() && cell > 0.) {
        return STEP_SHARE;
    }
    STEP_SHARE.max(f64::from(STEP_MIN_CELLS * cell / extent))
}

impl Workspace {
    /// Start arranging the focused pane of the shown Screen by keyboard.
    pub(super) fn start_arrange(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.arrange.take().is_some() {
            cx.notify();
            return;
        }
        if self.destination != Destination::Screens {
            return;
        }
        let Some(screen) = self.current_screen_key() else {
            return;
        };
        let Some(model) = self.selected_model() else {
            return;
        };
        let Some(pane) = model.focused_pane() else {
            return;
        };
        self.arrange = Some(ArrangeMode {
            source: model.source_token(),
            screen,
            pane: pane.to_owned(),
            sent: None,
        });
        cx.notify();
    }

    /// Whether the mode still has its pane: the same Screen is shown with
    /// the pane focused, and nothing else took the window's keys.
    fn arrange_holds(&self) -> bool {
        let Some(mode) = &self.arrange else {
            return false;
        };
        self.destination == Destination::Screens
            && self.palette.is_none()
            && self.template_dialog.is_none()
            && self.terminal_menu.is_none()
            && self.tab_context_menu.is_none()
            && self.rename.is_none()
            && self.profile_picker.is_none()
            && self.current_screen_key().as_ref() == Some(&mode.screen)
            && self.selected_model().is_some_and(|model| {
                model.source_token() == mode.source
                    && model.focused_pane() == Some(mode.pane.as_str())
                    && model.focused_slot_id().is_some()
            })
    }

    /// End the mode where it lost its pane; true while it holds.
    pub(super) fn sync_arrange(&mut self) -> bool {
        if self.arrange.is_some() && !self.arrange_holds() {
            self.arrange = None;
        }
        self.arrange.is_some()
    }

    /// The line the window shows while the mode is on.
    pub(super) fn arrange_hint(&mut self) -> Option<String> {
        self.sync_arrange().then(|| HINT.to_owned())
    }

    /// The accent outline of the group the mode acts on, over the group.
    pub(super) fn render_arrange_mark(&self, focused: bool) -> Option<AnyElement> {
        if !focused || self.arrange.is_none() {
            return None;
        }
        let width = px(if self.appearance.bevel.is_some() {
            Relief::Etched.width()
        } else {
            2.
        });
        Some(
            bordered(div(), width)
                .id("arrange-mark")
                .when(cfg!(all(test, feature = "ui-test-support")), |mark| {
                    mark.debug_selector(|| "arrange-mark".into())
                })
                .absolute()
                .inset_0()
                .border_color(rgb(self.appearance.accent))
                .into_any_element(),
        )
    }

    /// A key while the mode is on. Returns whether the mode took it; then no
    /// other binding runs and the terminal does not see it.
    pub(super) fn arrange_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.sync_arrange() {
            cx.notify();
            return false;
        }
        if self.scratchpad_focused(window, cx) {
            // A text field took the keyboard: its typing is not the mode's.
            self.end_arrange(cx);
            return false;
        }
        cx.stop_propagation();
        let key = &event.keystroke;
        if self.key_command(key, window, cx) == Some(KeyCommand::ArrangePanes) {
            self.end_arrange(cx);
            return true;
        }
        let modifiers = key.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return true;
        }
        let direction = match key.key.as_str() {
            "escape" | "enter" => {
                self.end_arrange(cx);
                return true;
            }
            "left" => Direction::Left,
            "right" => Direction::Right,
            "up" => Direction::Up,
            "down" => Direction::Down,
            _ => return true,
        };
        if modifiers.shift {
            self.arrange_move(direction, cx);
        } else {
            self.arrange_resize(direction, cx);
        }
        true
    }

    fn end_arrange(&mut self, cx: &mut Context<Self>) {
        self.arrange = None;
        cx.notify();
    }

    /// Move the edge of the focused group toward `direction` by one step,
    /// as the end of a splitter drag does.
    fn arrange_resize(&mut self, direction: Direction, cx: &mut Context<Self>) {
        let Some((source, node, current, next)) = self.arrange_resize_sizes(direction) else {
            return;
        };
        if let Some(mode) = &mut self.arrange {
            mode.sent = Some((node.clone(), current, next.clone()));
        }
        let command = Command::ResizeSplit {
            screen: node.screen().clone(),
            node: node.node_id().to_owned(),
            sizes: next,
        };
        self.send_captured(source, command, cx);
    }

    /// The split a press resizes, its sizes in the model and its next sizes.
    fn arrange_resize_sizes(
        &self,
        direction: Direction,
    ) -> Option<(SourceToken, LayoutKey, Vec<f64>, Vec<f64>)> {
        let mode = self.arrange.as_ref()?;
        let model = self.selected_model()?;
        let root = model.presentation_root()?;
        let slot = model.focused_slot_id()?;
        let edge = arrange_edge(root, slot, direction)?;
        let node = mode.screen.layout_key(&edge.node);
        let (axis, _, current) = resize_split(model, &node)?;
        let sizes = match &mode.sent {
            Some((sent_node, before, sent)) if *sent_node == node && before == current => {
                sent.as_slice()
            }
            _ => current,
        };
        let horizontal = axis == Axis::Horizontal;
        let extent = self.split_bounds.get(&node).map(|bounds| {
            f32::from(if horizontal {
                bounds.size.width
            } else {
                bounds.size.height
            })
        });
        let cell = self.arrange_cell(horizontal);
        let share = extent.map_or(STEP_SHARE, |extent| arrange_step(extent, cell));
        let total: f64 = sizes.iter().sum();
        let delta = share
            * total
            * match direction {
                Direction::Right | Direction::Down => 1.,
                Direction::Left | Direction::Up => -1.,
            };
        let next = resized_pair(sizes, edge.index, delta)?;
        // At the minimum size the press changes nothing and sends nothing.
        (next.as_slice() != sizes).then(|| (model.source_token(), node, current.to_vec(), next))
    }

    /// The width or height of one terminal cell at the terminals' zoom.
    fn arrange_cell(&self, horizontal: bool) -> f32 {
        let size = self.appearance.terminal_font_size;
        let font = size * self.appearance.zoom.terminal_scale(size);
        font * if horizontal { CELL_EM.0 } else { CELL_EM.1 }
    }

    /// Move the focused pane into the neighboring group toward `direction`
    /// as a tab there, as a drop on that group's body does.
    fn arrange_move(&mut self, direction: Direction, cx: &mut Context<Self>) {
        match self.arrange_move_command(direction) {
            Ok(Some((source, command))) => self.send_captured(source, command, cx),
            Ok(None) => {}
            Err(error) => {
                self.status = error.to_string();
                cx.notify();
            }
        }
    }

    fn arrange_move_command(
        &self,
        direction: Direction,
    ) -> std::io::Result<Option<(SourceToken, Command)>> {
        let (Some(mode), Some(model)) = (&self.arrange, self.selected_model()) else {
            return Ok(None);
        };
        let (Some(root), Some(shown), Some(slot)) = (
            model.presentation_root(),
            model.shown(),
            model.focused_slot_id(),
        ) else {
            return Ok(None);
        };
        let mut slots = Vec::new();
        visible_slots(shown, &mode.screen, root, &self.slot_bounds, &mut slots);
        for entry in model.presented_floats() {
            visible_slots(
                shown,
                &mode.screen,
                &entry.root,
                &self.slot_bounds,
                &mut slots,
            );
        }
        let Some(target) = nearest_slot(&slots, slot, direction).map(|slot| slot.id.clone()) else {
            return Ok(None);
        };
        let Some(drag) = model.capture_drag(&mode.pane) else {
            return Ok(None);
        };
        if !model.check_move(&drag, &target, MovePlacement::Tab)? {
            return Ok(None);
        }
        Ok(Some((
            drag.source_token(),
            Command::MovePane {
                drag,
                target,
                placement: MovePlacement::Tab,
            },
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{arrange_edge, arrange_step, SplitEdge, STEP_SHARE};
    use splitty_client_core::keymap::Direction;
    use splitty_client_core::workspace::LayoutNode;

    /// `left | (top / bottom)`: one split side by side, one stacked.
    fn layout() -> LayoutNode {
        serde_json::from_value(serde_json::json!({
            "type":"split","id":"outer","axis":"horizontal","sizes":[0.5,0.5],"children":[
                {"type":"slot","id":"left","paneIds":["a"],"activePaneId":"a"},
                {"type":"split","id":"inner","axis":"vertical","sizes":[0.5,0.5],"children":[
                    {"type":"slot","id":"top","paneIds":["b"],"activePaneId":"b"},
                    {"type":"slot","id":"bottom","paneIds":["c"],"activePaneId":"c"}
                ]}
            ]
        }))
        .unwrap()
    }

    fn edge(node: &str, index: usize) -> Option<SplitEdge> {
        Some(SplitEdge {
            node: node.to_owned(),
            index,
        })
    }

    #[test]
    fn an_arrow_takes_the_nearest_edge_on_its_side_or_else_the_opposite_one() {
        let root = layout();
        // A neighbor on the arrow's side: the edge between them.
        assert_eq!(
            arrange_edge(&root, "left", Direction::Right),
            edge("outer", 0)
        );
        assert_eq!(
            arrange_edge(&root, "top", Direction::Down),
            edge("inner", 0)
        );
        assert_eq!(
            arrange_edge(&root, "top", Direction::Left),
            edge("outer", 0)
        );
        // None on that side: the edge on the other side moves the same way.
        assert_eq!(
            arrange_edge(&root, "left", Direction::Left),
            edge("outer", 0)
        );
        assert_eq!(
            arrange_edge(&root, "bottom", Direction::Down),
            edge("inner", 0)
        );
        assert_eq!(
            arrange_edge(&root, "top", Direction::Right),
            edge("outer", 0)
        );
        // No split along the arrow's axis, or no such group.
        assert_eq!(arrange_edge(&root, "left", Direction::Up), None);
        assert_eq!(arrange_edge(&root, "gone", Direction::Right), None);
    }

    #[test]
    fn a_step_is_a_share_of_the_split_and_at_least_two_cells() {
        assert_eq!(arrange_step(1000., 8.), STEP_SHARE);
        assert!((arrange_step(200., 8.) - 0.08).abs() < 1e-6);
        assert_eq!(arrange_step(0., 8.), STEP_SHARE);
        assert_eq!(arrange_step(f32::NAN, 8.), STEP_SHARE);
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod view_tests {
    use crate::workspace::Workspace;
    use crate::workspace::{chrome::view_tests::with_view, Command, Destination, WorkerCommand};
    use gpui_kit::{px, size, Entity, VisualTestContext};
    use splitty_client_core::workspace::{MovePlacement, ScreenKey, WindowWorkspace};
    use tokio::sync::mpsc;
    use uuid::Uuid;

    /// Two panes side by side on "work", with "left" focused, and a second
    /// Screen "other".
    fn model() -> (WindowWorkspace, ScreenKey) {
        let owner = Uuid::new_v4();
        let mut model = WindowWorkspace::new(owner);
        let keys = model
            .ui_test_screen_order_fixture(&[("work", "Work"), ("other", "Other")], None)
            .unwrap();
        let source = model.lifecycle_token();
        let mut workspace = model.source_mut(source.source_id()).unwrap();
        let connection = workspace.connection_id();
        workspace
            .apply_snapshot(
                connection,
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
        workspace.focus_pane("p0");
        drop(workspace);
        (model, keys[1].clone())
    }

    fn setup(
        window: &mut VisualTestContext,
        view: &Entity<Workspace>,
    ) -> (mpsc::Receiver<WorkerCommand>, ScreenKey) {
        let (sender, mut receiver) = mpsc::channel(128);
        let (model, other) = model();
        window.simulate_resize(size(px(800.), px(500.)));
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.commands = sender.into();
                workspace.destination = Destination::Screens;
                workspace.drawer_visible = false;
                workspace.drawer_overlay_open = false;
                workspace.reconcile(model, window, cx);
            });
            window.draw(cx).clear(cx);
            view.update(cx, |workspace, cx| workspace.focus_current(window, cx));
            window.draw(cx).clear(cx);
        });
        while receiver.try_recv().is_ok() {}
        (receiver, other)
    }

    fn press(
        window: &mut VisualTestContext,
        receiver: &mut mpsc::Receiver<WorkerCommand>,
        keys: &str,
    ) -> Vec<WorkerCommand> {
        window.simulate_keystrokes(keys);
        window.run_until_parked();
        let mut sent = Vec::new();
        while let Ok(command) = receiver.try_recv() {
            // Size reports and focus follow drawing, not the key.
            if matches!(
                command.kind(),
                Command::ResizeSplit { .. }
                    | Command::MovePane { .. }
                    | Command::Input { .. }
                    | Command::Split { .. }
            ) {
                sent.push(command);
            }
        }
        sent
    }

    /// The commands of `sent`, without their sources.
    fn kinds(sent: &[WorkerCommand]) -> Vec<&Command> {
        sent.iter().map(WorkerCommand::kind).collect()
    }

    /// The share of the left pane in the one resize `keys` sent.
    fn left_share(
        window: &mut VisualTestContext,
        receiver: &mut mpsc::Receiver<WorkerCommand>,
        keys: &str,
    ) -> f64 {
        let sent = press(window, receiver, keys);
        let [Command::ResizeSplit { node, sizes, .. }] = kinds(&sent)[..] else {
            panic!("one resize: {}", sent.len());
        };
        assert_eq!(node, "split");
        assert!((sizes[0] + sizes[1] - 1.).abs() < 1e-9);
        sizes[0]
    }

    fn arrange_key(window: &mut VisualTestContext) -> &'static str {
        if window.update(|_, cx| cx.platform_info().is_mac()) {
            "cmd-r"
        } else {
            "alt-r"
        }
    }

    fn arranging(window: &mut VisualTestContext, view: &Entity<Workspace>) -> bool {
        window.update(|_, cx| view.read(cx).arrange.is_some())
    }

    fn inputs(sent: &[WorkerCommand]) -> usize {
        sent.iter()
            .filter(|command| matches!(command.kind(), Command::Input { .. }))
            .count()
    }

    #[test]
    fn arrows_resize_and_shift_arrows_move_while_no_key_reaches_the_terminal() {
        with_view(|window, view| {
            let (mut receiver, _) = setup(window, &view);
            let start = arrange_key(window);
            assert!(press(window, &mut receiver, start).is_empty());
            assert!(arranging(window, &view));
            window.update(|window, cx| window.draw(cx).clear(cx));
            assert!(window.debug_bounds("arrange-mark").is_some());
            assert!(window.debug_bounds("workspace-status").is_some());
            let hint =
                window.update(|_, cx| view.update(cx, |workspace, _| workspace.arrange_hint()));
            assert!(hint.is_some_and(|hint| hint.contains("Esc")));

            // Right in the left pane grows it toward its neighbor, step by
            // step before the model has the first step.
            let first = left_share(window, &mut receiver, "right");
            let second = left_share(window, &mut receiver, "right");
            assert!(0.5 < first && first < second, "{first} {second}");
            // Left in the leftmost pane moves the same edge back.
            let back = left_share(window, &mut receiver, "left");
            assert!((back - first).abs() < 1e-9, "{back}");
            // No split along the arrow: nothing.
            assert!(press(window, &mut receiver, "up").is_empty());

            // Shift with an arrow moves the pane into the group on that side.
            let sent = press(window, &mut receiver, "shift-right");
            let sent = kinds(&sent);
            let [Command::MovePane {
                target, placement, ..
            }] = sent[..]
            else {
                panic!("one move: {}", sent.len());
            };
            assert_eq!((target.as_str(), *placement), ("right", MovePlacement::Tab));
            assert!(press(window, &mut receiver, "shift-left").is_empty());

            // Typing and other bindings reach neither the terminal nor a command.
            assert!(press(window, &mut receiver, "a ctrl-c alt-d").is_empty());
            assert!(arranging(window, &view));

            assert!(press(window, &mut receiver, "escape").is_empty());
            assert!(!arranging(window, &view));
            assert_eq!(inputs(&press(window, &mut receiver, "a ctrl-c")), 2);
        });
    }

    #[test]
    fn enter_the_arrange_key_and_switching_the_screen_end_the_mode() {
        with_view(|window, view| {
            let (mut receiver, other) = setup(window, &view);
            let start = arrange_key(window);
            for end in ["enter", start] {
                press(window, &mut receiver, start);
                assert!(arranging(window, &view));
                assert!(press(window, &mut receiver, end).is_empty());
                assert!(!arranging(window, &view), "{end}");
            }

            press(window, &mut receiver, start);
            assert!(arranging(window, &view));
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let model = workspace.model.as_mut().unwrap();
                    let owner = model.lifecycle_token();
                    model.select(owner, &other).unwrap();
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            assert!(!arranging(window, &view));
        });
    }
}
