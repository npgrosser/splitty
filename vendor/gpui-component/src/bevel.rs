//! The classic 3D edge a theme can give the controls in place of their flat
//! border. A bevel is drawn square, as two rings of lines (or one for the
//! thin reliefs) in the element's own border space, so an element keeps its
//! content box whichever theme draws it.
use gpui::{
    App, Bounds, Edges, Hsla, InteractiveElement as _, IntoElement, ParentElement, Pixels, Styled,
    canvas, div, fill, point, prelude::FluentBuilder as _, px, size,
};

use crate::ActiveTheme as _;

/// The four colors of a bevel, from the lit side to the shaded one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bevel {
    pub highlight: Hsla,
    pub light: Hsla,
    pub shadow: Hsla,
    pub dark_shadow: Hsla,
}

/// What an edge makes its element look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relief {
    /// A control that can be pressed: buttons.
    Raised,
    /// A surface that floats over the window: menus, popovers, dialogs.
    Frame,
    /// A field that holds content: inputs, selects, check boxes; also a
    /// control while it is pressed or selected.
    Sunken,
    /// A groove: group outlines and separators.
    Etched,
    /// One line of `Raised`, for small or dense parts.
    ThinRaised,
    /// One line of `Sunken`.
    ThinSunken,
}

impl Relief {
    /// The edge's width in pixels.
    pub fn width(self) -> f32 {
        match self {
            Self::ThinRaised | Self::ThinSunken => 1.,
            _ => 2.,
        }
    }

    /// The rings from the outside in, each as its top-left and its
    /// bottom-right color, and how many of them the relief draws.
    fn rings(self, bevel: &Bevel) -> ([(Hsla, Hsla); 2], usize) {
        let Bevel {
            highlight,
            light,
            shadow,
            dark_shadow,
        } = *bevel;
        let none = Hsla::transparent_black();
        match self {
            Self::Raised => ([(highlight, dark_shadow), (light, shadow)], 2),
            Self::Frame => ([(light, dark_shadow), (highlight, shadow)], 2),
            Self::Sunken => ([(shadow, highlight), (dark_shadow, light)], 2),
            Self::Etched => ([(shadow, highlight), (highlight, shadow)], 2),
            Self::ThinRaised => ([(highlight, shadow), (none, none)], 1),
            Self::ThinSunken => ([(shadow, highlight), (none, none)], 1),
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
    sides: Edges<bool>,
    bevel: &Bevel,
) -> Vec<(f32, f32, f32, f32, Hsla)> {
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
/// `sides`. It takes no pointer input. The parent's own border must be
/// transparent, since a border paints over the children.
pub fn bevel_lines(bevel: Bevel, relief: Relief, sides: Edges<bool>) -> impl IntoElement {
    let width = relief.width();
    let lines = canvas(
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
                    color,
                ));
            }
        },
    )
    .size_full();
    div()
        .when(cfg!(test), |this| {
            this.debug_selector(move || format!("bevel-{relief:?}"))
        })
        .absolute()
        .inset_0()
        .child(lines)
}

/// The sides of `element` that have a border so far.
fn bordered_sides<T: Styled>(element: &mut T) -> Edges<bool> {
    let has = |width: Option<gpui::AbsoluteLength>| {
        width.is_some_and(|width| width.to_pixels(px(16.)) > Pixels::ZERO)
    };
    let widths = &element.style().border_widths;
    Edges {
        top: has(widths.top),
        right: has(widths.right),
        bottom: has(widths.bottom),
        left: has(widths.left),
    }
}

/// With the theme's bevel, replaces the border `element` has so far by the
/// bevel of `relief` on the same sides, and leaves its border transparent;
/// without one, or without a border, returns it unchanged.
///
/// Call it last on the element's style: a border color set after it, also
/// one of a hover or focus state, paints over the bevel.
pub(crate) fn bevel_border<T: Styled + ParentElement>(
    mut element: T,
    relief: Relief,
    cx: &App,
) -> T {
    let Some(bevel) = cx.theme().bevel else {
        return element;
    };
    let sides = bordered_sides(&mut element);
    if !sides.any(|side| *side) {
        return element;
    }
    let width = px(relief.width());
    let widths = &mut element.style().border_widths;
    for (side, slot) in [
        (sides.top, &mut widths.top),
        (sides.right, &mut widths.right),
        (sides.bottom, &mut widths.bottom),
        (sides.left, &mut widths.left),
    ] {
        if side {
            *slot = Some(width.into());
        }
    }
    element
        .border_color(Hsla::transparent_black())
        .child(bevel_lines(bevel, relief, sides))
}

