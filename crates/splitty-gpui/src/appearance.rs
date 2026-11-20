//! The native presentation of the selected UI theme and terminal palette.
//!
//! A UI theme is a theme of the component toolkit, applied to its components
//! as it is. The workspace chrome takes its own colors and sizes from the
//! theme's optional `splitty` block and derives the rest from the toolkit
//! colors. The terminal takes its colors from a palette in Ghostty's format.
//! This module never writes settings or changes terminal font metrics.
use crate::terminal::TerminalColors;
use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::theme::{Theme, ThemeColor, ThemeConfig, ThemeMode};
use gpui_kit::{
    linear_color_stop, linear_gradient, px, rgb, App, Background, FontWeight, Hsla, Rgba,
};
use splitty_client_core::appearance::{
    terminal_palette_name, AppearanceSnapshot, ColorScheme, RememberedAppearance,
};
use splitty_client_core::settings::SettingsField;
use splitty_client_core::zoom::{Zoom, ZoomSteps};
use std::rc::Rc;

mod catalog;
pub mod edges;
mod palette;
pub mod titlebar;

pub use catalog::{ChromeBlock, FillBlock, Maybe, ThemeCatalog, TitleBarBlock, UiTheme};
pub use palette::TerminalPalette;

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

/// The colors of 3D edges, lightest to darkest. A theme with a bevel draws
/// raised controls, sunken fields and frames with them (see `edges`) where a
/// flat theme draws a one-color border.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bevel {
    pub highlight: u32,
    pub light: u32,
    pub shadow: u32,
    pub dark_shadow: u32,
}

/// A solid color, or a gradient from the start to the end of the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarFill {
    Solid(u32),
    Gradient { from: u32, to: u32 },
}

impl BarFill {
    pub fn background(self) -> Background {
        match self {
            Self::Solid(color) => rgb(color).into(),
            Self::Gradient { from, to } => linear_gradient(
                90.,
                linear_color_stop(rgb(from), 0.),
                linear_color_stop(rgb(to), 1.),
            ),
        }
    }

    /// The color at the start of the bar, where only one color can be used.
    pub fn start(self) -> u32 {
        match self {
            Self::Solid(color) | Self::Gradient { from: color, .. } => color,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TitleAlign {
    #[default]
    Center,
    Start,
}

/// How an application-drawn title bar shows its window buttons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptionButtons {
    /// Symbols with a hover background.
    #[default]
    Plain,
    /// Small buttons with the theme's raised edge.
    Boxed,
    /// Cells as tall as the bar, the close cell red under the pointer.
    Wide,
}

/// The theme's own title bar: its colors apply while title-bar colors follow
/// the theme, its shape while the decoration is Splitty's. Absent parts keep
/// what the chrome derives.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TitleBarTheme {
    pub background: Option<BarFill>,
    pub foreground: Option<u32>,
    pub inactive_background: Option<BarFill>,
    pub inactive_foreground: Option<u32>,
    pub height: Option<f32>,
    pub align: TitleAlign,
    pub weight: Option<FontWeight>,
    pub buttons: CaptionButtons,
}

#[derive(Clone, Debug)]
pub struct NativeAppearance {
    pub system_style: bool,
    pub window_decoration: titlebar::WindowDecoration,
    pub window_buttons: titlebar::WindowButtons,
    pub title_bar_style: titlebar::TitleBarStyle,
    pub surface: u32,
    pub raised: u32,
    pub overlay: u32,
    pub text: u32,
    pub secondary: u32,
    pub muted: u32,
    pub border: u32,
    pub subtle_border: u32,
    pub active: u32,
    /// Primary text on `active`, readable there.
    pub active_text: u32,
    pub hover: u32,
    /// The row of a menu under the pointer or the keyboard focus, and
    /// primary text readable on it.
    pub menu_hover: u32,
    pub menu_hover_text: u32,
    pub accent: u32,
    pub font_family: String,
    pub font_size: f32,
    /// Group tab bar sizing.
    pub tab_height: f32,
    pub tab_font_size: f32,
    /// The selected tab's line; `None` when the theme has none.
    pub tab_accent: Option<TabAccent>,
    /// Menus: destructive entries and the corner radius.
    pub danger: u32,
    pub danger_hover: u32,
    /// Corners of menus, panels and dialogs.
    pub radius: f32,
    /// The largest corner of a control; see `corner`.
    pub control_radius: f32,
    /// 3D edges; `None` for flat borders.
    pub bevel: Option<Bevel>,
    /// The background of fields that hold text; `None` keeps each field's
    /// own background.
    pub field: Option<u32>,
    pub title_bar: TitleBarTheme,
    /// Status colors, as host connection dots use them.
    pub success: u32,
    pub warning: u32,
    /// Translucent `0xRRGGBBAA` colors drawn over content: the drop marker's
    /// fill and menu shadows.
    pub hover_overlay: u32,
    pub shadow: u32,
    /// The drop marker's border, if the theme has one.
    pub drag_over_border: Option<DragOverBorder>,
    pub terminal: TerminalColors,
    pub selection_foreground: u32,
    pub selection_background: u32,
    pub zoom: Zoom,
    /// The levels zoom steps through, the local service's.
    pub zoom_steps: ZoomSteps,
    /// What of the selection could not be shown as asked.
    pub notice: Option<String>,
    /// The mode of the shown UI theme, which can differ from the system's.
    pub scheme: ColorScheme,
    /// The toolkit theme the components use; `None` keeps the toolkit default.
    config: Option<Rc<ThemeConfig>>,
}

/// The theme names of the settings and the system's preference.
#[derive(Clone, Copy, Debug)]
pub struct ThemeSelection<'a> {
    pub ui_theme: &'a str,
    pub os_scheme: ColorScheme,
    /// A palette name, or `default` for the pair below.
    pub terminal_theme: &'a str,
    /// The palettes by the UI theme's mode: a name, or `default` to follow
    /// the UI theme.
    pub terminal_theme_dark: &'a str,
    pub terminal_theme_light: &'a str,
    /// 0 for the theme's size.
    pub font_size: i32,
    /// Empty for the theme's family.
    pub font_family: &'a str,
}

