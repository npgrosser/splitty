//! Convert service-owned theme values into the bounded native presentation subset.
//! This adapter never reads settings files or changes terminal font metrics.
use crate::terminal::TerminalColors;
use gpui_kit::component::theme::{Theme, ThemeColor, ThemeMode};
use gpui_kit::{px, rgb, App, Hsla, Rgba};
use splitty_client_core::appearance::{AppearanceSnapshot, ColorScheme};
use splitty_client_core::zoom::Zoom;

/// A solid border of the given width and color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragOverBorder {
    pub width: f32,
    pub color: u32,
}

/// A line along the selected tab of a group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabAccent {
    pub color: u32,
    pub thickness: f32,
    pub bottom: bool,
}

#[derive(Clone, Debug)]
pub struct NativeAppearance {
    pub surface: u32,
    pub raised: u32,
    pub overlay: u32,
    pub text: u32,
    pub secondary: u32,
    pub muted: u32,
    pub border: u32,
    pub subtle_border: u32,
    pub active: u32,
    pub hover: u32,
    pub accent: u32,
    pub font_family: String,
    pub font_size: f32,
    /// Group tab bar sizing from the theme's `tab-height` and `tab-font-size`.
    pub tab_height: f32,
    pub tab_font_size: f32,
    /// The selected tab's line from the theme's `tab-accent-*` tokens; `None`
    /// when the theme makes it transparent.
    pub tab_accent: Option<TabAccent>,
    /// Menus: the theme's `danger`, `danger-hover` and `border-radius`.
    pub danger: u32,
    pub danger_hover: u32,
    pub radius: f32,
    /// Status colors from the theme's `success` and `warning`, as host
    /// connection dots use them.
    pub success: u32,
    pub warning: u32,
    /// Translucent `0xRRGGBBAA` colors drawn over content: the drop marker's
    /// fill (`hover-bg`) and menu shadows (`shadow`).
    pub hover_overlay: u32,
    pub shadow: u32,
    /// The drop marker's border from `drag-over-border`, if the theme has one.
    pub drag_over_border: Option<DragOverBorder>,
    pub terminal: TerminalColors,
    pub selection_foreground: u32,
    pub selection_background: u32,
    pub zoom: Zoom,
    pub notice: Option<&'static str>,
    component_palette: ComponentPalette,
}

impl NativeAppearance {
    pub fn fallback(ui_family: &str) -> Self {
        Self {
            surface: 0x10151c,
            raised: 0x18232f,
            overlay: 0x293b4d,
            text: 0xd9e2ef,
            secondary: 0x93a6b9,
            muted: 0x8ea3b7,
            border: 0x416783,
            subtle_border: 0x263544,
            active: 0x30475b,
            hover: 0x1f2c3a,
            accent: 0x76b8e8,
            font_family: ui_family.into(),
            font_size: 13.,
            tab_height: 32.,
            tab_font_size: 12.,
            tab_accent: None,
            danger: 0xe06c75,
            danger_hover: 0x3a2730,
            radius: 8.,
            success: 0x98c379,
            warning: 0xe5c07b,
            hover_overlay: 0xffffff0f,
            shadow: 0x00000073,
            drag_over_border: None,
            terminal: TerminalColors::default(),
            selection_foreground: 0xd9e2ef,
            selection_background: 0x345778,
            zoom: Zoom::from_values(1.0, "window"),
            notice: None,
            component_palette: ComponentPalette::default(),
        }
    }