/// The border color of a control that may carry a bevel: transparent with
/// the theme's bevel, so the border cannot paint over it, else `color`.
pub(crate) fn bevel_border_color(color: Hsla, cx: &App) -> Hsla {
    if cx.theme().bevel.is_some() {
        Hsla::transparent_black()
    } else {
        color
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, rgb};
    use std::{cell::Cell, rc::Rc};

    fn bevel() -> Bevel {
        Bevel {
            highlight: rgb(0xffffff).into(),
            light: rgb(0xdfdfdf).into(),
            shadow: rgb(0x808080).into(),
            dark_shadow: rgb(0x000000).into(),
        }
    }

    fn color(value: u32) -> Hsla {
        rgb(value).into()
    }

    const ALL: Edges<bool> = Edges {
        top: true,
        right: true,
        bottom: true,
        left: true,
    };

    fn only(top: bool, right: bool, bottom: bool, left: bool) -> Edges<bool> {
        Edges {
            top,
            right,
            bottom,
            left,
        }
    }

    #[test]
    fn a_raised_edge_is_two_rings_lit_from_the_top_left() {
        let lines = lines((10., 20., 30., 32.), Relief::Raised, ALL, &bevel());
        assert_eq!(
            lines,
            vec![
                // Outer ring: the lit lines stop one pixel short of the
                // dark ones, which own the bottom-right corner.
                (10., 20., 19., 1., color(0xffffff)),
                (10., 20., 1., 11., color(0xffffff)),
                (10., 31., 20., 1., color(0x000000)),
                (29., 20., 1., 12., color(0x000000)),
                (11., 21., 17., 1., color(0xdfdfdf)),
                (11., 21., 1., 9., color(0xdfdfdf)),
                (11., 30., 18., 1., color(0x808080)),
                (28., 21., 1., 10., color(0x808080)),
            ]
        );
    }

    #[test]
    fn reliefs_differ_in_their_rings_and_width() {
        let colors = |relief| {
            lines((0., 0., 8., 8.), relief, ALL, &bevel())
                .into_iter()
                .map(|line| line.4)
                .collect::<Vec<_>>()
        };
        assert_eq!(colors(Relief::Sunken)[0], color(0x808080));
        assert_eq!(colors(Relief::Sunken)[2], color(0xffffff));
        assert_eq!(colors(Relief::Sunken)[4], color(0x000000));
        assert_eq!(colors(Relief::Frame)[0], color(0xdfdfdf));
        assert_eq!(colors(Relief::Etched)[4], color(0xffffff));
        assert_eq!(
            colors(Relief::ThinRaised),
            [
                color(0xffffff),
                color(0xffffff),
                color(0x808080),
                color(0x808080)
            ]
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
            lines(
                (0., 0., 40., 2.),
                Relief::Etched,
                only(false, false, true, false),
                &bevel()
            ),
            vec![
                (0., 1., 40., 1., color(0xffffff)),
                (0., 0., 40., 1., color(0x808080))
            ]
        );
        assert_eq!(
            lines(
                (0., 0., 2., 40.),
                Relief::Etched,
                only(false, true, false, false),
                &bevel()
            ),
            vec![
                (1., 0., 1., 40., color(0xffffff)),
                (0., 0., 1., 40., color(0x808080))
            ]
        );
        assert!(lines((0., 0., 0.5, 9.), Relief::Raised, ALL, &bevel()).is_empty());
    }

    #[test]
    fn a_bevel_takes_over_the_sides_that_have_a_border() {
        let mut flat = gpui::div().border_t_1().border_l_1();
        assert_eq!(bordered_sides(&mut flat), only(true, false, false, true));
        assert_eq!(
            bordered_sides(&mut gpui::div().border_0()),
            only(false, false, false, false)
        );
    }

    /// What the harness renders.
    #[derive(Clone, Copy)]
    enum Control {
        Button,
        Ghost,
        Input,
        Checkbox,
    }

    struct Harness {
        control: Control,
        clicks: Rc<Cell<usize>>,
        input: Option<gpui::Entity<crate::input::InputState>>,
    }

    impl gpui::Render for Harness {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            use crate::button::{Button, ButtonVariants as _};
            use gpui::InteractiveElement as _;
            let clicks = self.clicks.clone();
            let control = match self.control {
                Control::Button | Control::Ghost => Button::new("control")
                    .label("Apply")
                    .when(matches!(self.control, Control::Ghost), |this| this.ghost())
                    .debug_selector(|| "control".into())
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                    .into_any_element(),
                Control::Input => crate::input::Input::new(self.input.as_ref().unwrap())
                    .prefix(div().debug_selector(|| "prefix".into()).size(px(4.)))
                    .into_any_element(),
                Control::Checkbox => crate::checkbox::Checkbox::new("check")
                    .checked(true)
                    .into_any_element(),
            };
            div().p(px(10.)).w(px(200.)).child(control)
        }
    }

    fn set_bevel(cx: &mut gpui::VisualTestContext, on: bool) {
        cx.update(|window, cx| {
            crate::Theme::update(cx, |theme| theme.bevel = on.then(bevel));
            window.draw(cx).clear(cx);
        });
    }

    fn open(
        control: Control,
        cx: &mut gpui::TestAppContext,
    ) -> (Rc<Cell<usize>>, &mut gpui::VisualTestContext) {
        cx.update(crate::init);
        let clicks = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view({
            let clicks = clicks.clone();
            move |window, cx| Harness {
                control,
                clicks,
                input: matches!(control, Control::Input)
                    .then(|| cx.new(|cx| crate::input::InputState::new(window, cx))),
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (clicks, cx)
    }

    #[gpui::test]
    fn a_theme_config_clears_the_bevel_and_the_field_color(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::init(cx);
            let flat = crate::Theme::global(cx).input_background();
            crate::Theme::update(cx, |theme| {
                theme.bevel = Some(bevel());
                theme.field = Some(color(0xffffff));
            });
            let theme = crate::Theme::global(cx);
            assert_eq!(theme.input_background(), color(0xffffff));
            assert_eq!(theme.face_background(), flat);

            crate::Theme::change(crate::ThemeMode::Dark, None, cx);
            let theme = crate::Theme::global(cx);
            assert_eq!((theme.bevel, theme.field), (None, None));
        });
    }

    #[gpui::test]
    fn a_button_is_raised_and_sinks_while_it_is_held(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, MouseButton};
        let (clicks, cx) = open(Control::Button, cx);
        assert!(cx.debug_bounds("bevel-Raised").is_none());
        let flat = cx.debug_bounds("control").unwrap();

        set_bevel(cx, true);
        let button = cx.debug_bounds("control").unwrap();
        assert_eq!(button, flat, "the bevel takes the border's space");
        let edge = cx.debug_bounds("bevel-Raised").unwrap();
        assert_eq!(edge.origin, button.origin + point(px(2.), px(2.)));

        let inside = button.center();
        cx.simulate_mouse_down(inside, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("bevel-Sunken").is_some());
        assert!(cx.debug_bounds("bevel-Raised").is_none());

        cx.simulate_mouse_up(inside, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("bevel-Raised").is_some());
        assert_eq!(clicks.get(), 1);
    }

    #[gpui::test]
    fn a_ghost_button_has_no_edge(cx: &mut gpui::TestAppContext) {
        let (_, cx) = open(Control::Ghost, cx);
        set_bevel(cx, true);
        assert!(cx.debug_bounds("control").is_some());
        assert!(cx.debug_bounds("bevel-Raised").is_none());
    }

    #[gpui::test]
    fn a_field_is_sunken_in_a_border_one_pixel_wider(cx: &mut gpui::TestAppContext) {
        let (_, cx) = open(Control::Input, cx);
        assert!(cx.debug_bounds("bevel-Sunken").is_none());
        let flat = cx.debug_bounds("prefix").unwrap();

        set_bevel(cx, true);
        assert!(cx.debug_bounds("bevel-Sunken").is_some());
        let beveled = cx.debug_bounds("prefix").unwrap();
        assert_eq!(beveled.origin.x, flat.origin.x + px(1.));
    }

    #[gpui::test]
    fn a_checked_box_is_sunken(cx: &mut gpui::TestAppContext) {
        let (_, cx) = open(Control::Checkbox, cx);
        set_bevel(cx, true);
        assert!(cx.debug_bounds("bevel-Sunken").is_some());
    }
}
