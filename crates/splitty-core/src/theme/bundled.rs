//! Compile-time bundled themes (dark, light, classic, retro).
//!
//! Each theme is embedded as a TOML string via `include_str!` and parsed on first access.

use super::types::ThemeFile;

const SPLITTY_DARK_TOML: &str = include_str!("bundled/splitty-dark.toml");
const SPLITTY_LIGHT_TOML: &str = include_str!("bundled/splitty-light.toml");
const SPLITTY_CLASSIC_TOML: &str = include_str!("bundled/splitty-classic.toml");
const SPLITTY_CLASSIC_LIGHT_TOML: &str = include_str!("bundled/splitty-classic-light.toml");
const SPLITTY_RETRO_TOML: &str = include_str!("bundled/splitty-retro.toml");
const SPLITTY_AURORA_TOML: &str = include_str!("bundled/splitty-aurora.toml");
const SPLITTY_NEON_TOML: &str = include_str!("bundled/splitty-neon.toml");
const SPLITTY_COZY_TOML: &str = include_str!("bundled/splitty-cozy.toml");
const SPLITTY_LIGHT_GRAY_TOML: &str = include_str!("bundled/splitty-light-gray.toml");
const SPLITTY_CLASSIC_DARK_GRAY_TOML: &str = include_str!("bundled/splitty-classic-dark-gray.toml");
const SPLITTY_CLASSIC_LIGHT_GRAY_TOML: &str =
    include_str!("bundled/splitty-classic-light-gray.toml");
const ADWAITA_DARK_TOML: &str = include_str!("bundled/adwaita-dark.toml");
const ADWAITA_LIGHT_TOML: &str = include_str!("bundled/adwaita-light.toml");

pub fn splitty_dark() -> ThemeFile {
    toml::from_str(SPLITTY_DARK_TOML).expect("bundled splitty-dark.toml is valid")
}

pub fn splitty_light() -> ThemeFile {
    toml::from_str(SPLITTY_LIGHT_TOML).expect("bundled splitty-light.toml is valid")
}

pub fn splitty_classic() -> ThemeFile {
    toml::from_str(SPLITTY_CLASSIC_TOML).expect("bundled splitty-classic.toml is valid")
}

pub fn splitty_classic_light() -> ThemeFile {
    toml::from_str(SPLITTY_CLASSIC_LIGHT_TOML).expect("bundled splitty-classic-light.toml is valid")
}

pub fn splitty_retro() -> ThemeFile {
    toml::from_str(SPLITTY_RETRO_TOML).expect("bundled splitty-retro.toml is valid")
}

pub fn splitty_aurora() -> ThemeFile {
    toml::from_str(SPLITTY_AURORA_TOML).expect("bundled splitty-aurora.toml is valid")
}

pub fn splitty_neon() -> ThemeFile {
    toml::from_str(SPLITTY_NEON_TOML).expect("bundled splitty-neon.toml is valid")
}

pub fn splitty_cozy() -> ThemeFile {
    toml::from_str(SPLITTY_COZY_TOML).expect("bundled splitty-cozy.toml is valid")
}

pub fn splitty_light_gray() -> ThemeFile {
    toml::from_str(SPLITTY_LIGHT_GRAY_TOML).expect("bundled splitty-light-gray.toml is valid")
}

pub fn splitty_classic_dark_gray() -> ThemeFile {
    toml::from_str(SPLITTY_CLASSIC_DARK_GRAY_TOML)
        .expect("bundled splitty-classic-dark-gray.toml is valid")
}

pub fn splitty_classic_light_gray() -> ThemeFile {
    toml::from_str(SPLITTY_CLASSIC_LIGHT_GRAY_TOML)
        .expect("bundled splitty-classic-light-gray.toml is valid")
}

pub fn adwaita_dark() -> ThemeFile {
    toml::from_str(ADWAITA_DARK_TOML).expect("bundled adwaita-dark.toml is valid")
}

pub fn adwaita_light() -> ThemeFile {
    toml::from_str(ADWAITA_LIGHT_TOML).expect("bundled adwaita-light.toml is valid")
}

pub fn bundled_theme(name: &str) -> Option<ThemeFile> {
    match name {
        "splitty-dark" => Some(splitty_dark()),
        "splitty-light" => Some(splitty_light()),
        "splitty-classic" => Some(splitty_classic()),
        "splitty-classic-light" => Some(splitty_classic_light()),
        "splitty-retro" => Some(splitty_retro()),
        "splitty-aurora" => Some(splitty_aurora()),
        "splitty-neon" => Some(splitty_neon()),
        "splitty-cozy" => Some(splitty_cozy()),
        "splitty-light-gray" => Some(splitty_light_gray()),
        "splitty-classic-dark-gray" => Some(splitty_classic_dark_gray()),
        "splitty-classic-light-gray" => Some(splitty_classic_light_gray()),
        "adwaita-dark" => Some(adwaita_dark()),
        "adwaita-light" => Some(adwaita_light()),
        _ => None,
    }
}

pub fn bundled_names() -> Vec<String> {
    vec![
        "splitty-dark".into(),
        "splitty-light".into(),
        "splitty-classic".into(),
        "splitty-classic-light".into(),
        "splitty-retro".into(),
        "splitty-aurora".into(),
        "splitty-neon".into(),
        "splitty-cozy".into(),
        "splitty-light-gray".into(),
        "splitty-classic-dark-gray".into(),
        "splitty-classic-light-gray".into(),
        "adwaita-dark".into(),
        "adwaita-light".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bundled_dark_parses() {
        let theme = splitty_dark();
        assert_eq!(theme.name, "Splitty Dark");
        assert_eq!(theme.palette.gray_900, "#0f0f0f");
        assert_eq!(theme.palette.accent, "#63a0ff");
    }

    #[test]
    fn test_bundled_light_parses() {
        let theme = splitty_light();
        assert_eq!(theme.name, "Splitty Light");
        assert_eq!(theme.palette.gray_0, "#f0f0f0");
    }

    #[test]
    fn test_bundled_aurora_parses() {
        let theme = splitty_aurora();
        assert_eq!(theme.name, "Splitty Aurora");
        assert_eq!(theme.palette.accent, "#a78bfa");
        assert_eq!(theme.effects.get("glow").map(|s| s.as_str()), Some("true"));
    }

    #[test]
    fn test_bundled_neon_parses() {
        let theme = splitty_neon();
        assert_eq!(theme.name, "Splitty Neon");
        assert_eq!(theme.palette.accent, "#ff2d95");
        assert_eq!(
            theme.effects.get("scanlines").map(|s| s.as_str()),
            Some("true")
        );
    }

    #[test]
    fn test_bundled_retro_has_effects() {
        let theme = splitty_retro();
        assert_eq!(
            theme.effects.get("scanlines").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(theme.effects.get("glow").map(|s| s.as_str()), Some("true"));
    }

    #[test]
    fn test_bundled_lookup() {
        assert!(bundled_theme("splitty-dark").is_some());
        assert!(bundled_theme("splitty-light").is_some());
        assert!(bundled_theme("splitty-aurora").is_some());
        assert!(bundled_theme("splitty-neon").is_some());
        assert!(bundled_theme("nonexistent").is_none());
    }
}
