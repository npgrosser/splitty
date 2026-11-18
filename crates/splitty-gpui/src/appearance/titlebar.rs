//! Resolves window chrome separately from application and terminal appearance.
//! Desktop conventions own geometry and controls; the selected source owns all
//! colors of client decorations. Server decorations keep the compositor's colors.
use super::NativeAppearance;

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

#[derive(Clone, Debug)]
pub struct TitleBarAppearance {
    pub background: u32,
    pub foreground: u32,
    pub button: u32,
    pub hover: u32,
    pub height: f32,
    pub font: SystemFont,
    pub profile: DecorationProfile,
    pub desktop_gnome: bool,
    pub icons: DesktopWindowIcons,
    pub radius: f32,
    pub buttons: WindowButtons,
}

impl TitleBarAppearance {
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
        let (background, foreground, button, hover) = match style {
            TitleBarStyle::Theme => (app.raised, app.text, app.hover, app.active),
            TitleBarStyle::System if dark => (0x303030, 0xffffff, 0x454545, 0x505050),
            TitleBarStyle::System => (0xebebeb, 0x2e3436, 0xd9d9d9, 0xcccccc),
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
        Self {
            background,
            foreground,
            button,
            hover,
            height: if adwaita {
                46_f32.max(font.size + 24.)
            } else {
                36_f32.max(font.size + 16.)
            },
            font,
            profile: if adwaita {
                DecorationProfile::Adwaita
            } else {
                DecorationProfile::Standard
            },
            desktop_gnome: gnome,
            icons: DesktopWindowIcons::default(),
            radius: if adwaita {
                15.
            } else if system_geometry {
                8.
            } else {
                app.radius.clamp(6., 16.)
            },
            buttons: app.window_buttons,
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
            (app.raised, app.text)
        );
        assert_eq!(themed.height, system.height);
        assert_eq!(themed.font, system.font);
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
