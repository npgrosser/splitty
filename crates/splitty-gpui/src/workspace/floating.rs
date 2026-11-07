//! Window-local projection and pointer gesture for canonical floating groups.
use gpui_kit::{point, px, size, Bounds, Pixels, Point};
use splitty_client_core::workspace::{FloatBounds, FloatingTarget};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FloatGestureKind {
    Move,
    Resize,
}

#[derive(Clone, Debug)]
pub(super) struct FloatGesture {
    pub target: FloatingTarget,
    pub connection: Uuid,
    pub screen: String,
    pub kind: FloatGestureKind,
    pub origin: Point<Pixels>,
    pub viewport: Bounds<Pixels>,
    pub zoom: f32,
    pub initial: Bounds<Pixels>,
    pub preview: Bounds<Pixels>,
}

fn scalar(value: Pixels) -> f32 {
    f32::from(value)
}

fn clamp_extent(extent: f32, viewport: f32, minimum: f32) -> f32 {
    extent.clamp(minimum.min(viewport), viewport)
}

/// Canonical bounds are intentionally untouched: even an offscreen float is
/// projected into a reachable rectangle until the user explicitly moves it.
pub(super) fn project(bounds: &FloatBounds, viewport: Bounds<Pixels>, zoom: f32) -> Bounds<Pixels> {
    let fallback = FloatBounds::default();
    let bounds = if bounds.is_valid() { bounds } else { &fallback };
    let width = scalar(viewport.size.width).max(1.);
    let height = scalar(viewport.size.height).max(1.);
    let desired_width = (bounds.w * f64::from(width)).clamp(1., f64::from(f32::MAX)) as f32;
    let desired_height = (bounds.h * f64::from(height)).clamp(1., f64::from(f32::MAX)) as f32;
    let w = clamp_extent(desired_width, width, 180. * zoom);
    let h = clamp_extent(desired_height, height, 100. * zoom);
    let x = (bounds.x * f64::from(width)).clamp(0., f64::from((width - w).max(0.))) as f32;
    let y = (bounds.y * f64::from(height)).clamp(0., f64::from((height - h).max(0.))) as f32;
    Bounds::new(
        point(viewport.left() + px(x), viewport.top() + px(y)),
        size(px(w), px(h)),
    )
}

pub(super) fn moved(gesture: &FloatGesture, pointer: Point<Pixels>) -> Bounds<Pixels> {
    let delta_x = scalar(pointer.x - gesture.origin.x);
    let delta_y = scalar(pointer.y - gesture.origin.y);
    preview(
        gesture.kind,
        gesture.initial,
        gesture.viewport,
        gesture.zoom,
        delta_x,
        delta_y,
    )
}

pub(super) fn preview(
    kind: FloatGestureKind,
    old: Bounds<Pixels>,
    viewport: Bounds<Pixels>,
    zoom: f32,
    delta_x: f32,
    delta_y: f32,
) -> Bounds<Pixels> {
    let v_w = scalar(viewport.size.width).max(1.);
    let v_h = scalar(viewport.size.height).max(1.);
    let (x, y, w, h) = match kind {
        FloatGestureKind::Move => (
            (scalar(old.left() - viewport.left()) + delta_x)
                .clamp(0., (v_w - scalar(old.size.width)).max(0.)),
            (scalar(old.top() - viewport.top()) + delta_y)
                .clamp(0., (v_h - scalar(old.size.height)).max(0.)),
            scalar(old.size.width),
            scalar(old.size.height),
        ),
        FloatGestureKind::Resize => (
            scalar(old.left() - viewport.left()),
            scalar(old.top() - viewport.top()),
            clamp_extent(
                scalar(old.size.width) + delta_x,
                v_w - scalar(old.left() - viewport.left()),
                180. * zoom,
            ),
            clamp_extent(
                scalar(old.size.height) + delta_y,
                v_h - scalar(old.top() - viewport.top()),
                100. * zoom,
            ),
        ),
    };
    Bounds::new(
        point(viewport.left() + px(x), viewport.top() + px(y)),
        size(px(w), px(h)),
    )
}

/// Preserve window-local z-order across maximize, then drop only identities
/// absent from the canonical Screen and append newly created groups.
pub(super) fn reconcile_order(order: &mut Vec<String>, canonical: &[String]) {
    order.retain(|id| canonical.contains(id));
    for id in canonical {
        if !order.contains(id) {
            order.push(id.clone());
        }
    }
}