const DEFAULT_FONT_SIZE: f32 = 12.;

impl NativeAppearance {
    /// The corner radius of a control whose own radius is `natural`: the
    /// theme's control radius bounds it, so a square theme is square
    /// throughout. Round marks (dots, pills) do not use this.
    pub fn corner(&self, natural: f32) -> f32 {
        natural.min(self.control_radius)
    }

    /// `color` as it reads on `active`: itself where it stands out there,
    /// else moved toward the text of selected entries. For secondary and
    /// muted text of a selected entry.
    pub fn on_active(&self, color: u32) -> u32 {
        if contrast(rgb(color).into(), rgb(self.active).into()) >= 3. {
            color
        } else {
            mix(color, self.active_text, 0.65)
        }
    }

    fn system_theme(scheme: ColorScheme) -> &'static str {
        match scheme {
            ColorScheme::Dark => "adwaita-dark",
            ColorScheme::Light => "adwaita-light",
        }
    }

    /// System mode owns its typography and density. Named theme preferences stay saved.
    pub fn apply_desktop(&mut self, cx: &App) {
        if !self.system_style {
            return;
        }
        if let Some(desktop) = cx.try_global::<titlebar::DesktopAppearance>() {
            self.font_family = desktop.font.family.clone();
            self.font_size = desktop.font.size;
            self.tab_font_size = desktop.font.size;
            self.tab_height = if desktop.gnome { 38. } else { 34. };
            self.radius = if desktop.gnome { 12. } else { 6. };
            self.control_radius = self.radius;
        }
    }

    pub fn fallback(ui_family: &str) -> Self {
        Self {
            system_style: false,
            window_decoration: titlebar::WindowDecoration::System,
            window_buttons: titlebar::WindowButtons::All,
            title_bar_style: titlebar::TitleBarStyle::Theme,
            surface: 0x10151c,
            raised: 0x18232f,
            overlay: 0x293b4d,
            text: 0xd9e2ef,
            secondary: 0x93a6b9,
            muted: 0x8ea3b7,
            border: 0x416783,
            subtle_border: 0x263544,
            active: 0x30475b,
            active_text: 0xd9e2ef,
            hover: 0x1f2c3a,
            menu_hover: 0x1f2c3a,
            menu_hover_text: 0xd9e2ef,
            accent: 0x76b8e8,
            font_family: ui_family.into(),
            font_size: 13.,
            tab_height: 32.,
            tab_font_size: 12.,
            tab_accent: None,
            danger: 0xe06c75,
            danger_hover: 0x3a2730,
            radius: 8.,
            control_radius: 8.,
            bevel: None,
            field: None,
            title_bar: TitleBarTheme::default(),
            success: 0x98c379,
            warning: 0xe5c07b,
            hover_overlay: 0xffffff0f,
            shadow: 0x00000073,
            drag_over_border: None,
            terminal: TerminalColors::default(),
            selection_foreground: 0xd9e2ef,
            selection_background: 0x345778,
            zoom: Zoom::default(),
            zoom_steps: ZoomSteps::default(),
            notice: None,
            scheme: ColorScheme::Dark,
            config: None,
        }
    }

    pub fn from_snapshot(
        snapshot: &AppearanceSnapshot,
        catalog: &ThemeCatalog,
        available_fonts: &[String],
        ui_family: &str,
        mono_family: &str,
    ) -> Self {
        let settings = snapshot.settings();
        let terminal = snapshot.terminal();
        let mut result = Self::resolve(
            &ThemeSelection {
                ui_theme: if settings.interface_style == "system" {
                    Self::system_theme(snapshot.os_scheme())
                } else {
                    snapshot.ui_theme()
                },
                os_scheme: snapshot.os_scheme(),
                terminal_theme: terminal.theme(),
                terminal_theme_dark: terminal.theme_dark(),
                terminal_theme_light: terminal.theme_light(),
                font_size: settings.font_size,
                font_family: &settings.font_family,
            },
            catalog,
            available_fonts,
            ui_family,
            mono_family,
        );
        result.title_bar_style = titlebar::TitleBarStyle::from_setting(&settings.title_bar_style);
        result.system_style = settings.interface_style == "system";
        result.window_decoration =
            titlebar::WindowDecoration::from_setting(&settings.window_decoration);
        result.window_buttons = titlebar::WindowButtons::from_setting(&settings.window_buttons);
        // The levels to step through and show: the service's limits, or the
        // fallback ones of a service without metadata.
        result.zoom_steps = ZoomSteps::from_limits(snapshot.limits(SettingsField::Zoom));
        result.zoom = Zoom::from_values(settings.zoom, &settings.zoom_scope, &result.zoom_steps);
        result
    }

    /// The presentation of the selection a window applied last, for the
    /// start of the next one: resolved as a snapshot of the same settings is,
    /// with the zoom levels of a service that names none.
    pub fn from_remembered(
        remembered: &RememberedAppearance,
        os_scheme: ColorScheme,
        catalog: &ThemeCatalog,
        available_fonts: &[String],
        ui_family: &str,
        mono_family: &str,
    ) -> Self {
        let mut result = Self::resolve(
            &ThemeSelection {
                ui_theme: if remembered.interface_style == "system" {
                    Self::system_theme(os_scheme)
                } else {
                    remembered.ui_theme(os_scheme)
                },
                os_scheme,
                terminal_theme: &remembered.terminal_theme,
                terminal_theme_dark: &remembered.terminal_theme_dark,
                terminal_theme_light: &remembered.terminal_theme_light,
                font_size: remembered.font_size,
                font_family: &remembered.font_family,
            },
            catalog,
            available_fonts,
            ui_family,
            mono_family,
        );
        result.title_bar_style = titlebar::TitleBarStyle::from_setting(&remembered.title_bar_style);
        result.system_style = remembered.interface_style == "system";
        result.window_decoration =
            titlebar::WindowDecoration::from_setting(&remembered.window_decoration);
        result.window_buttons = titlebar::WindowButtons::from_setting(&remembered.window_buttons);
        result.zoom =
            Zoom::from_values(remembered.zoom, &remembered.zoom_scope, &result.zoom_steps);
        result
    }

    /// The presentation of a selection. A name the catalog does not have
    /// falls back to a default and is reported in `notice`; the stored
    /// setting is not touched.
    pub fn resolve(
        selection: &ThemeSelection,
        catalog: &ThemeCatalog,
        available_fonts: &[String],
        ui_family: &str,
        mono_family: &str,
    ) -> Self {
        let mut result = Self::fallback(ui_family);
        let mut problems = Vec::new();
        let theme = catalog.ui(selection.ui_theme).or_else(|| {
            problems.push(format!(
                "Theme “{}” is unavailable; the default theme is shown.",
                selection.ui_theme
            ));
            catalog.default_ui(selection.os_scheme)
        });
        let Some(theme) = theme else {
            result.notice = Some(problems.join(" "));
            return result;
        };
        result.scheme = theme.scheme;
        result.config = Some(theme.config.clone());
        let dark = theme.scheme == ColorScheme::Dark;
        let mut kit = Theme::from(&*if dark {
            ThemeColor::dark()
        } else {
            ThemeColor::light()
        });
        kit.apply_config(&theme.config);
        result.derive_chrome(&kit, dark);
        let (theme_font_size, theme_font_family, (active_text, menu_hover), valid) =
            result.apply_block(&theme.chrome);
        if !valid {
            problems.push(format!(
                "Some `splitty` values of theme “{}” are unsupported; derived values are in use.",
                theme.name
            ));
        }
        // Primary text is kept readable on the surface, while secondary and
        // muted text keep the theme's values: their low contrast is how a
        // theme subdues idle entries against running and selected ones.
        result.text = readable_rgb(result.text, result.surface);
        result.active_text =
            active_text.unwrap_or_else(|| readable_rgb(result.text, result.active));
        result.menu_hover = menu_hover.unwrap_or(result.hover);
        result.menu_hover_text = readable_rgb(result.text, result.menu_hover);

        result.font_size = if (8..=24).contains(&selection.font_size) {
            selection.font_size as f32
        } else {
            theme_font_size.unwrap_or(DEFAULT_FONT_SIZE)
        };
        let family = if selection.font_family.trim().is_empty() {
            theme_font_family
        } else {
            Some(selection.font_family)
        };
        if let Some(family) = family {
            match choose_font(family, available_fonts, ui_family, mono_family) {
                Some(family) => result.font_family = family,
                None => problems.push("The interface font is not installed.".to_owned()),
            }
        }

        match terminal_palette(selection, theme, catalog, &mut problems) {
            Some(palette) => {
                result.terminal = palette.colors;
                result.selection_background = palette.selection_background.unwrap_or_else(|| {
                    mix(palette.colors.background, palette.colors.foreground, 0.25)
                });
                result.selection_foreground = palette
                    .selection_foreground
                    .unwrap_or(palette.colors.foreground);
            }
            None => problems.push("No terminal palette is available.".to_owned()),
        }
        result.notice = (!problems.is_empty()).then(|| problems.join(" "));
        result
    }

    /// The chrome a theme without a `splitty` block gets from its toolkit
    /// colors.
    fn derive_chrome(&mut self, kit: &Theme, dark: bool) {
        let colors = &kit.colors;
        self.surface = solid(colors.background, if dark { 0 } else { 0xffffff });
        self.raised = solid(colors.title_bar, self.surface);
        self.overlay = solid(colors.popover, self.surface);
        self.text = solid(colors.foreground, self.surface);
        self.secondary = solid(colors.muted_foreground, self.surface);
        self.muted = mix(self.secondary, self.surface, 0.35);
        self.border = solid(colors.border, self.surface);
        self.subtle_border = solid(colors.title_bar_border, self.raised);
        self.active = solid(colors.accent, self.raised);
        // A selected entry has to stand out from the surface it lies on.
        if contrast(rgb(self.active).into(), rgb(self.raised).into()) < 1.08 {
            self.active = mix(self.raised, self.text, 0.12);
        }
        self.hover = mix(self.raised, self.active, 0.5);
        self.accent = solid(colors.primary, self.surface);
        self.danger = solid(colors.danger, self.overlay);
        self.danger_hover = mix(self.overlay, self.danger, 0.18);
        self.success = solid(colors.success, self.surface);
        self.warning = solid(colors.warning, self.surface);
        self.radius = f32::from(kit.radius).clamp(0., 24.);
        self.control_radius = self.radius;
        self.bevel = None;
        self.field = None;
        self.title_bar = TitleBarTheme::default();
        self.hover_overlay = (self.text << 8) | 0x0f;
        self.shadow = if dark { 0x00000073 } else { 0x00000033 };
        self.drag_over_border = None;
        self.tab_height = 32.;
        self.tab_font_size = 12.;
        self.tab_accent = None;
    }

    /// Lay the theme's `splitty` block over the derived chrome. Returns the
    /// block's font size and family, its text color on `active` and its menu
    /// hover color, and whether every present value was supported; an
    /// unsupported one keeps its derived value.
    #[allow(clippy::type_complexity)]
    fn apply_block<'a>(
        &mut self,
        block: &'a ChromeBlock,
    ) -> (
        Option<f32>,
        Option<&'a str>,
        (Option<u32>, Option<u32>),
        bool,
    ) {
        let mut valid = true;
        let mut color = |target: &mut u32, value: &Option<String>, under: u32| {
            if let Some(value) = value {
                match parse_color(value) {
                    Some(color) => *target = color.over(under),
                    None => valid = false,
                }
            }
        };
        let surface = self.surface;
        color(&mut self.surface, &block.surface, surface);
        let surface = self.surface;
        color(&mut self.raised, &block.raised, surface);
        let raised = self.raised;
        color(&mut self.overlay, &block.overlay, raised);
        let overlay = self.overlay;
        color(&mut self.text, &block.text, surface);
        color(&mut self.secondary, &block.text_secondary, surface);
        color(&mut self.muted, &block.text_muted, surface);
        color(&mut self.border, &block.border, surface);
        color(&mut self.subtle_border, &block.border_subtle, surface);
        color(&mut self.active, &block.active, raised);
        color(&mut self.hover, &block.hover, raised);
        color(&mut self.accent, &block.accent, surface);
        color(&mut self.danger, &block.danger, overlay);
        color(&mut self.danger_hover, &block.danger_hover, overlay);
        color(&mut self.success, &block.success, surface);
        color(&mut self.warning, &block.warning, surface);
        let mut translucent = |target: &mut u32, value: &Option<String>| {
            if let Some(value) = value {
                match parse_color(value) {
                    Some(color) => *target = color.rgba(),
                    None => valid = false,
                }
            }
        };
        translucent(&mut self.hover_overlay, &block.hover_overlay);
        translucent(&mut self.shadow, &block.shadow);
        let mut length = |target: &mut f32, value: Option<f32>, min: f32, max: f32| {
            if let Some(value) = value {
                if value.is_finite() && (min..=max).contains(&value) {
                    *target = value;
                } else {
                    valid = false;
                }
            }
        };
        length(&mut self.tab_height, block.tab_height, 16., 64.);
        length(&mut self.tab_font_size, block.tab_font_size, 8., 24.);
        length(&mut self.radius, block.radius, 0., 24.);
        self.control_radius = self.radius;
        length(&mut self.control_radius, block.control_radius, 0., 24.);
        let mut title_height = 0.;
        let title_block = block.title_bar.as_ref();
        length(
            &mut title_height,
            title_block.and_then(|bar| bar.height),
            20.,
            64.,
        );
        let mut font_size = DEFAULT_FONT_SIZE;
        length(&mut font_size, block.font_size, 8., 24.);
        let font_size = block.font_size.map(|_| font_size);
        let bounded = |value: f32| value.is_finite() && (0. ..=8.).contains(&value);
        match &block.drag_over_border {
            None => {}
            Some(Maybe::Keyword(keyword)) if keyword == "none" => self.drag_over_border = None,
            Some(Maybe::Value(border)) if bounded(border.width) => {
                match parse_color(&border.color) {
                    Some(color) => {
                        self.drag_over_border =
                            (border.width > 0. && color.alpha > 0.).then(|| DragOverBorder {
                                width: border.width,
                                color: color.over(0),
                            })
                    }
                    None => valid = false,
                }
            }
            Some(_) => valid = false,
        }
        match &block.tab_accent {
            None => {}
            Some(Maybe::Keyword(keyword)) if keyword == "none" => self.tab_accent = None,
            Some(Maybe::Value(accent))
                if bounded(accent.thickness)
                    && matches!(accent.position.as_str(), "" | "top" | "bottom") =>
            {
                match parse_color(&accent.color) {
                    Some(color) => {
                        self.tab_accent =
                            (accent.thickness > 0. && color.alpha > 0.).then(|| TabAccent {
                                color: color.over(self.raised),
                                thickness: accent.thickness,
                                bottom: accent.position == "bottom",
                            })
                    }
                    None => valid = false,
                }
            }
            Some(_) => valid = false,
        }
        let active_text = block.active_text.as_ref().and_then(|value| {
            let color = parse_color(value).map(|color| color.over(self.active));
            valid &= color.is_some();
            color
        });
        let menu_hover = block.menu_hover.as_ref().and_then(|value| {
            let color = parse_color(value).map(|color| color.over(self.overlay));
            valid &= color.is_some();
            color
        });
        if let Some(value) = &block.field {
            let color = parse_color(value).map(|color| color.over(self.surface));
            valid &= color.is_some();
            self.field = color.or(self.field);
        }
        match &block.bevel {
            None => {}
            Some(Maybe::Keyword(keyword)) if keyword == "none" => self.bevel = None,
            Some(Maybe::Value(bevel)) => {
                let color = |value: &str| parse_color(value).map(|color| color.over(self.surface));
                match (
                    color(&bevel.highlight),
                    color(&bevel.light),
                    color(&bevel.shadow),
                    color(&bevel.dark_shadow),
                ) {
                    (Some(highlight), Some(light), Some(shadow), Some(dark_shadow)) => {
                        self.bevel = Some(Bevel {
                            highlight,
                            light,
                            shadow,
                            dark_shadow,
                        })
                    }
                    _ => valid = false,
                }
            }
            Some(_) => valid = false,
        }
        if let Some(bar) = title_block {
            let raised = self.raised;
            let color = |value: &str| parse_color(value).map(|color| color.over(raised));
            let fill = |value: &FillBlock| match value {
                FillBlock::Color(value) => color(value).map(BarFill::Solid),
                FillBlock::Gradient { from, to } => color(from)
                    .zip(color(to))
                    .map(|(from, to)| BarFill::Gradient { from, to }),
            };
            // Each present value is taken alone, so one unsupported value
            // does not cost the theme the rest of its title bar.
            let mut take = |target: &mut dyn FnMut(), ok: bool| {
                if ok {
                    target()
                } else {
                    valid = false
                }
            };
            let theme = &mut self.title_bar;
            if let Some(value) = &bar.background {
                let fill = fill(value);
                take(&mut || theme.background = fill, fill.is_some());
            }
            if let Some(value) = &bar.inactive_background {
                let fill = fill(value);
                take(&mut || theme.inactive_background = fill, fill.is_some());
            }
            if let Some(value) = &bar.foreground {
                let color = color(value);
                take(&mut || theme.foreground = color, color.is_some());
            }
            if let Some(value) = &bar.inactive_foreground {
                let color = color(value);
                take(&mut || theme.inactive_foreground = color, color.is_some());
            }
            if bar.height.is_some() && title_height > 0. {
                theme.height = Some(title_height);
            }
            if let Some(value) = &bar.align {
                let align = match value.as_str() {
                    "center" => Some(TitleAlign::Center),
                    "start" => Some(TitleAlign::Start),
                    _ => None,
                };
                take(
                    &mut || theme.align = align.unwrap_or_default(),
                    align.is_some(),
                );
            }
            if let Some(value) = &bar.weight {
                let weight = match value.as_str() {
                    "normal" => Some(FontWeight::NORMAL),
                    "medium" => Some(FontWeight::MEDIUM),
                    "semibold" => Some(FontWeight::SEMIBOLD),
                    "bold" => Some(FontWeight::BOLD),
                    _ => None,
                };
                take(&mut || theme.weight = weight, weight.is_some());
            }
            if let Some(value) = &bar.buttons {
                let buttons = match value.as_str() {
                    "plain" => Some(CaptionButtons::Plain),
                    "boxed" => Some(CaptionButtons::Boxed),
                    "wide" => Some(CaptionButtons::Wide),
                    _ => None,
                };
                take(
                    &mut || theme.buttons = buttons.unwrap_or_default(),
                    buttons.is_some(),
                );
            }
        }
        (
            font_size,
            block.font_family.as_deref(),
            (active_text, menu_hover),
            valid,
        )
    }

    /// Give the components the toolkit theme, with the chrome's font.
    pub fn apply_component_theme(&self, cx: &mut App) {
        Theme::change(
            match self.scheme {
                ColorScheme::Light => ThemeMode::Light,
                ColorScheme::Dark => ThemeMode::Dark,
            },
            None,
            cx,
        );
        Theme::update(cx, |theme| {
            if let Some(config) = &self.config {
                theme.apply_config(config);
                // A theme without highlight colors takes the toolkit's for
                // its mode, not those of the theme shown before.
                if config.highlight.is_none() {
                    theme.highlight_theme = if config.mode.is_dark() {
                        HighlightTheme::default_dark()
                    } else {
                        HighlightTheme::default_light()
                    };
                }
            }
            theme.font_family = self.font_family.clone().into();
            theme.font_size = px(16. * self.zoom.window_scale());
        });
    }
}

