//! Theme resolution: derives semantic UI and terminal color tokens from a palette.
//!
//! Takes a `ThemeFile` (palette + variant + optional overrides) and produces a
//! `ResolvedTheme` ready to be sent to the frontend as JSON. Falls back to
//! `splitty-dark` if a requested theme cannot be found or parsed.

use super::bundled;
use super::types::*;

fn derive_ui_tokens(palette: &Palette, variant: &Variant) -> UiTokens {
    match variant {
        Variant::Dark => UiTokens {
            surface_base: palette.gray_900.clone(),
            surface_raised: palette.gray_850.clone(),
            surface_overlay: palette.gray_800.clone(),
            text_primary: palette.gray_100.clone(),
            text_secondary: palette.gray_400.clone(),
            text_muted: palette.gray_500.clone(),
            border_default: palette.gray_700.clone(),
            border_subtle: palette.gray_800.clone(),
            hover_bg: "rgba(255, 255, 255, 0.06)".into(),
            active_bg: "rgba(255, 255, 255, 0.10)".into(),
            accent: palette.accent.clone(),
            accent_muted: format!("{}66", palette.accent),
            danger: palette.danger.clone(),
            danger_hover: format!("{}29", palette.danger),
            warning: palette.warning.clone(),
            success: palette.success.clone(),
            shadow: "rgba(0, 0, 0, 0.45)".into(),
            scrollbar: "rgba(255, 255, 255, 0.08)".into(),
            font_family: "system-ui, -apple-system, \"Segoe UI\", Roboto, sans-serif".into(),
            font_size: "12px".into(),
            border_radius: "8px".into(),
            tab_height: "32px".into(),
            tab_font_size: "12px".into(),
            tab_bar_border: "transparent".into(),
            tab_border_radius: "0px".into(),
            drag_over_border: "none".into(),
            pane_gap: "0px".into(),
            pane_padding: "0px".into(),
            pane_border_radius: "0px".into(),
            sash_border_radius: "0px".into(),
            sash_color: "transparent".into(),
            sash_hover_color: "transparent".into(),
            tab_accent_color: "transparent".into(),
            tab_accent_thickness: "2px".into(),
            tab_accent_position: "top".into(),
            tab_margin: "0".into(),
            dropdown_border_radius: "0px".into(),
            tab_close_size: "9px".into(),
        },
        Variant::Light => UiTokens {
            surface_base: palette.gray_0.clone(),
            surface_raised: palette.gray_50.clone(),
            surface_overlay: palette.gray_100.clone(),
            text_primary: palette.gray_900.clone(),
            text_secondary: palette.gray_600.clone(),
            text_muted: palette.gray_400.clone(),
            border_default: palette.gray_200.clone(),
            border_subtle: palette.gray_100.clone(),
            hover_bg: "rgba(0, 0, 0, 0.04)".into(),
            active_bg: "rgba(0, 0, 0, 0.08)".into(),
            accent: palette.accent.clone(),
            accent_muted: format!("{}66", palette.accent),
            danger: palette.danger.clone(),
            danger_hover: format!("{}29", palette.danger),
            warning: palette.warning.clone(),
            success: palette.success.clone(),
            shadow: "rgba(0, 0, 0, 0.15)".into(),
            scrollbar: "rgba(0, 0, 0, 0.08)".into(),
            font_family: "system-ui, -apple-system, \"Segoe UI\", Roboto, sans-serif".into(),
            font_size: "12px".into(),
            border_radius: "8px".into(),
            tab_height: "32px".into(),
            tab_font_size: "12px".into(),
            tab_bar_border: "transparent".into(),
            tab_border_radius: "0px".into(),
            drag_over_border: "none".into(),
            pane_gap: "0px".into(),
            pane_padding: "0px".into(),
            pane_border_radius: "0px".into(),
            sash_border_radius: "0px".into(),
            sash_color: "transparent".into(),
            sash_hover_color: "transparent".into(),
            tab_accent_color: "transparent".into(),
            tab_accent_thickness: "2px".into(),
            tab_accent_position: "top".into(),
            tab_margin: "0".into(),
            dropdown_border_radius: "0px".into(),
            tab_close_size: "9px".into(),
        },
    }
}

