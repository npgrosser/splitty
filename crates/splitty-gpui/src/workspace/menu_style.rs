//! The web client's context menu look and placement, shared by the tab menu
//! and the new-tab profile picker. Colors and the panel radius come from the
//! theme (`NativeAppearance`); the sizes here are the web client's fixed values.
use super::*;

/// Inner padding of the panel and its minimum width.
const PANEL_PADDING: f32 = 4.;
const PANEL_MIN_WIDTH: f32 = 200.;
/// The panel shadow: `0 8px 24px` in the theme's shadow color.
const SHADOW_OFFSET: f32 = 8.;
const SHADOW_BLUR: f32 = 24.;
/// Rows: padding, corner radius, text size and the gap before their meta text.
const ITEM_PADDING_X: f32 = 10.;
const ITEM_PADDING_Y: f32 = 8.;
const ITEM_RADIUS: f32 = 4.;
const ITEM_TEXT: f32 = 12.;
const ITEM_META_GAP: f32 = 20.;
/// Secondary text: details, section captions, shortcuts and badges.
const DETAIL_TEXT: f32 = 10.;
const BADGE_TEXT: f32 = 9.;
/// A separator line inset from the panel edge, with its space above and below.
const SEPARATOR_INSET: f32 = 6.;
const SEPARATOR_ABOVE: f32 = 4.;
const SEPARATOR_BELOW: f32 = 3.;
/// Detail lines: their widest extent and their space under the label.
const DETAIL_MAX_WIDTH: f32 = 260.;
const DETAIL_GAP: f32 = 2.;
/// The gap between a row's trailing badges, shortcut and submenu chevron.
const META_GAP: f32 = 6.;
/// A submenu's distance from its parent panel, and the space every panel
/// keeps from the window edge.
const SUBMENU_GAP: f32 = 4.;
const VIEWPORT_MARGIN: f32 = 8.;

pub(super) fn panel(appearance: &NativeAppearance) -> Div {
    div()
        .flex()
        .flex_col()
        .p(chrome_rems(PANEL_PADDING))
        .min_w(chrome_rems(PANEL_MIN_WIDTH))
        .bg(rgb(appearance.overlay))
        .border_1()
        .border_color(rgb(appearance.border))
        .rounded(chrome_rems(appearance.radius))
        .shadow(vec![BoxShadow {
            color: rgba(appearance.shadow).into(),
            offset: point(px(0.), px(SHADOW_OFFSET)),
            blur_radius: px(SHADOW_BLUR),
            spread_radius: px(0.),
            inset: false,
        }])
        .text_color(rgb(appearance.text))
        .text_size(chrome_rems(ITEM_TEXT))
        .font_weight(FontWeight::MEDIUM)
}

/// A row. `highlighted` marks the keyboard focus or the row whose submenu is
/// open, with the plain hover background even on a danger row; only pointer
/// hover turns a danger row red.
pub(super) fn item(appearance: &NativeAppearance, highlighted: bool, danger: bool) -> Div {
    let (background, text) = if danger {
        (appearance.danger_hover, appearance.danger)
    } else {
        (appearance.hover, appearance.text)
    };
    let focus = appearance.hover;
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(chrome_rems(ITEM_META_GAP))
        .px(chrome_rems(ITEM_PADDING_X))
        .py(chrome_rems(ITEM_PADDING_Y))
        .rounded(chrome_rems(ITEM_RADIUS))
        .cursor_pointer()
        .when(highlighted, |row| row.bg(rgb(focus)))
        .hover(move |row| row.bg(rgb(background)).text_color(rgb(text)))
}

/// A row that cannot be chosen.
pub(super) fn disabled_item(appearance: &NativeAppearance) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(chrome_rems(ITEM_META_GAP))
        .px(chrome_rems(ITEM_PADDING_X))
        .py(chrome_rems(ITEM_PADDING_Y))
        .text_color(rgb(appearance.text))
        .opacity(0.4)
}

pub(super) fn separator(appearance: &NativeAppearance) -> Div {
    div()
        .flex_none()
        .mt(chrome_rems(SEPARATOR_ABOVE))
        .mb(chrome_rems(SEPARATOR_BELOW))
        .mx(chrome_rems(SEPARATOR_INSET))
        .h(px(1.))
        .bg(rgb(appearance.subtle_border))
}