/// The terminal palette of a selection: the one the server's rule chooses
/// for the UI theme's mode (the named palette, else the one named for the
/// mode), looked up in the catalog; else the palette with the UI theme's
/// name; else the default palette of that mode.
fn terminal_palette<'a>(
    selection: &ThemeSelection,
    theme: &UiTheme,
    catalog: &'a ThemeCatalog,
    problems: &mut Vec<String>,
) -> Option<&'a TerminalPalette> {
    terminal_palette_name(
        selection.terminal_theme,
        selection.terminal_theme_dark,
        selection.terminal_theme_light,
        theme.scheme,
    )
    .and_then(|name| {
        let palette = catalog.palette(name);
        if palette.is_none() {
            problems.push(format!(
                "Terminal palette “{name}” is unavailable; the theme's palette is shown."
            ));
        }
        palette
    })
    .or_else(|| catalog.palette_for_theme(&theme.id))
    .or_else(|| catalog.default_palette(theme.scheme))
}

/// A toolkit color as `0xRRGGBB`, composed over `under` where it is translucent.
fn solid(color: Hsla, under: u32) -> u32 {
    let color = Rgba::from(color);
    Color {
        rgb: [color.r * 255., color.g * 255., color.b * 255.],
        alpha: color.a,
    }
    .over(under)
}

