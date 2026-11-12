//! The themes the app can show: UI themes in the toolkit's theme set format
//! and terminal palettes in Ghostty's theme file format, bundled at build time
//! and read from the themes directory of this computer.
use super::palette::{slug, TerminalPalette, MAX_PALETTE_BYTES};
use gpui_kit::component::theme::ThemeConfig;
use serde::Deserialize;
use splitty_client_core::appearance::ColorScheme;
use std::{collections::HashSet, path::Path, rc::Rc};

include!(concat!(env!("OUT_DIR"), "/bundled_themes.rs"));

pub const DEFAULT_DARK: &str = "splitty-dark";
pub const DEFAULT_LIGHT: &str = "splitty-light";

const MAX_THEME_SET_BYTES: u64 = 1024 * 1024;
const MAX_THEMES_PER_SET: usize = 64;
const MAX_DIRECTORY_ENTRIES: usize = 2048;
/// Settings hold a theme name in at most this many bytes.
const MAX_ID_BYTES: usize = 64;

/// A value that is either the keyword `none` or a structure.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Maybe<T> {
    Keyword(String),
    Value(T),
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct BorderBlock {
    pub width: f32,
    pub color: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TabAccentBlock {
    pub color: String,
    pub thickness: f32,
    #[serde(default)]
    pub position: String,
}

/// The optional `splitty` object of a theme: the workspace chrome's own
/// tokens, which the toolkit's format does not define. Every absent key is
/// derived from the theme's toolkit colors.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
pub struct ChromeBlock {
    /// The name settings store, where it differs from the display name's.
    pub id: Option<String>,
    pub surface: Option<String>,
    pub raised: Option<String>,
    pub overlay: Option<String>,
    pub text: Option<String>,
    pub text_secondary: Option<String>,
    pub text_muted: Option<String>,
    pub border: Option<String>,
    pub border_subtle: Option<String>,
    pub active: Option<String>,
    pub hover: Option<String>,
    pub accent: Option<String>,
    pub danger: Option<String>,
    pub danger_hover: Option<String>,
    pub success: Option<String>,
    pub warning: Option<String>,
    /// `#rrggbbaa`: the drop marker's fill.
    pub hover_overlay: Option<String>,
    /// `#rrggbbaa`: menu shadows.
    pub shadow: Option<String>,
    pub drag_over_border: Option<Maybe<BorderBlock>>,
    pub tab_height: Option<f32>,
    pub tab_font_size: Option<f32>,
    pub tab_accent: Option<Maybe<TabAccentBlock>>,
    pub radius: Option<f32>,
    pub font_size: Option<f32>,
    /// A comma-separated family list; `system-ui` and `monospace` name the
    /// platform's fonts.
    pub font_family: Option<String>,
}

#[derive(Clone, Debug)]
pub struct UiTheme {
    pub id: String,
    pub name: String,
    pub scheme: ColorScheme,
    pub config: Rc<ThemeConfig>,
    pub chrome: ChromeBlock,
}

#[derive(Debug, Default)]
pub struct ThemeCatalog {
    ui: Vec<UiTheme>,
    palettes: Vec<TerminalPalette>,
    /// Files of the themes directory that were not loaded, for the settings view.
    problems: Vec<String>,
}

impl ThemeCatalog {
    /// The bundled themes and the files of `directory`. A bundled theme keeps
    /// its name against a file of the same name.
    pub fn load(directory: Option<&Path>) -> Self {
        let mut catalog = Self::default();
        let mut seen = HashSet::new();
        for (file, text) in UI_THEME_SETS {
            if let Err(problem) = catalog.add_theme_set(text, &mut seen) {
                catalog.problems.push(format!("{file}: {problem}"));
            }
        }
        let mut seen_palettes = HashSet::new();
        for (name, text) in TERMINAL_PALETTES {
            catalog.add_palette(name, text, &mut seen_palettes);
        }
        if let Some(directory) = directory {
            catalog.add_directory(directory, &mut seen, &mut seen_palettes);
        }
        catalog
    }

    pub fn ui_themes(&self) -> &[UiTheme] {
        &self.ui
    }
    pub fn palettes(&self) -> &[TerminalPalette] {
        &self.palettes
    }
    pub fn problems(&self) -> &[String] {
        &self.problems
    }
    pub fn ui(&self, id: &str) -> Option<&UiTheme> {
        self.ui.iter().find(|theme| theme.id == id)
    }
    pub fn palette(&self, id: &str) -> Option<&TerminalPalette> {
        self.palettes.iter().find(|palette| palette.id == id)
    }
    /// The palette that goes with a UI theme: the one with its identifier,
    /// or one whose identifier differs only in separators (`tokyo-night`,
    /// `tokyonight`).
    pub fn palette_for_theme(&self, id: &str) -> Option<&TerminalPalette> {
        let plain = |id: &str| id.replace('-', "");
        self.palette(id).or_else(|| {
            let wanted = plain(id);
            self.palettes
                .iter()
                .find(|palette| plain(&palette.id) == wanted)
        })
    }
    /// The default UI theme of a mode; `None` only if the bundled set is broken.
    pub fn default_ui(&self, scheme: ColorScheme) -> Option<&UiTheme> {
        self.ui(match scheme {
            ColorScheme::Dark => DEFAULT_DARK,
            ColorScheme::Light => DEFAULT_LIGHT,
        })
    }
    pub fn default_palette(&self, scheme: ColorScheme) -> Option<&TerminalPalette> {
        self.palette(match scheme {
            ColorScheme::Dark => DEFAULT_DARK,
            ColorScheme::Light => DEFAULT_LIGHT,
        })
    }

    fn add_theme_set(&mut self, text: &str, seen: &mut HashSet<String>) -> Result<(), String> {
        let set: serde_json::Value =
            serde_json::from_str(text).map_err(|error| error.to_string())?;
        let themes = set
            .get("themes")
            .and_then(|themes| themes.as_array())
            .ok_or("not a theme set: no `themes` list")?;
        let mut problem = None;
        for value in themes.iter().take(MAX_THEMES_PER_SET) {
            let config = match serde_json::from_value::<ThemeConfig>(value.clone()) {
                Ok(config) if !config.name.trim().is_empty() => config,
                Ok(_) => {
                    problem = Some("a theme has no name".to_owned());
                    continue;
                }
                Err(error) => {
                    problem = Some(error.to_string());
                    continue;
                }
            };
            let chrome = match value.get("splitty") {
                None => ChromeBlock::default(),
                Some(block) => serde_json::from_value(block.clone()).unwrap_or_else(|error| {
                    problem = Some(format!("{}: `splitty`: {error}", config.name));
                    ChromeBlock::default()
                }),
            };
            let id = chrome.id.clone().unwrap_or_else(|| slug(&config.name));
            if !valid_id(&id) {
                problem = Some(format!("{}: no usable identifier", config.name));
                continue;
            }
            if !seen.insert(id.clone()) {
                continue;
            }
            self.ui.push(UiTheme {
                id,
                name: config.name.to_string(),
                scheme: if config.mode.is_dark() {
                    ColorScheme::Dark
                } else {
                    ColorScheme::Light
                },
                config: Rc::new(config),
                chrome,
            });
        }
        problem.map_or(Ok(()), Err)
    }

    fn add_palette(&mut self, name: &str, text: &str, seen: &mut HashSet<String>) -> bool {
        match TerminalPalette::parse(name, text) {
            Some(palette) if valid_id(&palette.id) => {
                if seen.insert(palette.id.clone()) {
                    self.palettes.push(palette);
                }
                true
            }
            _ => false,
        }
    }

    /// `*.json` files are theme sets; `*.toml` files are the web client's
    /// themes and are left alone; every other file is a terminal palette.
    fn add_directory(
        &mut self,
        directory: &Path,
        seen: &mut HashSet<String>,
        seen_palettes: &mut HashSet<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut files: Vec<_> = entries
            .filter_map(Result::ok)
            .take(MAX_DIRECTORY_ENTRIES)
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let metadata = std::fs::metadata(entry.path()).ok()?;
                (metadata.is_file() && !name.starts_with('.')).then_some((
                    name,
                    entry.path(),
                    metadata.len(),
                ))
            })
            .collect();
        files.sort();
        for (name, path, size) in files {
            let extension = path.extension().and_then(|extension| extension.to_str());
            match extension {
                Some("toml") => {}
                Some("json") => {
                    let result = if size > MAX_THEME_SET_BYTES {
                        Err("larger than a theme set may be".to_owned())
                    } else {
                        std::fs::read_to_string(&path)
                            .map_err(|error| error.to_string())
                            .and_then(|text| self.add_theme_set(&text, seen))
                    };
                    if let Err(problem) = result {
                        self.problems.push(format!("{name}: {problem}"));
                    }
                }
                _ => {
                    let palette_name = name.strip_suffix(".conf").unwrap_or(&name);
                    let loaded = size <= MAX_PALETTE_BYTES as u64
                        && std::fs::read_to_string(&path)
                            .is_ok_and(|text| self.add_palette(palette_name, &text, seen_palettes));
                    if !loaded {
                        self.problems
                            .push(format!("{name}: not a terminal palette"));
                    }
                }
            }
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_BYTES && id != "default"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_catalog_loads_completely_and_pairs_the_default_themes() {
        let catalog = ThemeCatalog::load(None);
        assert_eq!(catalog.problems(), &[] as &[String]);
        assert_eq!(catalog.palettes().len(), TERMINAL_PALETTES.len());
        for scheme in [ColorScheme::Dark, ColorScheme::Light] {
            let theme = catalog.default_ui(scheme).unwrap();
            assert_eq!(theme.scheme, scheme);
            let palette = catalog.default_palette(scheme).unwrap();
            assert_eq!(palette.is_dark(), scheme == ColorScheme::Dark);
        }
        // The names stored by earlier versions still select a theme and a
        // terminal palette of the same name.
        for id in [
            "splitty-dark",
            "splitty-light",
            "splitty-classic",
            "splitty-classic-light",
            "splitty-retro",
            "splitty-aurora",
            "splitty-neon",
            "splitty-cozy",
            "splitty-light-gray",
            "splitty-classic-dark-gray",
            "splitty-classic-light-gray",
            "adwaita-dark",
            "adwaita-light",
        ] {
            assert!(catalog.ui(id).is_some(), "{id}");
            assert!(catalog.palette(id).is_some(), "{id}");
        }
        assert!(catalog.ui("catppuccin-mocha").is_some());
        assert!(catalog.palette("catppuccin-mocha").is_some());
        let mut ids: Vec<_> = catalog.ui_themes().iter().map(|theme| &theme.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), catalog.ui_themes().len());
    }

    #[test]
    fn the_themes_directory_adds_theme_sets_and_palettes_and_reports_other_files() {
        let directory = std::env::temp_dir().join(format!(
            "splitty-theme-catalog-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(directory.join("nested")).unwrap();
        let write = |name: &str, text: &str| std::fs::write(directory.join(name), text).unwrap();
        write(
            "mine.json",
            r##"{"name": "Mine", "themes": [
                {"name": "Mine Dark", "mode": "dark", "colors": {"background": "#101010"},
                 "splitty": {"tab-height": 28, "tab-accent": "none"}},
                {"name": "Renamed", "mode": "light", "splitty": {"id": "mine-light"}},
                {"name": "Splitty Dark", "mode": "light"},
                {"name": "Broken Block", "mode": "dark", "splitty": {"tab-height": "tall"}}
            ]}"##,
        );
        write("broken.json", "{");
        write("web-theme.toml", "name = 'Web'\n");
        write("My Palette", "background = #000000\nforeground = #ffffff\n");
        write("Other.conf", "background = #ffffff\nforeground = #000000\n");
        write("notes.txt", "not a palette\n");
        write(".hidden", "background = #000000\nforeground = #ffffff\n");

        let catalog = ThemeCatalog::load(Some(&directory));
        let mine = catalog.ui("mine-dark").unwrap();
        assert_eq!(mine.scheme, ColorScheme::Dark);
        assert_eq!(mine.chrome.tab_height, Some(28.));
        assert_eq!(mine.chrome.tab_accent, Some(Maybe::Keyword("none".into())));
        assert_eq!(catalog.ui("mine-light").unwrap().name, "Renamed");
        // A bundled theme keeps its name.
        assert_eq!(
            catalog.ui("splitty-dark").unwrap().scheme,
            ColorScheme::Dark
        );
        // A theme with an unreadable block loads with derived chrome.
        assert_eq!(
            catalog.ui("broken-block").unwrap().chrome,
            ChromeBlock::default()
        );
        assert!(catalog.palette("my-palette").unwrap().is_dark());
        assert!(!catalog.palette("other").unwrap().is_dark());
        assert!(catalog.palette("hidden").is_none());
        assert!(catalog.ui("web").is_none());
        let problems = catalog.problems().join("\n");
        assert!(problems.contains("broken.json"), "{problems}");
        assert!(problems.contains("mine.json: Broken Block"), "{problems}");
        assert!(
            problems.contains("notes.txt: not a terminal palette"),
            "{problems}"
        );
        assert!(!problems.contains("web-theme.toml"), "{problems}");
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
