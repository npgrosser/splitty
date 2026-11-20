//! Resolves window chrome separately from application and terminal appearance.
//! Desktop conventions own geometry and controls; the selected source owns all
//! colors of client decorations. Server decorations keep the compositor's colors.
use super::{BarFill, Bevel, CaptionButtons, NativeAppearance, TitleAlign};

mod icons;
use gpui_kit::{FontWeight, Global, WindowAppearance, WindowButton, WindowButtonLayout};
pub use icons::DesktopWindowIcons;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowDecoration {
    #[default]
    System,
    Splitty,
}

impl WindowDecoration {
    pub fn from_setting(value: &str) -> Self {
        if value == "splitty" {
            Self::Splitty
        } else {
            Self::System
        }
    }

    pub fn native(self, colors: TitleBarStyle) -> bool {
        self == Self::System && colors == TitleBarStyle::System
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowButtons {
    System,
    #[default]
    All,
}

impl WindowButtons {
    pub fn from_setting(value: &str) -> Self {
        if value == "system" {
            Self::System
        } else {
            Self::All
        }
    }

    pub fn layout(self, desktop: Option<WindowButtonLayout>, gnome: bool) -> WindowButtonLayout {
        if self == Self::System {
            if let Some(layout) = desktop {
                return layout;
            }
            if gnome {
                return WindowButtonLayout {
                    left: [None; 3],
                    right: [None, None, Some(WindowButton::Close)],
                };
            }
        }
        WindowButtonLayout {
            left: [None; 3],
            right: [
                Some(WindowButton::Minimize),
                Some(WindowButton::Maximize),
                Some(WindowButton::Close),
            ],
        }
    }
}

/// Local desktop discovery, never written to the service's theme settings.
#[derive(Clone, Debug)]
pub struct DesktopAppearance {
    pub font: SystemFont,
    pub gnome: bool,
}
impl Global for DesktopAppearance {}

impl DesktopAppearance {
    pub fn detect(cx: &gpui_kit::App) -> Self {
        use gpui_kit::component::ActiveTheme as _;
        let gnome = cfg!(target_os = "linux")
            && std::env::var("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .split(':')
                .any(|name| name.eq_ignore_ascii_case("gnome"));
        let fonts = cx.text_system().all_font_names();
        let family = if gnome {
            ["Adwaita Sans", "Cantarell"]
                .into_iter()
                .find(|name| fonts.iter().any(|font| font == name))
                .map(str::to_owned)
                .unwrap_or_else(|| cx.theme().font_family.to_string())
        } else {
            cx.theme().font_family.to_string()
        };
        Self {
            font: SystemFont {
                family,
                size: if gnome { 14.666667 } else { 13. },
            },
            gnome,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TitleBarStyle {
    System,
    #[default]
    Theme,
}

impl TitleBarStyle {
    pub fn from_setting(value: &str) -> Self {
        match value {
            "system" => Self::System,
            _ => Self::Theme,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SystemFont {
    pub family: String,
    pub size: f32,
}

impl SystemFont {
    /// GNOME's interface font is a Pango description, with a trailing point
    /// size (or an explicit px size). Do not treat that size as physical pixels.
    #[cfg(any(target_os = "linux", test))]
    pub fn parse(description: &str) -> Option<Self> {
        if description.len() > 256 || description.chars().any(char::is_control) {
            return None;
        }
        let (family, size) = description.trim().rsplit_once(' ')?;
        let (size, scale) = size
            .strip_suffix("px")
            .map_or((size, 96. / 72.), |s| (s, 1.));
        let size: f32 = size.parse().ok()?;
        if family.trim().is_empty() || !size.is_finite() || !(4. ..=72.).contains(&size) {
            return None;
        }
        Some(Self {
            family: family.trim().into(),
            size: size * scale,
        })
    }
}

/// Explicit toolkit conventions, independent of the title-bar color source.
/// Metrics follow libadwaita 1.10 header-bar styles; symbolic artwork is loaded
/// from the installed icon theme, independently of this profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationProfile {
    Adwaita,
    Standard,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecorationMetrics {
    pub button_size: f32,
    pub button_gap: f32,
    pub face_size: f32,
    pub icon_size: f32,
    pub side_padding: f32,
    pub round_buttons: bool,
    pub title_weight: FontWeight,
    pub inactive_title_opacity: f32,
    pub inactive_controls_opacity: f32,
}

impl DecorationMetrics {
    pub fn title_inset(self, buttons: usize) -> f32 {
        let buttons = buttons.max(1);
        self.side_padding
            + buttons as f32 * self.button_size
            + (buttons - 1) as f32 * self.button_gap
            + 12.
    }
}

impl DecorationProfile {
    pub fn control_padding(self, frame_border: gpui_kit::Pixels) -> gpui_kit::Pixels {
        let padding = gpui_kit::px(self.metrics().side_padding);
        match self {
            // Adwaita's header extends 1px beyond each side. Its window outline
            // consumes no layout space; our client border does. Account for both,
            // including the missing border at a maximized or tiled edge.
            Self::Adwaita => padding - gpui_kit::px(1.) - frame_border,
            Self::Standard => padding,
        }
    }

    pub fn metrics(self) -> DecorationMetrics {
        match self {
            Self::Adwaita => DecorationMetrics {
                button_size: 34.,
                button_gap: 3.,
                face_size: 24.,
                icon_size: 16.,
                side_padding: 7.,
                round_buttons: true,
                title_weight: FontWeight::BOLD,
                inactive_title_opacity: 0.5,
                inactive_controls_opacity: 0.5,
            },
            Self::Standard => DecorationMetrics {
                button_size: 32.,
                button_gap: 8.,
                face_size: 24.,
                icon_size: 12.,
                side_padding: 8.,
                round_buttons: false,
                title_weight: FontWeight::SEMIBOLD,
                inactive_title_opacity: 0.55,
                inactive_controls_opacity: 1.,
            },
        }
    }
}

/// Size and spacing of the caption buttons of a bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptionMetrics {
    pub width: f32,
    pub height: f32,
    pub gap: f32,
    /// Extra space that sets close off from the other buttons.
    pub close_gap: f32,
    pub side_padding: f32,
}

#[derive(Clone, Debug)]
pub struct TitleBarAppearance {
    pub background: BarFill,
    pub foreground: u32,
    /// The bar of a window without focus; `None` keeps the active fill.
    pub inactive_background: Option<BarFill>,
    /// The title of a window without focus; `None` dims `foreground` by the
    /// profile's inactive opacities.
    pub inactive_foreground: Option<u32>,
    pub button: u32,
    pub hover: u32,
    pub height: f32,
    pub font: SystemFont,
    pub weight: FontWeight,
    pub align: TitleAlign,
    pub caption: CaptionButtons,
    /// Boxed caption buttons: their face, glyph color, the flat border used
    /// without a bevel, and the bevel. The bevel also draws the window frame.
    pub face: u32,
    pub glyph: u32,
    pub edge: u32,
    pub bevel: Option<Bevel>,
    pub profile: DecorationProfile,
    pub desktop_gnome: bool,
    pub icons: DesktopWindowIcons,
    pub radius: f32,
    pub buttons: WindowButtons,
}

impl TitleBarAppearance {
    /// Theme title-bar colors apply whenever title-bar colors follow the
    /// theme; the theme's title-bar shape and frame radius only under
    /// Splitty decoration, whose geometry the application owns.
    pub fn resolve(
        style: TitleBarStyle,
        app: &NativeAppearance,
        system: WindowAppearance,
        gnome: bool,
        font: SystemFont,
    ) -> Self {
        let dark = matches!(
            system,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let theme = &app.title_bar;
        let (background, foreground, button, hover) = match style {
            TitleBarStyle::Theme => (
                theme.background.unwrap_or(BarFill::Solid(app.raised)),
                theme.foreground.unwrap_or(app.text),
                app.hover,
                app.active,
            ),
            TitleBarStyle::System if dark => {
                (BarFill::Solid(0x303030), 0xffffff, 0x454545, 0x505050)
            }
            TitleBarStyle::System => (BarFill::Solid(0xebebeb), 0x2e3436, 0xd9d9d9, 0xcccccc),
        };
        let themed = style == TitleBarStyle::Theme;
        let (face, glyph, edge, bevel) = if themed {
            (app.surface, app.text, app.border, app.bevel)
        } else {
            (button, foreground, hover, None)
        };
        let system_geometry = app.window_decoration == WindowDecoration::System;
        let adwaita = gnome && system_geometry;
        let font = if system_geometry {
            font
        } else {
            SystemFont {
                family: app.font_family.clone(),
                size: app.font_size,
            }
        };
        let profile = if adwaita {
            DecorationProfile::Adwaita
        } else {
            DecorationProfile::Standard
        };
        let derived_height = if adwaita {
            46_f32.max(font.size + 24.)
        } else {
            36_f32.max(font.size + 16.)
        };
        let shape = (!system_geometry).then_some(theme);
        Self {
            background,
            foreground,
            inactive_background: themed.then_some(theme.inactive_background).flatten(),
            inactive_foreground: themed.then_some(theme.inactive_foreground).flatten(),
            button,
            hover,
            height: shape
                .and_then(|theme| theme.height)
                .unwrap_or(derived_height),
            font,
            weight: shape
                .and_then(|theme| theme.weight)
                .unwrap_or(profile.metrics().title_weight),
            align: shape.map(|theme| theme.align).unwrap_or_default(),
            caption: shape.map(|theme| theme.buttons).unwrap_or_default(),
            face,
            glyph,
            edge,
            bevel,
            profile,
            desktop_gnome: gnome,
            icons: DesktopWindowIcons::default(),
            radius: if adwaita {
                15.
            } else if system_geometry {
                8.
            } else {
                app.radius
            },
            buttons: app.window_buttons,
        }
    }

    /// The size and spacing of the bar's caption buttons.
    pub fn caption_metrics(&self) -> CaptionMetrics {
        let profile = self.profile.metrics();
        match self.caption {
            CaptionButtons::Plain => {
                let size = profile.button_size.min(self.height);
                CaptionMetrics {
                    width: size,
                    height: size,
                    gap: profile.button_gap,
                    close_gap: 0.,
                    side_padding: profile.side_padding,
                }
            }
            CaptionButtons::Boxed => {
                let height = (self.height - 8.).max(10.).round();
                CaptionMetrics {
                    width: height + 2.,
                    height,
                    gap: 0.,
                    close_gap: 2.,
                    side_padding: 3.,
                }
            }
            CaptionButtons::Wide => CaptionMetrics {
                width: 46.,
                height: self.height,
                gap: 0.,
                close_gap: 0.,
                side_padding: 0.,
            },
        }
    }

    /// The width of one group of caption buttons, without side padding.
    pub fn controls_width(&self, buttons: &[Option<WindowButton>]) -> f32 {
        let metrics = self.caption_metrics();
        let count = buttons.iter().flatten().count();
        let close = buttons.contains(&Some(WindowButton::Close));
        count as f32 * metrics.width
            + count.saturating_sub(1) as f32 * metrics.gap
            + if close && count > 1 {
                metrics.close_gap
            } else {
                0.
            }
    }

    /// The bar's fill for a window with or without focus.
    pub fn fill(&self, active: bool) -> BarFill {
        match self.inactive_background {
            Some(fill) if !active => fill,
            _ => self.background,
        }
    }

    /// The title's color and opacity for a window with or without focus.
    pub fn title_color(&self, active: bool) -> (u32, f32) {
        match self.inactive_foreground {
            _ if active => (self.foreground, 1.),
            Some(color) => (color, 1.),
            None => (
                self.foreground,
                self.profile.metrics().inactive_title_opacity,
            ),
        }
    }

    /// The color and opacity of plain and wide caption symbols.
    pub fn control_color(&self, active: bool) -> (u32, f32) {
        match self.inactive_foreground {
            _ if active => (self.foreground, 1.),
            Some(color) => (color, 1.),
            None => (
                self.foreground,
                self.profile.metrics().inactive_controls_opacity,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_colors_are_independent_of_app_and_terminal_colors() {
        let mut app = NativeAppearance::fallback("UI");
        let font = SystemFont {
            family: "Adwaita Sans".into(),
            size: 15.,
        };
        let resolve = |app: &NativeAppearance, style, os| {
            TitleBarAppearance::resolve(style, app, os, true, font.clone())
        };
        let system = resolve(&app, TitleBarStyle::System, WindowAppearance::Light);
        app.raised = 0x112233;
        app.text = 0xabcdef;
        app.terminal.background = 0x667788;
        assert_eq!(
            resolve(&app, TitleBarStyle::System, WindowAppearance::Light).background,
            system.background
        );
        assert_ne!(
            resolve(&app, TitleBarStyle::System, WindowAppearance::Dark).background,
            system.background
        );
        let themed = resolve(&app, TitleBarStyle::Theme, WindowAppearance::Light);
        assert_eq!(
            (themed.background, themed.foreground),
            (BarFill::Solid(app.raised), app.text)
        );
        assert_eq!(themed.height, system.height);
        assert_eq!(themed.font, system.font);
    }

    fn themed_bar() -> NativeAppearance {
        let mut app = NativeAppearance::fallback("UI");
        app.radius = 0.;
        app.surface = 0xc0c0c0;
        app.text = 0x000000;
        app.bevel = Some(Bevel {
            highlight: 0xffffff,
            light: 0xdfdfdf,
            shadow: 0x808080,
            dark_shadow: 0x000000,
        });
        app.title_bar = crate::appearance::TitleBarTheme {
            background: Some(BarFill::Gradient {
                from: 0x000080,
                to: 0x1084d0,
            }),
            foreground: Some(0xffffff),
            inactive_background: Some(BarFill::Gradient {
                from: 0x808080,
                to: 0xb5b5b5,
            }),
            inactive_foreground: Some(0xc0c0c0),
            height: Some(22.),
            align: TitleAlign::Start,
            weight: Some(FontWeight::BOLD),
            buttons: CaptionButtons::Boxed,
        };
        app
    }

    fn resolve(app: &NativeAppearance, style: TitleBarStyle) -> TitleBarAppearance {
        TitleBarAppearance::resolve(
            style,
            app,
            WindowAppearance::Light,
            true,
            SystemFont {
                family: "Desktop font".into(),
                size: 15.,
            },
        )
    }

    #[test]
    fn theme_title_bar_colors_apply_under_both_decorations() {
        let mut app = themed_bar();
        for decoration in [WindowDecoration::System, WindowDecoration::Splitty] {
            app.window_decoration = decoration;
            let bar = resolve(&app, TitleBarStyle::Theme);
            assert_eq!(bar.fill(true), app.title_bar.background.unwrap());
            assert_eq!(bar.fill(false), app.title_bar.inactive_background.unwrap());
            assert_eq!(bar.title_color(true), (0xffffff, 1.));
            assert_eq!(bar.title_color(false), (0xc0c0c0, 1.));
            assert_eq!(bar.control_color(false), (0xc0c0c0, 1.));
            assert_eq!((bar.face, bar.glyph, bar.bevel), (0xc0c0c0, 0, app.bevel));
        }
    }

    #[test]
    fn absent_theme_colors_keep_the_derived_bar_and_dimmed_inactive_title() {
        let mut app = NativeAppearance::fallback("UI");
        app.title_bar.foreground = Some(0x123456);
        let bar = resolve(&app, TitleBarStyle::Theme);
        assert_eq!(bar.fill(true), BarFill::Solid(app.raised));
        assert_eq!(bar.fill(false), bar.fill(true));
        let metrics = bar.profile.metrics();
        assert_eq!(
            bar.title_color(false),
            (0x123456, metrics.inactive_title_opacity)
        );
        assert_eq!(
            bar.control_color(false),
            (0x123456, metrics.inactive_controls_opacity)
        );
    }

    #[test]
    fn system_title_bar_colors_ignore_theme_tokens() {
        let app = themed_bar();
        let plain = resolve(&NativeAppearance::fallback("UI"), TitleBarStyle::System);
        let bar = resolve(&app, TitleBarStyle::System);
        assert_eq!(bar.background, plain.background);
        assert_eq!(bar.foreground, plain.foreground);
        assert_eq!(
            (bar.inactive_background, bar.inactive_foreground),
            (None, None)
        );
        assert_eq!(bar.title_color(false).1, plain.title_color(false).1);
        assert_eq!(bar.bevel, None);
        assert_eq!((bar.face, bar.glyph), (plain.button, plain.foreground));
    }

    #[test]
    fn theme_shape_applies_only_under_splitty_decoration() {
        let mut app = themed_bar();
        let system = resolve(&app, TitleBarStyle::Theme);
        let plain = resolve(&NativeAppearance::fallback("UI"), TitleBarStyle::Theme);
        assert_eq!(system.height, plain.height);
        assert_eq!(system.weight, plain.weight);
        assert_eq!(system.align, TitleAlign::Center);
        assert_eq!(system.caption, CaptionButtons::Plain);
        assert_eq!(system.radius, 15.);

        app.window_decoration = WindowDecoration::Splitty;
        for style in [TitleBarStyle::Theme, TitleBarStyle::System] {
            let splitty = resolve(&app, style);
            assert_eq!(splitty.height, 22.);
            assert_eq!(splitty.weight, FontWeight::BOLD);
            assert_eq!(splitty.align, TitleAlign::Start);
            assert_eq!(splitty.caption, CaptionButtons::Boxed);
            // A square theme has a square window frame.
            assert_eq!(splitty.radius, 0.);
        }

        // Absent shape tokens keep the derived height and profile weight,
        // and the frame takes the theme's radius as it is.
        let mut app = NativeAppearance::fallback("UI");
        app.window_decoration = WindowDecoration::Splitty;
        app.radius = 3.;
        let splitty = resolve(&app, TitleBarStyle::Theme);
        assert_eq!(splitty.height, 36_f32.max(app.font_size + 16.));
        assert_eq!(
            splitty.weight,
            DecorationProfile::Standard.metrics().title_weight
        );
        assert_eq!(splitty.radius, 3.);
    }

    #[test]
    fn caption_buttons_take_their_size_from_the_style_and_bar() {
        let mut app = themed_bar();
        app.window_decoration = WindowDecoration::Splitty;
        let all = [
            Some(WindowButton::Minimize),
            Some(WindowButton::Maximize),
            Some(WindowButton::Close),
        ];
        let mut bar = resolve(&app, TitleBarStyle::Theme);
        let boxed = bar.caption_metrics();
        assert_eq!((boxed.width, boxed.height), (16., 14.));
        assert_eq!(bar.controls_width(&all), 50.);
        assert_eq!(
            bar.controls_width(&[None, None, Some(WindowButton::Close)]),
            16.
        );
        bar.caption = CaptionButtons::Wide;
        bar.height = 32.;
        let wide = bar.caption_metrics();
        assert_eq!((wide.width, wide.height, wide.gap), (46., 32., 0.));
        assert_eq!(bar.controls_width(&all), 138.);
        // Plain buttons never grow past a short bar.
        bar.caption = CaptionButtons::Plain;
        bar.height = 22.;
        assert_eq!(bar.caption_metrics().height, 22.);
    }

    #[test]
    fn system_font_units_and_invalid_descriptions() {
        assert_eq!(SystemFont::parse("Adwaita Sans 12").unwrap().size, 16.);
        assert_eq!(SystemFont::parse("Cantarell 15px").unwrap().size, 15.);
        for bad in ["", "12", "Sans NaN", "Sans 200", "Sans\n12"] {
            assert!(SystemFont::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn decoration_geometry_and_button_policy_are_independent_of_colors() {
        let mut app = NativeAppearance::fallback("App font");
        let font = SystemFont {
            family: "Desktop font".into(),
            size: 15.,
        };
        let system = TitleBarAppearance::resolve(
            TitleBarStyle::Theme,
            &app,
            WindowAppearance::Dark,
            true,
            font.clone(),
        );
        app.window_decoration = WindowDecoration::Splitty;
        let splitty = TitleBarAppearance::resolve(
            TitleBarStyle::Theme,
            &app,
            WindowAppearance::Dark,
            true,
            font,
        );
        assert_eq!(system.background, splitty.background);
        assert!(system.height > splitty.height);
        assert_ne!(system.font, splitty.font);
        assert_ne!(system.radius, splitty.radius);
        assert_eq!(system.profile, DecorationProfile::Adwaita);
        assert_eq!(splitty.profile, DecorationProfile::Standard);
        // Desktop button policy remains GNOME's even with Splitty geometry.
        assert!(system.desktop_gnome && splitty.desktop_gnome);
        assert_eq!(
            TitleBarAppearance::resolve(
                TitleBarStyle::System,
                &NativeAppearance::fallback("App font"),
                WindowAppearance::Dark,
                true,
                system.font.clone(),
            )
            .profile,
            system.profile
        );

        let desktop = WindowButtonLayout {
            left: [Some(WindowButton::Close), None, None],
            right: [None; 3],
        };
        let system = WindowButtons::System.layout(Some(desktop), true);
        assert_eq!(system.left[0], Some(WindowButton::Close));
        assert!(system.right.iter().all(Option::is_none));
        let all = WindowButtons::All.layout(Some(desktop), true);
        assert_eq!(
            all.right,
            [
                Some(WindowButton::Minimize),
                Some(WindowButton::Maximize),
                Some(WindowButton::Close)
            ]
        );
        assert!(WindowDecoration::System.native(TitleBarStyle::System));
        assert!(!WindowDecoration::System.native(TitleBarStyle::Theme));
        assert!(!WindowDecoration::Splitty.native(TitleBarStyle::System));
    }
}
