//! Drawer Screen drag presentation. The lifted row follows the pointer
//! vertically while the other rows of its source group make room, as the web
//! drawer's sortable list does. The service keeps the canonical Screen order:
//! a drop submits one relative placement, and every offset here is transient
//! paint state that never reorders the model.
use super::*;
use std::panic::Location;

/// Vertical pointer travel before a pressed Screen row becomes a drag
/// (`SortableList` `threshold = 5`, measured on the vertical axis only).
const SCREEN_DRAG_ACTIVATION: f32 = 5.;
/// Opacity of the lifted Screen row (`Drawer.svelte`: `opacity: 0.5` while dragging).
pub(super) const SCREEN_DRAG_OPACITY: f32 = 0.5;
/// How long a dropped arrangement waits for the service's Screen order before
/// the rows return to the order the model holds. The web store reorders at
/// once; the native drawer has no optimistic order, so it holds the dropped
/// presentation instead of showing the old order for a moment.
const SCREEN_DROP_SETTLE: Duration = Duration::from_millis(1000);

/// Rows making room for the lifted one (`SortableList`:
/// `transform 200ms cubic-bezier(0.25, 1, 0.5, 1)`).
pub(super) fn screen_shift_transition() -> Transition {
    Transition::new(Duration::from_millis(200)).easing(Easing::CubicBezier {
        x1: 0.25,
        y1: 1.,
        x2: 0.5,
        y2: 1.,
    })
}

/// Lifting fade and its return on drop or cancel (`.drawer-screen`:
/// `transition: opacity 120ms ease`).
pub(super) fn screen_lift_transition() -> Transition {
    Transition::new(Duration::from_millis(120)).easing(Easing::Ease)
}

/// A Screen row's layout slot in list content coordinates (window position
/// minus the list's scroll offset), measured without any drag offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RowSlot {
    pub(super) top: f32,
    pub(super) height: f32,
}

impl RowSlot {
    fn center(self) -> f32 {
        self.top + self.height / 2.
    }
}

/// The index the lifted row takes once its center has moved by `delta`: it
/// passes a neighbor when its center crosses that neighbor's center, compared
/// against the slots measured when the drag began.
fn drop_index(slots: &[RowSlot], from: usize, delta: f32) -> usize {
    let Some(lifted) = slots.get(from) else {
        return from;
    };
    let center = lifted.center() + delta;
    let mut target = from;
    for (index, slot) in slots.iter().enumerate().skip(from + 1) {
        if center > slot.center() {
            target = index;
        } else {
            break;
        }
    }
    for index in (0..from).rev() {
        if center < slots[index].center() {
            target = index;
        } else {
            break;
        }
    }
    target
}

/// How far the other rows move to make room: the lifted row's height plus the
/// gap that separates neighboring rows, so they end where the new order puts them.
fn room(slots: &[RowSlot], from: usize) -> f32 {
    let Some(lifted) = slots.get(from) else {
        return 0.;
    };
    let gap = if let Some(next) = slots.get(from + 1) {
        next.top - lifted.top - lifted.height
    } else if let Some(previous) = from.checked_sub(1).and_then(|index| slots.get(index)) {
        lifted.top - previous.top - previous.height
    } else {
        0.
    };
    lifted.height + gap.max(0.)
}

/// Offset of a row that is not lifted while the lifted row is over `over`.
fn displaced_offset(slots: &[RowSlot], from: usize, over: usize, index: usize) -> f32 {
    if from < over && index > from && index <= over {
        -room(slots, from)
    } else if over < from && index >= over && index < from {
        room(slots, from)
    } else {
        0.
    }
}

/// Offset that puts the lifted row exactly into the gap the others left at `over`.
fn landing_offset(slots: &[RowSlot], from: usize, over: usize) -> f32 {
    let (Some(lifted), Some(target)) = (slots.get(from), slots.get(over)) else {
        return 0.;
    };
    if over > from {
        target.top + target.height - lifted.height - lifted.top
    } else {
        target.top - lifted.top
    }
}

