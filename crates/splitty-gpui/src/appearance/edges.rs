//! The borders of the chrome: the theme's 3D bevel where it has one, a flat
//! one-color line otherwise. A bevel is drawn square, as two rings of lines
//! (or one for the thin reliefs) in the border's own space, so an element
//! keeps its content box whichever theme draws it.
use super::{Bevel, NativeAppearance};
use gpui_kit::{
    canvas, fill, point, px, rgb, size, Bounds, IntoElement, ParentElement, Pixels, Styled,
};

/// What an edge makes its element look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relief {
    /// A control that can be pressed: buttons, tabs, handles.
    Raised,
    /// A surface that floats over the window: menus, panels, dialogs, the
    /// window frame.
    Frame,
    /// A field that holds content: inputs, lists, terminals; also a control
    /// while it is pressed or selected.
    Sunken,
    /// A groove: group outlines and separators.
    Etched,
    /// One line of `Raised`, for small or dense parts.
    ThinRaised,
    /// One line of `Sunken`.
    ThinSunken,
}

impl Relief {
    /// The edge's width in pixels with a bevel; a flat border is 1.
    pub fn width(self) -> f32 {
        match self {
            Self::ThinRaised | Self::ThinSunken => 1.,
            _ => 2.,
        }
    }

    /// The rings from the outside in, each as its top-left and its
    /// bottom-right color.
    fn rings(self, bevel: &Bevel) -> ([(u32, u32); 2], usize) {
        let Bevel {
            highlight,
            light,
            shadow,
            dark_shadow,
        } = *bevel;
        match self {
            Self::Raised => ([(highlight, dark_shadow), (light, shadow)], 2),
            Self::Frame => ([(light, dark_shadow), (highlight, shadow)], 2),
            Self::Sunken => ([(shadow, highlight), (dark_shadow, light)], 2),
            Self::Etched => ([(shadow, highlight), (highlight, shadow)], 2),
            Self::ThinRaised => ([(highlight, shadow), (0, 0)], 1),
            Self::ThinSunken => ([(shadow, highlight), (0, 0)], 1),
        }
    }
}

/// The sides of an element that carry the edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sides {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl Sides {
    pub const ALL: Self = Self::new(true, true, true, true);
    pub const TOP: Self = Self::new(true, false, false, false);
    pub const RIGHT: Self = Self::new(false, true, false, false);
    pub const BOTTOM: Self = Self::new(false, false, true, false);
    pub const LEFT: Self = Self::new(false, false, false, true);

    pub const fn new(top: bool, right: bool, bottom: bool, left: bool) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }
}

/// The lines of a bevel around the box `(left, top, right, bottom)`, as
/// `(left, top, width, height, color)`. The top and left lines stop before
/// the bottom-right ones, which run the full length, as the classic 3D edge
/// does.
fn lines(
    outer: (f32, f32, f32, f32),
    relief: Relief,
    sides: Sides,
    bevel: &Bevel,
) -> Vec<(f32, f32, f32, f32, u32)> {
    let (rings, count) = relief.rings(bevel);
    let mut result = Vec::with_capacity(count * 4);
    for (ring, (top_left, bottom_right)) in rings.into_iter().take(count).enumerate() {
        let inset = ring as f32;
        let step = |side: bool| if side { inset } else { 0. };
        let (left, top) = (outer.0 + step(sides.left), outer.1 + step(sides.top));
        let (right, bottom) = (outer.2 - step(sides.right), outer.3 - step(sides.bottom));
        if right - left < 1. || bottom - top < 1. {
            break;
        }
        let end = |side: bool| if side { 1. } else { 0. };
        if sides.top {
            let width = right - left - end(sides.right);
            result.push((left, top, width, 1., top_left));
        }
        if sides.left {
            let height = bottom - top - end(sides.bottom);
            result.push((left, top, 1., height, top_left));
        }
        if sides.bottom {
            result.push((left, bottom - 1., right - left, 1., bottom_right));
        }
        if sides.right {
            result.push((right - 1., top, 1., bottom - top, bottom_right));
        }
    }
    result
}

