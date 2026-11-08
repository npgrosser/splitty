//! Retained host forms. The local worker validates and owns every mutation.
use super::*;
use gpui_kit::component::input::{Input as KitInput, InputGroup};
use gpui_kit::component::{dialog::DialogFooter, WindowExt as _};
use splitty_client_core::hub::HostManagementRequest;
use std::cell::Cell;

#[derive(Clone)]
struct ConnectOption {
    target: String,
    title: String,
    detail: &'static str,
}

fn connect_options(
    snapshot: Option<&splitty_client_core::hub::HostDirectorySnapshot>,
    query: &str,
) -> Vec<ConnectOption> {
    let needle = query.trim().to_lowercase();
    let mut rows: Vec<_> = snapshot
        .into_iter()
        .flat_map(|snapshot| &snapshot.directory().hosts)
        .filter(|host| {
            needle.is_empty()
                || host.target.to_lowercase().contains(&needle)
                || host
                    .alias
                    .as_ref()
                    .is_some_and(|alias| alias.to_lowercase().contains(&needle))
        })
        .take(64)
        .map(|host| ConnectOption {
            target: host.target.clone(),
            title: display(host.alias.as_deref().unwrap_or(&host.target)),
            detail: "Known host",
        })
        .collect();
    let typed = query;
    if HostManagementRequest::connect(typed, None).is_ok()
        && !rows.iter().any(|row| row.target == typed)
    {
        rows.push(ConnectOption {
            target: typed.to_owned(),
            title: display(typed),
            detail: "Entered SSH target",
        });
    }
    rows
}