fn derive_terminal_tokens(palette: &Palette, variant: &Variant) -> TerminalTokens {
    match variant {
        Variant::Dark => TerminalTokens {
            bg: palette.gray_900.clone(),
            fg: palette.gray_100.clone(),
            cursor: palette.gray_100.clone(),
            cursor_accent: palette.gray_900.clone(),
            selection_bg: palette.gray_700.clone(),
            selection_fg: palette.gray_100.clone(),
            black: palette.gray_900.clone(),
            red: palette.danger.clone(),
            green: palette.success.clone(),
            yellow: palette.warning.clone(),
            blue: palette.accent.clone(),
            magenta: "#c678dd".into(),
            cyan: "#56b6c2".into(),
            white: palette.gray_100.clone(),
            bright_black: palette.gray_400.clone(),
            bright_red: "#e88891".into(),
            bright_green: "#a9d48b".into(),
            bright_yellow: "#ecd08e".into(),
            bright_blue: "#7db4ff".into(),
            bright_magenta: "#d48ce6".into(),
            bright_cyan: "#6bc5d0".into(),
            bright_white: palette.gray_0.clone(),
        },
        Variant::Light => TerminalTokens {
            bg: palette.gray_0.clone(),
            fg: palette.gray_900.clone(),
            cursor: palette.gray_900.clone(),
            cursor_accent: palette.gray_0.clone(),
            selection_bg: palette.gray_200.clone(),
            selection_fg: palette.gray_900.clone(),
            black: palette.gray_700.clone(),
            red: palette.danger.clone(),
            green: palette.success.clone(),
            yellow: palette.warning.clone(),
            blue: palette.accent.clone(),
            magenta: "#a24bc2".into(),
            cyan: "#2b919d".into(),
            white: palette.gray_500.clone(),
            bright_black: palette.gray_400.clone(),
            bright_red: "#d4545e".into(),
            bright_green: "#5d8c41".into(),
            bright_yellow: "#a07030".into(),
            bright_blue: "#3f84c1".into(),
            bright_magenta: "#b55dd0".into(),
            bright_cyan: "#318b97".into(),
            bright_white: palette.gray_600.clone(),
        },
    }
}

fn apply_ui_overrides(
    tokens: &mut UiTokens,
    overrides: &std::collections::HashMap<String, String>,
) {
    for (key, val) in overrides {
        match key.as_str() {
            "surface-base" => tokens.surface_base = val.clone(),
            "surface-raised" => tokens.surface_raised = val.clone(),
            "surface-overlay" => tokens.surface_overlay = val.clone(),
            "text-primary" => tokens.text_primary = val.clone(),
            "text-secondary" => tokens.text_secondary = val.clone(),
            "text-muted" => tokens.text_muted = val.clone(),
            "border-default" => tokens.border_default = val.clone(),
            "border-subtle" => tokens.border_subtle = val.clone(),
            "hover-bg" => tokens.hover_bg = val.clone(),
            "active-bg" => tokens.active_bg = val.clone(),
            "accent" => tokens.accent = val.clone(),
            "accent-muted" => tokens.accent_muted = val.clone(),
            "danger" => tokens.danger = val.clone(),
            "danger-hover" => tokens.danger_hover = val.clone(),
            "warning" => tokens.warning = val.clone(),
            "success" => tokens.success = val.clone(),
            "shadow" => tokens.shadow = val.clone(),
            "scrollbar" => tokens.scrollbar = val.clone(),
            "font-family" => tokens.font_family = val.clone(),
            "font-size" => tokens.font_size = val.clone(),
            "border-radius" => tokens.border_radius = val.clone(),
            "tab-height" => tokens.tab_height = val.clone(),
            "tab-font-size" => tokens.tab_font_size = val.clone(),
            "tab-bar-border" => tokens.tab_bar_border = val.clone(),
            "tab-border-radius" => tokens.tab_border_radius = val.clone(),
            "drag-over-border" => tokens.drag_over_border = val.clone(),
            "pane-gap" => tokens.pane_gap = val.clone(),
            "pane-padding" => tokens.pane_padding = val.clone(),
            "pane-border-radius" => tokens.pane_border_radius = val.clone(),
            "sash-border-radius" => tokens.sash_border_radius = val.clone(),
            "sash-color" => tokens.sash_color = val.clone(),
            "sash-hover-color" => tokens.sash_hover_color = val.clone(),
            "tab-accent-color" => tokens.tab_accent_color = val.clone(),
            "tab-accent-thickness" => tokens.tab_accent_thickness = val.clone(),
            "tab-accent-position" => tokens.tab_accent_position = val.clone(),
            "tab-margin" => tokens.tab_margin = val.clone(),
            "dropdown-border-radius" => tokens.dropdown_border_radius = val.clone(),
            "tab-close-size" => tokens.tab_close_size = val.clone(),
            _ => {}
        }
    }
}

