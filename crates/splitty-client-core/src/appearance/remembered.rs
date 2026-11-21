//! The appearance selection a window last applied from its local service,
//! kept in a file of the application so that the next window starts with it
//! before that service answers. A presentation aid, not settings: only the
//! application's own start reads it, nothing sends it to a service, and the
//! service's answer replaces what it shows.
use super::{ui_theme_name, AppearanceSnapshot, ColorScheme};
use serde::{Deserialize, Serialize};
use std::io::Read as _;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "last-appearance.json";
/// More than a file of this module ever holds.
const MAX_BYTES: u64 = 16 * 1024;

/// The settings a window resolves its look from: the names of the UI theme
/// and the terminal palette, the interface font, the terminal font and the
/// zoom.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RememberedAppearance {
    #[serde(default = "default_title_bar_style")]
    pub interface_style: String,
    #[serde(default = "default_window_decoration")]
    pub window_decoration: String,
    #[serde(default = "default_window_buttons")]
    pub window_buttons: String,
    #[serde(default = "default_title_bar_style")]
    pub title_bar_style: String,
    pub theme_mode: String,
    pub theme: String,
    pub theme_dark: String,
    pub theme_light: String,
    /// 0 for the theme's size.
    pub font_size: i32,
    /// Empty for the theme's family.
    pub font_family: String,
    pub zoom: f64,
    pub zoom_scope: String,
    pub terminal_theme: String,
    pub terminal_theme_dark: String,
    pub terminal_theme_light: String,
    /// Empty for the default; absent in files of earlier versions.
    #[serde(default)]
    pub terminal_font_family: String,
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: u32,
}

fn default_terminal_font_size() -> u32 {
    splitty_core::config::DEFAULT_TERMINAL_FONT_SIZE
}

fn default_window_decoration() -> String {
    "system".into()
}
fn default_window_buttons() -> String {
    "all".into()
}

fn default_title_bar_style() -> String {
    "theme".into()
}

impl RememberedAppearance {
    pub fn from_snapshot(snapshot: &AppearanceSnapshot) -> Self {
        let settings = snapshot.settings();
        let terminal = snapshot.terminal();
        Self {
            interface_style: settings.interface_style.clone(),
            window_decoration: settings.window_decoration.clone(),
            window_buttons: settings.window_buttons.clone(),
            title_bar_style: settings.title_bar_style.clone(),
            theme_mode: settings.theme_mode.clone(),
            theme: settings.theme.clone(),
            theme_dark: settings.theme_dark.clone(),
            theme_light: settings.theme_light.clone(),
            font_size: settings.font_size,
            font_family: settings.font_family.clone(),
            zoom: settings.zoom,
            zoom_scope: settings.zoom_scope.clone(),
            terminal_theme: terminal.theme().to_owned(),
            terminal_theme_dark: terminal.theme_dark().to_owned(),
            terminal_theme_light: terminal.theme_light().to_owned(),
            terminal_font_family: terminal.font_family().to_owned(),
            terminal_font_size: terminal.font_size(),
        }
    }

    /// The name of the UI theme for the operating system's preference, by
    /// the rule of a snapshot ([`AppearanceSnapshot::ui_theme`]).
    pub fn ui_theme(&self, scheme: ColorScheme) -> &str {
        ui_theme_name(
            &self.theme_mode,
            &self.theme,
            &self.theme_dark,
            &self.theme_light,
            scheme,
        )
    }
}

/// The file a window keeps its last applied appearance in, and what it holds.
#[derive(Debug)]
pub struct AppearanceMemory {
    path: PathBuf,
    stored: Option<RememberedAppearance>,
}

impl AppearanceMemory {
    /// The file in the configuration directory of the computer the
    /// application runs on, beside its themes directory.
    pub fn default_path() -> PathBuf {
        splitty_core::config::config_dir().join(FILE_NAME)
    }

    /// Whether that directory is one the start names itself, apart from the
    /// user's own.
    pub fn in_own_directory() -> bool {
        splitty_core::config::config_dir_override().is_some()
    }

    /// Reads `path`. A missing, oversized or malformed file holds nothing.
    pub fn open(path: PathBuf) -> Self {
        let stored = read(&path);
        Self { path, stored }
    }

    pub fn stored(&self) -> Option<&RememberedAppearance> {
        self.stored.as_ref()
    }

    /// Writes `appearance` unless the file holds it already. A failed write
    /// leaves nothing remembered, so the next call writes again.
    pub fn remember(&mut self, appearance: RememberedAppearance) -> std::io::Result<()> {
        if self.stored.as_ref() == Some(&appearance) {
            return Ok(());
        }
        self.stored = None;
        let contents = serde_json::to_vec(&appearance).map_err(std::io::Error::other)?;
        splitty_core::config::write_private_file(&self.path, &contents)?;
        self.stored = Some(appearance);
        Ok(())
    }
}