/// What one Drawer row paints for the current Screen drag.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct RowMotion {
    /// Vertical paint offset from the row's layout slot.
    pub(super) offset: f32,
    /// Ease toward `offset` with the web list's shift curve; otherwise jump.
    pub(super) animate: bool,
    /// The row is the lifted one: faded and painted above its neighbors. Its
    /// controls cannot click on release: GPUI drops pending clicks while a
    /// drag is active, as the web list suppresses the click after a drag.
    pub(super) lifted: bool,
}

/// One Screen drag, from the press until the service's order arrives.
pub(super) struct ScreenDragSession {
    pub(super) owner: SourceToken,
    pub(super) screen: ScreenKey,
    /// The dragged Screen's source group in model order when the drag began.
    pub(super) group: Vec<ScreenKey>,
    slots: Vec<RowSlot>,
    from: usize,
    start: f32,
    delta: f32,
    over: usize,
    active: bool,
    /// Present after a submitted drop; dropping the session cancels the timer.
    settling: Option<Task<()>>,
}

impl ScreenDragSession {
    pub(super) fn new(
        owner: SourceToken,
        screen: ScreenKey,
        group: Vec<ScreenKey>,
        slots: Vec<RowSlot>,
        start: f32,
    ) -> Option<Self> {
        let from = group.iter().position(|key| key == &screen)?;
        (slots.len() == group.len()).then_some(Self {
            owner,
            screen,
            group,
            slots,
            from,
            start,
            delta: 0.,
            over: from,
            active: false,
            settling: None,
        })
    }

    pub(super) fn is_dragging(&self) -> bool {
        self.settling.is_none()
    }

    /// The drag passed its activation travel and the row follows the pointer.
    pub(super) fn is_lifted(&self) -> bool {
        self.is_dragging() && self.active
    }

    /// `screen` is the pressed row of a drag that has not ended.
    pub(super) fn holds(&self, screen: &ScreenKey) -> bool {
        self.is_dragging() && &self.screen == screen
    }

    /// Follows the pointer, given in list content coordinates. Returns whether
    /// the presentation changed.
    pub(super) fn track(&mut self, pointer: f32) -> bool {
        if !self.is_dragging() {
            return false;
        }
        let delta = pointer - self.start;
        if !self.active && delta.abs() < SCREEN_DRAG_ACTIVATION {
            return false;
        }
        let changed = !self.active || delta != self.delta;
        self.active = true;
        self.delta = delta;
        self.over = drop_index(&self.slots, self.from, delta);
        changed
    }

    /// The relative placement a release submits: `None` when the drag never
    /// activated or ends where it began; otherwise the Screen to insert before,
    /// with `None` inside meaning the end of the source group.
    pub(super) fn placement(&self) -> Option<Option<ScreenKey>> {
        if !self.is_lifted() || self.over == self.from {
            return None;
        }
        Some(if self.over < self.from {
            Some(self.group[self.over].clone())
        } else {
            self.group.get(self.over + 1).cloned()
        })
    }

    /// Keeps the dropped arrangement after its placement was submitted.
    pub(super) fn settle(mut self, timer: Task<()>) -> Self {
        self.settling = Some(timer);
        self
    }

    pub(super) fn motion(&self, screen: &ScreenKey) -> RowMotion {
        let Some(index) = self.group.iter().position(|key| key == screen) else {
            return RowMotion::default();
        };
        let dragging = self.is_dragging();
        if index == self.from {
            return match (dragging, self.active) {
                (true, true) => RowMotion {
                    offset: self.delta,
                    animate: false,
                    lifted: true,
                },
                (false, _) => RowMotion {
                    offset: landing_offset(&self.slots, self.from, self.over),
                    ..RowMotion::default()
                },
                (true, false) => RowMotion::default(),
            };
        }
        RowMotion {
            offset: if self.active {
                displaced_offset(&self.slots, self.from, self.over, index)
            } else {
                0.
            },
            animate: dragging,
            lifted: false,
        }
    }
}

/// The source group of `screen` in model order.
fn screen_group(model: &WindowWorkspace, screen: &ScreenKey) -> Vec<ScreenKey> {
    model
        .opened()
        .iter()
        .filter(|entry| entry.key().source_id() == screen.source_id())
        .map(|entry| entry.key().clone())
        .collect()
}