impl Workspace {
    pub(super) fn open_host_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.destination != Destination::Hosts || self.hosts.connect.is_some() {
            return;
        }
        let Some(owner) = self.local_model().map(SourceWorkspace::source_token) else {
            self.hosts.command_error = Some("Local host service is unavailable".into());
            cx.notify();
            return;
        };
        let target = cx.new(|cx| InputState::new(window, cx).placeholder("user@host[:port]"));
        let alias = cx.new(|cx| InputState::new(window, cx).placeholder("Optional"));
        self.hosts.connect = Some(HostConnectForm {
            owner,
            target: target.clone(),
            alias,
            selected: 0,
            pending: None,
            error: None,
        });
        self.host_connect_trigger.focus(window, cx);
        let weak = cx.entity().downgrade();
        self.hosts.connect_subscription = Some(cx.subscribe_in(
            &target,
            window,
            move |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Some(form) = this.hosts.connect.as_mut() {
                        form.selected = 0;
                        form.error = None;
                        cx.notify();
                        window.refresh();
                    }
                }
            },
        ));
        let first = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            let Some(view) = weak.upgrade() else {
                return dialog.title("Connect to host");
            };
            let Some((target, alias, selected, pending, error, snapshot)) = ({
                let workspace = view.read(cx);
                workspace.hosts.connect.as_ref().map(|form| {
                    (
                        form.target.clone(),
                        form.alias.clone(),
                        form.selected,
                        form.pending.is_some(),
                        form.error.clone(),
                        workspace.hosts.snapshot().cloned(),
                    )
                })
            }) else {
                return dialog.title("Connect to host");
            };
            if first.replace(false) {
                target.read(cx).focus_handle(cx).focus(window, cx);
            }
            let query = target.read(cx).value().to_string();
            let options = connect_options(snapshot.as_deref(), &query);
            let selected = selected.min(options.len().saturating_sub(1));
            let key_weak = weak.clone();
            let mut suggestions = div()
                .id("host-connect-suggestions")
                .flex()
                .flex_col()
                .max_h(chrome_rems(128.))
                .overflow_y_scroll();
            if options.is_empty() {
                suggestions =
                    suggestions.child(div().py_2().text_color(cx.theme().muted_foreground).child(
                        if query.trim().is_empty() {
                            "No known hosts yet. Enter an SSH target."
                        } else {
                            "Enter a valid [user@]host[:port]."
                        },
                    ));
            }
            for (index, option) in options.into_iter().enumerate() {
                let item_target = option.target.clone();
                let item_weak = weak.clone();
                suggestions = suggestions.child(
                    Button::new(SharedString::from(format!(
                        "host-connect-option-{}-{}",
                        item_target.len(),
                        item_target
                    )))
                    .accessibility_label(format!("Connect to {}", option.target))
                    .ghost()
                    .small()
                    .w_full()
                    .disabled(pending)
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(option.title),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(option.detail),
                            ),
                    )
                    .selected(index == selected)
                    .on_click(move |_, window, cx| {
                        let _ = item_weak.update(cx, |this, cx| {
                            this.submit_host_connect(Some(&item_target), window, cx)
                        });
                    }),
                );
            }
            let footer_weak = weak.clone();
            let cancel_weak = weak.clone();
            dialog
                .title("Connect to host")
                .overlay_closable(false)
                .keyboard(true)
                .close_button(false)
                .on_cancel({
                    let weak = weak.clone();
                    move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| this.close_host_connect(window, cx));
                        false
                    }
                })
                .on_ok({
                    let weak = weak.clone();
                    move |_, window, cx| {
                        let _ =
                            weak.update(cx, |this, cx| this.submit_host_connect(None, window, cx));
                        false
                    }
                })
                .child(
                    div()
                        .id("host-connect-form")
                        .flex()
                        .flex_col()
                        .flex_none()
                        .min_h(chrome_rems(206.))
                        .gap_3()
                        .capture_key_down(move |event: &KeyDownEvent, window, cx| {
                            let _ = key_weak.update(cx, |this, cx| {
                                this.host_connect_key(&event.keystroke, window, cx)
                            });
                        })
                        .child(
                            div().flex().flex_col().gap_1().child("SSH target").child(
                                div()
                                    .h(chrome_rems(34.))
                                    .when(cfg!(all(test, feature = "ui-test-support")), |view| {
                                        view.debug_selector(|| "host-connect-target-input".into())
                                    })
                                    .child(
                                        InputGroup::new("host-connect-target-group")
                                            .small()
                                            .aria_label("SSH target")
                                            .disabled(pending)
                                            .input(KitInput::new(&target)),
                                    ),
                            ),
                        )
                        .child(suggestions)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child("Display name (optional)")
                                .child(
                                    div()
                                        .h(chrome_rems(34.))
                                        .when(
                                            cfg!(all(test, feature = "ui-test-support")),
                                            |view| {
                                                view.debug_selector(|| {
                                                    "host-connect-alias-input".into()
                                                })
                                            },
                                        )
                                        .child(
                                            InputGroup::new("host-connect-alias-group")
                                                .small()
                                                .aria_label("Display name (optional)")
                                                .disabled(pending)
                                                .input(KitInput::new(&alias)),
                                        ),
                                ),
                        )
                        .when_some(error, |view, error| {
                            view.child(div().text_color(cx.theme().danger).child(display(&error)))
                        }),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("host-connect-cancel")
                                .label("Cancel")
                                .outline()
                                .on_click(move |_, window, cx| {
                                    let _ = cancel_weak
                                        .update(cx, |this, cx| this.close_host_connect(window, cx));
                                }),
                        )
                        .child(
                            Button::new("host-connect-submit")
                                .label(if pending { "Connecting…" } else { "Connect" })
                                .disabled(pending)
                                .on_click(move |_, window, cx| {
                                    let _ = footer_weak.update(cx, |this, cx| {
                                        this.submit_host_connect(None, window, cx)
                                    });
                                }),
                        ),
                )
        });
        cx.notify();
    }

    fn host_connect_key(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.hosts.connect.as_ref() else {
            return;
        };
        if key.modifiers.control
            || key.modifiers.alt
            || key.modifiers.platform
            || key.modifiers.function
        {
            return;
        }
        match key.key.to_ascii_lowercase().as_str() {
            "up" | "down" if form.target.read(cx).focus_handle(cx).is_focused(window) => {
                let count = connect_options(
                    self.hosts.snapshot().map(Arc::as_ref),
                    &form.target.read(cx).value(),
                )
                .len();
                if count > 0 {
                    let selected = &mut self.hosts.connect.as_mut().expect("form").selected;
                    *selected = if key.key.eq_ignore_ascii_case("up") {
                        (*selected + count - 1) % count
                    } else {
                        (*selected + 1) % count
                    };
                    cx.stop_propagation();
                    cx.notify();
                    window.refresh();
                }
            }
            "enter" | "return" => {
                cx.stop_propagation();
                let choice = form
                    .target
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
                    .then(|| {
                        connect_options(
                            self.hosts.snapshot().map(Arc::as_ref),
                            &form.target.read(cx).value(),
                        )
                    })
                    .and_then(|options| {
                        options
                            .get(form.selected)
                            .map(|option| option.target.clone())
                    });
                self.submit_host_connect(choice.as_deref(), window, cx);
            }
            _ => {}
        }
    }

    fn submit_host_connect(
        &mut self,
        chosen: Option<&str>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = self.hosts.connect.as_ref() else {
            return;
        };
        if form.pending.is_some() || self.lifecycle_token() != Some(form.owner) {
            return;
        }
        let target = chosen
            .map(str::to_owned)
            .unwrap_or_else(|| form.target.read(cx).value().to_string());
        let alias = form.alias.read(cx).value().to_string();
        let request = match HostManagementRequest::connect(
            &target,
            (!alias.is_empty()).then_some(alias.as_str()),
        ) {
            Ok(request) => request,
            Err(error) => {
                self.hosts.connect.as_mut().expect("form").error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let canonical_target = request.target().to_owned();
        let request_id = Uuid::new_v4();
        match self.commands.try_send(WorkerCommand::new(
            form.owner,
            Command::ManageHost {
                request_id,
                request,
            },
        )) {
            Ok(()) => {
                let form = self.hosts.connect.as_mut().expect("form");
                form.pending = Some((request_id, canonical_target));
                form.error = None;
                form.target
                    .update(cx, |input, cx| input.set_disabled(true, cx));
                form.alias
                    .update(cx, |input, cx| input.set_disabled(true, cx));
            }
            Err(error) => {
                self.hosts.connect.as_mut().expect("form").error =
                    Some(format!("Connect was not sent: {error}"))
            }
        }
        cx.notify();
    }

    pub(super) fn close_host_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.hosts.connect.as_ref() else {
            return;
        };
        if let Some((request_id, _)) = form.pending {
            if self.lifecycle_token() == Some(form.owner) {
                if let Err(error) =
                    admit_withdraw_host_connect(&self.commands, form.owner, request_id)
                {
                    self.hosts.connect.as_mut().expect("form").error =
                        Some(format!("Cancel was not sent: {error}. Try again."));
                    cx.notify();
                    window.refresh();
                    return;
                }
            }
        }
        if self.hosts.connect.take().is_some() {
            self.hosts.connect_subscription = None;
            window.close_dialog(cx);
            cx.notify();
        }
    }

    pub(super) fn edit_host_alias(
        &mut self,
        target: &str,
        alias: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self.local_model().map(SourceWorkspace::source_token) else {
            return;
        };
        if self
            .hosts
            .mutations
            .get(target)
            .is_some_and(|mutation| mutation.outcome == HostManagementOutcome::Pending)
        {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx).default_value(alias.unwrap_or_default().to_owned())
        });
        let target = target.to_owned();
        let listener_target = target.clone();
        self.hosts.alias_subscription =
            Some(
                cx.subscribe_in(&input, window, move |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.submit_host_alias(&listener_target, cx);
                    }
                }),
            );
        self.hosts.alias_editor = Some(HostAliasEditor {
            owner,
            target,
            input: input.clone(),
            error: None,
        });
        let weak = cx.entity().downgrade();
        let first = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            let Some(view) = weak.upgrade() else {
                return dialog.title("Change display name");
            };
            let Some((name, target, error, pending)) = ({
                let workspace = view.read(cx);
                workspace.hosts.alias_editor.as_ref().map(|editor| {
                    let target = editor.target.clone();
                    let pending = workspace
                        .hosts
                        .mutations
                        .get(&target)
                        .is_some_and(|mutation| mutation.outcome == HostManagementOutcome::Pending);
                    (editor.input.clone(), target, editor.error.clone(), pending)
                })
            }) else {
                return dialog.title("Change display name");
            };
            if first.replace(false) {
                name.read(cx).focus_handle(cx).focus(window, cx);
            }
            let save_weak = weak.clone();
            let cancel_weak = weak.clone();
            dialog
                .title("Change display name")
                .overlay_closable(false)
                .keyboard(true)
                .close_button(false)
                .on_cancel({
                    let weak = weak.clone();
                    move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| this.cancel_host_alias(window, cx));
                        false
                    }
                })
                .on_ok({
                    let weak = weak.clone();
                    let target = target.clone();
                    move |_, _, cx| {
                        let _ = weak.update(cx, |this, cx| this.submit_host_alias(&target, cx));
                        false
                    }
                })
                .child(
                    div()
                        .id("host-alias-form")
                        .flex()
                        .flex_col()
                        .flex_none()
                        .min_h(chrome_rems(136.))
                        .gap_2()
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("SSH target: {target}")),
                        )
                        .child(
                            div().flex().flex_col().gap_1().child("Display name").child(
                                div()
                                    .h(chrome_rems(34.))
                                    .when(cfg!(all(test, feature = "ui-test-support")), |view| {
                                        view.debug_selector(|| "host-alias-input".into())
                                    })
                                    .child(
                                        InputGroup::new("host-alias-group")
                                            .small()
                                            .aria_label("Display name")
                                            .disabled(pending)
                                            .input(KitInput::new(&name)),
                                    ),
                            ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Leave empty to show the SSH target."),
                        )
                        .when_some(error, |view, error| {
                            view.child(div().text_color(cx.theme().danger).child(display(&error)))
                        }),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("host-alias-cancel")
                                .label("Cancel")
                                .outline()
                                .on_click(move |_, window, cx| {
                                    let _ = cancel_weak
                                        .update(cx, |this, cx| this.cancel_host_alias(window, cx));
                                }),
                        )
                        .child(
                            Button::new("host-alias-save")
                                .label(if pending { "Saving…" } else { "Save" })
                                .disabled(pending)
                                .on_click(move |_, _, cx| {
                                    let _ = save_weak
                                        .update(cx, |this, cx| this.submit_host_alias(&target, cx));
                                }),
                        ),
                )
        });
        cx.notify();
    }

    fn cancel_host_alias(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hosts.alias_editor.take().is_some() {
            self.hosts.alias_subscription = None;
            window.close_dialog(cx);
            cx.notify();
        }
    }

    fn submit_host_alias(&mut self, target: &str, cx: &mut Context<Self>) {
        let Some((owner, input)) = self
            .hosts
            .alias_editor
            .as_ref()
            .filter(|editor| editor.target == target)
            .map(|editor| (editor.owner, editor.input.clone()))
        else {
            return;
        };
        if self.lifecycle_token() != Some(owner)
            || self
                .hosts
                .mutations
                .get(target)
                .is_some_and(|mutation| mutation.outcome == HostManagementOutcome::Pending)
        {
            return;
        }
        if !self.hosts.can_record_mutation(target) {
            self.hosts.alias_editor.as_mut().expect("editor").error =
                Some("Too many host changes are pending. Try again shortly.".into());
            cx.notify();
            return;
        }
        let value = input.read(cx).value().to_string();
        let request = match HostManagementRequest::set_alias(target, &value) {
            Ok(request) => request,
            Err(error) => {
                self.hosts.alias_editor.as_mut().expect("editor").error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let request_id = Uuid::new_v4();
        match self.commands.try_send(WorkerCommand::new(
            owner,
            Command::ManageHost {
                request_id,
                request,
            },
        )) {
            Ok(()) => {
                self.hosts.record_mutation(
                    target.to_owned(),
                    HostMutation {
                        owner,
                        request_id,
                        action: HostManagementAction::SetAlias,
                        outcome: HostManagementOutcome::Pending,
                        reason: None,
                    },
                );
                self.hosts.alias_editor.as_mut().expect("editor").error = None;
                input.update(cx, |input, cx| input.set_disabled(true, cx));
            }
            Err(error) => {
                self.hosts.alias_editor.as_mut().expect("editor").error =
                    Some(format!("Display name was not sent: {error}"))
            }
        }
        cx.notify();
    }

    pub(super) fn forget_host(&mut self, target: &str, cx: &mut Context<Self>) {
        let Some(owner) = self.local_model().map(SourceWorkspace::source_token) else {
            return;
        };
        if self.hosts.snapshot().is_none_or(|snapshot| {
            !snapshot
                .directory()
                .hosts
                .iter()
                .any(|host| host.target == target)
        }) {
            return;
        }
        if self
            .hosts
            .mutations
            .get(target)
            .is_some_and(|mutation| mutation.outcome == HostManagementOutcome::Pending)
        {
            return;
        }
        if !self.hosts.can_record_mutation(target) {
            self.hosts.command_error =
                Some("Too many host changes are pending. Try again shortly.".into());
            cx.notify();
            return;
        }
        let Ok(request) = HostManagementRequest::forget(target) else {
            return;
        };
        let request_id = Uuid::new_v4();
        match self.commands.try_send(WorkerCommand::new(
            owner,
            Command::ManageHost {
                request_id,
                request,
            },
        )) {
            Ok(()) => {
                self.hosts.record_mutation(
                    target.to_owned(),
                    HostMutation {
                        owner,
                        request_id,
                        action: HostManagementAction::Forget,
                        outcome: HostManagementOutcome::Pending,
                        reason: None,
                    },
                );
            }
            Err(error) => {
                self.hosts.command_error = Some(format!("Remove was not sent: {error}"));
            }
        }
        cx.notify();
    }

    pub(crate) fn host_management_result(
        &mut self,
        result: HostManagementResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.lifecycle_token() != Some(result.owner())
            || self.hosts.owner != Some(result.owner().connection_id())
        {
            return;
        }
        if result.action() == HostManagementAction::Connect {
            let matched = self.hosts.connect.as_ref().is_some_and(|form| {
                form.owner == result.owner()
                    && form.pending.as_ref().is_some_and(|(id, target)| {
                        *id == result.request_id() && target == result.target()
                    })
            });
            if !matched {
                return;
            }
            match result.outcome() {
                HostManagementOutcome::Pending => {}
                HostManagementOutcome::Applied => {
                    self.hosts.connect.as_mut().expect("form").pending = None;
                    self.close_host_connect(window, cx);
                }
                HostManagementOutcome::Rejected | HostManagementOutcome::Uncertain => {
                    let form = self.hosts.connect.as_mut().expect("form");
                    form.pending = None;
                    form.error = Some(format!(
                        "{}: {}",
                        if result.outcome() == HostManagementOutcome::Uncertain {
                            "Connect status unknown"
                        } else {
                            "Could not connect"
                        },
                        result
                            .reason()
                            .unwrap_or("The local service did not confirm this request")
                    ));
                    form.target
                        .update(cx, |input, cx| input.set_disabled(false, cx));
                    form.alias
                        .update(cx, |input, cx| input.set_disabled(false, cx));
                }
            }
            cx.notify();
            return;
        }
        let Some(mutation) = self.hosts.mutations.get_mut(result.target()) else {
            return;
        };
        if mutation.owner != result.owner()
            || mutation.request_id != result.request_id()
            || mutation.action != result.action()
        {
            return;
        }
        mutation.outcome = result.outcome();
        mutation.reason = result.reason().map(str::to_owned);
        if mutation.outcome == HostManagementOutcome::Applied
            && mutation.action == HostManagementAction::SetAlias
            && self
                .hosts
                .alias_editor
                .as_ref()
                .is_some_and(|editor| editor.target == result.target())
        {
            self.hosts.alias_editor = None;
            self.hosts.alias_subscription = None;
            window.close_dialog(cx);
        }
        if mutation.outcome == HostManagementOutcome::Rejected
            || mutation.outcome == HostManagementOutcome::Uncertain
        {
            if let Some(editor) = self
                .hosts
                .alias_editor
                .as_mut()
                .filter(|editor| editor.target == result.target())
            {
                editor.error = Some(format!(
                    "{}: {}",
                    if mutation.outcome == HostManagementOutcome::Uncertain {
                        "Save status unknown"
                    } else {
                        "Could not save display name"
                    },
                    mutation
                        .reason
                        .as_deref()
                        .unwrap_or("The local service did not confirm this request")
                ));
                editor
                    .input
                    .update(cx, |input, cx| input.set_disabled(false, cx));
            }
        }
        cx.notify();
    }

    pub(crate) fn host_alias_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self.hosts.alias_editor.as_ref() else {
            return false;
        };
        if !editor.input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        if key.key.eq_ignore_ascii_case("escape") {
            self.cancel_host_alias(window, cx);
            cx.stop_propagation();
            cx.notify();
        }
        true
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::{
        Destination, HostManagementAction, HostManagementOutcome, HostManagementResult, Workspace,
    };
    use gpui_kit::{
        px, size, AppContext, Focusable, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext,
    };
    use splitty_client_core::{
        hub::{HostDirectorySnapshot, HostDirectoryState, HubUpdate},
        native_workspace::{CommandKind, Worker},
        workspace::{SourceWorkspace, WindowWorkspace},
    };
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{atomic::AtomicBool, Arc},
    };
    use tokio::sync::mpsc;
    use uuid::Uuid;

    fn assert_connect_inputs_above_footer(window: &mut VisualTestContext) {
        let target = window.debug_bounds("host-connect-target-input").unwrap();
        let alias = window.debug_bounds("host-connect-alias-input").unwrap();
        let surface = window.debug_bounds("dialog-0").unwrap();
        assert!(target.size.height >= px(30.));
        assert!(alias.size.height >= px(30.));
        assert!(alias.origin.y >= target.origin.y + target.size.height);
        assert!(
            alias.origin.y + alias.size.height + px(48.) <= surface.origin.y + surface.size.height,
            "alias input must fit above the dialog footer: alias={alias:?}, surface={surface:?}"
        );
    }

    #[test]
    fn retained_connect_form_requires_admitted_withdraw_and_exact_result_identity() {
        let mut app = TestAppContext::single();
        app.update(gpui_kit::init);
        let worker = Rc::new(RefCell::new(None));
        let content = Rc::new(RefCell::new(None));
        let (_, window) = app.add_window_view({
            let worker = worker.clone();
            let content = content.clone();
            move |window, cx| {
                let view = cx.new(|cx| {
                    let (workspace, running) = Workspace::new_with_worker(
                        false,
                        Arc::new(AtomicBool::new(false)),
                        window,
                        cx,
                        |_, _, _| Worker::ui_test_fixture(),
                    );
                    *worker.borrow_mut() = Some(running);
                    workspace
                });
                *content.borrow_mut() = Some(view.clone());
                gpui_kit::base::Root::new(view, window, cx)
            }
        });
        let view = content.borrow_mut().take().unwrap();
        worker.borrow_mut().take().unwrap().shutdown().unwrap();
        let owner = Uuid::new_v4();
        let (commands, mut receiver) = mpsc::channel(1);
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.commands = commands;
                workspace.model = Some(WindowWorkspace::new(owner));
                workspace.hosts.set_owner(owner);
                assert!(workspace.hosts.accept(HubUpdate {
                    connection_id: owner,
                    generation: 1,
                    state: HostDirectoryState::Ready(Arc::new(
                        HostDirectorySnapshot::ui_test_fixture(owner, 1, &["beta.example"])
                            .unwrap(),
                    )),
                }));
                workspace.destination = Destination::Hosts;
                workspace.open_host_connect(window, cx);
            });
            window.draw(cx).clear(cx);
        });
        assert_connect_inputs_above_footer(window);
        window.simulate_resize(size(px(640.), px(420.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert_connect_inputs_above_footer(window);
        window.simulate_input("beta.example");
        let (token, request_id) = window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                let form = workspace.hosts.connect.as_ref().unwrap();
                assert!(form.target.read(cx).focus_handle(cx).is_focused(window));
                assert_eq!(form.target.read(cx).value(), "beta.example");
                form.alias
                    .update(cx, |input, cx| input.set_value("Beta", window, cx));
                workspace.submit_host_connect(None, window, cx);
                let form = workspace.hosts.connect.as_ref().unwrap();
                (form.owner, form.pending.as_ref().unwrap().0)
            })
        });
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| workspace.close_host_connect(window, cx));
            let form = view.read(cx).hosts.connect.as_ref().unwrap();
            assert_eq!(form.pending.as_ref().unwrap().0, request_id);
            assert!(form
                .error
                .as_deref()
                .unwrap()
                .contains("Cancel was not sent"));
        });
        let queued = receiver.try_recv().unwrap();
        assert_eq!(queued.source_token(), Some(token));
        assert!(
            matches!(queued.kind(), CommandKind::ManageHost { request_id: id, request } if *id == request_id && request.target() == "beta.example" && request.action() == HostManagementAction::Connect)
        );

        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.host_management_result(
                    HostManagementResult::ui_test_fixture(
                        token,
                        Uuid::new_v4(),
                        "beta.example".into(),
                        HostManagementAction::Connect,
                        HostManagementOutcome::Rejected,
                        Some("stale request".into()),
                    ),
                    window,
                    cx,
                );
                assert_eq!(
                    workspace
                        .hosts
                        .connect
                        .as_ref()
                        .unwrap()
                        .pending
                        .as_ref()
                        .unwrap()
                        .0,
                    request_id
                );
                workspace.host_management_result(
                    HostManagementResult::ui_test_fixture(
                        token,
                        request_id,
                        "beta.example".into(),
                        HostManagementAction::Connect,
                        HostManagementOutcome::Rejected,
                        Some("service refused".into()),
                    ),
                    window,
                    cx,
                );
                let form = workspace.hosts.connect.as_ref().unwrap();
                assert!(form.pending.is_none());
                assert!(form.error.as_deref().unwrap().contains("service refused"));
                assert_eq!(form.target.read(cx).value(), "beta.example");
                workspace.close_host_connect(window, cx);
                assert!(workspace.hosts.connect.is_none());
            });
        });
        assert!(receiver.try_recv().is_err());
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.update(|window, cx| {
            assert!(view.read(cx).host_connect_trigger.is_focused(window));
        });
        window.simulate_keystrokes("enter");
        window.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("enter").unwrap(),
        });
        window.update(|_, cx| assert!(view.read(cx).hosts.connect.is_some()));
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| workspace.show_screens(window, cx));
            assert!(view.read(cx).destination == Destination::Screens);
        });
        let queued = receiver.try_recv().unwrap();
        assert_eq!(queued.source_token(), Some(token));
        assert!(matches!(queued.kind(), CommandKind::ReleaseHostBrowsing));
    }

    #[test]
    fn uncertain_remove_remains_visible_after_authoritative_directory_drops_row() {
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
        let (commands, mut receiver) = mpsc::channel(2);
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.commands = commands;
                workspace.model = Some(WindowWorkspace::new(owner));
                workspace.hosts.set_owner(owner);
                workspace.destination = Destination::Hosts;
                assert!(workspace.hosts.accept(HubUpdate {
                    connection_id: owner,
                    generation: 1,
                    state: HostDirectoryState::Ready(Arc::new(
                        HostDirectorySnapshot::ui_test_fixture(owner, 1, &["beta.example"])
                            .unwrap(),
                    )),
                }));
                workspace.edit_host_alias("beta.example", Some("Beta"), window, cx);
            });
            window.draw(cx).clear(cx);
        });
        window.simulate_resize(size(px(640.), px(420.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        let alias = window.debug_bounds("host-alias-input").unwrap();
        let surface = window.debug_bounds("dialog-0").unwrap();
        assert!(alias.size.height >= px(30.));
        assert!(
            alias.origin.y + alias.size.height + px(48.) <= surface.origin.y + surface.size.height,
            "alias editor must fit above its footer: alias={alias:?}, surface={surface:?}"
        );
        let (token, request_id) = window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.cancel_host_alias(window, cx);
                workspace.forget_host("beta.example", cx);
                let mutation = workspace.hosts.mutations.get("beta.example").unwrap();
                (mutation.owner, mutation.request_id)
            })
        });
        let queued = receiver.try_recv().unwrap();
        assert_eq!(queued.source_token(), Some(token));
        assert!(
            matches!(queued.kind(), CommandKind::ManageHost { request_id: id, request } if *id == request_id && request.action() == HostManagementAction::Forget && request.target() == "beta.example")
        );
        let result = |owner, id, outcome| {
            HostManagementResult::ui_test_fixture(
                owner,
                id,
                "beta.example".into(),
                HostManagementAction::Forget,
                outcome,
                Some("server response was lost".into()),
            )
        };
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.host_management_result(
                    result(
                        SourceWorkspace::new(Uuid::new_v4()).source_token(),
                        request_id,
                        HostManagementOutcome::Applied,
                    ),
                    window,
                    cx,
                );
                workspace.host_management_result(
                    result(token, Uuid::new_v4(), HostManagementOutcome::Applied),
                    window,
                    cx,
                );
                assert_eq!(
                    workspace.hosts.mutations["beta.example"].outcome,
                    HostManagementOutcome::Pending
                );
                workspace.host_management_result(
                    result(token, request_id, HostManagementOutcome::Uncertain),
                    window,
                    cx,
                );
                assert_eq!(
                    workspace.hosts.mutations["beta.example"].outcome,
                    HostManagementOutcome::Uncertain
                );
                assert!(workspace.hosts.accept(HubUpdate {
                    connection_id: owner,
                    generation: 2,
                    state: HostDirectoryState::Ready(Arc::new(
                        HostDirectorySnapshot::ui_test_fixture(owner, 2, &[]).unwrap(),
                    )),
                }));
                assert!(workspace
                    .hosts
                    .snapshot()
                    .unwrap()
                    .directory()
                    .hosts
                    .is_empty());
            });
            window.draw(cx).clear(cx);
        });
        assert!(window.debug_bounds("hosts-detached-feedback").is_some());
        assert!(receiver.try_recv().is_err());
    }
}