fn apply_terminal_overrides(
    tokens: &mut TerminalTokens,
    overrides: &std::collections::HashMap<String, String>,
) {
    for (key, val) in overrides {
        match key.as_str() {
            "bg" => tokens.bg = val.clone(),
            "fg" => tokens.fg = val.clone(),
            "cursor" => tokens.cursor = val.clone(),
            "cursor-accent" => tokens.cursor_accent = val.clone(),
            "selection-bg" => tokens.selection_bg = val.clone(),
            "selection-fg" => tokens.selection_fg = val.clone(),
            "black" => tokens.black = val.clone(),
            "red" => tokens.red = val.clone(),
            "green" => tokens.green = val.clone(),
            "yellow" => tokens.yellow = val.clone(),
            "blue" => tokens.blue = val.clone(),
            "magenta" => tokens.magenta = val.clone(),
            "cyan" => tokens.cyan = val.clone(),
            "white" => tokens.white = val.clone(),
            "bright-black" => tokens.bright_black = val.clone(),
            "bright-red" => tokens.bright_red = val.clone(),
            "bright-green" => tokens.bright_green = val.clone(),
            "bright-yellow" => tokens.bright_yellow = val.clone(),
            "bright-blue" => tokens.bright_blue = val.clone(),
            "bright-magenta" => tokens.bright_magenta = val.clone(),
            "bright-cyan" => tokens.bright_cyan = val.clone(),
            "bright-white" => tokens.bright_white = val.clone(),
            _ => {}
        }
    }
}

pub fn resolve_theme_file(theme: &ThemeFile) -> ResolvedTheme {
    let mut ui = derive_ui_tokens(&theme.palette, &theme.variant);
    let mut terminal = derive_terminal_tokens(&theme.palette, &theme.variant);
    apply_ui_overrides(&mut ui, &theme.overrides);
    apply_terminal_overrides(&mut terminal, &theme.terminal);
    ResolvedTheme {
        name: theme.name.clone(),
        variant: theme.variant.clone(),
        ui,
        terminal,
        effects: theme.effects.clone(),
        layout: theme.layout.clone(),
    }
}