/// Muted secondary text under or beside a label.
pub(super) fn detail(appearance: &NativeAppearance, text: impl Into<SharedString>) -> Div {
    div()
        .max_w(chrome_rems(DETAIL_MAX_WIDTH))
        .text_size(chrome_rems(DETAIL_TEXT))
        .font_weight(FontWeight::NORMAL)
        .text_color(rgb(appearance.muted))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(text.into())
}

/// A row's label with an optional detail line under it.
pub(super) fn label(
    appearance: &NativeAppearance,
    text: impl Into<SharedString>,
    detail_text: Option<SharedString>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(chrome_rems(DETAIL_GAP))
        .child(text.into())
        .when_some(detail_text, |label, text| {
            label.child(detail(appearance, text))
        })
}

/// A row's trailing badges, shortcut or submenu chevron.
pub(super) fn meta() -> Div {
    div()
        .flex()
        .flex_none()
        .items_start()
        .gap(chrome_rems(META_GAP))
}

/// The glyph at the end of a row that opens a submenu, in the row's text color.
pub(super) fn chevron() -> Div {
    div()
        .flex_none()
        .text_size(chrome_rems(ITEM_TEXT))
        .child("›")
}

/// A submenu's surface, the same as its parent panel's.
pub(super) fn submenu_panel(appearance: &NativeAppearance) -> Div {
    panel(appearance)
}

/// A caption above a group of rows.
pub(super) fn caption(appearance: &NativeAppearance, text: impl Into<SharedString>) -> Div {
    detail(appearance, text)
        .px(chrome_rems(ITEM_PADDING_X))
        .pt(chrome_rems(SEPARATOR_BELOW))
        .pb(chrome_rems(SEPARATOR_ABOVE))
}

/// A small outlined label such as "Default".
pub(super) fn badge(appearance: &NativeAppearance, text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px(chrome_rems(4.))
        .py(px(1.))
        .border_1()
        .border_color(rgb(appearance.border))
        .rounded(chrome_rems(3.))
        .text_size(chrome_rems(BADGE_TEXT))
        .text_color(rgb(appearance.muted))
        .child(text.into())
}

/// Bounds that a panel or row records while it is laid out, read by a submenu
/// placed later in the same frame.
pub(super) type PlacedBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

/// Where a menu panel goes; resolved against the window when it is laid out.
pub(super) enum Placement {
    /// The top-left corner at the pointer, shifted (never flipped) to stay
    /// inside the window.
    Pointer(Point<Pixels>),
    /// Beside the parent panel: right of it, or left when that does not fit,
    /// level with the top of the row that opened it.
    Beside {
        parent: PlacedBounds,
        row: PlacedBounds,
    },
}

/// Lays `panel` out at `placement`, recording its final bounds in `resolved`.
/// The panel blocks the pointer from what is under it.
pub(super) fn placed(
    placement: Placement,
    resolved: Option<PlacedBounds>,
    panel: impl IntoElement,
) -> Placed {
    Placed {
        placement,
        resolved,
        child: panel.into_any_element(),
    }
}

pub(super) struct Placed {
    placement: Placement,
    resolved: Option<PlacedBounds>,
    child: AnyElement,
}

/// The part of the window a panel may cover: the viewport less any
/// client-drawn frame and the fixed margin.
fn menu_area(window: &Window) -> Bounds<Pixels> {
    let margin = chrome_rems(VIEWPORT_MARGIN).to_pixels(window.rem_size());
    let inset = window.client_inset().unwrap_or_default();
    let frame = match window.window_decorations() {
        Decorations::Server => Edges::default(),
        Decorations::Client { tiling } => Edges {
            top: if tiling.top { px(0.) } else { inset },
            right: if tiling.right { px(0.) } else { inset },
            bottom: if tiling.bottom { px(0.) } else { inset },
            left: if tiling.left { px(0.) } else { inset },
        },
    };
    let viewport = window.viewport_size();
    let top_left = point(frame.left + margin, frame.top + margin);
    let bottom_right = point(
        (viewport.width - frame.right - margin).max(top_left.x),
        (viewport.height - frame.bottom - margin).max(top_left.y),
    );
    Bounds::from_corners(top_left, bottom_right)
}

/// The tallest a panel may be; a longer one scrolls.
pub(super) fn panel_max_height(window: &Window) -> Pixels {
    menu_area(window).size.height
}