fn read(path: &Path) -> Option<RememberedAppearance> {
    let mut contents = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut contents)
        .ok()?;
    if contents.len() as u64 > MAX_BYTES {
        return None;
    }
    serde_json::from_slice(&contents).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn appearance() -> RememberedAppearance {
        RememberedAppearance {
            interface_style: "theme".into(),
            window_decoration: "system".into(),
            window_buttons: "all".into(),
            title_bar_style: "theme".into(),
            theme_mode: "auto".into(),
            theme: "splitty-dark".into(),
            theme_dark: "tokyo-night".into(),
            theme_light: "splitty-light".into(),
            font_size: 14,
            font_family: "Inter".into(),
            zoom: 1.25,
            zoom_scope: "window".into(),
            terminal_theme: "default".into(),
            terminal_theme_dark: "dracula".into(),
            terminal_theme_light: "default".into(),
            terminal_font_family: "Fira Code".into(),
            terminal_font_size: 16,
        }
    }

    fn directory() -> PathBuf {
        std::env::temp_dir().join(format!("splitty-appearance-{}", Uuid::new_v4().simple()))
    }

    #[test]
    fn decoration_selection_survives_cache_and_old_caches_default_to_theme() {
        let mut selection = appearance();
        selection.title_bar_style = "system".into();
        selection.interface_style = "system".into();
        selection.window_decoration = "splitty".into();
        selection.window_buttons = "system".into();
        let value = serde_json::to_value(&selection).unwrap();
        assert_eq!(
            serde_json::from_value::<RememberedAppearance>(value.clone()).unwrap(),
            selection
        );
        let mut old = value;
        for key in [
            "titleBarStyle",
            "interfaceStyle",
            "windowDecoration",
            "windowButtons",
        ] {
            old.as_object_mut().unwrap().remove(key);
        }
        let restored = serde_json::from_value::<RememberedAppearance>(old.clone()).unwrap();
        assert_eq!(restored.interface_style, "theme");
        assert_eq!(restored.window_decoration, "system");
        assert_eq!(restored.window_buttons, "all");
        assert_eq!(
            serde_json::from_value::<RememberedAppearance>(old)
                .unwrap()
                .title_bar_style,
            "theme"
        );
    }

    /// settings, Native Appearance Before The Local Service Answers, File
    /// from before the terminal font was remembered.
    #[test]
    fn the_terminal_font_is_remembered_and_older_files_take_the_default() {
        let value = serde_json::to_value(appearance()).unwrap();
        assert_eq!(value["terminalFontFamily"], "Fira Code");
        assert_eq!(value["terminalFontSize"], 16);
        let mut old = value;
        for key in ["terminalFontFamily", "terminalFontSize"] {
            old.as_object_mut().unwrap().remove(key);
        }
        let restored = serde_json::from_value::<RememberedAppearance>(old).unwrap();
        assert_eq!(restored.terminal_font_family, "");
        assert_eq!(restored.terminal_font_size, 14);
        assert_eq!(restored.theme_dark, appearance().theme_dark);
    }

    #[test]
    fn the_theme_follows_the_mode_and_the_systems_preference() {
        let mut appearance = appearance();
        assert_eq!(appearance.ui_theme(ColorScheme::Dark), "tokyo-night");
        assert_eq!(appearance.ui_theme(ColorScheme::Light), "splitty-light");
        appearance.theme_mode = "manual".into();
        assert_eq!(appearance.ui_theme(ColorScheme::Light), "splitty-dark");
    }

    #[test]
    fn a_remembered_appearance_is_read_by_the_next_start() {
        let directory = directory();
        let path = directory.join(FILE_NAME);
        let mut memory = AppearanceMemory::open(path.clone());
        assert_eq!(memory.stored(), None, "nothing before the first write");
        memory.remember(appearance()).unwrap();
        assert_eq!(memory.stored(), Some(&appearance()));
        assert_eq!(
            AppearanceMemory::open(path.clone()).stored(),
            Some(&appearance())
        );

        // What the file holds already is not written again.
        std::fs::remove_file(&path).unwrap();
        memory.remember(appearance()).unwrap();
        assert!(!path.exists());
        let mut light = appearance();
        light.theme_mode = "manual".into();
        light.theme = "splitty-light".into();
        memory.remember(light.clone()).unwrap();
        assert_eq!(AppearanceMemory::open(path).stored(), Some(&light));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_file_that_is_not_a_remembered_appearance_holds_nothing() {
        let directory = directory();
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(FILE_NAME);
        let valid = serde_json::to_value(appearance()).unwrap();
        let mut incomplete = valid.clone();
        incomplete.as_object_mut().unwrap().remove("themeDark");
        let mut mistyped = valid.clone();
        mistyped["zoom"] = "large".into();
        let mut oversized = valid.clone();
        oversized["fontFamily"] = "x".repeat(MAX_BYTES as usize).into();
        for contents in [
            String::new(),
            "not json".to_owned(),
            "[]".to_owned(),
            incomplete.to_string(),
            mistyped.to_string(),
            oversized.to_string(),
        ] {
            std::fs::write(&path, &contents).unwrap();
            assert_eq!(
                AppearanceMemory::open(path.clone()).stored(),
                None,
                "{}",
                &contents[..contents.len().min(60)]
            );
        }
        // A field of a later version is no reason to drop the others.
        let mut extended = valid;
        extended["borderRadius"] = 6.into();
        std::fs::write(&path, extended.to_string()).unwrap();
        assert_eq!(AppearanceMemory::open(path).stored(), Some(&appearance()));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_failed_write_is_tried_again() {
        let directory = directory();
        std::fs::create_dir_all(&directory).unwrap();
        // A directory in the file's place cannot be replaced by a file.
        let path = directory.join(FILE_NAME);
        std::fs::create_dir(&path).unwrap();
        let mut memory = AppearanceMemory::open(path.clone());
        assert!(memory.remember(appearance()).is_err());
        assert_eq!(memory.stored(), None);
        std::fs::remove_dir(&path).unwrap();
        memory.remember(appearance()).unwrap();
        assert_eq!(AppearanceMemory::open(path).stored(), Some(&appearance()));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