impl Workspace {
    /// Records where a Screen row was pressed, in list content coordinates, so
    /// the lifted row keeps the grabbed point under the pointer.
    pub(super) fn press_screen_row(&mut self, screen: &ScreenKey, position: Point<Pixels>) {
        let scroll = self.drawer_scroll.offset().y;
        self.drawer_press = Some((screen.clone(), f32::from(position.y - scroll)));
    }

    pub(super) fn begin_screen_drag(
        &mut self,
        owner: SourceToken,
        screen: &ScreenKey,
        position: Point<Pixels>,
    ) {
        self.drawer_drag = None;
        let press = self.drawer_press.take();
        let Some(model) = self.model.as_ref() else {
            return;
        };
        if model.lifecycle_token() != owner || !model.can_move_screen(screen) {
            return;
        }
        let group = screen_group(model, screen);
        let slots = {
            let measured = self.drawer_row_slots.borrow();
            group
                .iter()
                .map(|key| measured.get(key).copied())
                .collect::<Option<Vec<_>>>()
        };
        let Some(slots) = slots else {
            return;
        };
        let start = press
            .filter(|(pressed, _)| pressed == screen)
            .map(|(_, y)| y)
            .unwrap_or_else(|| f32::from(position.y - self.drawer_scroll.offset().y));
        self.drawer_drag = ScreenDragSession::new(owner, screen.clone(), group, slots, start);
    }

    pub(super) fn track_screen_drag(
        &mut self,
        event: &DragMoveEvent<ScreenDrag>,
        cx: &mut Context<Self>,
    ) {
        let drag = event.drag(cx);
        if drag.view != cx.entity().entity_id() {
            return;
        }
        let pointer = f32::from(event.event.position.y - self.drawer_scroll.offset().y);
        let Some(session) = self.drawer_drag.as_mut().filter(|session| {
            session.is_dragging() && session.owner == drag.owner && session.screen == drag.screen
        }) else {
            return;
        };
        if session.track(pointer) {
            cx.notify();
        }
    }

    /// A release anywhere ends the drag. A moved row submits its relative
    /// placement and keeps the dropped arrangement until the service's order
    /// arrives; anything else returns the rows to their slots at once.
    pub(super) fn finish_screen_drag(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.drawer_drag.take_if(|session| session.is_dragging()) else {
            return;
        };
        cx.notify();
        let Some(before) = session.placement() else {
            return;
        };
        let current = self
            .model
            .as_ref()
            .filter(|model| model.lifecycle_token() == session.owner)
            .map(|model| screen_group(model, &session.screen));
        if current.as_ref() != Some(&session.group)
            || !self.place_screen(session.owner, session.screen.clone(), before, cx)
        {
            return;
        }
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SCREEN_DROP_SETTLE).await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .drawer_drag
                    .take_if(|session| !session.is_dragging())
                    .is_some()
                {
                    cx.notify();
                }
            });
        });
        self.drawer_drag = Some(session.settle(timer));
    }

    /// Ends a drag whose geometry or authority is no longer current. A held
    /// drop is not a drag and stays until its order arrives.
    pub(super) fn cancel_screen_drag(&mut self) -> bool {
        self.drawer_drag
            .take_if(|session| session.is_dragging())
            .is_some()
    }

    /// A model update keeps a drag only while its owner and group order stay;
    /// a held drop ends as soon as the group's order changes, which is the
    /// service's answer to the submitted placement.
    pub(super) fn reconcile_screen_drag(
        &mut self,
        next: &WindowWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.drawer_drag.as_ref() else {
            return;
        };
        let current = next.lifecycle_token() == session.owner
            && screen_group(next, &session.screen) == session.group
            && (!session.is_dragging() || next.can_move_screen(&session.screen));
        if current {
            return;
        }
        if self
            .drawer_drag
            .take()
            .is_some_and(|session| session.is_dragging())
        {
            cx.stop_active_drag(window);
        }
        cx.notify();
    }

    /// A drag can end without the Drawer seeing the release, for example when
    /// the Drawer was not painted; the platform drag is then already gone.
    pub(super) fn expire_screen_drag(&mut self, cx: &App) {
        if !cx.has_active_drag() {
            self.cancel_screen_drag();
        }
    }
}