/// Shifts `origin` so a panel of `size` stays inside `area`; its left and top
/// edges win when it is larger.
fn shift_into(origin: Point<Pixels>, size: Size<Pixels>, area: Bounds<Pixels>) -> Point<Pixels> {
    point(
        origin.x.min(area.right() - size.width).max(area.left()),
        origin.y.min(area.bottom() - size.height).max(area.top()),
    )
}

/// The submenu's origin: `gap` right of its parent when it fits there,
/// otherwise `gap` left of it, at the row's top; then shifted inside `area`.
fn beside_origin(
    parent: Bounds<Pixels>,
    row: Bounds<Pixels>,
    size: Size<Pixels>,
    area: Bounds<Pixels>,
    gap: Pixels,
) -> Point<Pixels> {
    let right = parent.right() + gap;
    let x = if right + size.width <= area.right() {
        right
    } else {
        parent.left() - size.width - gap
    };
    shift_into(point(x, row.top()), size, area)
}

impl IntoElement for Placed {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Placed {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, LayoutId) {
        let child = self.child.request_layout(window, cx);
        let style = Style {
            position: Position::Absolute,
            display: Display::Flex,
            ..Style::default()
        };
        (window.request_layout(style, [child], cx), child)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        child: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let laid_out = window.layout_bounds(*child);
        let area = menu_area(window);
        let origin = match &self.placement {
            Placement::Pointer(position) => shift_into(*position, laid_out.size, area),
            Placement::Beside { parent, row } => {
                let parent = parent.get().unwrap_or_default();
                beside_origin(
                    parent,
                    row.get().unwrap_or(parent),
                    laid_out.size,
                    area,
                    chrome_rems(SUBMENU_GAP).to_pixels(window.rem_size()),
                )
            }
        };
        let origin = point(origin.x.round(), origin.y.round());
        let bounds = Bounds::new(origin, laid_out.size);
        if let Some(resolved) = &self.resolved {
            resolved.set(Some(bounds));
        }
        // Ahead of the panel's own hitboxes, so it blocks only what is behind it.
        window.insert_hitbox(bounds, HitboxBehavior::BlockMouse);
        window.with_element_offset(origin - laid_out.origin, |window| {
            self.child.prepaint(window, cx)
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut LayoutId,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{beside_origin, shift_into};
    use gpui_kit::{point, px, size, Bounds};

    fn area() -> Bounds<gpui_kit::Pixels> {
        Bounds::from_corners(point(px(8.), px(8.)), point(px(792.), px(592.)))
    }

    #[test]
    fn pointer_menus_shift_inside_the_margin_without_flipping() {
        let menu = size(px(200.), px(150.));
        assert_eq!(
            shift_into(point(px(100.), px(40.)), menu, area()),
            point(px(100.), px(40.))
        );
        assert_eq!(
            shift_into(point(px(750.), px(580.)), menu, area()),
            point(px(592.), px(442.)),
            "near the bottom-right corner the menu moves up and left, not to the pointer's other side"
        );
        assert_eq!(
            shift_into(point(px(2.), px(1.)), menu, area()),
            point(px(8.), px(8.))
        );
        assert_eq!(
            shift_into(point(px(100.), px(40.)), size(px(900.), px(150.)), area()),
            point(px(8.), px(40.)),
            "a panel wider than the window keeps its left edge inside"
        );
    }

    #[test]
    fn submenus_open_right_flip_left_and_follow_their_row() {
        let parent = Bounds::new(point(px(100.), px(50.)), size(px(200.), px(240.)));
        let row = Bounds::new(point(px(104.), px(210.)), size(px(192.), px(31.)));
        let menu = size(px(220.), px(100.));
        assert_eq!(
            beside_origin(parent, row, menu, area(), px(4.)),
            point(px(304.), px(210.))
        );
        let right_edge = Bounds::new(point(px(560.), px(50.)), size(px(200.), px(240.)));
        assert_eq!(
            beside_origin(right_edge, row, menu, area(), px(4.)),
            point(px(336.), px(210.)),
            "without room on the right it opens left of the parent"
        );
        let low_row = Bounds::new(point(px(104.), px(560.)), size(px(192.), px(31.)));
        assert_eq!(
            beside_origin(parent, low_row, menu, area(), px(4.)),
            point(px(304.), px(492.)),
            "it stays inside the window's bottom margin"
        );
    }
}
