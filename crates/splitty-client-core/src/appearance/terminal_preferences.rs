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
}
