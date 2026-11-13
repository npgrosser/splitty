//! Presentation of service-owned local shell choices for one captured tab source.
use super::*;

struct ProfileSource {
    route: TerminalToken,
    slot: LayoutKey,
}

#[cfg(all(test, feature = "ui-test-support"))]
mod view_tests {
    use super::{Destination, Workspace};
    use gpui_kit::{
        px, size, AppContext, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
        TestAppContext, VisualTestContext,
    };
    use splitty_client_core::{native_workspace::Worker, workspace::WindowWorkspace};
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{atomic::AtomicBool, Arc},
    };
    use uuid::Uuid;

    fn with_view(run: impl FnOnce(&mut VisualTestContext, Entity<Workspace>)) {
        let mut app = TestAppContext::single();
        app.update(gpui_kit::init);
        let content = Rc::new(RefCell::new(None));
        let (_, window) = app.add_window_view({
            let content = content.clone();
            move |window, cx| {
                let view = cx.new(|cx| {
                    let (workspace, worker) = Workspace::new_with_worker(
                        false,
                        Arc::new(AtomicBool::new(false)),
                        window,
                        cx,
                        |_, _, _| Worker::ui_test_fixture(),
                    );
                    worker.shutdown().unwrap();
                    workspace
                });
                *content.borrow_mut() = Some(view.clone());
                gpui_kit::base::Root::new(view, window, cx)
            }
        });
        run(window, content.borrow_mut().take().unwrap());
    }

    fn click(window: &mut VisualTestContext, position: gpui_kit::Point<gpui_kit::Pixels>) {
        window.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        window.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            modifiers: Modifiers::default(),
        });
    }

    #[test]
    fn picker_follows_its_group_arrow_and_stays_inside_window() {
        with_view(|window, view| {
            let connection = Uuid::new_v4();
            let mut model = WindowWorkspace::new(connection);
            model.ui_test_local_screen_fixture("Work").unwrap();
            let owner = model.lifecycle_token();
            model
                .source_mut(owner.source_id())
                .unwrap()
                .apply_snapshot(
                    connection,
                    &serde_json::json!({"type":"mux.screen_snapshot","screen":{
                        "id":"work","revision":2,
                        "root":{"type":"split","id":"split","axis":"horizontal",
                            "sizes":[1,1],"children":[
                                {"type":"slot","id":"left","paneIds":["p0"],"activePaneId":"p0"},
                                {"type":"slot","id":"right","paneIds":["p1"],"activePaneId":"p1"}
                            ]},
                        "panes":{"p0":{"id":"p0","sessionId":"s0"},
                                 "p1":{"id":"p1","sessionId":"s1"}},
                        "sessions":{"s0":{"id":"s0","status":"running"},
                                    "s1":{"id":"s1","status":"running"}}
                    }}),
                )
                .unwrap();
            model
                .source_mut(owner.source_id())
                .unwrap()
                .observe_spawn(connection, "p0", "s0");
            model
                .source_mut(owner.source_id())
                .unwrap()
                .observe_spawn(connection, "p1", "s1");
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            window.simulate_resize(size(px(900.), px(500.)));
            window.update(|window, cx| window.draw(cx).clear(cx));
            let left_arrow = window.debug_bounds("shell-profile-trigger-left").unwrap();
            click(window, left_arrow.center());
            let open_focus = window.update(|window, cx| window.focused(cx));
            window.update(|_, cx| assert!(view.read(cx).profile_picker.is_some()));
            window.update(|window, cx| window.draw(cx).clear(cx));
            let left_menu = window.debug_bounds("shell-profile-picker").unwrap();
            let left_delta = (f32::from(left_menu.left()) - f32::from(left_arrow.left())).abs();
            assert!(left_delta <= 1., "left arrow/menu delta: {left_delta}");
            assert!(left_menu.top() >= left_arrow.bottom());

            window.simulate_keystrokes("escape");
            let after_focus = window.update(|window, cx| window.focused(cx));
            assert!(
                after_focus.is_some(),
                "Escape must return focus to the arrow"
            );
            assert_ne!(after_focus, open_focus, "the popup must release focus");
            window.update(|_, cx| assert!(view.read(cx).profile_picker.is_none()));
            window.simulate_keystrokes("enter");
            window.update(|_, cx| assert!(view.read(cx).profile_picker.is_some()));
            window.simulate_keystrokes("escape");
            window.update(|_, cx| assert!(view.read(cx).profile_picker.is_none()));
            window.simulate_resize(size(px(520.), px(380.)));
            window.update(|window, cx| window.draw(cx).clear(cx));
            let right_arrow = window.debug_bounds("shell-profile-trigger-right").unwrap();
            click(window, right_arrow.center());
            window.update(|_, cx| assert!(view.read(cx).profile_picker.is_some()));
            window.update(|window, cx| window.draw(cx).clear(cx));
            let right_menu = window.debug_bounds("shell-profile-picker").unwrap();
            assert!(right_arrow.left() > px(250.));
            assert!(right_menu.right() <= px(512.));
            assert!(right_menu.left() <= right_arrow.left());
            assert!(right_menu.top() >= right_arrow.bottom());
        });
    }
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

    pub(super) fn profile_picker_open_for(&self, slot: &str) -> bool {
        self.profile_picker
            .as_ref()
            .is_some_and(|picker| picker.source.slot.node_id() == slot)
    }

    pub(super) fn dismiss_profile_picker(
        &mut self,
        slot: &str,
        target: &PaneActionTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.profile_picker.as_ref().is_some_and(|picker| {
            picker.source.slot.node_id() == slot
                && picker.source.slot.screen().screen_id() == target.screen_id.as_str()
                && picker.source.route.source_token() == target.source_token()
                && picker.source.route.pane_id() == target.pane_id.as_str()
                && picker.source.route.session_id() == target.session_id.as_str()
        }) {
            self.profile_picker = None;
            cx.notify();
            return true;
        }
        false
    }

    pub(super) fn toggle_profile_picker(
        &mut self,
        slot: &str,
        pane: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_context_menu = None;
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
            // The anchored popover owns Escape so it can restore focus to its trigger.
            "escape" => return,
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

    pub(super) fn profile_picker_content(
        &self,
        slot: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let picker = self.profile_picker.as_ref()?;
        if picker.source.slot.node_id() != slot {
            return None;
        }
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
                let default = profile.id == snapshot.state().default_profile_id;
                let badge = match (default, profile.experimental) {
                    (true, true) => Some("Default · Experimental"),
                    (true, false) => Some("Default"),
                    (false, true) => Some("Experimental"),
                    (false, false) => None,
                };
                let detail = (!available).then(|| {
                    profile
                        .reason
                        .as_deref()
                        .filter(|reason| !reason.is_empty())
                        .unwrap_or("Unavailable")
                        .to_owned()
                });
                let label = div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(format!(
                        "{}  {}",
                        profile_glyph(&profile.command),
                        profile.name
                    ))
                    .when_some(detail, |label, reason| {
                        label.child(menu_style::detail(colors, reason).mt(px(2.)))
                    });
                let meta = div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(chrome_rems(6.))
                    .when_some(badge, |meta, badge| {
                        meta.child(menu_style::badge(colors, badge))
                    })
                    .when(default, |meta| {
                        meta.child(menu_style::detail(
                            colors,
                            if cfg!(target_os = "macos") {
                                "⌘T"
                            } else {
                                "Alt+T"
                            },
                        ))
                    });
                let row = if available {
                    menu_style::item(colors, selected, false)
                        .id(SharedString::from(format!("shell-profile-option-{index}")))
                        .child(label)
                        .child(meta)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.choose_profile(&id, window, cx);
                        }))
                        .into_any_element()
                } else {
                    menu_style::disabled_item(colors)
                        .id(SharedString::from(format!("shell-profile-option-{index}")))
                        .child(label)
                        .child(meta)
                        .into_any_element()
                };
                entries = entries.child(row);
            }
        }
        if count == 0 {
            entries = entries.child(menu_style::disabled_item(colors).child(
                if self.profiles.is_some() {
                    "No enabled shell profiles"
                } else {
                    "Loading shell profiles…"
                },
            ));
        }
        let menu = menu_style::panel(colors)
            .id("shell-profile-picker")
            .debug_selector(|| "shell-profile-picker".into())
            .max_w(px((f32::from(window.viewport_size().width) - 16.).max(0.)))
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .when_some(self.profiles_error.as_ref(), |menu, reason| {
                menu.child(menu_style::caption(colors, format!("Profiles: {reason}")))
            })
            .child(entries);
        Some(menu.into_any_element())
    }
}

