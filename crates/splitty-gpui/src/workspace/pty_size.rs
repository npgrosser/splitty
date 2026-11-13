//! The window's part in the servers' PTY size policy (terminal-sessions,
//! Client Activity Reports, Client Display of the PTY Size): it reports its
//! user's interaction and its visibility and focus to the worker, which
//! decides what each connection hears, and hands each session's PTY size to
//! the terminal that shows it.
#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
use super::*;
use splitty_client_core::native_workspace::{PtySize, WindowPresence};

impl Workspace {
    /// The window's visibility or focus may have changed.
    pub(super) fn window_presence_changed(&self, window: &Window) {
        self.presence.set(WindowPresence {
            visible: window.is_visible(),
            focused: window.is_window_active(),
        });
    }

    /// The user acted in the window: a key, a pointer button, activating or
    /// resizing it. The server of the shown Screen counts the window as in use.
    pub(super) fn user_acted(&self) {
        self.presence.acted();
    }

    /// A session's PTY size changed on its connection.
    pub(super) fn pty_size_changed(
        &mut self,
        source: SourceToken,
        session: String,
        size: PtySize,
        cx: &mut Context<Self>,
    ) {
        if self.execution_token(source.source_id()) != Some(source) {
            return;
        }
        let grid = terminal_grid(size);
        for view in self.panes.values() {
            if view.route.source_token() == source && view.route.session_id() == session {
                view.terminal
                    .update(cx, |terminal, cx| terminal.set_pty_size(Some(grid), cx));
            }
        }
        // A view made later, as for output before its layout, takes it then.
        self.pty_sizes.insert((source, session), size);
    }

    /// A new view of `route` shows the session at its known PTY size.
    pub(super) fn apply_known_pty_size(
        &self,
        route: &TerminalToken,
        terminal: &Entity<Terminal>,
        cx: &mut Context<Self>,
    ) {
        if let Some(size) = self
            .pty_sizes
            .get(&(route.source_token(), route.session_id().to_owned()))
        {
            let grid = terminal_grid(*size);
            terminal.update(cx, |terminal, cx| terminal.set_pty_size(Some(grid), cx));
        }
    }

    /// Forget the sizes of sessions no current route shows.
    pub(super) fn retain_pty_sizes(&mut self, window_model: &WindowWorkspace) {
        let shown: HashSet<_> = window_model
            .routes()
            .into_iter()
            .map(|route| {
                let terminal = route.terminal();
                (terminal.source_token(), terminal.session_id().to_owned())
            })
            .collect();
        self.pty_sizes.retain(|key, _| shown.contains(key));
    }
}

fn terminal_grid(size: PtySize) -> terminal::PtyGrid {
    terminal::PtyGrid {
        cols: size.cols,
        rows: size.rows,
        takeover: size.offers_takeover(),
    }
}
