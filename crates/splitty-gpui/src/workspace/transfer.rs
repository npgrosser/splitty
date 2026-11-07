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

impl Workspace {
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
        let Some(model) = self.model.as_ref() else {
            return;
        };
        match model.capture_pane_action(pane) {
            Ok(source) => {
                self.profile_picker = None;
                self.transfer_menu = Some(TransferMenu {
                    source,
                    selected: 0,
                    scroll: ScrollHandle::new(),
                });
            }
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
            .model
            .as_ref()
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
                let destination = self.model.as_ref().and_then(|model| {
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
        let Some(model) = self.model.as_ref() else {
            return;
        };
        if let Err(error) = model.validate_pane_action(&menu.source) {
            self.status = format!("Move target changed: {error}");
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
                screen_id: screen_id.to_owned(),
            }
        } else {
            ScreenTransferTarget::New {
                screen_id: Uuid::new_v4().to_string(),
            }
        };
        let request_id = Uuid::new_v4();
        let connection_id = menu.source.connection_id;
        match self.commands.try_send(Command::TransferPane {
            request_id,
            source: menu.source,
            target,
        }) {
            Ok(()) => {
                self.pending_transfer = Some(PendingTransfer {
                    connection_id,
                    request_id,
                });
            }
            Err(error) => self.status = format!("Move not sent: {error}"),
        }
        cx.notify();
    }

    pub(super) fn render_transfer_menu(&self, pane: &str, cx: &mut Context<Self>) -> AnyElement {
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
            .model
            .as_ref()
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
        let trigger_id = SharedString::from(format!("tab-transfer-{pane}"));
        let colors = self.appearance.clone();
        let busy = self.pending_transfer.is_some()
            || self
                .model
                .as_ref()
                .is_some_and(|model| model.transfer_pending() || model.routes_reconciling());
        Popover::new(SharedString::from(format!("transfer-menu-{pane}")))
            .open(open)
            .mouse_button(MouseButton::Left)
            .trigger_with(move |_, _, _| {
                div()
                    .id(trigger_id)
                    .px_1()
                    .cursor_pointer()
                    .child("⋯")
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .into_any_element()
            })
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

#[cfg(test)]
mod tests {
    use super::{destination_ids, PaneActionTarget, PendingTransfer};
    use splitty_client_core::workspace::ScreenInfo;
    use uuid::Uuid;

    #[test]
    fn tab_menu_targets_captured_source_and_ignores_unrelated_transfer_result() {
        let owner = Uuid::new_v4();
        let source = PaneActionTarget {
            connection_id: owner,
            screen_id: "screen-b".into(),
            pane_id: "nonselected-pane".into(),
            session_id: "session-b".into(),
        };
        let screens = [
            ScreenInfo {
                id: "screen-a".into(),
                name: Some("First".into()),
            },
            ScreenInfo {
                id: "screen-b".into(),
                name: Some("Current".into()),
            },
            ScreenInfo {
                id: "screen-c".into(),
                name: Some("Third".into()),
            },
        ];
        assert_eq!(destination_ids(&screens, &source), ["screen-a", "screen-c"]);
        assert_eq!(source.pane_id, "nonselected-pane");
        let pending = PendingTransfer {
            connection_id: owner,
            request_id: Uuid::new_v4(),
        };
        assert!(pending.matches(owner, pending.request_id));
        assert!(!pending.matches(owner, Uuid::new_v4()));
        assert!(!pending.matches(Uuid::new_v4(), pending.request_id));
    }
}