/// The lines of a bevel as an element that fills its parent's content box
/// and paints in the parent's border space of `relief.width()` pixels on
/// `sides`. It takes no pointer input. Give it to the element that owns the
/// border, not to a scrolled one, whose children move with the content.
pub fn bevel_lines(bevel: Bevel, relief: Relief, sides: Sides) -> impl IntoElement {
    let width = relief.width();
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, (), window, _| {
            let grow = |side: bool| if side { width } else { 0. };
            let outer = (
                f32::from(bounds.left()) - grow(sides.left),
                f32::from(bounds.top()) - grow(sides.top),
                f32::from(bounds.right()) + grow(sides.right),
                f32::from(bounds.bottom()) + grow(sides.bottom),
            );
            for (left, top, width, height, color) in lines(outer, relief, sides, &bevel) {
                window.paint_quad(fill(
                    Bounds::new(point(px(left), px(top)), size(px(width), px(height))),
                    rgb(color),
                ));
            }
        },
    )
    .absolute()
    .inset_0()
}

/// Borders that follow the theme's edge style.
pub trait Edged: Styled + ParentElement + Sized {
    /// A border on every side: the bevel of `relief`, or a flat line in the
    /// theme's border color.
    fn edged(self, appearance: &NativeAppearance, relief: Relief) -> Self {
        self.edged_sides(appearance, relief, Sides::ALL, appearance.border)
    }

    /// A border on `sides`: the bevel of `relief`, or a flat line in `flat`.
    /// Call it before adding children that should paint over nothing of it;
    /// the border keeps its own space either way.
    fn edged_sides(
        mut self,
        appearance: &NativeAppearance,
        relief: Relief,
        sides: Sides,
        flat: u32,
    ) -> Self {
        let width = px(if appearance.bevel.is_some() {
            relief.width()
        } else {
            1.
        });
        let widths = &mut self.style().border_widths;
        if sides.top {
            widths.top = Some(width.into());
        }
        if sides.right {
            widths.right = Some(width.into());
        }
        if sides.bottom {
            widths.bottom = Some(width.into());
        }
        if sides.left {
            widths.left = Some(width.into());
        }
        match appearance.bevel {
            Some(bevel) => self.child(bevel_lines(bevel, relief, sides)),
            None => self.border_color(rgb(flat)),
        }
    }

    /// The bevel of `relief` on `sides` in a bevel theme; `flat` styles the
    /// element otherwise. For borders whose flat color is not a theme color
    /// of the appearance, and for elements that have no border when flat.
    fn edged_or(
        self,
        appearance: &NativeAppearance,
        relief: Relief,
        sides: Sides,
        flat: impl FnOnce(Self) -> Self,
    ) -> Self {
        if appearance.bevel.is_some() {
            self.edged_sides(appearance, relief, sides, 0)
        } else {
            flat(self)
        }
    }
}

impl<E: Styled + ParentElement + Sized> Edged for E {}

