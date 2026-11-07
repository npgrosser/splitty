//! Presentation of service-owned local shell choices for one captured tab source.
use super::*;

struct ProfileSource {
    route: TerminalToken,
    slot: LayoutKey,
}

pub(super) struct ProfilePicker {
    source: ProfileSource,
    selected: Option<String>,
    scroll: ScrollHandle,
}

impl ProfilePicker {
    pub(super) fn valid_for(&self, model: &SourceWorkspace) -> bool {
        let source = &self.source;
        if model.source_token() != source.route.source_token()
            || model.shown_screen_id() != Some(source.slot.screen().screen_id())
            || model.screen_key(source.slot.screen().screen_id()) != *source.slot.screen()
        {
            return false;
        }
        let Some(screen) = model.shown() else {
            return false;
        };
        let Some(slot @ LayoutNode::Slot { .. }) =
            presented_slot_by_id(model, source.slot.node_id())
        else {
            return false;
        };
        source_matches_slot(screen, source, slot)
    }
}

fn source_matches_slot(screen: &ScreenModel, source: &ProfileSource, slot: &LayoutNode) -> bool {
    screen.selected_pane(slot) == Some(source.route.pane_id())
        && screen.session_for(source.route.pane_id()) == Some(source.route.session_id())
}

fn next_id<'a>(ids: &[&'a str], current: Option<&str>, down: bool) -> Option<&'a str> {
    if ids.is_empty() {
        return None;
    }
    let index = current.and_then(|id| ids.iter().position(|candidate| *candidate == id));
    let next = match (index, down) {
        (Some(index), true) => (index + 1).min(ids.len() - 1),
        (Some(index), false) => index.saturating_sub(1),
        (None, true) => 0,
        (None, false) => ids.len() - 1,
    };
    ids.get(next).copied()
}

impl Workspace {
    pub(super) fn apply_profiles(
        &mut self,
        snapshot: Arc<ShellProfilesSnapshot>,
        cx: &mut Context<Self>,
    ) {
        if !self
            .local_model()
            .is_some_and(|model| model.connection_id() == snapshot.connection_id())
            || snapshot.generation() <= self.profiles_generation
        {
            return;
        }
        if let Some(picker) = self.profile_picker.as_mut() {
            let available: Vec<_> = snapshot
                .state()
                .profiles
                .iter()
                .filter(|profile| profile.enabled && profile.available)
                .map(|profile| profile.id.as_str())
                .collect();
            if !picker
                .selected
                .as_deref()
                .is_some_and(|id| available.contains(&id))
            {
                picker.selected = available.first().map(|id| (*id).to_owned());
            }
        }
        self.profiles_generation = snapshot.generation();
        self.profiles = Some(snapshot);
        self.profiles_error = None;
        cx.notify();
    }

    pub(super) fn profile_error(
        &mut self,
        connection: &str,
        reason: String,
        cx: &mut Context<Self>,
    ) {
        if self
            .local_model()
            .is_some_and(|model| model.connection_id().to_string() == connection)
        {
            self.profiles_error = Some(reason);
            cx.notify();
        }
    }