    pub fn from_snapshot(
        snapshot: &AppearanceSnapshot,
        available_fonts: &[String],
        ui_family: &str,
        mono_family: &str,
    ) -> Self {
        let mut result = Self::fallback(ui_family);
        let ui = &snapshot.theme().ui;
        let terminal = &snapshot.terminal_theme().terminal;
        let mut fallback_used = false;
        let mut color = |value: &str, background: u32, fallback: u32| {
            parse_color(value)
                .map(|color| color.over(background))
                .unwrap_or_else(|| {
                    fallback_used = true;
                    fallback
                })
        };
        result.surface = color(&ui.surface_base, result.surface, result.surface);
        result.raised = color(&ui.surface_raised, result.surface, result.raised);
        result.overlay = color(&ui.surface_overlay, result.raised, result.overlay);
        result.text = color(&ui.text_primary, result.surface, result.text);
        result.secondary = color(&ui.text_secondary, result.surface, result.secondary);
        result.muted = color(&ui.text_muted, result.surface, result.muted);
        result.border = color(&ui.border_default, result.surface, result.border);
        result.subtle_border = color(&ui.border_subtle, result.surface, result.subtle_border);
        result.active = color(&ui.active_bg, result.raised, result.active);
        result.hover = color(&ui.hover_bg, result.raised, result.hover);
        result.danger = color(&ui.danger, result.overlay, result.danger);
        result.danger_hover = color(&ui.danger_hover, result.overlay, result.danger_hover);
        result.success = color(&ui.success, result.surface, result.success);
        result.warning = color(&ui.warning, result.surface, result.warning);
        result.accent = color(&ui.accent, result.surface, result.accent);
        result.terminal.background =
            color(&terminal.bg, result.surface, result.terminal.background);
        let background = result.terminal.background;
        result.terminal.foreground = color(&terminal.fg, background, result.terminal.foreground);
        result.terminal.cursor = color(&terminal.cursor, background, result.terminal.cursor);
        result.selection_background = color(
            &terminal.selection_bg,
            background,
            result.selection_background,
        );
        result.selection_foreground = color(
            &terminal.selection_fg,
            result.selection_background,
            result.terminal.foreground,
        );
        let ansi = [
            &terminal.black,
            &terminal.red,
            &terminal.green,
            &terminal.yellow,
            &terminal.blue,
            &terminal.magenta,
            &terminal.cyan,
            &terminal.white,
            &terminal.bright_black,
            &terminal.bright_red,
            &terminal.bright_green,
            &terminal.bright_yellow,
            &terminal.bright_blue,
            &terminal.bright_magenta,
            &terminal.bright_cyan,
            &terminal.bright_white,
        ];
        for (target, value) in result.terminal.ansi.iter_mut().zip(ansi) {
            *target = color(value, background, *target);
        }
        let mut sizing = |value: &str, min: f32, max: f32, fallback: f32| {
            theme_px(value, min, max).unwrap_or_else(|| {
                fallback_used = true;
                fallback
            })
        };
        result.tab_height = sizing(&ui.tab_height, 16., 64., result.tab_height);
        result.tab_font_size = sizing(&ui.tab_font_size, 8., 24., result.tab_font_size);
        let accent_thickness = sizing(&ui.tab_accent_thickness, 0., 8., 2.);
        result.radius = sizing(&ui.border_radius, 0., 24., result.radius);
        result.tab_accent = tab_accent(
            &ui.tab_accent_color,
            accent_thickness,
            &ui.tab_accent_position,
            result.raised,
        )
        .unwrap_or_else(|| {
            fallback_used = true;
            None
        });
        let mut translucent = |value: &str, fallback: u32| {
            parse_color(value).map(Color::rgba).unwrap_or_else(|| {
                fallback_used = true;
                fallback
            })
        };
        result.hover_overlay = translucent(&ui.hover_bg, result.hover_overlay);
        result.shadow = translucent(&ui.shadow, result.shadow);
        result.drag_over_border = drag_over_border(&ui.drag_over_border).unwrap_or_else(|| {
            fallback_used = true;
            None
        });
        let settings = snapshot.settings();
        result.zoom = Zoom::from_values(settings.zoom, &settings.zoom_scope);
        let (size, invalid_size) = chrome_font_size(settings.font_size, &ui.font_size);
        result.font_size = size;
        fallback_used |= invalid_size;
        let family = if settings.font_family.trim().is_empty() {
            &ui.font_family
        } else {
            &settings.font_family
        };
        match choose_font(family, available_fonts, ui_family, mono_family) {
            Some(family) => result.font_family = family,
            None => fallback_used = true,
        }
        // Terminal colors remain exactly the selected palette.
        (result.text, result.secondary, result.muted) =
            chrome_text(result.text, result.secondary, result.muted, result.surface);
        let mut token = |value: &str, background| {
            let parsed = parse_color(value).map(|color| color.over(background));
            fallback_used |= parsed.is_none();
            parsed
        };
        result.component_palette = ComponentPalette {
            surface: token(&ui.surface_base, result.surface),
            raised: token(&ui.surface_raised, result.surface),
            overlay: token(&ui.surface_overlay, result.raised),
            text: token(&ui.text_primary, result.surface),
            secondary: token(&ui.text_secondary, result.surface),
            border: token(&ui.border_default, result.surface),
            subtle_border: token(&ui.border_subtle, result.raised),
            hover: token(&ui.hover_bg, result.raised),
            active: token(&ui.active_bg, result.raised),
            accent: token(&ui.accent, result.surface),
            danger: token(&ui.danger, result.surface),
            warning: token(&ui.warning, result.surface),
            success: token(&ui.success, result.surface),
            scrollbar: token(&ui.scrollbar, result.raised),
        };
        result.notice = fallback_used
            .then_some("Some theme values are unsupported; native defaults are in use.");
        result
    }

