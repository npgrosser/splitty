use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalPreferences {
    theme: String,
    close_on_exit: String,
    /// Absent in older stored settings, which means shown.
    #[serde(default)]
    scroll_to_bottom: Option<bool>,
}
impl TerminalPreferences {
    pub fn theme(&self) -> &str {
        &self.theme
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
