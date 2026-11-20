//! The Keyboard page: every command with its keys, to read and to rebind.
use super::*;

impl Workspace {
    /// Show the Keyboard page. Until it exists, the settings open.
    pub(super) fn show_keyboard_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_settings(window, cx);
    }
}