    /// Mode defaults are loaded before the product palette. `Theme::update`
    /// synchronizes solid colors, render tokens and the Base layer together.
    pub fn apply_component_theme(&self, scheme: ColorScheme, cx: &mut App) {
        Theme::change(
            match scheme {
                ColorScheme::Light => ThemeMode::Light,
                ColorScheme::Dark => ThemeMode::Dark,
            },
            None,
            cx,
        );
        Theme::update(cx, |theme| {
            self.component_palette.project(&mut theme.colors);
            theme.font_family = self.font_family.clone().into();
            theme.font_size = px(16. * self.zoom.window_scale());
        });
    }
}

/// Validated product colors. Absent tokens retain the active Kit mode's
/// defaults; no CSS or theme resolution belongs in individual controls.
#[derive(Clone, Debug, Default)]
struct ComponentPalette {
    surface: Option<u32>,
    raised: Option<u32>,
    overlay: Option<u32>,
    text: Option<u32>,
    secondary: Option<u32>,
    border: Option<u32>,
    subtle_border: Option<u32>,
    hover: Option<u32>,
    active: Option<u32>,
    accent: Option<u32>,
    danger: Option<u32>,
    warning: Option<u32>,
    success: Option<u32>,
    scrollbar: Option<u32>,
}

impl ComponentPalette {
    fn project(&self, colors: &mut ThemeColor) {
        let set = |target: &mut Hsla, value: Option<u32>| {
            if let Some(value) = value {
                *target = rgb(value).into();
            }
        };
        set(&mut colors.background, self.surface);
        set(&mut colors.foreground, self.text);
        set(&mut colors.title_bar, self.raised);
        set(&mut colors.title_bar_border, self.subtle_border);
        set(&mut colors.popover, self.overlay);
        set(&mut colors.popover_foreground, self.text);
        set(&mut colors.sidebar, self.raised);
        set(&mut colors.sidebar_foreground, self.text);
        set(&mut colors.sidebar_border, self.subtle_border);
        set(&mut colors.group_box, self.raised);
        set(&mut colors.group_box_foreground, self.text);
        set(&mut colors.secondary, self.raised);
        set(&mut colors.secondary_foreground, self.text);
        set(&mut colors.muted, self.raised);
        set(&mut colors.muted_foreground, self.secondary);
        set(&mut colors.border, self.border);
        set(&mut colors.input, self.border);
        set(&mut colors.accent, self.active);
        set(&mut colors.accent_foreground, self.text);
        set(&mut colors.primary, self.accent);
        set(&mut colors.ring, self.accent);
        set(&mut colors.caret, self.text);
        set(&mut colors.danger, self.danger);
        set(&mut colors.warning, self.warning);
        set(&mut colors.success, self.success);
        set(&mut colors.scrollbar_thumb, self.scrollbar);
        set(&mut colors.scrollbar_thumb_hover, self.secondary);
        set(&mut colors.list, self.surface);
        set(&mut colors.list_head, self.raised);
        set(&mut colors.list_even, self.raised);
        set(&mut colors.list_hover, self.hover);
        set(&mut colors.list_active, self.active);
        set(&mut colors.list_active_border, self.accent);
        set(&mut colors.selection, self.active);
        colors.sidebar_accent = colors.accent;
        colors.sidebar_accent_foreground = readable_text(colors.accent_foreground, colors.accent);
        colors.sidebar_primary = colors.primary;
        colors.sidebar_primary_foreground =
            readable_text(colors.primary_foreground, colors.primary);

        // A valid custom color can still make a label unreadable. Keep its
        // requested surface and choose a contrasting foreground when needed.
        colors.foreground = readable_text(colors.foreground, colors.background);
        colors.popover_foreground = readable_text(colors.popover_foreground, colors.popover);
        colors.sidebar_foreground = readable_text(colors.sidebar_foreground, colors.sidebar);
        colors.group_box_foreground = readable_text(colors.group_box_foreground, colors.group_box);
        colors.muted_foreground = readable_text(colors.muted_foreground, colors.background);
        colors.accent_foreground = readable_text(colors.accent_foreground, colors.accent);
        colors.secondary_foreground = readable_text(colors.secondary_foreground, colors.secondary);
        colors.primary_foreground = readable_text(colors.primary_foreground, colors.primary);
        colors.danger_foreground = readable_text(colors.danger_foreground, colors.danger);
        colors.warning_foreground = readable_text(colors.warning_foreground, colors.warning);
        colors.success_foreground = readable_text(colors.success_foreground, colors.success);

        colors.button = colors.secondary;
        colors.button_foreground = colors.secondary_foreground;
        colors.button_hover = self
            .hover
            .map_or(colors.button_hover, |value| rgb(value).into());
        colors.button_active = colors.accent;
        colors.secondary_hover = colors.button_hover;
        colors.secondary_active = colors.button_active;
        colors.primary_hover = state_surface(colors.primary, colors.primary_foreground, 0.08);
        colors.primary_active = state_surface(colors.primary, colors.primary_foreground, 0.16);
        colors.danger_hover = state_surface(colors.danger, colors.danger_foreground, 0.08);
        colors.danger_active = state_surface(colors.danger, colors.danger_foreground, 0.16);
        colors.warning_hover = state_surface(colors.warning, colors.warning_foreground, 0.08);
        colors.warning_active = state_surface(colors.warning, colors.warning_foreground, 0.16);
        colors.success_hover = state_surface(colors.success, colors.success_foreground, 0.08);
        colors.success_active = state_surface(colors.success, colors.success_foreground, 0.16);
        colors.button_primary = colors.primary;
        colors.button_primary_foreground = colors.primary_foreground;
        colors.button_primary_hover = colors.primary_hover;
        colors.button_primary_active = colors.primary_active;
        colors.button_secondary = colors.secondary;
        colors.button_secondary_foreground = colors.secondary_foreground;
        colors.button_secondary_hover = colors.secondary_hover;
        colors.button_secondary_active = colors.secondary_active;
        colors.button_danger = colors.danger;
        colors.button_danger_foreground = colors.danger_foreground;
        colors.button_danger_hover = colors.danger_hover;
        colors.button_danger_active = colors.danger_active;
        colors.button_warning = colors.warning;
        colors.button_warning_foreground = colors.warning_foreground;
        colors.button_warning_hover = colors.warning_hover;
        colors.button_warning_active = colors.warning_active;
        colors.button_success = colors.success;
        colors.button_success_foreground = colors.success_foreground;
        colors.button_success_hover = colors.success_hover;
        colors.button_success_active = colors.success_active;
    }
}