pub(super) fn normalized(bounds: Bounds<Pixels>, viewport: Bounds<Pixels>) -> FloatBounds {
    let width = scalar(viewport.size.width).max(1.);
    let height = scalar(viewport.size.height).max(1.);
    FloatBounds {
        x: f64::from(scalar(bounds.left() - viewport.left()) / width),
        y: f64::from(scalar(bounds.top() - viewport.top()) / height),
        w: f64::from(scalar(bounds.size.width) / width),
        h: f64::from(scalar(bounds.size.height) / height),
    }
}

pub(super) fn changed(before: Bounds<Pixels>, after: Bounds<Pixels>) -> bool {
    (scalar(before.left() - after.left())).abs() >= 1.
        || (scalar(before.top() - after.top())).abs() >= 1.
        || (scalar(before.size.width - after.size.width)).abs() >= 1.
        || (scalar(before.size.height - after.size.height)).abs() >= 1.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_keeps_offscreen_and_oversize_group_reachable_without_mutating_canonical_bounds() {
        let canonical = FloatBounds {
            x: 4.,
            y: -2.,
            w: 3.,
            h: 2.,
        };
        let viewport = Bounds::new(point(px(10.), px(20.)), size(px(800.), px(500.)));
        let projected = project(&canonical, viewport, 1.);
        assert_eq!(projected.origin, viewport.origin);
        assert_eq!(projected.size, viewport.size);
        assert_eq!(canonical.x, 4.);
        assert_eq!(canonical.w, 3.);
    }

    #[test]
    fn normalized_preview_is_finite_and_does_not_send_an_unchanged_projection() {
        let viewport = Bounds::new(point(px(0.), px(0.)), size(px(800.), px(500.)));
        let rect = project(&FloatBounds::default(), viewport, 1.);
        assert!(!changed(rect, rect));
        let next = Bounds::new(point(rect.left() + px(30.), rect.top()), rect.size);
        assert!(changed(rect, next));
        let relative = normalized(next, viewport);
        assert!((relative.x - 0.2875).abs() < 0.0001);
        assert!(relative.is_valid());
    }

    #[test]
    fn tiny_viewport_and_invalid_legacy_bounds_remain_finite_and_reachable() {
        let viewport = Bounds::new(point(px(5.), px(8.)), size(px(75.), px(54.)));
        let invalid = FloatBounds {
            x: f64::NAN,
            y: -10.,
            w: -1.,
            h: 0.,
        };
        let rect = project(&invalid, viewport, 3.);
        assert_eq!(rect.size, viewport.size);
        assert_eq!(rect.origin, viewport.origin);
        assert!(normalized(rect, viewport).is_valid());
    }

    #[test]
    fn move_and_resize_previews_clamp_at_viewport_edges_and_unchanged_release_is_noop() {
        let viewport = Bounds::new(point(px(10.), px(20.)), size(px(800.), px(500.)));
        let initial = project(&FloatBounds::default(), viewport, 1.);
        assert_eq!(
            preview(FloatGestureKind::Move, initial, viewport, 1., 0., 0.),
            initial
        );
        assert!(!changed(
            initial,
            preview(FloatGestureKind::Move, initial, viewport, 1., 0.3, 0.3)
        ));
        let moved = preview(
            FloatGestureKind::Move,
            initial,
            viewport,
            1.,
            10_000.,
            -10_000.,
        );
        assert_eq!(moved.left(), viewport.right() - initial.size.width);
        assert_eq!(moved.top(), viewport.top());
        let resized = preview(
            FloatGestureKind::Resize,
            initial,
            viewport,
            1.,
            10_000.,
            -10_000.,
        );
        assert_eq!(resized.right(), viewport.right());
        assert_eq!(resized.size.height, px(100.));
        assert!(changed(initial, resized));
        assert!(normalized(resized, viewport).is_valid());
    }

    #[test]
    fn canonical_stack_survives_maximize_restore_and_prunes_deleted_group() {
        let mut order = vec!["a".into(), "b".into()];
        order.remove(0);
        order.push("a".into());
        let canonical = vec!["a".into(), "b".into()];
        reconcile_order(&mut order, &canonical); // maximized projection is empty
        assert_eq!(order, ["b", "a"]);
        reconcile_order(&mut order, &["b".into(), "c".into()]);
        assert_eq!(order, ["b", "c"]);
    }
}
