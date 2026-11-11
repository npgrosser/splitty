//! Whole-host dragging reuses the measured Screen motion and local metadata owner.
use super::hosts::HostGroupTarget;
use super::*;

#[derive(Clone)]
pub(super) struct HostDrag {
    owner: SourceToken,
    source: SourceId,
    view: EntityId,
}

pub(super) fn opened_host_groups(model: &WindowWorkspace) -> Vec<SourceId> {
    let local = model.local_source().source_id();
    let mut groups = Vec::new();
    for entry in model.opened() {
        let source = entry.key().source_id();
        if source != local && !groups.contains(&source) {
            groups.push(source);
        }
    }
    groups
}

impl Workspace {
    pub(super) fn host_drag_caption(
        &self,
        group: HostGroupTarget,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let source = group.source_id();
        let owner = group.owner();
        let weak = cx.entity().downgrade();
        div()
            .id(SharedString::from(format!("host-drag-caption-{source}")))
            .flex()
            .flex_1()
            .min_w_0()
            .cursor_grab()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    this.drawer_host_press = Some((
                        source,
                        f32::from(event.position.y - this.drawer_scroll.offset().y),
                    ));
                }),
            )
            .on_drag(
                HostDrag {
                    owner,
                    source,
                    view: cx.entity().entity_id(),
                },
                move |drag, _, window, cx| {
                    let position = window.mouse_position();
                    let _ = weak.update(cx, |this, cx| {
                        this.begin_host_drag(drag.owner, drag.source, position);
                        cx.notify();
                    });
                    cx.new(|_| super::drawer::ScreenDragPreview)
                },
            )
    }

    fn begin_host_drag(&mut self, owner: SourceToken, source: SourceId, position: Point<Pixels>) {
        self.drawer_drag = None;
        self.drawer_host_drag = None;
        let Some(model) = self.model.as_ref().filter(|m| m.lifecycle_token() == owner) else {
            return;
        };
        let groups = opened_host_groups(model);
        if !groups.contains(&source) || self.hosts_navigation_blocked() {
            return;
        }
        let Some(slots) = groups
            .iter()
            .map(|id| self.drawer_host_slots.borrow().get(id).copied())
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        let start = self
            .drawer_host_press
            .take()
            .filter(|(id, _)| *id == source)
            .map(|(_, y)| y)
            .unwrap_or_else(|| f32::from(position.y - self.drawer_scroll.offset().y));
        self.drawer_host_drag = ScreenDragSession::new(owner, source, groups, slots, start);
    }

    pub(super) fn track_host_drag(
        &mut self,
        event: &DragMoveEvent<HostDrag>,
        cx: &mut Context<Self>,
    ) {
        let drag = event.drag(cx);
        if drag.view != cx.entity().entity_id() {
            return;
        }
        let pointer = f32::from(event.event.position.y - self.drawer_scroll.offset().y);
        if self
            .drawer_host_drag
            .as_mut()
            .filter(|s| s.owner == drag.owner && s.screen == drag.source)
            .is_some_and(|s| s.track(pointer))
        {
            cx.notify();
        }
    }

    pub(super) fn finish_host_drag(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.drawer_host_drag.take_if(|s| s.is_dragging()) else {
            return;
        };
        cx.notify();
        let Some(before) = session.placement() else {
            return;
        };
        if !self.model.as_ref().is_some_and(|m| {
            m.lifecycle_token() == session.owner && opened_host_groups(m) == session.group
        }) || !self.place_host_group(session.owner, session.screen, before, cx)
        {
            return;
        }
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1000))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.drawer_host_drag.take_if(|s| !s.is_dragging());
                cx.notify();
            });
        });
        self.drawer_host_drag = Some(session.settle(timer));
    }

    pub(super) fn reconcile_host_drag(
        &mut self,
        model: &WindowWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let groups = opened_host_groups(model);
        self.drawer_host_slots
            .borrow_mut()
            .retain(|source, _| groups.contains(source));
        if self
            .drawer_host_drag
            .as_ref()
            .is_some_and(|s| s.owner != model.lifecycle_token() || s.group != groups)
        {
            if self
                .drawer_host_drag
                .take()
                .is_some_and(|s| s.is_dragging())
            {
                cx.stop_active_drag(window);
            }
            cx.notify();
        }
    }

    pub(super) fn render_drawer_group(
        &self,
        source: SourceId,
        header: AnyElement,
        screens: Stateful<Div>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let motion = self
            .drawer_host_drag
            .as_ref()
            .map(|s| s.motion(&source))
            .unwrap_or_default();
        let offset = transition(
            format!("host-drag-offset-{source}"),
            motion.offset,
            if motion.animate {
                screen_drag::screen_shift_transition()
            } else {
                Transition::new(Duration::ZERO)
            },
            window,
            cx,
        );
        let opacity = transition(
            format!("host-drag-opacity-{source}"),
            if motion.lifted {
                screen_drag::SCREEN_DRAG_OPACITY
            } else {
                1.
            },
            screen_drag::screen_lift_transition(),
            window,
            cx,
        );
        let slots = self.drawer_host_slots.clone();
        let scroll = self.drawer_scroll.clone();
        let block = div()
            .id(SharedString::from(format!("drawer-group-{source}")))
            .debug_selector(move || format!("drawer-group-{source}"))
            .relative()
            .top(px(offset))
            .flex()
            .flex_col()
            .when(opacity < 1., |v| v.opacity(opacity))
            .child(
                canvas(
                    move |bounds, _, _| {
                        slots.borrow_mut().insert(
                            source,
                            RowSlot {
                                top: f32::from(bounds.top() - scroll.offset().y) - offset,
                                height: f32::from(bounds.size.height),
                            },
                        );
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(header)
            .child(screens);
        if motion.lifted {
            screen_drag::RaisedInClip::new(block).into_any_element()
        } else {
            block.into_any_element()
        }
    }
}