fn state_surface(background: Hsla, foreground: Hsla, amount: f32) -> Hsla {
    let black = rgb(0).into();
    let white = rgb(0xffffff).into();
    let target = if contrast(foreground, black) >= contrast(foreground, white) {
        black
    } else {
        white
    };
    blend(background, target, amount)
}

fn blend(background: Hsla, foreground: Hsla, amount: f32) -> Hsla {
    let a = Rgba::from(background);
    let b = Rgba::from(foreground);
    Rgba {
        r: a.r + (b.r - a.r) * amount,
        g: a.g + (b.g - a.g) * amount,
        b: a.b + (b.b - a.b) * amount,
        a: 1.,
    }
    .into()
}

fn luminance(color: Hsla) -> f32 {
    let color = Rgba::from(color);
    let linear = |value: f32| {
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn contrast(a: Hsla, b: Hsla) -> f32 {
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn readable_text(requested: Hsla, background: Hsla) -> Hsla {
    if contrast(requested, background) >= 4.5 {
        return requested;
    }
    let black = rgb(0).into();
    let white = rgb(0xffffff).into();
    let target = if contrast(black, background) >= contrast(white, background) {
        black
    } else {
        white
    };
    // Preserve the intended hierarchy and hue: move only as far as needed,
    // rather than turning every subdued label into pure white or black.
    let mut low = 0.;
    let mut high = 1.;
    for _ in 0..12 {
        let middle = (low + high) * 0.5;
        if contrast(blend(requested, target, middle), background) >= 4.6 {
            high = middle;
        } else {
            low = middle;
        }
    }
    blend(requested, target, high)
}

/// Workspace chrome text: primary text is kept readable on the surface, while
/// secondary and muted text keep the theme's values. Their low contrast is how
/// a theme subdues idle entries against running and selected ones, as in the
/// web client; lifting them would erase that difference.
fn chrome_text(text: u32, secondary: u32, muted: u32, surface: u32) -> (u32, u32, u32) {
    (readable_rgb(text, surface), secondary, muted)
}

fn readable_rgb(requested: u32, background: u32) -> u32 {
    u32::from(Rgba::from(readable_text(
        rgb(requested).into(),
        rgb(background).into(),
    ))) >> 8
}

#[derive(Clone, Copy, Debug)]
struct Color {
    rgb: [f32; 3],
    alpha: f32,
}

impl Color {
    /// This color as `0xRRGGBBAA`, keeping its alpha.
    fn rgba(self) -> u32 {
        let channel = |value: f32| (value.clamp(0., 255.).round() as u32) & 255;
        let [r, g, b] = self.rgb;
        (channel(r) << 24) | (channel(g) << 16) | (channel(b) << 8) | channel(self.alpha * 255.)
    }

    fn over(self, background: u32) -> u32 {
        let mut packed = 0;
        for (channel, shift) in self.rgb.into_iter().zip([16, 8, 0]) {
            let base = ((background >> shift) & 255) as f32;
            let mixed = (channel * self.alpha + base * (1. - self.alpha)).round() as u32;
            packed |= mixed << shift;
        }
        packed
    }
}

// A deliberately small CSS subset. Reject unsupported expressions rather than
// passing strings into an engine API or pretending to interpret arbitrary CSS.
fn parse_color(value: &str) -> Option<Color> {
    if value.len() > 128 || !value.is_ascii() {
        return None;
    }
    let value = value.trim();
    if value.eq_ignore_ascii_case("transparent") {
        return Some(Color {
            rgb: [0.; 3],
            alpha: 0.,
        });
    }
    if let Some(hex) = value.strip_prefix('#') {
        let mut rgba = [255_u8; 4];
        match hex.len() {
            3 | 4 => {
                for (target, byte) in rgba.iter_mut().zip(hex.bytes()) {
                    *target = char::from(byte).to_digit(16)? as u8 * 17;
                }
            }
            6 | 8 => {
                for (index, target) in rgba.iter_mut().take(hex.len() / 2).enumerate() {
                    *target = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
                }
            }
            _ => return None,
        }
        return Some(Color {
            rgb: [rgba[0] as f32, rgba[1] as f32, rgba[2] as f32],
            alpha: rgba[3] as f32 / 255.,
        });
    }
    let (components, has_alpha) = if let Some(inner) = value.strip_prefix("rgba(") {
        (inner.strip_suffix(')')?, true)
    } else {
        (value.strip_prefix("rgb(")?.strip_suffix(')')?, false)
    };
    let mut components = components.split(',');
    let mut rgb = [0.; 3];
    for target in &mut rgb {
        *target = bounded_number(components.next()?, 255.)?;
    }
    let alpha = if has_alpha {
        bounded_number(components.next()?, 1.)?
    } else {
        1.
    };
    if components.next().is_some() {
        return None;
    }
    Some(Color { rgb, alpha })
}

fn bounded_number(value: &str, max: f32) -> Option<f32> {
    let value: f32 = value.trim().parse().ok()?;
    (value.is_finite() && (0. ..=max).contains(&value)).then_some(value)
}

/// A `drag-over-border` value: `none`, or `<width>px solid <color>`. `None`
/// means unsupported, `Some(None)` no border.
fn drag_over_border(value: &str) -> Option<Option<DragOverBorder>> {
    let value = value.trim();
    if value.is_empty() || value == "none" || value == "0" {
        return Some(None);
    }
    let (width, rest) = value.split_once(char::is_whitespace)?;
    let (style, color) = rest.trim_start().split_once(char::is_whitespace)?;
    if style != "solid" {
        return None;
    }
    let width = theme_px(width, 0., 8.)?;
    let color = parse_color(color.trim())?;
    Some((width > 0. && color.alpha > 0.).then(|| DragOverBorder {
        width,
        color: color.over(0),
    }))
}

/// The selected tab's line, `Some(None)` for a transparent or zero-width line
/// and `None` for an unsupported color.
fn tab_accent(
    color: &str,
    thickness: f32,
    position: &str,
    raised: u32,
) -> Option<Option<TabAccent>> {
    let color = parse_color(color)?;
    Some((color.alpha > 0. && thickness > 0.).then(|| TabAccent {
        color: color.over(raised),
        thickness,
        bottom: position.trim() == "bottom",
    }))
}

/// A theme length such as `28px` within `min..=max`.
fn theme_px(value: &str, min: f32, max: f32) -> Option<f32> {
    if value.len() > 32 {
        return None;
    }
    let value = value.trim();
    let number = if value == "0" {
        "0"
    } else {
        value.strip_suffix("px")?
    };
    let number: f32 = number.trim().parse().ok()?;
    (number.is_finite() && (min..=max).contains(&number)).then_some(number)
}

fn chrome_font_size(override_size: i32, theme_size: &str) -> (f32, bool) {
    if (8..=24).contains(&override_size) {
        return (override_size as f32, false);
    }
    if override_size == 0 && theme_size.len() <= 32 {
        if let Some(size) = theme_size
            .trim()
            .strip_suffix("px")
            .and_then(|value| value.trim().parse::<f32>().ok())
        {
            if size.is_finite() && (8. ..=24.).contains(&size) {
                return (size, false);
            }
        }
    }
    (13., true)
}

fn choose_font(
    value: &str,
    available: &[String],
    ui_family: &str,
    mono_family: &str,
) -> Option<String> {
    if value.len() > 4096 {
        return None;
    }
    // Match a bounded CSS family list, including quoted family names. Escapes,
    // embedded commas and CSS expressions are outside this native subset.
    for part in value.split(',').take(32) {
        let part = part.trim();
        let family = if part.starts_with('"') || part.starts_with('\'') {
            let quote = part.as_bytes()[0] as char;
            part.strip_prefix(quote)?.strip_suffix(quote)?
        } else {
            part
        };
        if family.is_empty() || family.len() > 256 || family.contains(['"', '\'', '\\']) {
            continue;
        }
        match family.to_ascii_lowercase().as_str() {
            "system-ui" | "-apple-system" | "sans-serif" => return Some(ui_family.into()),
            "monospace" | "ui-monospace" => return Some(mono_family.into()),
            _ => {
                if let Some(name) = available
                    .iter()
                    .find(|name| name.eq_ignore_ascii_case(family))
                {
                    return Some(name.clone());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subdued_text_gains_readability_without_overtaking_primary_text() {
        let background = rgb(0x10151c).into();
        let primary = rgb(0xd9e2ef).into();
        let subdued = readable_text(rgb(0x6b6b6b).into(), background);
        assert!(contrast(subdued, background) >= 4.5);
        assert!(luminance(subdued) < luminance(primary));
        assert_eq!(readable_text(primary, background), primary);
        let packed: Hsla = rgb(readable_rgb(0x6b6b6b, 0x10151c)).into();
        assert!(contrast(packed, background) >= 4.5);
    }

    #[test]
    fn chrome_keeps_subdued_theme_text_and_only_lifts_unreadable_primary_text() {
        // splitty-classic-dark-gray: secondary #585858 and muted #484848 on
        // #1e1e1e stay distinct from primary text and from each other.
        assert_eq!(
            chrome_text(0xa0a0a0, 0x585858, 0x484848, 0x1e1e1e),
            (0xa0a0a0, 0x585858, 0x484848)
        );
        let (primary, _, _) = chrome_text(0x303030, 0x585858, 0x484848, 0x1e1e1e);
        assert!(contrast(rgb(primary).into(), rgb(0x1e1e1e).into()) >= 4.5);
    }

    #[test]
    fn component_palette_keeps_text_readable_across_modes_and_custom_surfaces() {
        for defaults in [ThemeColor::light(), ThemeColor::dark()] {
            for surface in [0x10151c, 0xf4f4f4, 0x765438] {
                let palette = ComponentPalette {
                    surface: Some(surface),
                    raised: Some(surface),
                    overlay: Some(surface),
                    text: Some(surface), // deliberately invalid contrast
                    secondary: Some(surface),
                    accent: Some(0x76b8e8),
                    danger: Some(0xff6666),
                    warning: Some(0xffcc33),
                    success: Some(0x448855),
                    ..ComponentPalette::default()
                };
                let mut colors = *defaults;
                palette.project(&mut colors);
                assert_eq!(colors.background, Hsla::from(rgb(surface)));
                for (foreground, background) in [
                    (colors.foreground, colors.background),
                    (colors.popover_foreground, colors.popover),
                    (colors.sidebar_foreground, colors.sidebar),
                    (colors.muted_foreground, colors.background),
                    (colors.button_primary_foreground, colors.button_primary),
                    (
                        colors.button_primary_foreground,
                        colors.button_primary_hover,
                    ),
                    (
                        colors.button_primary_foreground,
                        colors.button_primary_active,
                    ),
                    (colors.button_danger_foreground, colors.button_danger),
                    (colors.button_warning_foreground, colors.button_warning),
                    (colors.button_success_foreground, colors.button_success),
                ] {
                    assert!(contrast(foreground, background) >= 4.5);
                }
            }
        }
    }

    #[test]
    fn unsupported_component_tokens_keep_the_new_modes_palette() {
        let palette = ComponentPalette::default();
        for defaults in [ThemeColor::light(), ThemeColor::dark()] {
            let mut colors = *defaults;
            palette.project(&mut colors);
            assert_eq!(colors.background, defaults.background);
            assert_eq!(colors.popover, defaults.popover);
            assert_eq!(colors.sidebar, defaults.sidebar);
            assert_eq!(colors.primary, defaults.primary);
            assert_eq!(colors.border, defaults.border);
            assert_eq!(colors.input, defaults.input);
            assert_eq!(colors.danger, defaults.danger);
        }
    }

    #[test]
    fn supported_colors_compose_alpha_against_the_actual_background() {
        assert_eq!(parse_color("#abc").unwrap().over(0), 0xaabbcc);
        assert_eq!(parse_color("#abcd").unwrap().over(0), 0x93a2b1);
        assert_eq!(parse_color("#123456").unwrap().over(0), 0x123456);
        assert_eq!(parse_color("#ffffff80").unwrap().over(0), 0x808080);
        assert_eq!(
            parse_color("rgba(255, 0, 0, 0.5)").unwrap().over(0x0000ff),
            0x800080
        );
        assert_eq!(parse_color("rgb(12, 34, 56)").unwrap().over(0), 0x0c2238);
        assert_eq!(
            parse_color(" transparent ").unwrap().over(0xabcdef),
            0xabcdef
        );
    }

    #[test]
    fn malformed_or_unbounded_colors_do_not_reach_native_rendering() {
        for value in [
            "#12",
            "#界界",
            "#12345g",
            "red",
            "var(--red)",
            "rgb(256,0,0)",
            "rgb(-1,0,0)",
            "rgb(NaN,0,0)",
            "rgba(0,0,0,inf)",
            "rgba(0,0,0,1.1)",
            "rgb(1,2,3,4)",
            "rgb(1 2 3)",
        ] {
            assert!(parse_color(value).is_none(), "{value}");
        }
        assert!(parse_color(&" ".repeat(129)).is_none());
    }

    #[test]
    fn chrome_defaults_use_theme_tokens_and_choose_available_family_names() {
        assert_eq!(chrome_font_size(0, "12px"), (12., false));
        assert_eq!(chrome_font_size(18, "nonsense"), (18., false));
        assert_eq!(chrome_font_size(0, "2rem"), (13., true));
        assert_eq!(chrome_font_size(-3, "12px"), (13., true));
        let fonts = vec!["Noto Sans".into(), "DejaVu Sans Mono".into()];
        let choose = |value| choose_font(value, &fonts, "Noto Sans", "DejaVu Sans Mono");
        assert_eq!(
            choose("'Missing', \"noto sans\", sans-serif"),
            Some("Noto Sans".into())
        );
        assert_eq!(choose("system-ui"), Some("Noto Sans".into()));
        assert_eq!(choose("monospace"), Some("DejaVu Sans Mono".into()));
        assert_eq!(choose("Missing"), None);
        assert_eq!(choose("\"unterminated"), None);
        // Chrome changes cannot alter terminal colors or native font metrics:
        // this adapter only exports chrome typography and RGB terminal input.
        let fallback = NativeAppearance::fallback("Noto Sans");
        assert_eq!(fallback.terminal, TerminalColors::default());
    }

    #[test]
    fn tab_sizing_and_accent_tokens_are_bounded_theme_values() {
        assert_eq!(theme_px("28px", 16., 64.), Some(28.));
        assert_eq!(theme_px(" 12.5px ", 8., 24.), Some(12.5));
        assert_eq!(theme_px("0", 0., 8.), Some(0.));
        assert_eq!(theme_px("2rem", 8., 24.), None);
        assert_eq!(theme_px("200px", 16., 64.), None);
        assert_eq!(theme_px("NaNpx", 0., 8.), None);
        assert_eq!(
            tab_accent("#e0e0e0", 2., "bottom", 0x242424),
            Some(Some(TabAccent {
                color: 0xe0e0e0,
                thickness: 2.,
                bottom: true,
            }))
        );
        assert_eq!(
            tab_accent("#63a0ff", 2., "top", 0x1a1a1a).map(|line| line.map(|line| line.bottom)),
            Some(Some(false))
        );
        assert_eq!(tab_accent("transparent", 2., "top", 0x1a1a1a), Some(None));
        assert_eq!(tab_accent("#e0e0e0", 0., "top", 0x1a1a1a), Some(None));
        assert_eq!(tab_accent("var(--x)", 2., "top", 0x1a1a1a), None);
    }

    #[cfg(feature = "ui-test-support")]
    #[test]
    fn status_tokens_come_from_the_theme_with_distinct_fallbacks() {
        use splitty_client_core::appearance::test_support::{settings_snapshot, Availability};
        let snapshot = settings_snapshot(uuid::Uuid::new_v4(), 1, Availability::Full);
        let ui = &snapshot.theme().ui;
        let native = NativeAppearance::from_snapshot(&snapshot, &[], "Sans", "Mono");
        let expected = |value: &str| parse_color(value).unwrap().over(native.surface);
        assert_eq!(native.success, expected(&ui.success));
        assert_eq!(native.warning, expected(&ui.warning));
        assert_ne!(native.success, native.warning);
        // Without a theme the status colors stay apart from each other and
        // from the danger color, so every host state keeps its own dot.
        let fallback = NativeAppearance::fallback("Sans");
        let colors = [fallback.success, fallback.warning, fallback.danger];
        assert!(colors
            .iter()
            .enumerate()
            .all(|(index, color)| !colors[index + 1..].contains(color)));
    }

    #[test]
    fn drop_marker_and_shadow_tokens_keep_translucency_and_parse_borders() {
        assert_eq!(
            parse_color("rgba(255, 255, 255, 0.06)").map(Color::rgba),
            Some(0xffffff0f)
        );
        assert_eq!(parse_color("#000000").map(Color::rgba), Some(0x000000ff));
        assert_eq!(
            drag_over_border("1px solid #585858"),
            Some(Some(DragOverBorder {
                width: 1.,
                color: 0x585858,
            }))
        );
        assert_eq!(drag_over_border("none"), Some(None));
        assert_eq!(drag_over_border(""), Some(None));
        assert_eq!(drag_over_border("1px solid transparent"), Some(None));
        assert_eq!(drag_over_border("1px dashed #585858"), None);
        assert_eq!(drag_over_border("thin solid #585858"), None);
    }
}
