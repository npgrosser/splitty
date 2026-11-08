//! Captured native Host Update confirmation; the worker owns admission and execution.
use super::*;
use gpui_kit::component::dialog::DialogFooter;
use splitty_client_core::workspace::{HostActionTarget, HostUpdatePlan};

impl Workspace {
    pub(super) fn refresh_backend_version(&mut self, cx: &mut Context<Self>) {
        if let Some(owner) = self.lifecycle_token() {
            self.send_captured(owner, Command::RefreshBackendVersion, cx);
        }
    }

    pub(super) fn request_host_update(
        &mut self,
        target: HostActionTarget,
        trigger: FocusHandle,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.destination != Destination::Hosts
            || self.hosts.has_open_form()
            || self.hosts.update_prompt.is_some()
        {
            return;
        }
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let Some(host) = model
            .host(target.source_id())
            .filter(|host| host.action_target() == &target && host.is_update_available())
        else {
            return;
        };
        if host
            .update_offer()
            .is_some_and(|offer| offer.to_version().is_none())
            && model.backend_version().is_none()
        {
            self.hosts.command_error = Some(
                "The assigned backend version is unavailable. Refresh it before updating.".into(),
            );
            cx.notify();
            return;
        }
        let owner = target.local_owner();
        if self.lifecycle_token() != Some(owner) {
            return;
        }
        let request_id = Uuid::new_v4();
        if let Err(error) = self.commands.try_send(WorkerCommand::new(
            owner,
            Command::RequestHostUpdate {
                request_id,
                target: target.clone(),
            },
        )) {
            self.hosts.command_error = Some(format!("Update check could not be sent: {error}"));
            cx.notify();
            return;
        }
        self.hosts.command_error = None;
        self.hosts.update_prompt = Some(hosts::HostUpdatePrompt {
            request_id,
            target,
            trigger,
            plan: None,
            error: None,
        });
        cx.notify();
    }

    pub(super) fn host_update_prepared(
        &mut self,
        plan: Arc<HostUpdatePlan>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = self.hosts.update_prompt.as_ref().is_some_and(|prompt| {
            prompt.request_id == plan.request_id()
                && prompt.target == *plan.target()
                && prompt.plan.is_none()
        });
        if !pending {
            return;
        }
        if self.destination != Destination::Hosts
            || !self
                .model
                .as_ref()
                .is_some_and(|model| model.accepts_host_update_plan(&plan))
        {
            self.dismiss_host_update(plan.request_id(), window, cx);
            self.hosts.command_error = Some("Host update changed. Open Update again.".into());
            cx.notify();
            return;
        }
        if let Some(prompt) = self.hosts.update_prompt.as_mut() {
            prompt.plan = Some(plan);
        }
        self.open_host_update_dialog(window, cx);
        cx.notify();
    }

    pub(super) fn host_update_rejected(
        &mut self,
        owner: SourceToken,
        request_id: Uuid,
        reason: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.hosts.update_prompt.as_ref().is_some_and(|prompt| {
            prompt.request_id == request_id && prompt.target.local_owner() == owner
        }) {
            return;
        }
        self.dismiss_host_update(request_id, window, cx);
        self.hosts.command_error = Some(format!("Update unavailable: {}", hosts::display(&reason)));
        cx.notify();
    }