/// `amount` of the way from `from` to `to`, per channel.
fn mix(from: u32, to: u32, amount: f32) -> u32 {
    let mut packed = 0;
    for shift in [16, 8, 0] {
        let a = ((from >> shift) & 255) as f32;
        let b = ((to >> shift) & 255) as f32;
        packed |= ((a + (b - a) * amount).round() as u32) << shift;
    }
    packed
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

    fn selection(ui_theme: &str) -> ThemeSelection<'_> {
        ThemeSelection {
            ui_theme,
            os_scheme: ColorScheme::Dark,
            terminal_theme: "default",
            terminal_theme_dark: "default",
            terminal_theme_light: "default",
            font_size: 0,
            font_family: "",
        }
    }
    fn resolve(selection: &ThemeSelection, catalog: &ThemeCatalog) -> NativeAppearance {
        NativeAppearance::resolve(selection, catalog, &[], "UI", "Mono")
    }

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
    fn the_default_theme_keeps_its_chrome_and_terminal_colors() {
        let catalog = ThemeCatalog::load(None);
        let dark = resolve(&selection("splitty-dark"), &catalog);
        assert_eq!(dark.notice, None);
        assert_eq!(dark.scheme, ColorScheme::Dark);
        assert_eq!(
            (dark.surface, dark.raised, dark.overlay, dark.text),
            (0x0f0f0f, 0x1a1a1a, 0x1e1e1e, 0xc8c8c8)
        );
        assert_eq!((dark.secondary, dark.muted), (0x6b6b6b, 0x505050));
        assert_eq!(
            (dark.active, dark.hover, dark.accent),
            (0x313131, 0x282828, 0x63a0ff)
        );
        assert_eq!((dark.hover_overlay, dark.shadow), (0xffffff0f, 0x00000073));
        assert_eq!(
            (dark.tab_height, dark.tab_font_size, dark.radius),
            (32., 12., 8.)
        );
        assert_eq!((dark.tab_accent, dark.drag_over_border), (None, None));
        assert_eq!((dark.font_size, dark.font_family.as_str()), (12., "UI"));
        assert_eq!(dark.terminal.background, 0x0f0f0f);
        assert_eq!(dark.terminal.ansi[4], 0x63a0ff);
        assert_eq!(
            (dark.selection_background, dark.selection_foreground),
            (0x2a2a2a, 0xc8c8c8)
        );
        let light = resolve(&selection("splitty-light"), &catalog);
        assert_eq!(
            (light.notice.as_deref(), light.scheme),
            (None, ColorScheme::Light)
        );
        assert!(luminance(rgb(light.surface).into()) > luminance(rgb(light.text).into()));
        assert!(luminance(rgb(light.terminal.background).into()) > 0.5);
    }

    /// settings, Native Appearance Before The Local Service Answers.
    #[test]
    fn a_remembered_selection_resolves_as_the_settings_it_was_taken_from() {
        let catalog = ThemeCatalog::load(None);
        let mut remembered = RememberedAppearance {
            interface_style: "theme".into(),
            window_decoration: "system".into(),
            window_buttons: "all".into(),
            title_bar_style: "theme".into(),
            theme_mode: "auto".into(),
            theme: "splitty-dark".into(),
            theme_dark: "splitty-dark".into(),
            theme_light: "splitty-light".into(),
            font_size: 15,
            font_family: String::new(),
            zoom: 1.25,
            zoom_scope: "window".into(),
            terminal_theme: "default".into(),
            terminal_theme_dark: "default".into(),
            terminal_theme_light: "default".into(),
        };
        let shown = |remembered: &RememberedAppearance, scheme| {
            NativeAppearance::from_remembered(remembered, scheme, &catalog, &[], "UI", "Mono")
        };
        let original = remembered.clone();
        remembered.interface_style = "system".into();
        let system = shown(&remembered, ColorScheme::Light);
        assert!(system.system_style);
        assert_eq!(system.scheme, ColorScheme::Light);
        assert_eq!(
            system.surface,
            resolve(&selection("adwaita-light"), &catalog).surface
        );
        remembered.interface_style = "theme".into();
        assert_eq!(remembered, original);
        let light = shown(&remembered, ColorScheme::Light);
        let expected = resolve(
            &ThemeSelection {
                os_scheme: ColorScheme::Light,
                font_size: 15,
                ..selection("splitty-light")
            },
            &catalog,
        );
        assert_eq!(
            (light.notice.as_deref(), light.scheme),
            (None, ColorScheme::Light)
        );
        assert_eq!(
            (
                light.surface,
                light.text,
                light.font_size,
                light.terminal.background
            ),
            (
                expected.surface,
                expected.text,
                15.,
                expected.terminal.background
            )
        );
        assert_eq!(
            (light.zoom.level(), light.zoom.window_scale()),
            (1.25, 1.25)
        );
        assert_eq!(
            shown(&remembered, ColorScheme::Dark).scheme,
            ColorScheme::Dark
        );

        // A level the built-in steps do not hold waits for the service's.
        remembered.zoom = 40.;
        remembered.zoom_scope = "terminals".into();
        let zoom = shown(&remembered, ColorScheme::Dark).zoom;
        assert_eq!((zoom.level(), zoom.window_scale()), (1., 1.));
        // A theme that is gone shows the default one of the preference.
        remembered.theme_light = "removed-theme".into();
        let missing = shown(&remembered, ColorScheme::Light);
        assert_eq!(missing.scheme, ColorScheme::Light);
        assert!(missing.notice.is_some());
    }

    #[test]
    fn every_bundled_theme_resolves_with_readable_chrome_and_a_palette_of_its_mode() {
        let catalog = ThemeCatalog::load(None);
        for theme in catalog.ui_themes() {
            let shown = resolve(&selection(&theme.id), &catalog);
            assert_eq!(shown.notice, None, "{}", theme.id);
            assert_eq!(shown.scheme, theme.scheme, "{}", theme.id);
            assert!(
                contrast(rgb(shown.text).into(), rgb(shown.surface).into()) >= 4.5,
                "{}",
                theme.id
            );
            let terminal_dark = luminance(rgb(shown.terminal.background).into())
                < luminance(rgb(shown.terminal.foreground).into());
            assert_eq!(
                terminal_dark,
                theme.scheme == ColorScheme::Dark,
                "{}",
                theme.id
            );
        }
        // A third-party theme derives its chrome from the toolkit colors and
        // takes the terminal palette of its own name.
        let mocha = resolve(&selection("catppuccin-mocha"), &catalog);
        assert_eq!(mocha.terminal.background, 0x1e1e2e);
        assert_ne!(mocha.surface, NativeAppearance::fallback("UI").surface);
        assert_eq!((mocha.tab_height, mocha.font_size), (32., 12.));
        // Names that differ only in their separators still pair.
        let tokyo = resolve(&selection("tokyo-night"), &catalog);
        assert_eq!(
            tokyo.terminal.background,
            catalog.palette("tokyonight").unwrap().colors.background
        );
        for theme in catalog.ui_themes() {
            let shown = resolve(&selection(&theme.id), &catalog);
            assert!(
                contrast(rgb(shown.active).into(), rgb(shown.raised).into()) >= 1.08,
                "{}",
                theme.id
            );
        }
    }

    #[test]
    fn the_terminal_palette_follows_the_setting_then_the_mode_pair_then_the_theme() {
        let catalog = ThemeCatalog::load(None);
        let background = |selection: &ThemeSelection| {
            let shown = resolve(selection, &catalog);
            (shown.terminal.background, shown.notice)
        };
        let of = |id: &str| catalog.palette(id).unwrap().colors.background;
        let mut chosen = selection("splitty-dark");
        chosen.terminal_theme_dark = "catppuccin-mocha";
        chosen.terminal_theme_light = "catppuccin-latte";
        assert_eq!(background(&chosen), (of("catppuccin-mocha"), None));
        chosen.ui_theme = "splitty-light";
        assert_eq!(background(&chosen), (of("catppuccin-latte"), None));
        chosen.terminal_theme = "nord";
        assert_eq!(background(&chosen), (of("nord"), None));
        // An unavailable palette shows the theme's and says so.
        chosen.terminal_theme = "no-such-palette";
        let (shown, notice) = background(&chosen);
        assert_eq!(shown, of("splitty-light"));
        assert!(notice.unwrap().contains("no-such-palette"));
        // A theme without a palette of its name takes the default of its mode.
        let unpaired = catalog
            .ui_themes()
            .iter()
            .find(|theme| {
                catalog.palette(&theme.id).is_none() && theme.scheme == ColorScheme::Light
            })
            .expect("a bundled light theme without a palette of its name");
        assert_eq!(
            background(&selection(&unpaired.id)),
            (of("splitty-light"), None)
        );
    }

    #[test]
    fn an_unavailable_theme_shows_the_default_of_the_system_mode_and_says_so() {
        let catalog = ThemeCatalog::load(None);
        let mut missing = selection("no-such-theme");
        missing.os_scheme = ColorScheme::Light;
        let shown = resolve(&missing, &catalog);
        assert_eq!(shown.scheme, ColorScheme::Light);
        assert_eq!(
            shown.surface,
            resolve(&selection("splitty-light"), &catalog).surface
        );
        assert!(shown.notice.unwrap().contains("no-such-theme"));
    }

    #[test]
    fn the_splitty_block_overrides_derived_chrome_within_bounds() {
        let block: ChromeBlock = serde_json::from_value(serde_json::json!({
            "surface": "#101010", "raised": "#ffffff80", "hover-overlay": "#00000020",
            "tab-height": 28, "tab-font-size": 11, "radius": 0, "font-size": 14,
            "font-family": "monospace",
            "tab-accent": {"color": "#ff0000", "thickness": 2, "position": "bottom"},
            "drag-over-border": {"width": 1, "color": "#00ff00"},
        }))
        .unwrap();
        let mut shown = NativeAppearance::fallback("UI");
        assert_eq!(
            shown.apply_block(&block),
            (Some(14.), Some("monospace"), (None, None), true)
        );
        // Controls follow the panel radius until the theme sets their own.
        assert_eq!((shown.control_radius, shown.corner(6.)), (0., 0.));
        assert_eq!((shown.surface, shown.raised), (0x101010, 0x888888));
        assert_eq!(shown.hover_overlay, 0x00000020);
        assert_eq!(
            (shown.tab_height, shown.tab_font_size, shown.radius),
            (28., 11., 0.)
        );
        assert_eq!(
            shown.tab_accent,
            Some(TabAccent {
                color: 0xff0000,
                thickness: 2.,
                bottom: true
            })
        );
        assert_eq!(
            shown.drag_over_border,
            Some(DragOverBorder {
                width: 1.,
                color: 0x00ff00
            })
        );

        let unsupported: ChromeBlock = serde_json::from_value(serde_json::json!({
            "surface": "var(--red)", "tab-height": 200, "font-size": 2,
            "tab-accent": "dotted", "drag-over-border": {"width": 40, "color": "#fff"},
            "accent": "#123456",
        }))
        .unwrap();
        let derived = NativeAppearance::fallback("UI");
        let mut shown = derived.clone();
        assert_eq!(
            shown.apply_block(&unsupported),
            (Some(DEFAULT_FONT_SIZE), None, (None, None), false)
        );
        assert_eq!(
            (
                shown.surface,
                shown.tab_height,
                shown.tab_accent,
                shown.drag_over_border
            ),
            (derived.surface, derived.tab_height, None, None)
        );
        assert_eq!(shown.accent, 0x123456);
    }

    /// settings, Native Theme Catalog: the shape tokens.
    #[test]
    fn the_splitty_block_gives_a_theme_its_edges_corners_and_title_bar() {
        let block: ChromeBlock = serde_json::from_value(serde_json::json!({
            "surface": "#c0c0c0", "raised": "#c0c0c0", "active": "#000080",
            "active-text": "#ffffff", "menu-hover": "#000080", "field": "#ffffff", "radius": 8,
            "control-radius": 4,
            "bevel": {"highlight": "#ffffff", "light": "#dfdfdf",
                      "shadow": "#808080", "dark-shadow": "#000000"},
            "title-bar": {
                "background": {"from": "#000080", "to": "#1084d0"},
                "foreground": "#ffffff",
                "inactive-background": "#808080",
                "inactive-foreground": "#c0c0c0",
                "height": 22, "align": "start", "weight": "bold", "buttons": "boxed",
            },
        }))
        .unwrap();
        let mut shown = NativeAppearance::fallback("UI");
        assert_eq!(
            shown.apply_block(&block),
            (None, None, (Some(0xffffff), Some(0x000080)), true)
        );
        assert_eq!(
            shown.bevel,
            Some(Bevel {
                highlight: 0xffffff,
                light: 0xdfdfdf,
                shadow: 0x808080,
                dark_shadow: 0x000000
            })
        );
        assert_eq!((shown.radius, shown.control_radius), (8., 4.));
        assert_eq!(shown.field, Some(0xffffff));
        assert_eq!((shown.corner(6.), shown.corner(3.)), (4., 3.));
        assert_eq!(
            shown.title_bar,
            TitleBarTheme {
                background: Some(BarFill::Gradient {
                    from: 0x000080,
                    to: 0x1084d0
                }),
                foreground: Some(0xffffff),
                inactive_background: Some(BarFill::Solid(0x808080)),
                inactive_foreground: Some(0xc0c0c0),
                height: Some(22.),
                align: TitleAlign::Start,
                weight: Some(FontWeight::BOLD),
                buttons: CaptionButtons::Boxed,
            }
        );
        assert_eq!(BarFill::Solid(0x808080).start(), 0x808080);

        // An unsupported value costs only itself.
        let partly: ChromeBlock = serde_json::from_value(serde_json::json!({
            "control-radius": 99, "active-text": "navy", "bevel": {
                "highlight": "#fff", "light": "#ddd", "shadow": "gray", "dark-shadow": "#000"},
            "title-bar": {"background": {"from": "#000080", "to": "blue"}, "foreground": "#fff",
                          "height": 4, "align": "right", "weight": "black", "buttons": "wide"},
        }))
        .unwrap();
        let mut shown = NativeAppearance::fallback("UI");
        assert_eq!(
            shown.apply_block(&partly),
            (None, None, (None, None), false)
        );
        assert_eq!((shown.bevel, shown.control_radius), (None, 8.));
        assert_eq!(
            shown.title_bar,
            TitleBarTheme {
                foreground: Some(0xffffff),
                buttons: CaptionButtons::Wide,
                ..TitleBarTheme::default()
            }
        );
        let none: ChromeBlock =
            serde_json::from_value(serde_json::json!({"bevel": "none"})).unwrap();
        assert!(shown.apply_block(&none).3);
    }

    #[test]
    fn text_on_a_selected_entry_is_readable_without_a_token() {
        let catalog = ThemeCatalog::load(None);
        let dark = resolve(&selection("splitty-dark"), &catalog);
        // A theme whose text already reads on `active` keeps it.
        assert_eq!(dark.active_text, dark.text);
        assert_eq!(
            (dark.menu_hover, dark.menu_hover_text),
            (dark.hover, dark.text)
        );
        // Subdued text stays where it reads and is lifted where it does not.
        let mut navy = dark.clone();
        (navy.active, navy.active_text) = (0x000080, 0xffffff);
        assert_eq!(navy.on_active(0xc0c0c0), 0xc0c0c0);
        let lifted: Hsla = rgb(navy.on_active(0x404040)).into();
        assert!(contrast(lifted, rgb(0x000080).into()) >= 3.);
        let background: Hsla = rgb(0x000080).into();
        let lifted: Hsla = rgb(readable_rgb(0x000000, 0x000080)).into();
        assert!(contrast(lifted, background) >= 4.5);
    }

    #[test]
    fn fonts_come_from_the_setting_then_the_theme_and_must_be_installed() {
        let catalog = ThemeCatalog::load(None);
        let fonts = ["Inter".to_owned(), "JetBrains Mono".to_owned()];
        let show = |selection: &ThemeSelection| {
            NativeAppearance::resolve(selection, &catalog, &fonts, "UI", "Mono")
        };
        let mut chosen = selection("splitty-dark");
        chosen.font_size = 16;
        chosen.font_family = "'Missing Font', inter, sans-serif";
        let shown = show(&chosen);
        assert_eq!(
            (shown.font_size, shown.font_family.as_str()),
            (16., "Inter")
        );
        chosen.font_family = "ui-monospace";
        assert_eq!(show(&chosen).font_family, "Mono");
        chosen.font_size = 99;
        chosen.font_family = "Missing Font";
        let shown = show(&chosen);
        assert_eq!((shown.font_size, shown.font_family.as_str()), (12., "UI"));
        assert!(shown.notice.unwrap().contains("font"));
    }
}
