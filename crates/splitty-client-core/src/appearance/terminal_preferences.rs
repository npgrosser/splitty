use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalPreferences {
    theme: String,
    close_on_exit: String,
}
impl TerminalPreferences {
    pub fn theme(&self) -> &str {
        &self.theme
    }
    pub fn close_on_exit(&self) -> &str {
        &self.close_on_exit
    }
}