    pub(super) fn host_update_reconcile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.hosts.update_prompt.as_ref() else {
            return;
        };
        let valid = self.model.as_ref().is_some_and(|model| {
            model.lifecycle_token() == prompt.target.local_owner()
                && model.host(prompt.target.source_id()).is_some_and(|host| {
                    host.action_target() == &prompt.target && host.is_update_available()
                })
                && prompt
                    .plan
                    .as_ref()
                    .is_none_or(|plan| model.accepts_host_update_plan(plan))
        });
        if !valid {
            let request_id = prompt.request_id;
            self.dismiss_host_update(request_id, window, cx);
            self.hosts.command_error = Some("Host update changed. Open Update again.".into());
            cx.notify();
        }
    }

    fn open_host_update_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let Some(view) = weak.upgrade() else {
                return dialog.title("Update host");
            };
            let Some((request_id, target, from, to, legacy, error)) = ({
                let workspace = view.read(cx);
                workspace.hosts.update_prompt.as_ref().and_then(|prompt| {
                    let plan = prompt.plan.as_ref()?;
                    Some((
                        prompt.request_id,
                        hosts::display(plan.target().target()),
                        hosts::display(plan.from_version()),
                        hosts::display(plan.to_version()),
                        plan.legacy_sessions(),
                        prompt.error.clone(),
                    ))
                })
            }) else {
                return dialog.title("Update host");
            };
            let cancel_weak = weak.clone();
            let confirm_weak = weak.clone();
            dialog
                .title("Update host")
                .overlay_closable(false)
                .keyboard(true)
                .close_button(false)
                .on_cancel({
                    let weak = weak.clone();
                    move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.dismiss_host_update(request_id, window, cx)
                        });
                        false
                    }
                })
                .on_ok({
                    let weak = weak.clone();
                    move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.confirm_host_update(request_id, window, cx)
                        });
                        false
                    }
                })
                .child(
                    div()
                        .id("host-update-confirmation")
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(format!("SSH target: {target}"))
                        .child(format!("Current host: {from}"))
                        .child(format!("Replacement: {to}"))
                        .child("Keeper-managed sessions are preserved. The connection and open Screens will be briefly interrupted while the host reconnects.")
                        .when_some(legacy, |view, count| {
                            view.child(format!(
                                "Legacy migration: {count} sessions were observed. The backend checks them again before changing anything."
                            ))
                        })
                        .when_some(error, |view, error| {
                            view.child(div().text_color(cx.theme().danger).child(hosts::display(&error)))
                        }),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("host-update-cancel")
                                .label("Cancel")
                                .outline()
                                .on_click(move |_, window, cx| {
                                    let _ = cancel_weak.update(cx, |this, cx| {
                                        this.dismiss_host_update(request_id, window, cx)
                                    });
                                }),
                        )
                        .child(
                            Button::new("host-update-confirm")
                                .debug_selector(|| "host-update-confirm".into())
                                .label("Update host")
                                .on_click(move |_, window, cx| {
                                    let _ = confirm_weak.update(cx, |this, cx| {
                                        this.confirm_host_update(request_id, window, cx)
                                    });
                                }),
                        ),
                )
        });
    }

    fn confirm_host_update(
        &mut self,
        request_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = self
            .hosts
            .update_prompt
            .as_ref()
            .filter(|prompt| prompt.request_id == request_id)
        else {
            return;
        };
        let Some(plan) = prompt.plan.as_ref() else {
            return;
        };
        if !self
            .model
            .as_ref()
            .is_some_and(|model| model.accepts_host_update_plan(plan))
        {
            self.dismiss_host_update(request_id, window, cx);
            self.hosts.command_error = Some("Host update changed. Open Update again.".into());
            cx.notify();
            return;
        }
        let owner = plan.owner();
        let command = WorkerCommand::new(owner, Command::ConfirmHostUpdate { plan: plan.clone() });
        if let Err(error) = self.commands.try_send(command) {
            if let Some(prompt) = self.hosts.update_prompt.as_mut() {
                prompt.error = Some(format!("Update could not be sent: {error}"));
            }
            cx.notify();
            window.refresh();
            return;
        }
        let Some(prompt) = self.hosts.update_prompt.take() else {
            return;
        };
        window.close_dialog(cx);
        prompt.trigger.focus(window, cx);
        cx.notify();
    }

    pub(super) fn dismiss_host_update(
        &mut self,
        request_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .hosts
            .update_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.request_id == request_id)
        {
            return;
        }
        let Some(prompt) = self.hosts.update_prompt.take() else {
            return;
        };
        if let Err(error) = self.commands.try_send(WorkerCommand::new(
            prompt.target.local_owner(),
            Command::DismissHostUpdate { request_id },
        )) {
            self.hosts.command_error = Some(format!("Could not dismiss update check: {error}"));
        }
        if prompt.plan.is_some() {
            window.close_dialog(cx);
        }
        prompt.trigger.focus(window, cx);
        cx.notify();
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::{
        Command, Destination, HostActionTarget, Update, WindowWorkspace, WorkerCommand, Workspace,
    };
    use gpui_kit::{
        px, size, AppContext, Entity, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers,
        TestAppContext, VisualTestContext,
    };
    use splitty_client_core::{
        hub::{HostDirectorySnapshot, HostDirectoryState, HubUpdate},
        native_workspace::Worker,
    };
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{atomic::AtomicBool, Arc},
    };
    use tokio::sync::mpsc;
    use uuid::Uuid;

    fn with_host(
        legacy: bool,
        run: impl FnOnce(
            &mut VisualTestContext,
            Entity<Workspace>,
            HostActionTarget,
            mpsc::Receiver<WorkerCommand>,
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
                    worker.shutdown().expect("owned fixture worker stops");
                    workspace
                });
                *content.borrow_mut() = Some(view.clone());
                gpui_kit::base::Root::new(view, window, cx)
            }
        });
        let view = content.borrow_mut().take().expect("host view exists");
        let owner = Uuid::new_v4();
        let mut model = WindowWorkspace::new(owner);
        let target = model
            .ui_test_host_update_fixture("update.test", legacy)
            .expect("offer fixture");
        let (commands, receiver) = mpsc::channel(8);
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.commands = commands;
                workspace.model = Some(model);
                workspace.hosts.set_owner(owner);
                assert!(workspace.hosts.accept(HubUpdate {
                    connection_id: owner,
                    generation: 1,
                    state: HostDirectoryState::Ready(Arc::new(
                        HostDirectorySnapshot::ui_test_fixture(owner, 1, &["update.test"])
                            .expect("directory fixture"),
                    )),
                }));
                workspace.destination = Destination::Hosts;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        window.simulate_resize(size(px(900.), px(560.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        run(window, view, target, receiver);
    }

    fn request_from_row(
        window: &mut VisualTestContext,
        view: &Entity<Workspace>,
        receiver: &mut mpsc::Receiver<WorkerCommand>,
    ) -> Uuid {
        let button = window
            .debug_bounds("host-update-update.test")
            .expect("actionable Update button is rendered");
        window.simulate_click(button.center(), Modifiers::default());
        let command = receiver.try_recv().expect("request sent to worker");
        let id = match command.kind() {
            Command::RequestHostUpdate { request_id, target } => {
                assert_eq!(target.target(), "update.test");
                *request_id
            }
            _ => panic!("unexpected host update command"),
        };
        window.update(|_, cx| {
            assert_eq!(
                view.read(cx)
                    .hosts
                    .update_prompt
                    .as_ref()
                    .map(|p| p.request_id),
                Some(id)
            );
        });
        id
    }

    fn deliver_plan(
        window: &mut VisualTestContext,
        view: &Entity<Workspace>,
        target: &HostActionTarget,
        request_id: Uuid,
    ) {
        let plan = window.update(|_, cx| {
            view.read(cx)
                .model
                .as_ref()
                .expect("model")
                .ui_test_host_update_plan(request_id, target)
                .expect("captured plan")
        });
        deliver_update(window, view, Update::HostUpdatePrepared(plan));
    }

    fn deliver_update(window: &mut VisualTestContext, view: &Entity<Workspace>, update: Update) {
        let (updates, mut receiver) = mpsc::channel(2);
        updates.try_send(update).expect("deliver update");
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.poll(None, &mut receiver, window, cx);
            });
            window.draw(cx).clear(cx);
        });
    }

    #[test]
    fn ordinary_offer_requires_worker_plan_before_confirming_once() {
        with_host(false, |window, view, target, mut receiver| {
            let id = request_from_row(window, &view, &mut receiver);
            assert!(window.debug_bounds("dialog-0").is_none());
            deliver_plan(window, &view, &target, id);
            assert!(window.debug_bounds("dialog-0").is_some());
            let confirm = window
                .debug_bounds("host-update-confirm")
                .expect("confirmation action");
            window.simulate_click(confirm.center(), Modifiers::default());
            let command = receiver.try_recv().expect("confirmed once");
            assert!(matches!(
                command.kind(),
                Command::ConfirmHostUpdate { plan }
                    if plan.request_id() == id
                        && plan.target() == &target
                        && plan.from_version() == "0.2.3"
                        && plan.to_version() == "0.2.4"
            ));
            assert!(receiver.try_recv().is_err());
            window.update(|_, cx| assert!(view.read(cx).hosts.update_prompt.is_none()));
        });
    }

    #[test]
    fn legacy_offer_shows_captured_plan_and_escape_only_dismisses() {
        with_host(true, |window, view, target, mut receiver| {
            let id = request_from_row(window, &view, &mut receiver);
            deliver_plan(window, &view, &target, id);
            window.update(|_, cx| {
                let plan = view
                    .read(cx)
                    .hosts
                    .update_prompt
                    .as_ref()
                    .unwrap()
                    .plan
                    .clone()
                    .unwrap();
                assert_eq!(plan.legacy_sessions(), Some(3));
                assert_eq!(plan.from_version(), "0.1.0");
                assert_eq!(plan.to_version(), "0.2.4");
            });
            let trigger = window.update(|_, cx| {
                view.read(cx)
                    .hosts
                    .update_prompt
                    .as_ref()
                    .unwrap()
                    .trigger
                    .clone()
            });
            window.simulate_keystrokes("escape");
            let command = receiver.try_recv().expect("dismiss sent");
            assert!(
                matches!(command.kind(), Command::DismissHostUpdate { request_id } if *request_id == id)
            );
            assert!(
                receiver.try_recv().is_err(),
                "Escape must not confirm update"
            );
            window.update(|_, cx| assert!(view.read(cx).hosts.update_prompt.is_none()));
            assert!(window.debug_bounds("dialog-0").is_none());
            assert!(
                window.update(|window, _| trigger.is_focused(window)),
                "Escape returns focus to Update"
            );
            window.update(|window, cx| window.draw(cx).clear(cx));
            let enter = Keystroke::parse("enter").expect("Enter keystroke");
            window.simulate_event(KeyDownEvent {
                keystroke: enter.clone(),
                is_held: false,
                prefer_character_input: false,
            });
            window.simulate_event(KeyUpEvent { keystroke: enter });
            let reopened = receiver
                .try_recv()
                .expect("Enter on focused Update requests again");
            assert!(matches!(
                reopened.kind(),
                Command::RequestHostUpdate { request_id, target: reopened_target }
                    if *request_id != id && reopened_target == &target
            ));
        });
    }

    #[test]
    fn stale_prepared_delivery_cannot_reopen_a_dismissed_confirmation() {
        with_host(false, |window, view, target, mut receiver| {
            let id = request_from_row(window, &view, &mut receiver);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.dismiss_host_update(id, window, cx)
                });
            });
            assert!(
                matches!(receiver.try_recv().unwrap().kind(), Command::DismissHostUpdate { request_id } if *request_id == id)
            );
            deliver_plan(window, &view, &target, id);
            assert!(window.debug_bounds("dialog-0").is_none());
            assert!(receiver.try_recv().is_err());
        });
    }

    #[test]
    fn changed_offer_revokes_confirmation_before_any_confirm_command() {
        with_host(false, |window, view, target, mut receiver| {
            let id = request_from_row(window, &view, &mut receiver);
            let plan = window.update(|_, cx| {
                view.read(cx)
                    .model
                    .as_ref()
                    .unwrap()
                    .ui_test_host_update_plan(id, &target)
                    .unwrap()
            });
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let mut changed = workspace.model.as_ref().unwrap().clone();
                    changed.ui_test_revoke_host_update_offer(&target).unwrap();
                    workspace.reconcile(changed, window, cx);
                });
            });
            assert!(
                matches!(receiver.try_recv().unwrap().kind(), Command::DismissHostUpdate { request_id } if *request_id == id)
            );
            deliver_update(window, &view, Update::HostUpdatePrepared(plan));
            assert!(window.debug_bounds("dialog-0").is_none());
            while let Ok(command) = receiver.try_recv() {
                assert!(
                    !matches!(command.kind(), Command::ConfirmHostUpdate { .. }),
                    "changed offer must not confirm"
                );
            }
        });
    }

    #[test]
    fn ordinary_offer_waits_for_backend_version_and_offers_refresh() {
        with_host(false, |window, view, _target, mut receiver| {
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace
                        .model
                        .as_mut()
                        .unwrap()
                        .ui_test_fail_backend_version();
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            let update = window
                .debug_bounds("host-update-update.test")
                .expect("Update remains visible");
            let refresh = window
                .debug_bounds("host-backend-refresh-update.test")
                .expect("failed version read offers Refresh");
            window.simulate_click(update.center(), Modifiers::default());
            assert!(
                receiver.try_recv().is_err(),
                "no request without assigned backend version"
            );
            window.simulate_click(refresh.center(), Modifiers::default());
            let command = receiver.try_recv().expect("refresh requested");
            assert!(matches!(command.kind(), Command::RefreshBackendVersion));
            assert!(receiver.try_recv().is_err());
        });
    }

    #[test]
    fn catalog_failure_keeps_update_offer_actionable() {
        with_host(false, |window, view, target, mut receiver| {
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace
                        .model
                        .as_mut()
                        .unwrap()
                        .ui_test_host_update_catalog_failed(&target)
                        .unwrap();
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            let id = request_from_row(window, &view, &mut receiver);
            deliver_plan(window, &view, &target, id);
            assert!(window.debug_bounds("dialog-0").is_some());
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.dismiss_host_update(id, window, cx)
                });
            });
            assert!(matches!(
                receiver.try_recv().unwrap().kind(),
                Command::DismissHostUpdate { request_id } if *request_id == id
            ));
        });
    }

    #[test]
    fn selecting_another_host_does_not_redirect_captured_confirmation() {
        with_host(false, |window, view, target, mut receiver| {
            let id = request_from_row(window, &view, &mut receiver);
            deliver_plan(window, &view, &target, id);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let mut model = workspace.model.as_ref().unwrap().clone();
                    let (other_screen, _) =
                        model.ui_test_remote_terminal_fixture("other.test").unwrap();
                    model
                        .select(model.lifecycle_token(), &other_screen)
                        .unwrap();
                    workspace.reconcile(model, window, cx);
                    assert_eq!(
                        workspace.model.as_ref().unwrap().shown_key(),
                        Some(other_screen.clone())
                    );
                    assert_eq!(
                        workspace
                            .hosts
                            .update_prompt
                            .as_ref()
                            .map(|prompt| &prompt.target),
                        Some(&target)
                    );
                });
            });
            while receiver.try_recv().is_ok() {}
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.confirm_host_update(id, window, cx)
                });
            });
            let command = receiver.try_recv().expect("captured host confirmation");
            assert!(matches!(
                command.kind(),
                Command::ConfirmHostUpdate { plan } if plan.target() == &target
            ));
            assert!(receiver.try_recv().is_err());
        });
    }
}
