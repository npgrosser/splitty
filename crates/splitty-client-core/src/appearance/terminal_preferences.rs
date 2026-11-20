use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalPreferences {
    theme: String,
    /// Absent on a service that predates the pair, which means `default`.
    #[serde(default)]
    theme_dark: Option<String>,
    #[serde(default)]
    theme_light: Option<String>,
    close_on_exit: String,
    /// Absent in older stored settings, which means shown.
    #[serde(default)]
    scroll_to_bottom: Option<bool>,
    /// Absent in older stored settings, which means allowed.
    #[serde(default)]
    allow_osc52_write: Option<bool>,
    /// Absent on a service that predates the terminal font settings, which
    /// means the default face.
    #[serde(default)]
    font_family: Option<String>,
    /// Absent there too, which means the default size.
    #[serde(default)]
    font_size: Option<u32>,
}
impl TerminalPreferences {
    pub fn theme(&self) -> &str {
        &self.theme
    }
    /// The palette for a dark UI theme while `theme` is `default`.
    pub fn theme_dark(&self) -> &str {
        self.theme_dark.as_deref().unwrap_or("default")
    }
    /// The palette for a light UI theme while `theme` is `default`.
    pub fn theme_light(&self) -> &str {
        self.theme_light.as_deref().unwrap_or("default")
    }
    pub fn close_on_exit(&self) -> &str {
        &self.close_on_exit
    }
    /// Whether a scrolled terminal shows the control that returns to its
    /// newest rows.
    pub fn scroll_to_bottom(&self) -> bool {
        self.scroll_to_bottom.unwrap_or(true)
    }
    /// Whether a program in a terminal may write the system clipboard
    /// (OSC 52). Reading it is never allowed.
    pub fn allow_osc52_write(&self) -> bool {
        self.allow_osc52_write.unwrap_or(true)
    }
    /// The family terminals draw text in; empty for the default (the UI
    /// theme's proposal, else the bundled font).
    pub fn font_family(&self) -> &str {
        self.font_family.as_deref().unwrap_or_default()
    }
    /// The terminal text size in pixels before zoom.
    pub fn font_size(&self) -> u32 {
        self.font_size
            .unwrap_or(splitty_core::config::DEFAULT_TERMINAL_FONT_SIZE)
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalPreferences;

    #[test]
    fn clipboard_writes_are_allowed_unless_the_stored_settings_turn_them_off() {
        let read = |terminal: serde_json::Value| {
            serde_json::from_value::<TerminalPreferences>(terminal)
                .expect("terminal settings")
                .allow_osc52_write()
        };
        let stored = serde_json::json!({"theme": "default", "closeOnExit": "never"});
        assert!(read(stored.clone()));
        for allowed in [false, true] {
            let mut stored = stored.clone();
            stored["allowOsc52Write"] = allowed.into();
            assert_eq!(read(stored), allowed);
        }
    }

    /// settings, Native Terminal Font Choice, Older local service: a service
    /// that does not send the font settings reads as their defaults.
    #[test]
    fn the_terminal_font_reads_as_the_default_where_the_service_sends_none() {
        let read = |terminal: serde_json::Value| {
            serde_json::from_value::<TerminalPreferences>(terminal).expect("terminal settings")
        };
        let older = read(serde_json::json!({"theme": "default", "closeOnExit": "never"}));
        assert_eq!((older.font_family(), older.font_size()), ("", 14));
        let chosen = read(serde_json::json!({
            "theme": "default", "closeOnExit": "never",
            "fontFamily": "Fira Code", "fontSize": 18
        }));
        assert_eq!(
            (chosen.font_family(), chosen.font_size()),
            ("Fira Code", 18)
        );
    }
}