/// Is this a name we are willing to turn into a file path?
///
/// The resolver used to build the path with a bare
/// `themes_dir.join(format!("{name}.toml"))` (security review
/// SR-16), so a name containing `../` walked out of the themes directory. That
/// was not arbitrary file *disclosure* — the content is parsed as a theme and
/// only colour values reach the client — but it let anything that could set a
/// theme name pull unexpected TOML into the UI.
///
/// A short identifier is all a theme name has ever needed, so the fix is a
/// whitelist rather than an attempt to filter traversal sequences.
fn is_valid_theme_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn resolve_theme(name: &str, themes_dir: &std::path::Path) -> ResolvedTheme {
    if let Some(theme_file) = bundled::bundled_theme(name) {
        return resolve_theme_file(&theme_file);
    }

    if !is_valid_theme_name(name) {
        tracing::warn!("rejected theme name {name:?}: not a plain identifier");
        return resolve_theme_file(&bundled::splitty_dark());
    }

    let path = themes_dir.join(format!("{name}.toml"));

    // Second gate: even a name that passes the character check must resolve to a
    // path inside the themes directory. This catches a symlink placed inside
    // `themes/` that points elsewhere, which the name check alone cannot see.
    let inside_themes_dir = match (path.canonicalize(), themes_dir.canonicalize()) {
        (Ok(resolved), Ok(root)) => resolved.starts_with(&root),
        // The file simply does not exist — fall through to the read below, which
        // fails and yields the default theme.
        _ => true,
    };
    if !inside_themes_dir {
        tracing::warn!("rejected theme {name:?}: resolves outside the themes directory");
        return resolve_theme_file(&bundled::splitty_dark());
    }

    if let Ok(content) = std::fs::read_to_string(&path) {
        match toml::from_str::<ThemeFile>(&content) {
            Ok(theme_file) => return resolve_theme_file(&theme_file),
            Err(e) => tracing::warn!("failed to parse custom theme '{name}': {e}"),
        }
    }
    resolve_theme_file(&bundled::splitty_dark())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_dark_tokens() {
        let theme = resolve_theme_file(&bundled::splitty_dark());
        assert_eq!(theme.name, "Splitty Dark");
        assert_eq!(theme.ui.surface_base, "#0f0f0f");
        assert_eq!(theme.ui.text_primary, "#c8c8c8");
        assert_eq!(theme.ui.accent, "#63a0ff");
        assert_eq!(theme.terminal.bg, "#0f0f0f");
        assert_eq!(theme.terminal.fg, "#c8c8c8");
        assert_eq!(theme.terminal.red, "#e06c75");
    }

    #[test]
    fn test_resolve_light_tokens() {
        let theme = resolve_theme_file(&bundled::splitty_light());
        assert_eq!(theme.name, "Splitty Light");
        assert_eq!(theme.ui.surface_base, "#f0f0f0");
        assert_eq!(theme.ui.text_primary, "#121212");
        assert_eq!(theme.terminal.bg, "#ffffff");
    }

    #[test]
    fn test_ui_overrides() {
        let mut theme_file = bundled::splitty_dark();
        theme_file
            .overrides
            .insert("surface-base".into(), "#000000".into());
        let resolved = resolve_theme_file(&theme_file);
        assert_eq!(resolved.ui.surface_base, "#000000");
        assert_eq!(resolved.ui.surface_raised, "#1a1a1a");
    }

    #[test]
    fn test_terminal_overrides() {
        let mut theme_file = bundled::splitty_dark();
        theme_file.terminal.insert("red".into(), "#ff0000".into());
        let resolved = resolve_theme_file(&theme_file);
        assert_eq!(resolved.terminal.red, "#ff0000");
        assert_eq!(resolved.terminal.green, "#98c379");
    }

    #[test]
    fn test_resolve_unknown_falls_back_to_dark() {
        let dir = tempfile::tempdir().unwrap();
        let theme = resolve_theme("nonexistent", dir.path());
        assert_eq!(theme.name, "Splitty Dark");
    }

    #[test]
    fn test_effects_pass_through() {
        let theme = resolve_theme_file(&bundled::splitty_aurora());
        assert_eq!(theme.effects.get("glow").map(|s| s.as_str()), Some("true"));
        assert_eq!(
            theme.effects.get("glow-color").map(|s| s.as_str()),
            Some("#a78bfa")
        );
    }

    #[test]
    fn test_no_effects_is_empty() {
        let theme = resolve_theme_file(&bundled::splitty_dark());
        assert!(theme.effects.is_empty());
    }

    #[test]
    fn test_resolve_aurora_tokens() {
        let theme = resolve_theme_file(&bundled::splitty_aurora());
        assert_eq!(theme.name, "Splitty Aurora");
        assert_eq!(theme.ui.accent, "#a78bfa");
        assert_eq!(theme.terminal.cursor, "#a78bfa");
    }

    #[test]
    fn test_resolve_neon_tokens() {
        let theme = resolve_theme_file(&bundled::splitty_neon());
        assert_eq!(theme.name, "Splitty Neon");
        assert_eq!(theme.ui.accent, "#ff2d95");
        assert!(theme.effects.contains_key("scanlines"));
    }

    #[test]
    fn test_resolve_custom_theme() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r##"
name = "My Custom"
variant = "dark"

[palette]
gray-0 = "#ffffff"
gray-50 = "#f5f5f5"
gray-100 = "#dddddd"
gray-200 = "#bbbbbb"
gray-300 = "#999999"
gray-400 = "#777777"
gray-500 = "#555555"
gray-600 = "#444444"
gray-700 = "#333333"
gray-800 = "#222222"
gray-850 = "#1b1b1b"
gray-900 = "#111111"
accent = "#ff6600"
danger = "#ff0000"
warning = "#ffaa00"
success = "#00ff00"
"##;
        std::fs::write(dir.path().join("my-custom.toml"), toml_content).unwrap();
        let theme = resolve_theme("my-custom", dir.path());
        assert_eq!(theme.name, "My Custom");
        assert_eq!(theme.ui.surface_base, "#111111");
        assert_eq!(theme.ui.accent, "#ff6600");
    }
}

