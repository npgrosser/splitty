//! Commands a service forwards to its windows (`control-api`, Forwarding
//! Commands to UI Clients): the pane actions of the terminal menu, asked for
//! from outside the window. A command is untrusted data. It reaches only the
//! Screen this window shows, of the service that sent it, and a host's
//! command never reads or writes this computer's clipboard.
#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
use super::*;
use crate::terminal::LocalAction;
use splitty_client_core::{Event, MessageType};

impl Workspace {
    pub(super) fn observe_pane_command(
        &mut self,
        source: SourceToken,
        event: &Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = event
            .json()
            .filter(|json| MessageType::Command.is(json))
            .map(|json| &json["command"])
        else {
            return;
        };
        let Some(kind) = command["type"].as_str() else {
            return;
        };
        let Some(model) = self
            .selected_model()
            .filter(|model| model.source_token() == source)
        else {
            return;
        };
        let Some(screen_id) = model.shown_screen_id() else {
            return;
        };
        let pane = match command.get("paneId") {
            None | Some(serde_json::Value::Null) => model.focused_pane(),
            Some(pane) => pane
                .as_str()
                .filter(|pane| model.screen_for(pane) == Some(screen_id)),
        };
        let Some(pane) = pane.map(str::to_owned) else {
            return;
        };
        let screen = model.screen_key(screen_id);
        let tiled = model.is_root_pane(&pane);
        let local = self
            .local_model()
            .is_some_and(|local| local.source_token() == source);
        let terminal = self
            .panes
            .get(&model.pane_key(&pane))
            .map(|view| view.terminal.clone());
        let act = |action: LocalAction, window: &mut Window, cx: &mut Context<Self>| {
            if let Some(terminal) = &terminal {
                terminal.update(cx, |terminal, cx| terminal.local_action(action, window, cx));
            }
        };
        match kind {
            "pane.activate" => self.select_captured(source, &screen, &pane, window, cx),
            "pane.copy" if local => act(LocalAction::Copy, window, cx),
            "pane.paste" if local => act(LocalAction::Paste, window, cx),
            "pane.clear" => act(LocalAction::ClearBuffer, window, cx),
            "pane.close" => self.request_close_captured(
                source,
                CloseTarget::Pane {
                    screen: screen.screen_id().to_owned(),
                    pane,
                },
                window,
                cx,
            ),
            "pane.split" if tiled => {
                let axis = match command["direction"].as_str() {
                    Some("right") => Axis::Horizontal,
                    Some("below") => Axis::Vertical,
                    _ => return,
                };
                self.send_captured(source, Command::Split { screen, pane, axis }, cx);
            }
            _ => {}
        }
    }
}