/// The web client's text glyph for a profile's shell, shown before its name.
fn profile_glyph(command: &str) -> &'static str {
    let executable = command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if executable.contains("powershell") || executable.starts_with("pwsh") {
        "❯_"
    } else if executable == "fish" {
        "><>"
    } else if executable == "zsh" {
        "%"
    } else if executable == "bash" {
        "$"
    } else {
        ">_"
    }
}

#[cfg(test)]
mod tests {
    use super::{profile_glyph, source_matches_slot, ProfileSource};
    use crate::workspace::presented_slot_in;
    #[cfg(not(target_os = "windows"))]
    use splitty_client_core::EmbeddedService;
    use splitty_client_core::{
        workspace::{ScreenModel, SourceWorkspace},
        ScreenIds,
    };

    #[test]
    fn profile_glyphs_follow_the_shell_executable() {
        assert_eq!(profile_glyph("/bin/zsh"), "%");
        assert_eq!(profile_glyph("/usr/local/bin/fish"), "><>");
        assert_eq!(profile_glyph("bash"), "$");
        assert_eq!(
            profile_glyph("C:\\Program Files\\PowerShell\\7\\pwsh.exe"),
            "❯_"
        );
        assert_eq!(profile_glyph("/bin/sh"), ">_");
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn captured_profile_source_resolves_float_and_rejects_replaced_session() {
        let service = EmbeddedService::isolated();
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