/// Paints its child after the rest of the window, as `deferred` does, while
/// keeping its parent's clip, so the lifted row passes over its neighbors yet
/// stays inside the scrolled Screen list.
pub(super) struct RaisedInClip {
    child: Option<AnyElement>,
}

impl RaisedInClip {
    pub(super) fn new(child: impl IntoElement) -> Self {
        Self {
            child: Some(child.into_any_element()),
        }
    }
}

impl IntoElement for RaisedInClip {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RaisedInClip {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = self.child.as_mut().expect("raised child before layout");
        (child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        let child = self.child.take().expect("raised child prepaints once");
        let offset = window.element_offset();
        let clip = window.content_mask();
        window.defer_draw(child, offset, 0, Some(clip));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::{
        displaced_offset, drop_index, landing_offset, room, RowMotion, RowSlot, ScreenDragSession,
    };
    use gpui_kit::Task;
    use splitty_client_core::workspace::{ScreenKey, WindowWorkspace};
    use uuid::Uuid;

    /// Three 30 px rows separated by 4 px, as the Drawer lays out Screens
    /// without panes.
    fn slots() -> Vec<RowSlot> {
        (0..3)
            .map(|index| RowSlot {
                top: 10. + 34. * index as f32,
                height: 30.,
            })
            .collect()
    }

    #[test]
    fn lifted_row_passes_a_neighbor_when_its_center_crosses_the_neighbors_center() {
        let slots = slots();
        assert_eq!(drop_index(&slots, 0, 0.), 0);
        assert_eq!(drop_index(&slots, 0, 34.), 0, "equal centers do not pass");
        assert_eq!(drop_index(&slots, 0, 34.5), 1);
        assert_eq!(drop_index(&slots, 0, 68.5), 2);
        assert_eq!(drop_index(&slots, 0, 500.), 2, "the group end bounds it");
        assert_eq!(drop_index(&slots, 2, -34.5), 1);
        assert_eq!(drop_index(&slots, 2, -68.5), 0);
        assert_eq!(drop_index(&slots, 2, -500.), 0);
        assert_eq!(drop_index(&slots, 1, 20.), 1);
        assert_eq!(drop_index(&slots, 1, -20.), 1);
        assert_eq!(drop_index(&slots, 7, 40.), 7, "an unknown row stays put");
    }

    #[test]
    fn rows_between_origin_and_target_make_room_by_the_lifted_rows_pitch() {
        let slots = slots();
        assert_eq!(room(&slots, 0), 34.);
        assert_eq!(room(&slots, 2), 34., "the last row uses the gap above it");
        assert_eq!(
            (0..3)
                .map(|index| displaced_offset(&slots, 0, 2, index))
                .collect::<Vec<_>>(),
            [0., -34., -34.]
        );
        assert_eq!(
            (0..3)
                .map(|index| displaced_offset(&slots, 2, 0, index))
                .collect::<Vec<_>>(),
            [34., 34., 0.]
        );
        assert_eq!(
            (0..3)
                .map(|index| displaced_offset(&slots, 1, 1, index))
                .collect::<Vec<_>>(),
            [0., 0., 0.],
            "an unmoved drag displaces nothing"
        );
    }

    #[test]
    fn landing_puts_the_lifted_row_into_the_gap_for_rows_of_any_height() {
        let slots = slots();
        assert_eq!(landing_offset(&slots, 0, 2), 68.);
        assert_eq!(landing_offset(&slots, 2, 0), -68.);
        assert_eq!(landing_offset(&slots, 1, 1), 0.);
        // A tall Screen with panes moving below two short ones lands at their
        // former bottom, and they move up by its height plus the gap.
        let mixed = [
            RowSlot {
                top: 0.,
                height: 72.,
            },
            RowSlot {
                top: 76.,
                height: 30.,
            },
            RowSlot {
                top: 110.,
                height: 30.,
            },
        ];
        assert_eq!(displaced_offset(&mixed, 0, 2, 1), -76.);
        assert_eq!(landing_offset(&mixed, 0, 2), 68.);
        assert_eq!(
            mixed[2].top + displaced_offset(&mixed, 0, 2, 2) + mixed[2].height + 4.,
            mixed[0].top + landing_offset(&mixed, 0, 2),
            "the lifted row follows the last displaced row after one gap"
        );
    }

    fn press(from: usize) -> (ScreenDragSession, Vec<ScreenKey>) {
        let model = WindowWorkspace::new(Uuid::new_v4());
        let keys: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|id| model.local_source().screen_key(id))
            .collect();
        // Pressed at the lifted row's center.
        let start = slots()[from].top + 15.;
        let session = ScreenDragSession::new(
            model.lifecycle_token(),
            keys[from].clone(),
            keys.clone(),
            slots(),
            start,
        )
        .unwrap();
        (session, keys)
    }

