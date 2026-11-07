//! A tab-local menu for moving the captured terminal to another local Screen.
use super::*;

fn destination_ids(
    screens: &[splitty_client_core::workspace::ScreenInfo],
    source: &PaneActionTarget,
) -> Vec<String> {
    screens
        .iter()
        .filter(|screen| screen.id.as_str() != source.screen_id)
        .map(|screen| screen.id.to_string())
        .collect()
}

fn tab_context_matches(
    model: &SourceWorkspace,
    source: SourceToken,
    screen: &ScreenKey,
    pane: &str,
) -> bool {
    model.source_token() == source
        && model.shown_screen_id() == Some(screen.screen_id())
        && model.screen_key(screen.screen_id()) == *screen
        && model.screen_for(pane) == Some(screen.screen_id())
}

impl Workspace {
    pub(super) fn open_transfer_menu_captured(
        &mut self,
        source: SourceToken,
        screen: &ScreenKey,
        pane: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_remote_selected() {
            return;
        }
        if !self
            .selected_model()
            .is_some_and(|model| tab_context_matches(model, source, screen, pane))
        {
            return;
        }
        self.open_transfer_menu(pane, window, cx);
    }
    pub(super) fn open_transfer_menu(
        &mut self,
        pane: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.rename.is_some()
            || self
                .pending_close
                .as_ref()
                .is_some_and(|pending| pending.prompting)
        {
            return;
        }
        let Some(model) = self.selected_model() else {
            return;
        };
        match model.capture_pane_action(pane) {
            Ok(source) if model.can_transfer_pane(&source) => {
                self.profile_picker = None;
                self.transfer_menu = Some(TransferMenu {
                    source,
                    selected: 0,
                    scroll: ScrollHandle::new(),
                });
            }
            Ok(_) => self.status = "Move unavailable for this terminal".into(),
            Err(error) => self.status = format!("Move unavailable: {error}"),
        }
        cx.notify();
    }

    pub(super) fn transfer_menu_change(
        &mut self,
        pane: &str,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if open {
            self.open_transfer_menu(pane, window, cx);
        } else if self
            .transfer_menu
            .as_ref()
            .is_some_and(|menu| menu.source.pane_id == pane)
        {
            self.transfer_menu = None;
            cx.notify();
        }
    }

    pub(super) fn transfer_menu_key(&mut self, key: &Keystroke, cx: &mut Context<Self>) {
        let Some(menu) = self.transfer_menu.as_ref() else {
            return;
        };
        let count = self
            .selected_model()
            .map(|model| destination_ids(model.screens(), &menu.source).len() + 1)
            .unwrap_or(1);
        match key.key.to_ascii_lowercase().as_str() {
            "escape" => {
                self.transfer_menu = None;
                cx.notify();
            }
            "up" => {
                if let Some(menu) = self.transfer_menu.as_mut() {
                    menu.selected = menu.selected.saturating_sub(1);
                    menu.scroll.scroll_to_item(menu.selected + 1);
                    cx.notify();
                }
            }
            "down" => {
                if let Some(menu) = self.transfer_menu.as_mut() {
                    menu.selected = (menu.selected + 1).min(count - 1);
                    menu.scroll.scroll_to_item(menu.selected + 1);
                    cx.notify();
                }
            }
            "enter" | "return" => {
                let source = &menu.source;
                let selected = menu.selected;
                let destination = self.selected_model().and_then(|model| {
                    destination_ids(model.screens(), source)
                        .get(selected)
                        .cloned()
                });
                self.choose_transfer(&source.pane_id.clone(), destination.as_deref(), cx);
            }
            _ => {}
        }
        cx.stop_propagation();
    }