#[cfg(test)]
mod path_safety_tests {
    use super::*;

    /// SR-16: theme names must not be able to name a path.
    #[test]
    fn traversal_and_separators_are_rejected() {
        for name in [
            "../../etc/passwd",
            "..",
            "a/b",
            "a\\b",
            "/etc/passwd",
            "C:\\windows",
            "theme\0name",
            "théme",      // non-ASCII
            "a\u{2044}b", // fraction slash, a lookalike separator
            "",
        ] {
            assert!(
                !is_valid_theme_name(name),
                "{name:?} must be rejected as a theme name",
            );
        }
    }

    #[test]
    fn plain_identifiers_are_accepted() {
        for name in ["default", "splitty-dark", "my_theme", "solarized2"] {
            assert!(
                is_valid_theme_name(name),
                "{name:?} should be a valid theme name"
            );
        }
    }

    #[test]
    fn overlong_names_are_rejected() {
        assert!(!is_valid_theme_name(&"a".repeat(65)));
        assert!(is_valid_theme_name(&"a".repeat(64)));
    }

    /// A traversing name must not read a file outside the themes directory —
    /// the resolver falls back to the bundled default instead.
    #[test]
    fn traversal_resolves_to_the_default_theme() {
        let root = tempfile::tempdir().unwrap();
        let themes = root.path().join("themes");
        std::fs::create_dir_all(&themes).unwrap();

        // A parseable theme file placed outside the themes directory.
        std::fs::write(
            root.path().join("outside.toml"),
            "name = \"outside\"\nvariant = \"dark\"\n",
        )
        .unwrap();

        let resolved = resolve_theme("../outside", &themes);
        assert_ne!(
            resolved.name, "outside",
            "must not load a theme from outside the directory"
        );
    }

    /// A symlink inside the themes directory pointing elsewhere is refused by
    /// the canonicalisation check, which the name check alone cannot catch.
    #[test]
    #[cfg(unix)]
    fn symlink_out_of_the_themes_directory_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let themes = root.path().join("themes");
        std::fs::create_dir_all(&themes).unwrap();

        std::fs::write(
            root.path().join("elsewhere.toml"),
            "name = \"elsewhere\"\nvariant = \"dark\"\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            root.path().join("elsewhere.toml"),
            themes.join("sneaky.toml"),
        )
        .unwrap();

        let resolved = resolve_theme("sneaky", &themes);
        assert_ne!(
            resolved.name, "elsewhere",
            "a symlink must not escape the themes directory"
        );
    }
}
