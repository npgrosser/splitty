//! Theme data types: `ThemeFile` (authored TOML), `Palette`, `Variant`,
//! `ResolvedTheme`, `UiTokens`, `TerminalTokens`, and effects.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A theme file as authored by the user (TOML).
#[derive(Clone, Debug, Deserialize)]
pub struct ThemeFile {
    pub name: String,
    pub variant: Variant,
    pub palette: Palette,
    #[serde(default)]
    pub overrides: HashMap<String, String>,
    #[serde(default)]
    pub terminal: HashMap<String, String>,
    #[serde(default)]
    pub effects: HashMap<String, String>,
    #[serde(default)]
    pub layout: LayoutTokens,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    Dark,
    Light,
}

/// The palette: ~15 base colors that everything derives from.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Palette {
    pub gray_0: String,
    pub gray_50: String,
    pub gray_100: String,
    pub gray_200: String,
    pub gray_300: String,
    pub gray_400: String,
    pub gray_500: String,
    pub gray_600: String,
    pub gray_700: String,
    pub gray_800: String,
    pub gray_850: String,
    pub gray_900: String,
    pub accent: String,
    pub danger: String,
    pub warning: String,
    pub success: String,
}

/// The fully resolved theme sent to the frontend as JSON.
#[derive(Clone, Debug, Serialize)]
pub struct ResolvedTheme {
    pub name: String,
    pub variant: Variant,
    pub ui: UiTokens,
    pub terminal: TerminalTokens,
    /// Optional visual effects (glow, scanlines, etc.). Passed to the
    /// frontend as `--splitty-effect-*` CSS custom properties.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub effects: HashMap<String, String>,
    /// Non-CSS behavioural layout tokens (e.g. DnD overlay behaviour).
    pub layout: LayoutTokens,
}

/// Semantic UI tokens derived from the palette.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct UiTokens {
    pub surface_base: String,
    pub surface_raised: String,
    pub surface_overlay: String,
    pub text_primary: String,
    pub text_secondary: String,
    pub text_muted: String,
    pub border_default: String,
    pub border_subtle: String,
    pub hover_bg: String,
    pub active_bg: String,
    pub accent: String,
    pub accent_muted: String,
    pub danger: String,
    pub danger_hover: String,
    pub warning: String,
    pub success: String,
    pub shadow: String,
    pub scrollbar: String,
    pub font_family: String,
    pub font_size: String,
    pub border_radius: String,
    pub tab_height: String,
    pub tab_font_size: String,
    pub tab_bar_border: String,
    pub tab_border_radius: String,
    pub drag_over_border: String,
    pub pane_gap: String,
    pub pane_padding: String,
    pub pane_border_radius: String,
    pub sash_border_radius: String,
    pub sash_color: String,
    pub sash_hover_color: String,
    pub tab_accent_color: String,
    pub tab_accent_thickness: String,
    pub tab_accent_position: String,
    pub tab_margin: String,
    pub dropdown_border_radius: String,
    pub tab_close_size: String,
}

/// Non-CSS behavioural layout tokens (DnD overlay behaviour, etc.).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LayoutTokens {
    /// Drop overlay scope: `"local"` (group-scoped) or `"global"` (full layout).
    #[serde(default = "default_dnd_overlay_scope")]
    pub dnd_overlay_scope: String,
    /// Drop overlay target: `"content"` (panel only) or `"group"` (includes tab bar).
    #[serde(default = "default_dnd_overlay_target")]
    pub dnd_overlay_target: String,
}

fn default_dnd_overlay_scope() -> String {
    "local".into()
}
fn default_dnd_overlay_target() -> String {
    "content".into()
}

/// Terminal color tokens derived from palette (or overridden).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TerminalTokens {
    pub bg: String,
    pub fg: String,
    pub cursor: String,
    pub cursor_accent: String,
    pub selection_bg: String,
    pub selection_fg: String,
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
    pub cyan: String,
    pub white: String,
    pub bright_black: String,
    pub bright_red: String,
    pub bright_green: String,
    pub bright_yellow: String,
    pub bright_blue: String,
    pub bright_magenta: String,
    pub bright_cyan: String,
    pub bright_white: String,
}