    pub(super) fn choose_transfer(
        &mut self,
        pane: &str,
        existing: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.transfer_menu.take() else {
            return;
        };
        if menu.source.pane_id != pane {
            self.transfer_menu = Some(menu);
            return;
        }
        let Some(model) = self.selected_model() else {
            return;
        };
        if let Err(error) = model.validate_pane_action(&menu.source) {
            self.status = format!("Move target changed: {error}");
            cx.notify();
            return;
        }
        if !model.can_transfer_pane(&menu.source) {
            self.status = "Move unavailable for this terminal".into();
            cx.notify();
            return;
        }
        if model.transfer_pending() || model.routes_reconciling() || self.pending_transfer.is_some()
        {
            self.status = "Screen move is still being verified".into();
            cx.notify();
            return;
        }
        let target = if let Some(screen_id) = existing {
            if screen_id == menu.source.screen_id
                || !model
                    .screens()
                    .iter()
                    .any(|screen| screen.id.as_str() == screen_id)
            {
                self.status = "Move destination changed; open the menu again".into();
                cx.notify();
                return;
            }
            ScreenTransferTarget::Existing {
                screen: model.screen_key(screen_id),
            }
        } else {
            ScreenTransferTarget::New {
                screen_id: Uuid::new_v4().to_string(),
            }
        };
        let request_id = Uuid::new_v4();
        let source_token = menu.source.source_token();
        match self.commands.try_send(WorkerCommand::new(
            source_token,
            Command::TransferPane {
                request_id,
                source: menu.source,
                target,
            },
        )) {
            Ok(()) => {
                self.pending_transfer = Some(PendingTransfer {
                    source: source_token,
                    request_id,
                });
            }
            Err(error) => self.status = format!("Move not sent: {error}"),
        }
        cx.notify();
    }

    pub(super) fn render_transfer_menu(&self, pane: &str, cx: &mut Context<Self>) -> AnyElement {
        if !self.selected_model().is_some_and(|model| {
            model
                .capture_pane_action(pane)
                .is_ok_and(|source| model.can_transfer_pane(&source))
        }) {
            return div().into_any_element();
        }
        let open = self
            .transfer_menu
            .as_ref()
            .is_some_and(|menu| menu.source.pane_id == pane);
        let selected = self
            .transfer_menu
            .as_ref()
            .map(|menu| menu.selected)
            .unwrap_or(0);
        let source = self
            .transfer_menu
            .as_ref()
            .filter(|menu| menu.source.pane_id == pane);
        let scroll = source.map(|menu| menu.scroll.clone()).unwrap_or_default();
        let destinations: Vec<_> = self
            .selected_model()
            .zip(source)
            .map(|(model, menu)| {
                model
                    .screens()
                    .iter()
                    .filter(|screen| screen.id.as_str() != menu.source.screen_id)
                    .map(|screen| {
                        (
                            screen.id.to_string(),
                            screen.name.clone().unwrap_or_else(|| "Screen".to_owned()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let weak = cx.entity().downgrade();
        let content_weak = weak.clone();
        let changed_pane = pane.to_owned();
        let menu_pane = pane.to_owned();
        let anchor_id = SharedString::from(format!("tab-transfer-anchor-{pane}"));
        let colors = self.appearance.clone();
        let busy = self.pending_transfer.is_some()
            || self
                .selected_model()
                .is_some_and(|model| model.transfer_pending() || model.routes_reconciling());
        Popover::new(SharedString::from(format!("transfer-menu-{pane}")))
            .absolute()
            .bottom_0()
            .left_0()
            .w_0()
            .h_0()
            .open(open)
            .mouse_button(MouseButton::Left)
            .trigger_with(move |_, _, _| div().id(anchor_id).w_0().h_0().into_any_element())
            .on_open_change(move |open, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.transfer_menu_change(&changed_pane, *open, window, cx)
                });
            })
            .content(move |_, window, _| {
                let weak = content_weak.clone();
                let new_index = destinations.len();
                let mut items = div()
                    .id(SharedString::from(format!("transfer-items-{menu_pane}")))
                    .flex()
                    .flex_col()
                    .min_w(chrome_rems(190.))
                    .max_w(chrome_rems(300.))
                    .max_h(px((300. * colors.zoom.window_scale())
                        .min(f32::from(window.viewport_size().height) * 0.55)))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .bg(rgb(colors.raised))
                    .text_color(rgb(colors.text))
                    .border_1()
                    .border_color(rgb(colors.border))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(div().px_3().py_1().child("Move to Screen"));
                for (index, (screen_id, name)) in destinations.into_iter().enumerate() {
                    let pane = menu_pane.clone();
                    let id = screen_id.clone();
                    let weak = weak.clone();
                    items = items.child(
                        div()
                            .id(SharedString::from(format!("transfer-existing-{pane}-{id}")))
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .bg(rgb(if index == selected {
                                colors.active
                            } else {
                                colors.raised
                            }))
                            .child(name)
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let _ = weak.update(cx, |this, cx| {
                                    this.choose_transfer(&pane, Some(&id), cx)
                                });
                            }),
                    );
                }
                let pane = menu_pane.clone();
                let new_selected = selected == new_index;
                items.child(
                    div()
                        .id(SharedString::from(format!("transfer-new-{pane}")))
                        .px_3()
                        .py_1()
                        .cursor_pointer()
                        .bg(rgb(if new_selected {
                            colors.active
                        } else {
                            colors.raised
                        }))
                        .child(if busy {
                            "Move in progress"
                        } else {
                            "New Screen"
                        })
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            let _ =
                                weak.update(cx, |this, cx| this.choose_transfer(&pane, None, cx));
                        }),
                )
            })
            .into_any_element()
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::tab_context_matches;
    use splitty_client_core::{workspace::SourceWorkspace, EmbeddedService};

