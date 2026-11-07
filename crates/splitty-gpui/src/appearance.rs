//! Convert service-owned theme values into the bounded native presentation subset.
//! This adapter never reads settings files or changes terminal font metrics.
use crate::terminal::TerminalColors;
use splitty_client_core::appearance::AppearanceSnapshot;
use splitty_client_core::zoom::Zoom;

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
    pub accent: u32,
    pub font_family: String,
    pub font_size: f32,
    pub terminal: TerminalColors,
    pub selection_foreground: u32,
    pub selection_background: u32,
    pub zoom: Zoom,
    pub notice: Option<&'static str>,
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
            accent: 0x76b8e8,
            font_family: ui_family.into(),
            font_size: 13.,
            terminal: TerminalColors::default(),
            selection_foreground: 0xd9e2ef,
            selection_background: 0x345778,
            zoom: Zoom::from_values(1.0, "window"),
            notice: None,
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
        let terminal = &snapshot.theme().terminal;
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
        result.notice = fallback_used
            .then_some("Some theme values are unsupported; native defaults are in use.");
        result
    }
}

#[derive(Clone, Copy, Debug)]
struct Color {
    rgb: [f32; 3],
    alpha: f32,
}

impl Color {
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
}