    fn offsets(session: &ScreenDragSession, keys: &[ScreenKey]) -> Vec<f32> {
        keys.iter().map(|key| session.motion(key).offset).collect()
    }

    #[test]
    fn a_press_lifts_only_after_five_pixels_of_vertical_travel() {
        let (mut session, keys) = press(2);
        let start = slots()[2].top + 15.;
        assert!(!session.track(start - 4.9));
        assert!(!session.is_lifted());
        assert_eq!(session.placement(), None, "an unlifted release is a click");
        assert_eq!(offsets(&session, &keys), [0., 0., 0.]);
        assert!(keys.iter().all(|key| !session.motion(key).lifted));
        assert!(session.track(start - 5.));
        assert!(session.is_lifted());
        assert_eq!(
            session.motion(&keys[2]),
            RowMotion {
                offset: -5.,
                animate: false,
                lifted: true,
            },
            "the lifted row follows the pointer without easing"
        );
        assert!(
            !session.track(start - 5.),
            "an unchanged pointer is no change"
        );
        assert!(
            session.track(start - 1.),
            "once lifted, small travel follows"
        );
        assert_eq!(session.motion(&keys[2]).offset, -1.);
    }

    #[test]
    fn crossing_rows_make_room_and_map_to_one_relative_placement() {
        let (mut session, keys) = press(2);
        let start = slots()[2].top + 15.;
        session.track(start - 40.);
        assert_eq!(offsets(&session, &keys), [0., 34., -40.]);
        assert!(session.motion(&keys[1]).animate);
        assert_eq!(session.placement(), Some(Some(keys[1].clone())));
        session.track(start - 80.);
        assert_eq!(offsets(&session, &keys), [34., 34., -80.]);
        assert_eq!(session.placement(), Some(Some(keys[0].clone())));
        session.track(start);
        assert_eq!(offsets(&session, &keys), [0., 0., 0.]);
        assert_eq!(session.placement(), None, "back at its slot is no move");

        let (mut session, keys) = press(0);
        let start = slots()[0].top + 15.;
        session.track(start + 40.);
        assert_eq!(session.placement(), Some(Some(keys[2].clone())));
        session.track(start + 80.);
        assert_eq!(offsets(&session, &keys), [80., -34., -34.]);
        assert_eq!(
            session.placement(),
            Some(None),
            "past the last row is the end"
        );
    }

    #[test]
    fn a_held_drop_shows_the_landed_order_without_motion_or_fade() {
        let (mut session, keys) = press(2);
        let start = slots()[2].top + 15.;
        session.track(start - 80.);
        let mut session = session.settle(Task::ready(()));
        assert!(!session.is_dragging());
        assert!(!session.is_lifted());
        assert_eq!(offsets(&session, &keys), [34., 34., -68.]);
        assert!(keys.iter().all(|key| {
            let motion = session.motion(key);
            !motion.animate && !motion.lifted
        }));
        assert!(!session.track(start), "a held drop ignores the pointer");
        assert_eq!(session.placement(), None, "a held drop never submits again");
    }
}