    #[tokio::test]
    async fn nonselected_tab_context_stays_valid_only_on_its_captured_source_and_screen() {
        let service = EmbeddedService::start();
        let mut client = service.connect().await.unwrap();
        let connection = client.connection_id();
        let mut model = SourceWorkspace::new(connection);
        model.set_catalog_supported(true);
        model.mark_catalog_dirty();
        model.flush_catalog(&client).unwrap();
        loop {
            let event = client.next_event().await.unwrap().unwrap();
            if event
                .json()
                .is_some_and(|json| json["type"] == "mux.workspace_snapshot")
            {
                model
                    .apply_catalog(connection, event.json().unwrap())
                    .unwrap();
                break;
            }
        }
        let ids = model.create_screen(&client, "Source").unwrap();
        model.flush_catalog(&client).unwrap();
        loop {
            let event = client.next_event().await.unwrap().unwrap();
            if event
                .json()
                .is_some_and(|json| json["type"] == "mux.workspace_snapshot")
            {
                model
                    .apply_catalog(connection, event.json().unwrap())
                    .unwrap();
                break;
            }
        }
        let inactive = uuid::Uuid::new_v4().to_string();
        let snapshot = serde_json::json!({
            "type":"mux.screen_snapshot",
            "screen":{"id":ids.screen_id,"revision":1,
                "root":{"type":"slot","id":"root","paneIds":[ids.pane_id,inactive],"activePaneId":ids.pane_id},
                "panes":{ids.pane_id.clone():{"id":ids.pane_id},inactive.clone():{"id":inactive}},
                "sessions":{}}
        });
        model.apply_snapshot(connection, &snapshot).unwrap();
        let screen = model.screen_key(&ids.screen_id);
        let source = model.source_token();
        assert!(
            !model.is_displayed_pane(&inactive),
            "the context target is deliberately not the selected terminal"
        );
        assert!(tab_context_matches(&model, source, &screen, &inactive));
        assert!(!tab_context_matches(
            &model,
            source,
            &model.screen_key("other"),
            &inactive
        ));
        assert!(!tab_context_matches(
            &model,
            SourceWorkspace::new(uuid::Uuid::new_v4()).source_token(),
            &screen,
            &inactive
        ));
        assert!(!tab_context_matches(&model, source, &screen, "missing"));
        client.finish().await.unwrap();
        service.close().await.unwrap();
    }
}