/// A classic 3D appearance for view tests: grey faces, bevels, square
/// corners, navy selection with white text.
#[cfg(test)]
pub fn bevel_test_appearance() -> NativeAppearance {
    let mut appearance = NativeAppearance::fallback("UI");
    appearance.bevel = Some(Bevel {
        highlight: 0xffffff,
        light: 0xdfdfdf,
        shadow: 0x808080,
        dark_shadow: 0x000000,
    });
    (appearance.radius, appearance.control_radius) = (0., 0.);
    (appearance.surface, appearance.raised, appearance.overlay) = (0xc0c0c0, 0xc0c0c0, 0xc0c0c0);
    (appearance.text, appearance.secondary, appearance.muted) = (0x000000, 0x000000, 0x808080);
    (appearance.active, appearance.active_text) = (0x000080, 0xffffff);
    (appearance.menu_hover, appearance.menu_hover_text) = (0x000080, 0xffffff);
    appearance.field = Some(0xffffff);
    appearance.shadow = 0x00000000;
    appearance
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEVEL: Bevel = Bevel {
        highlight: 0xffffff,
        light: 0xdfdfdf,
        shadow: 0x808080,
        dark_shadow: 0x000000,
    };

    #[test]
    fn a_bevel_theme_edges_where_a_flat_one_keeps_its_own_style() {
        use gpui_kit::{div, AbsoluteLength};
        let width = |element: &mut gpui_kit::Div| {
            element.style().border_widths.left.map(|width| match width {
                AbsoluteLength::Pixels(width) => f32::from(width),
                AbsoluteLength::Rems(_) => f32::NAN,
            })
        };
        let flat = NativeAppearance::fallback("UI");
        let mut kept = div().edged_or(&flat, Relief::Raised, Sides::LEFT, |element| element);
        assert_eq!(width(&mut kept), None);
        let mut lined = div().edged_or(&flat, Relief::Raised, Sides::LEFT, |element| {
            element.border_l_1()
        });
        assert_eq!(width(&mut lined), Some(1.));
        let mut beveled = flat.clone();
        beveled.bevel = Some(BEVEL);
        let mut raised = div().edged_or(&beveled, Relief::Raised, Sides::LEFT, |element| element);
        assert_eq!(width(&mut raised), Some(2.));
        let mut thin = div().edged_or(&beveled, Relief::ThinSunken, Sides::ALL, |element| element);
        assert_eq!(width(&mut thin), Some(1.));
    }

    #[test]
    fn a_raised_edge_is_two_rings_lit_from_the_top_left() {
        let lines = lines((10., 20., 30., 32.), Relief::Raised, Sides::ALL, &BEVEL);
        assert_eq!(
            lines,
            vec![
                // Outer ring: the lit lines stop one pixel short of the
                // dark ones, which own the bottom-right corner.
                (10., 20., 19., 1., 0xffffff),
                (10., 20., 1., 11., 0xffffff),
                (10., 31., 20., 1., 0x000000),
                (29., 20., 1., 12., 0x000000),
                (11., 21., 17., 1., 0xdfdfdf),
                (11., 21., 1., 9., 0xdfdfdf),
                (11., 30., 18., 1., 0x808080),
                (28., 21., 1., 10., 0x808080),
            ]
        );
    }

    #[test]
    fn reliefs_differ_in_their_rings_and_width() {
        let colors = |relief| {
            lines((0., 0., 8., 8.), relief, Sides::ALL, &BEVEL)
                .into_iter()
                .map(|line| line.4)
                .collect::<Vec<_>>()
        };
        assert_eq!(colors(Relief::Sunken)[0], 0x808080);
        assert_eq!(colors(Relief::Sunken)[2], 0xffffff);
        assert_eq!(colors(Relief::Sunken)[4], 0x000000);
        assert_eq!(colors(Relief::Frame)[0], 0xdfdfdf);
        assert_eq!(colors(Relief::Etched)[4], 0xffffff);
        assert_eq!(
            colors(Relief::ThinRaised),
            [0xffffff, 0xffffff, 0x808080, 0x808080]
        );
        assert_eq!(colors(Relief::ThinSunken).len(), 4);
        assert_eq!(
            (Relief::Raised.width(), Relief::ThinSunken.width()),
            (2., 1.)
        );
    }

    #[test]
    fn a_single_side_runs_the_full_length_and_a_tiny_box_draws_what_fits() {
        assert_eq!(
            lines((0., 0., 40., 2.), Relief::Etched, Sides::BOTTOM, &BEVEL),
            vec![(0., 1., 40., 1., 0xffffff), (0., 0., 40., 1., 0x808080)]
        );
        assert_eq!(
            lines((0., 0., 2., 40.), Relief::Etched, Sides::RIGHT, &BEVEL),
            vec![(1., 0., 1., 40., 0xffffff), (0., 0., 1., 40., 0x808080)]
        );
        assert!(lines((0., 0., 0.5, 9.), Relief::Raised, Sides::ALL, &BEVEL).is_empty());
    }
}