    pub(super) fn close_profile_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.profile_picker.take().is_some() {
            self.focus_current(window, cx);
            cx.notify();
        }
    }

    pub(super) fn toggle_profile_picker(
        &mut self,
        slot: &str,
        pane: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transfer_menu = None;
        if self
            .profile_picker
            .as_ref()
            .is_some_and(|picker| picker.source.slot.node_id() == slot)
        {
            self.close_profile_picker(window, cx);
            return;
        }
        let Some(model) = self.local_model() else {
            return;
        };
        let Some(screen) = model.shown_screen_id() else {
            return;
        };
        let Some(source_slot @ LayoutNode::Slot { .. }) = presented_slot_by_id(model, slot) else {
            return;
        };
        if model
            .shown()
            .and_then(|shown| shown.selected_pane(source_slot))
            != Some(pane)
        {
            return;
        }
        let Some(route) = model.terminal_token(pane) else {
            self.status = "Shell profile source is still starting".into();
            cx.notify();
            return;
        };
        let selected = self.profiles.as_ref().and_then(|snapshot| {
            let state = snapshot.state();
            state
                .profiles
                .iter()
                .find(|profile| {
                    profile.id == state.default_profile_id && profile.enabled && profile.available
                })
                .or_else(|| {
                    state
                        .profiles
                        .iter()
                        .find(|profile| profile.enabled && profile.available)
                })
                .map(|profile| profile.id.clone())
        });
        self.profile_picker = Some(ProfilePicker {
            source: ProfileSource {
                route,
                slot: model.screen_key(screen).layout_key(slot),
            },
            selected,
            scroll: ScrollHandle::new(),
        });
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn move_profile_choice(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(snapshot) = self.profiles.as_ref() else {
            return;
        };
        let ids: Vec<_> = snapshot
            .state()
            .profiles
            .iter()
            .filter(|profile| profile.enabled && profile.available)
            .map(|profile| profile.id.as_str())
            .collect();
        let Some(picker) = self.profile_picker.as_mut() else {
            return;
        };
        picker.selected = next_id(&ids, picker.selected.as_deref(), down).map(str::to_owned);
        if let Some(index) = picker.selected.as_deref().and_then(|id| {
            snapshot
                .state()
                .profiles
                .iter()
                .filter(|profile| profile.enabled)
                .position(|profile| profile.id == id)
        }) {
            picker.scroll.scroll_to_item(index);
        }
        cx.notify();
    }

    pub(super) fn profile_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match key.key.to_ascii_lowercase().as_str() {
            "escape" => self.close_profile_picker(window, cx),
            "up" => self.move_profile_choice(false, cx),
            "down" => self.move_profile_choice(true, cx),
            "enter" | "return" => {
                if let Some(id) = self
                    .profile_picker
                    .as_ref()
                    .and_then(|picker| picker.selected.clone())
                {
                    self.choose_profile(&id, window, cx);
                }
            }
            _ => {}
        }
        // The picker owns every key while open, including unrecognized ones.
        cx.stop_propagation();
    }

    pub(super) fn choose_profile(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.profile_picker.as_ref() else {
            return;
        };
        let Some(model) = self.local_model() else {
            return;
        };
        if !picker.valid_for(model) {
            self.status = "Shell profile target changed; open the picker again".into();
            self.close_profile_picker(window, cx);
            return;
        }
        if !self.profiles.as_ref().is_some_and(|snapshot| {
            snapshot.connection_id() == model.connection_id()
                && snapshot
                    .state()
                    .profiles
                    .iter()
                    .any(|profile| profile.id == id && profile.enabled && profile.available)
        }) {
            self.status = "Shell profile is no longer available".into();
            cx.notify();
            return;
        }
        let command = Command::ProfileTab {
            connection: picker.source.route.source_token().connection_id(),
            screen: picker.source.slot.screen().screen_id().to_owned(),
            pane: picker.source.route.pane_id().to_owned(),
            session: picker.source.route.session_id().to_owned(),
            profile_id: id.to_owned(),
        };
        self.send_captured(picker.source.route.source_token(), command, cx);
        self.close_profile_picker(window, cx);
    }

    pub(super) fn profile_picker_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let picker = self.profile_picker.as_ref()?;
        let colors = &self.appearance;
        let mut entries = div()
            .id("shell-profile-list")
            .flex()
            .flex_col()
            .max_h(px((360. * self.appearance.zoom.window_scale())
                .min(f32::from(window.viewport_size().height) * 0.55)))
            .overflow_y_scroll()
            .track_scroll(&picker.scroll);
        let mut count = 0;
        if let Some(snapshot) = &self.profiles {
            for (index, profile) in snapshot
                .state()
                .profiles
                .iter()
                .filter(|profile| profile.enabled)
                .enumerate()
            {
                count += 1;
                let id = profile.id.clone();
                let available = profile.available;
                let selected = picker.selected.as_deref() == Some(id.as_str());
                let mut title = profile.name.clone();
                if profile.id == snapshot.state().default_profile_id {
                    title.push_str(" · Default");
                }
                if profile.experimental {
                    title.push_str(" · Experimental");
                }
                let detail = (!available).then(|| {
                    profile
                        .reason
                        .as_deref()
                        .filter(|reason| !reason.is_empty())
                        .unwrap_or("Unavailable")
                        .to_owned()
                });
                let row = div()
                    .id(SharedString::from(format!("shell-profile-option-{index}")))
                    .flex()
                    .flex_col()
                    .px_2()
                    .py_1()
                    .bg(rgb(if selected {
                        colors.active
                    } else {
                        colors.raised
                    }))
                    .text_color(rgb(if available { colors.text } else { colors.muted }))
                    .child(title)
                    .when_some(detail, |row, reason| {
                        row.child(div().text_size(chrome_rems(11.)).child(reason))
                    })
                    .when(available, |row| {
                        row.cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.choose_profile(&id, window, cx);
                            }))
                    });
                entries = entries.child(row);
            }
        }
        if count == 0 {
            entries = entries.child(div().px_2().py_2().child(if self.profiles.is_some() {
                "No enabled shell profiles"
            } else {
                "Loading shell profiles…"
            }));
        }
        let menu = div()
            .id("shell-profile-picker")
            .absolute()
            .top(chrome_rems((self.appearance.font_size + 34.).max(47.)))
            .right(chrome_rems(12.))
            .w(chrome_rems(360.))
            .max_w(relative(0.9))
            .p_2()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.raised))
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px_2()
                    .py_1()
                    .font_weight(FontWeight::BOLD)
                    .child("New tab with shell"),
            )
            .when_some(self.profiles_error.as_ref(), |menu, reason| {
                menu.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_color(rgb(colors.secondary))
                        .child(format!("Profiles: {reason}")),
                )
            })
            .child(entries);
        Some(
            div()
                .absolute()
                .size_full()
                .child(
                    div()
                        .id("shell-profile-dismiss")
                        .absolute()
                        .size_full()
                        .bg(rgba(0x00000044))
                        .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_profile_picker(window, cx);
                        }))
                        .on_mouse_move(|_, _, cx| cx.stop_propagation())
                        .on_scroll(|_, _, cx| cx.stop_propagation()),
                )
                .child(menu)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{source_matches_slot, ProfileSource};
    use crate::workspace::presented_slot_in;
    use splitty_client_core::{
        workspace::{ScreenModel, SourceWorkspace},
        EmbeddedService, ScreenIds,
    };

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn captured_profile_source_resolves_float_and_rejects_replaced_session() {
        let service = EmbeddedService::start();
        let mut client = service.connect().await.unwrap();
        let connection = client.connection_id();
        let mut workspace = SourceWorkspace::new(connection);
        workspace.set_catalog_supported(true);
        workspace.mark_catalog_dirty();
        workspace.flush_catalog(&client).unwrap();
        loop {
            let event = client.next_event().await.unwrap().unwrap();
            if event
                .json()
                .is_some_and(|json| json["type"] == "mux.workspace_snapshot")
            {
                workspace
                    .apply_catalog(connection, event.json().unwrap())
                    .unwrap();
                break;
            }
        }
        let ids = workspace.create_screen(&client, "screen").unwrap();
        workspace.observe_spawn(connection, &ids.pane_id, "sf");
        let route = workspace.terminal_token(&ids.pane_id).unwrap();
        let mut screen = ScreenModel::new(
            ScreenIds {
                screen_id: "screen".into(),
                pane_id: ids.pane_id.clone(),
            },
            connection,
        );
        let snapshot = serde_json::json!({
            "type":"mux.screen_snapshot",
            "screen":{"id":"screen","revision":1,
                "root":{"type":"slot","id":"tile-slot","paneIds":["tiled"]},
                "floating":[{"id":"float","root":{"type":"slot","id":"float-slot","paneIds":[ids.pane_id]},
                    "bounds":{"x":0.25,"y":0.25,"w":0.5,"h":0.5}}],
                "panes":{"tiled":{"id":"tiled"},ids.pane_id.clone():{"id":ids.pane_id,"sessionId":"sf"}},
                "sessions":{"sf":{"id":"sf","status":"running"}}}
        });
        screen.apply_snapshot(connection, &snapshot).unwrap();
        let source = ProfileSource {
            route,
            slot: workspace.screen_key("screen").layout_key("float-slot"),
        };
        let slot = presented_slot_in(
            screen.presentation_root(),
            screen.presented_floats(),
            source.slot.node_id(),
        )
        .unwrap();
        assert!(source_matches_slot(&screen, &source, slot));
        let mut replaced = snapshot.clone();
        replaced["screen"]["revision"] = serde_json::json!(2);
        replaced["screen"]["panes"][source.route.pane_id()]["sessionId"] =
            serde_json::json!("replaced");
        replaced["screen"]["sessions"] =
            serde_json::json!({"replaced":{"id":"replaced","status":"running"}});
        screen.apply_snapshot(connection, &replaced).unwrap();
        let replaced_slot = presented_slot_in(
            screen.presentation_root(),
            screen.presented_floats(),
            source.slot.node_id(),
        )
        .unwrap();
        assert!(!source_matches_slot(&screen, &source, replaced_slot));
        client.finish().await.unwrap();
        service.close().await.unwrap();
    }
}
