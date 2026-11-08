//! The captured tab's Close action and, where supported, Screen transfer.
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
    pub(super) fn open_tab_context_menu_captured(
        &mut self,
        source: SourceToken,
        screen: &ScreenKey,
        pane: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .selected_model()
            .is_some_and(|model| tab_context_matches(model, source, screen, pane))
        {
            return;
        }
        self.open_tab_context_menu(pane, window, cx);
    }
    pub(super) fn open_tab_context_menu(
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
            Ok(source) => {
                let selected =
                    usize::from(!self.is_remote_selected() && model.can_transfer_pane(&source));
                self.profile_picker = None;
                self.tab_context_menu = Some(TabContextMenu {
                    source,
                    selected,
                    scroll: ScrollHandle::new(),
                });
            }
            Err(error) => self.status = format!("Tab action unavailable: {error}"),
        }
        cx.notify();
    }

    pub(super) fn tab_context_menu_change(
        &mut self,
        pane: &str,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if open {
            self.open_tab_context_menu(pane, window, cx);
        } else if self
            .tab_context_menu
            .as_ref()
            .is_some_and(|menu| menu.source.pane_id == pane)
        {
            self.tab_context_menu = None;
            cx.notify();
        }
    }

    pub(super) fn tab_context_menu_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.tab_context_menu.as_ref() else {
            return;
        };
        let count = self.selected_model().map_or(1, |model| {
            if !self.is_remote_selected() && model.can_transfer_pane(&menu.source) {
                destination_ids(model.screens(), &menu.source).len() + 2
            } else {
                1
            }
        });
        match key.key.to_ascii_lowercase().as_str() {
            "escape" => {
                self.tab_context_menu = None;
                cx.notify();
            }
            "up" => {
                if let Some(menu) = self.tab_context_menu.as_mut() {
                    menu.selected = menu.selected.saturating_sub(1);
                    menu.scroll.scroll_to_item(if menu.selected == 0 {
                        1
                    } else {
                        menu.selected + 3
                    });
                    cx.notify();
                }
            }
            "down" => {
                if let Some(menu) = self.tab_context_menu.as_mut() {
                    menu.selected = (menu.selected + 1).min(count - 1);
                    menu.scroll.scroll_to_item(if menu.selected == 0 {
                        1
                    } else {
                        menu.selected + 3
                    });
                    cx.notify();
                }
            }
            "enter" | "return" => {
                let source = &menu.source;
                let pane = source.pane_id.clone();
                let selected = menu.selected.min(count - 1);
                if selected == 0 {
                    self.choose_close_tab(&pane, window, cx);
                } else {
                    let destination = self.selected_model().and_then(|model| {
                        destination_ids(model.screens(), source)
                            .get(selected - 1)
                            .cloned()
                    });
                    self.choose_transfer(&pane, destination.as_deref(), cx);
                }
            }
            _ => {}
        }
        cx.stop_propagation();
    }

    pub(super) fn choose_close_tab(
        &mut self,
        pane: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.tab_context_menu.take() else {
            return;
        };
        if menu.source.pane_id != pane {
            self.tab_context_menu = Some(menu);
            return;
        }
        let Some(model) = self.selected_model() else {
            return;
        };
        if let Err(error) = model.validate_pane_action(&menu.source) {
            self.status = format!("Tab changed: {error}");
            cx.notify();
            return;
        }
        self.request_close_captured(
            menu.source.source_token(),
            CloseTarget::Pane {
                screen: menu.source.screen_id,
                pane: menu.source.pane_id,
            },
            window,
            cx,
        );
    }

    pub(super) fn choose_transfer(
        &mut self,
        pane: &str,
        existing: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.tab_context_menu.take() else {
            return;
        };
        if menu.source.pane_id != pane {
            self.tab_context_menu = Some(menu);
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
        if self.is_remote_selected() || !model.can_transfer_pane(&menu.source) {
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

    pub(super) fn render_tab_context_menu(&self, pane: &str, cx: &mut Context<Self>) -> AnyElement {
        if !self
            .selected_model()
            .is_some_and(|model| model.capture_pane_action(pane).is_ok())
        {
            return div().into_any_element();
        }
        let open = self
            .tab_context_menu
            .as_ref()
            .is_some_and(|menu| menu.source.pane_id == pane);
        let selected = self
            .tab_context_menu
            .as_ref()
            .map(|menu| menu.selected)
            .unwrap_or(0);
        let source = self
            .tab_context_menu
            .as_ref()
            .filter(|menu| menu.source.pane_id == pane);
        let can_transfer = !self.is_remote_selected()
            && self
                .selected_model()
                .zip(source)
                .is_some_and(|(model, menu)| model.can_transfer_pane(&menu.source));
        let selected = if can_transfer { selected } else { 0 };
        let scroll = source.map(|menu| menu.scroll.clone()).unwrap_or_default();
        let destinations: Vec<_> = self
            .selected_model()
            .zip(source)
            .filter(|_| can_transfer)
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
        let anchor_id = SharedString::from(format!("tab-context-anchor-{pane}"));
        let colors = self.appearance.clone();
        let busy = self.pending_transfer.is_some()
            || self
                .selected_model()
                .is_some_and(|model| model.transfer_pending() || model.routes_reconciling());
        Popover::new(SharedString::from(format!("tab-context-menu-{pane}")))
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
                    this.tab_context_menu_change(&changed_pane, *open, window, cx)
                });
            })
            .content(move |_, window, _| {
                let weak = content_weak.clone();
                let new_index = destinations.len() + 1;
                let mut items = div()
                    .id(SharedString::from(format!("tab-context-items-{menu_pane}")))
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
                    .child(div().px_3().py_1().child("Tab actions"));
                let close_pane = menu_pane.clone();
                let close_weak = weak.clone();
                items = items.child(
                    div()
                        .id(SharedString::from(format!(
                            "tab-context-close-{close_pane}"
                        )))
                        .debug_selector(|| "tab-context-close".into())
                        .px_3()
                        .py_1()
                        .cursor_pointer()
                        .bg(rgb(if selected == 0 {
                            colors.active
                        } else {
                            colors.raised
                        }))
                        .child("Close Tab")
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            let _ = close_weak.update(cx, |this, cx| {
                                this.choose_close_tab(&close_pane, window, cx)
                            });
                        }),
                );
                if !can_transfer {
                    return items;
                }
                items = items
                    .child(div().h(px(1.)).mx_2().bg(rgb(colors.border)))
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
                            .bg(rgb(if index + 1 == selected {
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
                        .debug_selector(|| "tab-context-transfer-new".into())
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

#[cfg(all(test, feature = "ui-test-support"))]
mod view_tests {
    use super::{CloseTarget, Command, Destination, Workspace};
    use gpui_kit::{
        AppContext, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext,
        VisualTestContext,
    };
    use splitty_client_core::{native_workspace::Worker, workspace::WindowWorkspace};
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{atomic::AtomicBool, Arc},
    };
    use tokio::sync::mpsc;
    use uuid::Uuid;

    fn model_with_tabs(owner: Uuid) -> WindowWorkspace {
        let mut model = WindowWorkspace::new(owner);
        model.ui_test_local_screen_fixture("Work").unwrap();
        let token = model.lifecycle_token();
        model
            .source_mut(token.source_id())
            .unwrap()
            .apply_snapshot(
                owner,
                &serde_json::json!({"type":"mux.screen_snapshot","screen":{
                    "id":"work","revision":2,
                    "root":{"type":"slot","id":"slot","paneIds":["p0","p1"],"activePaneId":"p0"},
                    "panes":{"p0":{"id":"p0","sessionId":"s0"},
                        "p1":{"id":"p1","sessionId":"s1"}},
                    "sessions":{"s0":{"id":"s0","status":"running","busy":false},
                        "s1":{"id":"s1","status":"running","busy":false}}
                }}),
            )
            .unwrap();
        model
    }

    fn with_menu(
        run: impl FnOnce(
            &mut VisualTestContext,
            Entity<Workspace>,
            Uuid,
            mpsc::Receiver<splitty_client_core::native_workspace::Command>,
        ),
    ) {
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
        let view = content.borrow_mut().take().unwrap();
        let owner = Uuid::new_v4();
        let model = model_with_tabs(owner);
        let token = model.lifecycle_token();
        let (commands, receiver) = mpsc::channel(32);
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.model = Some(model);
                workspace.commands = commands;
                workspace.destination = Destination::Screens;
                let screen = workspace
                    .model
                    .as_ref()
                    .unwrap()
                    .local_source()
                    .screen_key("work");
                workspace.open_tab_context_menu_captured(token, &screen, "p1", window, cx);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        run(window, view, owner, receiver);
    }

    fn next_close(
        receiver: &mut mpsc::Receiver<splitty_client_core::native_workspace::Command>,
    ) -> splitty_client_core::workspace::ClosePlan {
        loop {
            let command = receiver.try_recv().expect("assessed close sent");
            if let Command::RequestClose { plan, .. } = command.kind() {
                return plan.clone();
            }
        }
    }

    #[test]
    fn inactive_tab_close_pointer_uses_assessed_captured_pane() {
        with_menu(|window, _, owner, mut receiver| {
            let bounds = window
                .debug_bounds("tab-context-close")
                .expect("Close Tab shown");
            let center = bounds.center();
            window.simulate_event(MouseDownEvent {
                button: MouseButton::Left,
                position: center,
                click_count: 1,
                modifiers: Modifiers::default(),
                first_mouse: false,
            });
            window.simulate_event(MouseUpEvent {
                button: MouseButton::Left,
                position: center,
                click_count: 1,
                modifiers: Modifiers::default(),
            });
            let plan = next_close(&mut receiver);
            assert_eq!(plan.connection_id, Some(owner));
            assert_eq!(
                plan.target,
                CloseTarget::Pane {
                    screen: "work".into(),
                    pane: "p1".into()
                }
            );
        });
    }

    #[test]
    fn tab_close_keyboard_and_stale_owner_do_not_redirect() {
        with_menu(|window, view, _, mut receiver| {
            window.simulate_keystrokes("up enter");
            let plan = next_close(&mut receiver);
            assert_eq!(
                plan.target,
                CloseTarget::Pane {
                    screen: "work".into(),
                    pane: "p1".into()
                }
            );
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let old = workspace
                        .model
                        .as_ref()
                        .unwrap()
                        .local_source()
                        .source_token();
                    let screen = workspace
                        .model
                        .as_ref()
                        .unwrap()
                        .local_source()
                        .screen_key("work");
                    workspace.open_tab_context_menu_captured(old, &screen, "p1", window, cx);
                    workspace.model = Some(model_with_tabs(Uuid::new_v4()));
                    workspace.choose_close_tab("p1", window, cx);
                });
            });
            while let Ok(command) = receiver.try_recv() {
                assert!(
                    !matches!(command.kind(), Command::RequestClose { .. }),
                    "replacement owner cannot receive the old tab's close"
                );
            }
        });
    }

    #[test]
    fn remote_tab_offers_assessed_close_without_local_screen_transfer() {
        with_menu(|window, view, _, mut receiver| {
            let mut remote = WindowWorkspace::new(Uuid::new_v4());
            let (screen, terminal) = remote
                .ui_test_remote_terminal_fixture("demo@192.0.2.9")
                .unwrap();
            let source = terminal.source_token();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.model = Some(remote);
                    workspace.open_tab_context_menu_captured(source, &screen, "p0", window, cx);
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            assert!(window.debug_bounds("tab-context-close").is_some());
            assert!(window.debug_bounds("tab-context-transfer-new").is_none());
            let center = window.debug_bounds("tab-context-close").unwrap().center();
            window.simulate_event(MouseDownEvent {
                button: MouseButton::Left,
                position: center,
                click_count: 1,
                modifiers: Modifiers::default(),
                first_mouse: false,
            });
            window.simulate_event(MouseUpEvent {
                button: MouseButton::Left,
                position: center,
                click_count: 1,
                modifiers: Modifiers::default(),
            });
            let plan = next_close(&mut receiver);
            assert_eq!(plan.source_token(), Some(source));
            assert_eq!(
                plan.target,
                CloseTarget::Pane {
                    screen: "work".into(),
                    pane: "p0".into()
                }
            );
        });
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
