//! Retained Ports presentation. The local worker owns all forwarding effects.
use super::hosts::HostGroupTarget;
use super::*;
use gpui_kit::component::{
    checkbox::Checkbox,
    dialog::DialogFooter,
    input::{Input as KitInput, InputGroup},
};
use splitty_client_core::{
    external_links::WebUrl,
    ports::{
        ForwardAction, ForwardOutcome, ForwardRequest, ForwardResult, ForwardRow, ForwardSnapshot,
        ForwardState, MappingKey,
    },
};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct PortsView {
    owner: Option<SourceToken>,
    pub(super) snapshot: Option<Arc<ForwardSnapshot>>,
    form: Option<PortForm>,
    pub(super) feedback: HashMap<MappingKey, PortFeedback>,
    open_requests: HashMap<Uuid, OpenOrigin>,
    pub(super) error: Option<String>,
    pub(super) link_notice: Option<String>,
}

struct PortForm {
    owner: SourceToken,
    edit_key: Option<MappingKey>,
    edit_state: Option<ForwardState>,
    target: Entity<InputState>,
    remote: Entity<InputState>,
    local: Entity<InputState>,
    label: Entity<InputState>,
    restore: bool,
    edit_saved: bool,
    pending: Option<Uuid>,
    error: Option<String>,
}

pub(super) struct PortFeedback {
    owner: SourceToken,
    request_id: Uuid,
    action: ForwardAction,
    outcome: ForwardOutcome,
    reason: Option<String>,
    browser_opened: Option<bool>,
    settle_after: u64,
}
impl PortFeedback {
    pub(super) fn outcome(&self) -> ForwardOutcome {
        self.outcome
    }
}

#[derive(Clone)]
pub(super) enum OpenOrigin {
    Hosts,
    Terminal(TerminalToken),
    Drawer(HostGroupTarget),
}

impl OpenOrigin {
    fn notice_label(&self) -> Option<&'static str> {
        match self {
            Self::Hosts => None,
            Self::Terminal(_) => Some("Development link"),
            Self::Drawer(_) => Some("Port opening"),
        }
    }
    fn closed_notice(&self) -> Option<&'static str> {
        match self {
            Self::Hosts => None,
            Self::Terminal(_) => Some("Development link closed before it opened"),
            Self::Drawer(_) => Some("Port opening canceled because its host group closed"),
        }
    }
}

impl PortsView {
    const MAX_FEEDBACK: usize = 64;
    pub(super) fn has_open_form(&self) -> bool {
        self.form.is_some()
    }

    pub(super) fn can_manage(&self) -> bool {
        self.owner.is_some()
            && self
                .snapshot
                .as_ref()
                .is_none_or(|snapshot| snapshot.is_available())
    }
    fn set_owner(&mut self, owner: SourceToken) {
        if self.owner == Some(owner) {
            return;
        }
        self.revoke_open();
        self.owner = Some(owner);
        self.snapshot = None;
        self.form = None;
        self.feedback.clear();
        self.error = None;
        self.link_notice = None;
    }
    pub(super) fn accept(&mut self, snapshot: Arc<ForwardSnapshot>) -> bool {
        if self.owner != Some(snapshot.owner())
            || self
                .snapshot
                .as_ref()
                .is_some_and(|old| old.generation() > snapshot.generation())
        {
            return false;
        }
        let generation = snapshot.generation();
        self.snapshot = Some(snapshot);
        self.feedback.retain(|_, feedback| {
            feedback.outcome != ForwardOutcome::Succeeded || generation <= feedback.settle_after
        });
        true
    }
    pub(super) fn rows(&self, model: Option<&WindowWorkspace>) -> Vec<ForwardRow> {
        self.snapshot
            .as_ref()
            .zip(model)
            .map_or_else(Vec::new, |(snapshot, model)| snapshot.rows(model))
    }
    fn revoke_open(&mut self) -> Vec<Uuid> {
        let pending = self
            .open_requests
            .drain()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        pending
    }
    fn revoke_hosts_open(&mut self) -> Vec<Uuid> {
        let ids: Vec<_> = self
            .open_requests
            .iter()
            .filter_map(|(id, origin)| matches!(origin, OpenOrigin::Hosts).then_some(*id))
            .collect();
        for id in &ids {
            self.open_requests.remove(id);
        }
        ids
    }
    fn record(&mut self, key: MappingKey, feedback: PortFeedback) {
        if !self.feedback.contains_key(&key) && self.feedback.len() >= Self::MAX_FEEDBACK {
            if let Some(old) = self
                .feedback
                .iter()
                .find(|(_, value)| value.outcome != ForwardOutcome::Pending)
                .map(|(key, _)| key.clone())
            {
                self.feedback.remove(&old);
            } else {
                return;
            }
        }
        self.feedback.insert(key, feedback);
    }
    fn can_record(&self, key: &MappingKey) -> bool {
        self.feedback.contains_key(key)
            || self.feedback.len() < Self::MAX_FEEDBACK
            || self
                .feedback
                .values()
                .any(|feedback| feedback.outcome != ForwardOutcome::Pending)
    }
    pub(super) fn pending(&self, key: &MappingKey) -> bool {
        self.feedback
            .get(key)
            .is_some_and(|value| value.outcome == ForwardOutcome::Pending)
    }

    fn cancel_open_for_key(&mut self, key: &MappingKey) -> Option<Uuid> {
        let feedback = self.feedback.get(key)?;
        if feedback.action != ForwardAction::Open || feedback.outcome != ForwardOutcome::Pending {
            return None;
        }
        let id = feedback.request_id;
        if self
            .open_requests
            .remove(&id)
            .is_some_and(|origin| origin.notice_label().is_some())
        {
            self.link_notice =
                Some("Browser opening canceled by Stop; the port action is pending".into());
        }
        Some(id)
    }
}

fn decimal_port(raw: &str, optional: bool, label: &str) -> Result<Option<u16>, String> {
    if optional && raw.is_empty() {
        return Ok(None);
    }
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{label} must be a decimal port from 1 to 65535"));
    }
    let port = raw
        .parse::<u16>()
        .map_err(|_| format!("{label} must be from 1 to 65535"))?;
    if port == 0 {
        return Err(format!("{label} must be from 1 to 65535"));
    }
    Ok(Some(port))
}

pub(super) fn feedback_text(feedback: &PortFeedback) -> String {
    let action = match feedback.action {
        ForwardAction::Start => "Forward",
        ForwardAction::Open => "Open",
        ForwardAction::Stop => "Stop",
        ForwardAction::Save => "Save",
        ForwardAction::Forget => "Forget",
    };
    match feedback.outcome {
        ForwardOutcome::Pending => format!("{action} pending…"),
        ForwardOutcome::Succeeded => match feedback.browser_opened {
            Some(true) => "Browser opened".into(),
            Some(false) => "Forward confirmed · browser did not open".into(),
            None => format!("{action} confirmed"),
        },
        ForwardOutcome::Partial => format!(
            "{action} partly completed{}: {}",
            if feedback.browser_opened == Some(true) {
                " · browser opened"
            } else {
                ""
            },
            feedback
                .reason
                .as_deref()
                .unwrap_or("Check current mapping")
        ),
        ForwardOutcome::Failed | ForwardOutcome::Rejected => format!(
            "{action} failed: {}",
            feedback.reason.as_deref().unwrap_or("Try again")
        ),
        ForwardOutcome::Uncertain => format!(
            "{action} status unknown: {}",
            feedback
                .reason
                .as_deref()
                .unwrap_or("Refresh to inspect current state")
        ),
    }
}

fn state_text(state: ForwardState) -> &'static str {
    match state {
        ForwardState::Detected => "Detected",
        ForwardState::Saved => "Saved",
        ForwardState::Paused => "Paused",
        ForwardState::Failed => "Failed",
        ForwardState::Up => "Forwarding",
        ForwardState::Rebuilding => "Rebuilding",
    }
}

impl Workspace {
    pub(super) fn render_port_header(
        target: &str,
        owner: Option<SourceToken>,
        row_height: Rems,
        weak: WeakEntity<Self>,
        cx: &mut App,
    ) -> AnyElement {
        let target = target.to_owned();
        div()
            .h(row_height)
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_size(chrome_rems(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child("Forward to this computer"),
            )
            .child(div().flex_1())
            .child(
                Button::new(SharedString::from(format!(
                    "port-add:{}:{target}",
                    target.len()
                )))
                .focus_ring(focus_visible(cx))
                .label("Add port…")
                .xsmall()
                .h(chrome_rems(24.))
                .ghost()
                .disabled(owner.is_none())
                .accessibility_label(format!("Add port for {target}"))
                .on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if this.ports.owner != owner || this.lifecycle_token() != owner {
                            return;
                        }
                        this.open_port_form_for_target(&target, window, cx);
                    });
                }),
            )
            .into_any_element()
    }

    fn open_port_form_for_target(
        &mut self,
        target: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_port_form(None, window, cx);
        if let Some(form) = self.ports.form.as_ref() {
            form.target.update(cx, |input, cx| {
                input.set_value(target.to_owned(), window, cx);
                input.set_disabled(true, cx);
            });
        }
    }

    pub(super) fn render_port_row(
        row: &ForwardRow,
        owner: Option<SourceToken>,
        feedback: Option<(&str, ForwardOutcome)>,
        pending: bool,
        row_height: Rems,
        weak: WeakEntity<Self>,
        cx: &mut App,
    ) -> AnyElement {
        let key = row.key().clone();
        let active = row.state() == ForwardState::Up;
        let label = row
            .label()
            .filter(|s| !s.is_empty())
            .or(row.process())
            .map(super::hosts::display)
            .unwrap_or_else(|| format!("Port {}", key.remote()));
        let detail = match row.local() {
            Some(local) => format!("{} → localhost:{local}", key.remote()),
            None => format!("Remote {} · TCP", key.remote()),
        };
        let primary_key = key.clone();
        let primary_local = row.local();
        let settings_row = row.clone();
        let settings_weak = weak.clone();
        let primary = if active { "Copy address" } else { "Forward" };
        div()
            .id(SharedString::from(format!(
                "host-port:{}:{}",
                key.target(),
                key.remote()
            )))
            .h(row_height)
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Icon::new(IconName::Network)
                    .xsmall()
                    .h(chrome_rems(24.))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(chrome_rems(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(detail),
                    )
                    .when_some(row.error().map(super::hosts::display), |view, error| {
                        view.child(
                            div()
                                .text_size(chrome_rems(12.))
                                .text_color(cx.theme().danger)
                                .child(error),
                        )
                    })
                    .when_some(
                        feedback.map(|(text, state)| (text.to_owned(), state)),
                        |view, (text, state)| {
                            view.child(
                                div()
                                    .text_size(chrome_rems(12.))
                                    .text_color(match state {
                                        ForwardOutcome::Succeeded | ForwardOutcome::Pending => {
                                            cx.theme().muted_foreground
                                        }
                                        ForwardOutcome::Partial | ForwardOutcome::Uncertain => {
                                            cx.theme().warning
                                        }
                                        ForwardOutcome::Failed | ForwardOutcome::Rejected => {
                                            cx.theme().danger
                                        }
                                    })
                                    .child(super::hosts::display(&text)),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .text_size(chrome_rems(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(state_text(row.state())),
            )
            .child(
                Button::new(SharedString::from(format!(
                    "port-primary:{}:{}",
                    key.target(),
                    key.remote()
                )))
                .focus_ring(focus_visible(cx))
                .label(primary)
                .xsmall()
                .h(chrome_rems(24.))
                .disabled(owner.is_none() || pending)
                .accessibility_label(format!("{primary} {} port {}", key.target(), key.remote()))
                .on_click(move |_, _, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if this.ports.owner != owner || this.lifecycle_token() != owner {
                            return;
                        }
                        if active {
                            let valid = this.ports.rows(this.model.as_ref()).iter().any(|row| {
                                row.key() == &primary_key
                                    && row.state() == ForwardState::Up
                                    && row.local() == primary_local
                            });
                            if valid {
                                if let Some(local) = primary_local {
                                    cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                        "localhost:{local}"
                                    )));
                                }
                            }
                        } else {
                            match ForwardRequest::start(
                                primary_key.clone(),
                                primary_local,
                                false,
                                None,
                            ) {
                                Ok(request) => this.submit_forward(request, OpenOrigin::Hosts, cx),
                                Err(error) => {
                                    this.ports.error = Some(error.to_string());
                                    cx.notify();
                                }
                            }
                        }
                    });
                }),
            )
            .child(
                Button::new(SharedString::from(format!(
                    "port-settings:{}:{}",
                    key.target(),
                    key.remote()
                )))
                .focus_ring(focus_visible(cx))
                .debug_selector({
                    let key = key.clone();
                    move || format!("port-settings:{}:{}", key.target(), key.remote())
                })
                .icon(Icon::new(IconName::SlidersHorizontal))
                .ghost()
                .xsmall()
                .h(chrome_rems(24.))
                .accessibility_label(format!(
                    "Settings for port {} on {}",
                    key.remote(),
                    key.target()
                ))
                .tooltip("Local port settings…")
                .disabled(owner.is_none() || pending)
                .on_click(move |_, window, cx| {
                    let _ = settings_weak.update(cx, |this, cx| {
                        if this.ports.owner == owner && this.lifecycle_token() == owner {
                            this.open_port_form(Some(settings_row.clone()), window, cx);
                        }
                    });
                }),
            )
            .into_any_element()
    }

    pub(super) fn render_pending_port(
        key: &MappingKey,
        owner: Option<SourceToken>,
        feedback: &str,
        row_height: Rems,
        weak: WeakEntity<Self>,
        cx: &mut App,
    ) -> AnyElement {
        let key = key.clone();
        let stop_key = key.clone();
        div()
            .id(SharedString::from(format!(
                "host-port-pending:{}:{}",
                key.target(),
                key.remote()
            )))
            .h(row_height)
            .w_full()
            .flex()
            .items_center()
            .gap_3()
            .pl_8()
            .pr_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(format!("Port {}", key.remote()))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(super::hosts::display(feedback)),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!(
                    "port-stop-pending:{}:{}",
                    key.target(),
                    key.remote()
                )))
                .focus_ring(focus_visible(cx))
                .label("Stop")
                .xsmall()
                .h(chrome_rems(24.))
                .disabled(owner.is_none())
                .accessibility_label(format!(
                    "Stop pending port {} on {}",
                    key.remote(),
                    key.target()
                ))
                .on_click(move |_, _, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if this.ports.owner == owner && this.lifecycle_token() == owner {
                            this.submit_forward(
                                ForwardRequest::stop(stop_key.clone()),
                                OpenOrigin::Hosts,
                                cx,
                            );
                        }
                    });
                }),
            )
            .into_any_element()
    }

    pub(super) fn open_port_form(
        &mut self,
        row: Option<ForwardRow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.destination != Destination::Hosts
            || self.ports.form.is_some()
            || !self.ports.can_manage()
        {
            return;
        }
        let Some(owner) = self
            .ports
            .owner
            .filter(|owner| self.lifecycle_token() == Some(*owner))
        else {
            return;
        };
        let target_value = row
            .as_ref()
            .map(|row| row.key().target())
            .unwrap_or_default()
            .to_owned();
        let remote_value = row
            .as_ref()
            .map(|row| row.key().remote().to_string())
            .unwrap_or_default();
        let local_value = row
            .as_ref()
            .and_then(ForwardRow::local)
            .map(|port| port.to_string())
            .unwrap_or_default();
        let label_value = row
            .as_ref()
            .and_then(ForwardRow::label)
            .unwrap_or_default()
            .to_owned();
        let target = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("user@host[:ssh-port]")
                .default_value(target_value)
        });
        let remote = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Remote port")
                .default_value(remote_value)
        });
        let local = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Use remote port")
                .default_value(local_value)
        });
        let label = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Optional")
                .default_value(label_value)
        });
        let edit_saved = row.as_ref().is_some_and(ForwardRow::is_saved);
        let edit_key = row.as_ref().map(|row| row.key().clone());
        let edit_state = row.as_ref().map(ForwardRow::state);
        self.ports.form = Some(PortForm {
            owner,
            edit_key,
            edit_state,
            target: target.clone(),
            remote,
            local,
            label,
            restore: row.as_ref().is_some_and(ForwardRow::restores_on_connect),
            edit_saved,
            pending: None,
            error: None,
        });
        if let Some(form) = self
            .ports
            .form
            .as_ref()
            .filter(|form| form.edit_key.is_some())
        {
            form.target
                .update(cx, |input, cx| input.set_disabled(true, cx));
            form.remote
                .update(cx, |input, cx| input.set_disabled(true, cx));
        }
        let weak = cx.entity().downgrade();
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            let Some(view) = weak.upgrade() else {
                return dialog.title("Add port");
            };
            let Some((target, remote, local, label, restore, editing, edit_state, pending, error)) = ({
                let workspace = view.read(cx);
                workspace.ports.form.as_ref().map(|form| {
                    (
                        form.target.clone(),
                        form.remote.clone(),
                        form.local.clone(),
                        form.label.clone(),
                        form.restore,
                        form.edit_key.is_some(),
                        form.edit_state,
                        form.pending.is_some(),
                        form.error.clone(),
                    )
                })
            }) else {
                return dialog.title("Add port");
            };
            if first.replace(false) {
                if editing {
                    local.read(cx).focus_handle(cx).focus(window, cx);
                } else if !target.read(cx).value().is_empty() {
                    remote.read(cx).focus_handle(cx).focus(window, cx);
                } else {
                    target.read(cx).focus_handle(cx).focus(window, cx);
                }
            }
            let cancel = weak.clone();
            let forward = weak.clone();
            let open = weak.clone();
            let save = weak.clone();
            let restore_weak = weak.clone();
            let field = |id: &'static str,
                         label_text: &'static str,
                         input: &Entity<InputState>,
                         locked: bool| {
                div().flex().flex_col().gap_1().child(label_text).child(
                    div()
                        .h(chrome_rems(34.))
                        .when(cfg!(all(test, feature = "ui-test-support")), |view| {
                            view.debug_selector(move || id.into())
                        })
                        .child(
                            InputGroup::new(id)
                                .small()
                                .aria_label(label_text)
                                .disabled(pending || locked)
                                .input(KitInput::new(input)),
                        ),
                )
            };
            let mut footer = DialogFooter::new().flex_wrap().child(
                Button::new("port-cancel")
                    .label("Cancel")
                    .outline()
                    .on_click(move |_, window, cx| {
                        let _ = cancel.update(cx, |this, cx| this.cancel_port_form(window, cx));
                    }),
            );
            if editing {
                footer = footer.child(
                    Button::new("port-save")
                        .label(if pending { "Working…" } else { "Save" })
                        .primary()
                        .disabled(pending)
                        .on_click(move |_, window, cx| {
                            let _ = save.update(cx, |this, cx| {
                                this.submit_port_form(ForwardAction::Save, window, cx)
                            });
                        }),
                );
                let resume = weak.clone();
                footer = footer.child(
                    Button::new("port-forward-local")
                        .label("Forward")
                        .disabled(pending || matches!(edit_state, Some(ForwardState::Up | ForwardState::Rebuilding)))
                        .on_click(move |_, window, cx| {
                            let _ = resume.update(cx, |this, cx| this.submit_port_form(ForwardAction::Start, window, cx));
                        }),
                );
            } else {
                footer = footer.child(
                        Button::new("port-forward")
                            .label(if pending { "Working…" } else { "Forward" })
                            .primary()
                            .disabled(pending)
                            .on_click(move |_, window, cx| {
                                let _ = forward.update(cx, |this, cx| {
                                    this.submit_port_form(ForwardAction::Start, window, cx)
                                });
                            }),
                    );
            }
            let controls = view.read(cx).ports.form.as_ref().and_then(|form| form.edit_key.clone().map(|key| (key, form.edit_saved, form.owner)));
            let local_actions = div().flex().flex_wrap().gap_2()
                .when(!editing, |v| {
                    let save = weak.clone();
                    v.child(Button::new("port-save").label("Save without starting").small().outline()
                        .disabled(pending).on_click(move |_, window, cx| {
                            let _ = save.update(cx, |this, cx| this.submit_port_form(ForwardAction::Save, window, cx));
                        }))
                        .child(Button::new("port-open").label("Open as HTTP").small().outline()
                            .disabled(pending).on_click(move |_, window, cx| {
                                let _ = open.update(cx, |this, cx| this.submit_port_form(ForwardAction::Open, window, cx));
                            }))
                })
                .when_some(controls, |v, (key, saved, owner)| {
                let stop = weak.clone(); let stop_key = key.clone();
                let forget = weak.clone(); let open_http = weak.clone();
                v.child(Button::new("port-stop").debug_selector(|| "port-stop".into()).label("Stop forwarding").small()
                    .disabled(pending || !matches!(edit_state, Some(ForwardState::Up | ForwardState::Rebuilding | ForwardState::Saved | ForwardState::Failed)))
                    .on_click(move |_, window, cx| { let _ = stop.update(cx, |this, cx| {
                        if this.lifecycle_token() != Some(owner) { return; }
                        this.submit_forward(ForwardRequest::stop(stop_key.clone()), OpenOrigin::Hosts, cx);
                        this.cancel_port_form(window, cx);
                    }); }))
                .child(Button::new("port-open-http").label("Open as HTTP").small().disabled(pending)
                    .on_click(move |_, window, cx| { let _ = open_http.update(cx, |this, cx| {
                        if this.lifecycle_token() != Some(owner) { return; }
                        this.submit_port_form(ForwardAction::Open, window, cx);
                    }); }))
                .when(saved, |v| v.child(Button::new("port-forget").label("Forget settings").small().ghost()
                    .tooltip("Remove local settings; keep the current forward running")
                    .disabled(pending).on_click(move |_, window, cx| { let _ = forget.update(cx, |this, cx| {
                        if this.lifecycle_token() != Some(owner) { return; }
                        this.submit_forward(ForwardRequest::forget(key.clone()), OpenOrigin::Hosts, cx);
                        this.cancel_port_form(window, cx);
                    }); })))
            });
            dialog
                .title(if editing { "Local port settings" } else { "Add port" })
                .overlay_closable(false)
                .keyboard(true)
                .close_button(false)
                .on_cancel({
                    let weak = weak.clone();
                    move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| this.cancel_port_form(window, cx));
                        false
                    }
                })
                .on_ok({
                    let weak = weak.clone();
                    move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.submit_port_form(
                                if editing {
                                    ForwardAction::Save
                                } else {
                                    ForwardAction::Start
                                },
                                window,
                                cx,
                            )
                        });
                        false
                    }
                })
                .child(
                    div()
                        .id("port-form")
                        .text_size(chrome_rems(12.5))
                        .when(cfg!(all(test, feature = "ui-test-support")), |view| {
                            view.debug_selector(|| "port-form-body".into())
                        })
                        .flex()
                        .flex_col()
                        .flex_none()
                        .max_h(px((f32::from(window.viewport_size().height) - 160.).max(180.)))
                        .overflow_y_scroll()
                        .gap_3()
                        .child(field("port-target", "SSH target", &target, editing))
                        .child(field("port-remote", "Remote port", &remote, editing))
                        .child(field("port-local", "Local port (optional)", &local, matches!(edit_state, Some(ForwardState::Up | ForwardState::Rebuilding))))
                        .when(matches!(edit_state, Some(ForwardState::Up | ForwardState::Rebuilding)), |view| view.child(
                            div().text_sm().text_color(cx.theme().muted_foreground).child("Stop forwarding before changing the local port.")))
                        .when(editing, |view| view.child(
                            div().text_sm().text_color(cx.theme().muted_foreground)
                                .child("These settings belong to this computer. Saving does not start or stop forwarding."),
                        ))
                        .child(Checkbox::new("port-restore")
                            .label("Restore when I connect")
                            .checked(restore).disabled(pending)
                            .on_change(move |next, _, cx| {
                                let _ = restore_weak.update(cx, |this, cx| {
                                    if let Some(form) = this.ports.form.as_mut() { form.restore = *next; cx.notify(); }
                                });
                            }))
                        .child(field("port-label", "Personal label (optional)", &label, false))
                        .child(local_actions)
                        .when_some(error, |view, error| {
                            view.child(
                                div()
                                    .text_color(cx.theme().danger)
                                    .child(super::hosts::display(&error)),
                            )
                        }),
                )
                .footer(footer)
        });
        cx.notify();
    }

    fn submit_port_form(&mut self, action: ForwardAction, _: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.ports.form.as_ref() else {
            return;
        };
        if form.pending.is_some() || self.lifecycle_token() != Some(form.owner) {
            return;
        }
        if form.edit_key.is_some()
            && !matches!(
                action,
                ForwardAction::Save | ForwardAction::Start | ForwardAction::Open
            )
        {
            return;
        }
        let target = form.target.read(cx).value().to_string();
        let remote = form.remote.read(cx).value().to_string();
        let local = form.local.read(cx).value().to_string();
        let label = form.label.read(cx).value().to_string();
        let build = || -> Result<ForwardRequest, String> {
            let local = decimal_port(&local, true, "Local port")?;
            let key = if let Some(key) = &form.edit_key {
                key.clone()
            } else {
                let remote = decimal_port(&remote, false, "Remote port")?.expect("required port");
                MappingKey::new(&target, remote).map_err(|error| error.to_string())?
            };
            let label = (form.edit_saved || !label.is_empty()).then_some(label);
            match action {
                ForwardAction::Start => {
                    ForwardRequest::start(key, local, form.edit_saved || form.restore, label)
                }
                ForwardAction::Open => {
                    ForwardRequest::open(key, local, form.edit_saved || form.restore, label)
                }
                ForwardAction::Save => ForwardRequest::save(key, local, label),
                _ => unreachable!(),
            }
            .map(|request| request.with_restore(form.restore))
            .map_err(|error| error.to_string())
        };
        let request = match build() {
            Ok(request) => request,
            Err(error) => {
                self.ports.form.as_mut().expect("form").error = Some(error);
                cx.notify();
                return;
            }
        };
        let key = request.key().clone();
        if self.ports.pending(&key) {
            self.ports.form.as_mut().expect("form").error =
                Some("A port action is already pending".into());
            cx.notify();
            return;
        }
        if !self.ports.can_record(&key) {
            self.ports.form.as_mut().expect("form").error =
                Some("Too many port actions are pending; wait for one to finish".into());
            cx.notify();
            return;
        }
        let id = Uuid::new_v4();
        let owner = form.owner;
        match self.commands.try_send(WorkerCommand::new(
            owner,
            Command::ManageForward {
                request_id: id,
                request,
            },
        )) {
            Ok(()) => {
                if action == ForwardAction::Open {
                    self.ports.open_requests.insert(id, OpenOrigin::Hosts);
                }
                self.ports.record(
                    key,
                    PortFeedback {
                        owner,
                        request_id: id,
                        action,
                        outcome: ForwardOutcome::Pending,
                        reason: None,
                        browser_opened: None,
                        settle_after: 0,
                    },
                );
                let form = self.ports.form.as_mut().expect("form");
                form.pending = Some(id);
                form.error = None;
                for input in [&form.target, &form.remote, &form.local, &form.label] {
                    input.update(cx, |input, cx| input.set_disabled(true, cx));
                }
            }
            Err(error) => {
                self.ports.form.as_mut().expect("form").error =
                    Some(format!("Port action was not sent: {error}"))
            }
        }
        cx.notify();
    }

    fn cancel_port_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = self.ports.form.take() {
            if let Some(id) = form
                .pending
                .filter(|id| self.ports.open_requests.remove(id).is_some())
            {
                let _ = self.commands.try_send(WorkerCommand::new(
                    form.owner,
                    Command::CancelForwardOpen { request_id: id },
                ));
            }
            window.close_dialog(cx);
            self.port_trigger.focus(window, cx);
            cx.notify();
        }
    }

    pub(super) fn ports_owner_changed(
        &mut self,
        owner: SourceToken,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ports.owner == Some(owner) {
            return;
        }
        let old = self.ports.owner;
        let canceled = self.ports.revoke_open();
        if let Some(old) = old {
            for request_id in canceled {
                let _ = self.commands.try_send(WorkerCommand::new(
                    old,
                    Command::CancelForwardOpen { request_id },
                ));
            }
        }
        if self.ports.form.is_some() {
            window.close_dialog(cx);
        }
        self.ports.set_owner(owner);
        cx.notify();
    }

    pub(super) fn leave_ports(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let canceled = self.ports.revoke_hosts_open();
        if let Some(owner) = self.ports.owner {
            for request_id in canceled {
                let _ = self.commands.try_send(WorkerCommand::new(
                    owner,
                    Command::CancelForwardOpen { request_id },
                ));
            }
        }
        if self.ports.form.take().is_some() {
            window.close_dialog(cx);
        }
        cx.notify();
    }

    pub(super) fn revoke_missing_terminal_links(&mut self, model: &WindowWorkspace) {
        let stale: Vec<_> = self
            .ports
            .open_requests
            .iter()
            .filter_map(|(id, origin)| match origin {
                OpenOrigin::Terminal(terminal) if !model.accepts_terminal(terminal) => Some(*id),
                OpenOrigin::Drawer(group) if !group.is_current(model) => Some(*id),
                _ => None,
            })
            .collect();
        for id in stale {
            let origin = self.ports.open_requests.remove(&id);
            if let Some(owner) = self.ports.owner {
                let _ = self.commands.try_send(WorkerCommand::new(
                    owner,
                    Command::CancelForwardOpen { request_id: id },
                ));
            }
            if let Some(notice) = origin.as_ref().and_then(OpenOrigin::closed_notice) {
                self.ports.link_notice = Some(notice.into());
            }
        }
    }

    pub(super) fn revoke_screen_links(&mut self, screen: &ScreenKey, cx: &mut Context<Self>) {
        self.revoke_ports_popover_for_screen(screen, cx);
        let terminals: std::collections::HashSet<_> = self
            .model
            .as_ref()
            .into_iter()
            .flat_map(WindowWorkspace::routes)
            .filter(|route| route.screen() == screen)
            .map(|route| route.terminal().clone())
            .collect();
        let stale: Vec<_> = self
            .ports
            .open_requests
            .iter()
            .filter_map(|(id, origin)| match origin {
                OpenOrigin::Terminal(terminal) if terminals.contains(terminal) => Some(*id),
                OpenOrigin::Drawer(group)
                    if group.source_id() == screen.source_id()
                        && self.model.as_ref().is_some_and(|model| {
                            model.opened().iter().all(|entry| {
                                entry.key().source_id() != group.source_id()
                                    || entry.key() == screen
                            })
                        }) =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect();
        for id in stale {
            let origin = self.ports.open_requests.remove(&id);
            if let Some(owner) = self.ports.owner {
                let _ = self.commands.try_send(WorkerCommand::new(
                    owner,
                    Command::CancelForwardOpen { request_id: id },
                ));
            }
            if let Some(notice) = origin.as_ref().and_then(OpenOrigin::closed_notice) {
                self.ports.link_notice = Some(notice.into());
            }
            cx.notify();
        }
    }

    pub(super) fn revoke_close_links(&mut self, plan: &ClosePlan, cx: &mut Context<Self>) {
        let stale: Vec<_> = if plan.target == CloseTarget::Window {
            self.ports.open_requests.keys().copied().collect()
        } else {
            self.ports
                .open_requests
                .iter()
                .filter_map(|(id, origin)| match origin {
                    OpenOrigin::Terminal(terminal)
                        if Some(terminal.source_token()) == plan.source_token()
                            && plan.affected.iter().any(|affected| {
                                matches!(&affected.identity,
                            CloseIdentity::Pane { pane, session: Some(session), .. }
                                if pane == terminal.pane_id() && session == terminal.session_id())
                            }) =>
                    {
                        Some(*id)
                    }
                    _ => None,
                })
                .collect()
        };
        for id in stale {
            let origin = self.ports.open_requests.remove(&id);
            if let Some(owner) = self.ports.owner {
                let _ = self.commands.try_send(WorkerCommand::new(
                    owner,
                    Command::CancelForwardOpen { request_id: id },
                ));
            }
            if let Some(notice) = origin.as_ref().and_then(OpenOrigin::closed_notice) {
                self.ports.link_notice = Some(notice.into());
            }
            cx.notify();
        }
    }

    pub(super) fn refresh_ports(&mut self, cx: &mut Context<Self>) {
        let Some(owner) = self
            .ports
            .owner
            .filter(|owner| self.lifecycle_token() == Some(*owner))
        else {
            return;
        };
        if let Err(error) = self
            .commands
            .try_send(WorkerCommand::new(owner, Command::RefreshForwards))
        {
            self.ports.error = Some(format!("Ports refresh was not sent: {error}"));
        }
        cx.notify();
    }

    pub(super) fn submit_forward(
        &mut self,
        request: ForwardRequest,
        origin: OpenOrigin,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self
            .ports
            .owner
            .filter(|owner| self.lifecycle_token() == Some(*owner))
        else {
            return;
        };
        let key = request.key().clone();
        if let OpenOrigin::Drawer(group) = &origin {
            if group.owner() != owner
                || !self.ports.can_manage()
                || group.target() != key.target()
                || !self
                    .model
                    .as_ref()
                    .is_some_and(|model| group.is_current(model))
            {
                self.ports.link_notice =
                    Some("Port opening is no longer available for this host group".into());
                cx.notify();
                return;
            }
        }
        if self.ports.pending(&key) && request.action() != ForwardAction::Stop {
            if origin.notice_label().is_some() {
                self.ports.link_notice = Some(format!(
                    "Port {} on {} already has an action pending",
                    key.remote(),
                    key.target()
                ));
                cx.notify();
            }
            return;
        }
        if !self.ports.can_record(&key) {
            let message = "Too many port actions are pending; wait for one to finish".to_owned();
            if origin.notice_label().is_some() {
                self.ports.link_notice = Some(message.clone());
            }
            self.ports.error = Some(message);
            cx.notify();
            return;
        }
        let action = request.action();
        let notice_label = origin.notice_label();
        let superseded = (action == ForwardAction::Stop)
            .then(|| {
                self.ports
                    .feedback
                    .get(&key)
                    .filter(|feedback| feedback.outcome == ForwardOutcome::Pending)
                    .map(|feedback| feedback.request_id)
            })
            .flatten();
        if action == ForwardAction::Stop {
            if let Some(id) = self.ports.cancel_open_for_key(&key) {
                let _ = self.commands.try_send(WorkerCommand::new(
                    owner,
                    Command::CancelForwardOpen { request_id: id },
                ));
            }
        }
        let request_id = Uuid::new_v4();
        match self.commands.try_send(WorkerCommand::new(
            owner,
            Command::ManageForward {
                request_id,
                request,
            },
        )) {
            Ok(()) => {
                if self
                    .ports
                    .form
                    .as_ref()
                    .is_some_and(|form| form.pending == superseded && superseded.is_some())
                {
                    if let Some(form) = self.ports.form.as_mut() {
                        form.pending = None;
                        form.error = Some(
                            "Stop submitted. The earlier port action is no longer current.".into(),
                        );
                        for input in [&form.target, &form.remote, &form.local, &form.label] {
                            input.update(cx, |input, cx| input.set_disabled(false, cx));
                        }
                    }
                }
                if action == ForwardAction::Open {
                    self.ports.open_requests.insert(request_id, origin);
                }
                self.ports.record(
                    key.clone(),
                    PortFeedback {
                        owner,
                        request_id,
                        action,
                        outcome: ForwardOutcome::Pending,
                        reason: None,
                        browser_opened: None,
                        settle_after: 0,
                    },
                );
                self.ports.error = None;
                if let Some(label) = notice_label {
                    self.ports.link_notice = Some(format!(
                        "{label} pending on {}:{}…",
                        key.target(),
                        key.remote()
                    ));
                }
            }
            Err(error) => {
                let message = format!("Port action was not sent: {error}");
                if notice_label.is_some() {
                    self.ports.link_notice = Some(message.clone());
                }
                self.ports.error = Some(message);
            }
        }
        cx.notify();
    }

    pub(super) fn forward_result(
        &mut self,
        result: ForwardResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = result.owner();
        let id = result.request_id();
        let current = self.ports.owner == Some(owner) && self.lifecycle_token() == Some(owner);
        let tracked = self
            .ports
            .feedback
            .get(result.key())
            .is_some_and(|feedback| {
                feedback.owner == owner
                    && feedback.request_id == id
                    && feedback.action == result.action()
                    && feedback.outcome == ForwardOutcome::Pending
            });
        if !current || !tracked {
            if let Some(handle) = result.eligible_open() {
                handle.revoke();
            }
            return;
        }
        if result.outcome() == ForwardOutcome::Pending {
            return;
        }
        let origin = self.ports.open_requests.get(&id).cloned();
        let mut opened = false;
        if let Some(handle) = result.eligible_open() {
            let valid_origin = match self.ports.open_requests.get(&id) {
                Some(OpenOrigin::Hosts) => {
                    self.destination == Destination::Hosts && handle.terminal().is_none()
                }
                Some(OpenOrigin::Drawer(group)) => {
                    handle.terminal().is_none()
                        && group.target() == result.key().target()
                        && self
                            .model
                            .as_ref()
                            .is_some_and(|model| group.is_current(model))
                }
                Some(OpenOrigin::Terminal(terminal)) => {
                    handle.terminal() == Some(terminal)
                        && self
                            .model
                            .as_ref()
                            .is_some_and(|model| model.accepts_terminal(terminal))
                }
                None => false,
            };
            if !valid_origin {
                handle.revoke();
            } else {
                let terminal_valid = handle.terminal().is_none_or(|terminal| {
                    self.model
                        .as_ref()
                        .is_some_and(|model| model.accepts_terminal(terminal))
                });
                if terminal_valid && handle.consume() {
                    cx.open_url(handle.url().as_str());
                    opened = true;
                } else {
                    handle.revoke();
                }
            }
        }
        self.ports.open_requests.remove(&id);
        if let Some(label) = origin.as_ref().and_then(OpenOrigin::notice_label) {
            self.ports.link_notice = if opened {
                None
            } else {
                Some(match result.outcome() {
                    ForwardOutcome::Succeeded | ForwardOutcome::Partial => {
                        format!("{label} could not be verified for browser opening")
                    }
                    ForwardOutcome::Failed | ForwardOutcome::Rejected => {
                        format!("{label} failed: {}", result.reason().unwrap_or("Try again"))
                    }
                    ForwardOutcome::Uncertain => format!(
                        "{label} status unknown: {}",
                        result.reason().unwrap_or("Refresh Ports to inspect")
                    ),
                    ForwardOutcome::Pending => unreachable!(),
                })
            };
        }
        if let Some(feedback) = self.ports.feedback.get_mut(result.key()) {
            feedback.outcome = result.outcome();
            feedback.reason = result.reason().map(str::to_owned);
            feedback.browser_opened = (result.action() == ForwardAction::Open).then_some(opened);
            feedback.settle_after = self
                .ports
                .snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.generation());
        }
        if self
            .ports
            .form
            .as_ref()
            .is_some_and(|form| form.pending == Some(id))
        {
            if result.outcome() == ForwardOutcome::Succeeded
                && (result.action() != ForwardAction::Open || opened)
            {
                self.ports.form = None;
                window.close_dialog(cx);
                self.port_trigger.focus(window, cx);
            } else if let Some(form) = self.ports.form.as_mut() {
                form.pending = None;
                form.error = if result.action() == ForwardAction::Open
                    && result.outcome() == ForwardOutcome::Succeeded
                    && !opened
                {
                    Some("Forward confirmed, but browser opening was canceled. The forward may still be active.".into())
                } else {
                    result.reason().map(str::to_owned)
                };
                for input in [&form.target, &form.remote, &form.local, &form.label] {
                    input.update(cx, |input, cx| input.set_disabled(false, cx));
                }
            }
        }
        cx.notify();
    }

    pub(super) fn remote_link(
        &mut self,
        terminal: TerminalToken,
        url: WebUrl,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self
            .model
            .as_ref()
            .filter(|model| model.accepts_terminal(&terminal))
        else {
            return;
        };
        let Some(target) = model.source_target(terminal.source_token().source_id()) else {
            return;
        };
        let Some(remote) = url.port_or_known_default() else {
            return;
        };
        let Ok(key) = MappingKey::new(target, remote) else {
            return;
        };
        let Ok(request) = ForwardRequest::open_link(key, terminal.clone(), url) else {
            return;
        };
        self.submit_forward(request, OpenOrigin::Terminal(terminal), cx);
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::{
        CloseTarget, Command, Destination, ForwardAction, ForwardOutcome, ForwardRequest,
        ForwardResult, ForwardSnapshot, ForwardState, MappingKey, OpenOrigin, PortFeedback, WebUrl,
        Workspace,
    };
    use gpui_kit::{
        px, size, AppContext, Entity, Focusable, Modifiers, TestAppContext, VisualTestContext,
    };
    use splitty_client_core::{native_workspace::Worker, workspace::WindowWorkspace};
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{atomic::AtomicBool, Arc},
    };
    use tokio::sync::mpsc;
    use uuid::Uuid;

    pub(super) fn with_view(run: impl FnOnce(&mut VisualTestContext, Entity<Workspace>)) {
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
        run(window, view);
    }

    #[test]
    fn full_pending_feedback_rejects_untracked_open_until_terminal_result_frees_capacity() {
        with_view(|window, view| {
            let (commands, mut receiver) = mpsc::channel(4);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(WindowWorkspace::new(Uuid::new_v4()));
                    let owner = workspace.model.as_ref().unwrap().lifecycle_token();
                    workspace.ports.set_owner(owner);
                    workspace.destination = Destination::Hosts;
                    let mut first = None;
                    for port in 3000..3000 + super::PortsView::MAX_FEEDBACK as u16 {
                        let key = MappingKey::new("vm1", port).unwrap();
                        let id = Uuid::new_v4();
                        workspace.ports.record(key.clone(), PortFeedback {
                            owner, request_id: id, action: ForwardAction::Save,
                            outcome: ForwardOutcome::Pending, reason: None,
                            browser_opened: None, settle_after: 0,
                        });
                        if first.is_none() {
                            first = Some((key, id));
                        }
                    }
                    let existing_key = MappingKey::new("vm1", 3001).unwrap();
                    workspace.submit_forward(ForwardRequest::stop(existing_key.clone()), OpenOrigin::Hosts, cx);
                    assert!(matches!(receiver.try_recv().unwrap().kind(), Command::ManageForward { request, .. }
                        if request.action() == ForwardAction::Stop && request.key() == &existing_key));
                    assert_eq!(workspace.ports.feedback.len(), super::PortsView::MAX_FEEDBACK);
                    let new_key = MappingKey::new("vm1", 4000).unwrap();
                    workspace.submit_forward(
                        ForwardRequest::open(new_key.clone(), None, false, None).unwrap(),
                        OpenOrigin::Hosts,
                        cx,
                    );
                    assert_eq!(workspace.ports.feedback.len(), super::PortsView::MAX_FEEDBACK);
                    assert!(workspace.ports.open_requests.is_empty());
                    assert!(receiver.try_recv().is_err(), "capacity rejection sends no command");

                    workspace.open_port_form(None, window, cx);
                    let form = workspace.ports.form.as_ref().unwrap();
                    form.target.update(cx, |input, cx| input.set_value("vm1", window, cx));
                    form.remote.update(cx, |input, cx| input.set_value("4000", window, cx));
                    workspace.submit_port_form(ForwardAction::Open, window, cx);
                    assert!(workspace.ports.form.as_ref().unwrap().error.as_deref().unwrap().contains("pending"));
                    assert!(workspace.ports.open_requests.is_empty());
                    assert!(receiver.try_recv().is_err(), "form capacity rejection sends no command");

                    let (old_key, old_id) = first.unwrap();
                    let old_request = ForwardRequest::save(old_key.clone(), None, None).unwrap();
                    workspace.forward_result(
                        ForwardResult::ui_test_fixture(
                            owner, old_id, &old_request, ForwardOutcome::Failed,
                            Some("fixture failure".into()), None,
                        ).unwrap(),
                        window, cx,
                    );
                    assert_eq!(workspace.ports.feedback[&old_key].outcome, ForwardOutcome::Failed);
                    workspace.submit_forward(
                        ForwardRequest::open(new_key.clone(), None, false, None).unwrap(),
                        OpenOrigin::Hosts,
                        cx,
                    );
                    assert_eq!(workspace.ports.feedback.len(), super::PortsView::MAX_FEEDBACK);
                    assert!(!workspace.ports.feedback.contains_key(&old_key));
                    assert!(workspace.ports.feedback.contains_key(&new_key));
                    assert_eq!(workspace.ports.open_requests.len(), 1);
                    assert!(matches!(receiver.try_recv().unwrap().kind(), Command::ManageForward { request, .. }
                        if request.action() == ForwardAction::Open && request.key() == &new_key));
                });
            });
        });
    }

    #[test]
    fn retained_form_validates_raw_input_and_reenables_after_identified_failure() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let (commands, mut receiver) = mpsc::channel(4);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(WindowWorkspace::new(owner));
                    workspace.ports.set_owner(
                        workspace
                            .model
                            .as_ref()
                            .unwrap()
                            .local_source()
                            .source_token(),
                    );
                    workspace.destination = Destination::Hosts;
                    workspace.open_port_form(None, window, cx);
                });
                window.draw(cx).clear(cx);
            });
            window.simulate_resize(size(px(640.), px(420.)));
            window.update(|window, cx| window.draw(cx).clear(cx));
            let target = window.debug_bounds("port-target").unwrap();
            let label = window.debug_bounds("port-label").unwrap();
            let body = window.debug_bounds("port-form-body").unwrap();
            let dialog = window.debug_bounds("dialog-0").unwrap();
            assert!(target.size.height >= px(30.));
            assert!(label.size.height >= px(30.));
            assert!(
                body.origin.y + body.size.height + px(48.) <= dialog.origin.y + dialog.size.height,
                "scroll body must fit above footer: body={body:?}, dialog={dialog:?}"
            );
            window.simulate_input("bad target!");
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let form = workspace.ports.form.as_ref().unwrap();
                    assert_eq!(form.target.read(cx).value(), "bad target!");
                    form.remote
                        .update(cx, |input, cx| input.set_value("3000", window, cx));
                    workspace.submit_port_form(ForwardAction::Start, window, cx);
                    assert!(workspace.ports.form.as_ref().unwrap().error.is_some());
                    assert!(receiver.try_recv().is_err());
                    let form = workspace.ports.form.as_ref().unwrap();
                    form.target
                        .update(cx, |input, cx| input.set_value("vm1", window, cx));
                    form.local
                        .update(cx, |input, cx| input.set_value("4000x", window, cx));
                    workspace.submit_port_form(ForwardAction::Start, window, cx);
                    assert!(workspace
                        .ports
                        .form
                        .as_ref()
                        .unwrap()
                        .error
                        .as_deref()
                        .unwrap()
                        .contains("Local port"));
                    assert!(receiver.try_recv().is_err());
                    let form = workspace.ports.form.as_ref().unwrap();
                    form.local
                        .update(cx, |input, cx| input.set_value("4173", window, cx));
                    workspace.submit_port_form(ForwardAction::Start, window, cx);
                    assert!(!workspace
                        .ports
                        .form
                        .as_ref()
                        .unwrap()
                        .target
                        .read(cx)
                        .is_editable());
                });
            });
            let command = receiver.try_recv().unwrap();
            let (id, request) = match command.kind() {
                Command::ManageForward {
                    request_id,
                    request,
                } => (*request_id, request.clone()),
                _ => panic!("unexpected command"),
            };
            let token = command.source_token().unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.forward_result(
                        ForwardResult::ui_test_fixture(
                            token,
                            id,
                            &request,
                            ForwardOutcome::Pending,
                            None,
                            None,
                        )
                        .unwrap(),
                        window,
                        cx,
                    );
                    assert_eq!(workspace.ports.form.as_ref().unwrap().pending, Some(id));
                    workspace.forward_result(
                        ForwardResult::ui_test_fixture(
                            token,
                            Uuid::new_v4(),
                            &request,
                            ForwardOutcome::Failed,
                            Some("stale".into()),
                            None,
                        )
                        .unwrap(),
                        window,
                        cx,
                    );
                    assert_eq!(workspace.ports.form.as_ref().unwrap().pending, Some(id));
                    workspace.forward_result(
                        ForwardResult::ui_test_fixture(
                            token,
                            id,
                            &request,
                            ForwardOutcome::Rejected,
                            Some("service refused".into()),
                            None,
                        )
                        .unwrap(),
                        window,
                        cx,
                    );
                    let form = workspace.ports.form.as_ref().unwrap();
                    assert!(form.pending.is_none());
                    assert!(form.target.read(cx).is_editable());
                    assert_eq!(form.local.read(cx).value(), "4173");
                    assert!(form.error.as_deref().unwrap().contains("service refused"));
                });
            });
            window.simulate_keystrokes("escape");
            window.run_until_parked();
            window.update(|window, cx| {
                assert!(view.read(cx).ports.form.is_none());
                assert!(view.read(cx).port_trigger.is_focused(window));
            });
        });
    }

    #[test]
    fn canceled_open_and_stop_supersession_revoke_late_browser_consent() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let (commands, mut receiver) = mpsc::channel(8);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(WindowWorkspace::new(owner));
                    workspace.ports.set_owner(
                        workspace
                            .model
                            .as_ref()
                            .unwrap()
                            .local_source()
                            .source_token(),
                    );
                    workspace.destination = Destination::Hosts;
                    workspace.open_port_form(None, window, cx);
                    let form = workspace.ports.form.as_ref().unwrap();
                    form.target
                        .update(cx, |input, cx| input.set_value("vm1", window, cx));
                    form.remote
                        .update(cx, |input, cx| input.set_value("3000", window, cx));
                    workspace.submit_port_form(ForwardAction::Open, window, cx);
                });
            });
            let first = receiver.try_recv().unwrap();
            let (open_id, open_request) = match first.kind() {
                Command::ManageForward {
                    request_id,
                    request,
                } => (*request_id, request.clone()),
                _ => panic!("unexpected command"),
            };
            let owner = first.source_token().unwrap();
            let key = open_request.key().clone();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.submit_forward(
                        ForwardRequest::stop(key.clone()),
                        OpenOrigin::Hosts,
                        cx,
                    );
                    assert!(workspace.ports.form.as_ref().unwrap().pending.is_none());
                    assert!(workspace
                        .ports
                        .form
                        .as_ref()
                        .unwrap()
                        .target
                        .read(cx)
                        .is_editable());
                    let late = ForwardResult::ui_test_fixture(
                        owner,
                        open_id,
                        &open_request,
                        ForwardOutcome::Succeeded,
                        None,
                        Some(4173),
                    )
                    .unwrap();
                    let handle = late.eligible_open().unwrap().clone();
                    workspace.forward_result(late, window, cx);
                    assert!(!handle.is_active());
                    assert_eq!(workspace.ports.feedback[&key].action, ForwardAction::Stop);
                });
            });
            assert!(
                matches!(receiver.try_recv().unwrap().kind(), Command::CancelForwardOpen { request_id } if *request_id == open_id)
            );
            assert!(
                matches!(receiver.try_recv().unwrap().kind(), Command::ManageForward { request, .. } if request.action() == ForwardAction::Stop)
            );
        });
    }

    #[test]
    fn full_command_queue_cannot_keep_hosts_open_consent_alive_after_escape() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let (commands, mut receiver) = mpsc::channel(1);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(WindowWorkspace::new(owner));
                    workspace.ports.set_owner(
                        workspace
                            .model
                            .as_ref()
                            .unwrap()
                            .local_source()
                            .source_token(),
                    );
                    workspace.destination = Destination::Hosts;
                    workspace.open_port_form(None, window, cx);
                    let form = workspace.ports.form.as_ref().unwrap();
                    form.target
                        .update(cx, |input, cx| input.set_value("vm1", window, cx));
                    form.remote
                        .update(cx, |input, cx| input.set_value("3000", window, cx));
                    workspace.submit_port_form(ForwardAction::Open, window, cx);
                    assert_eq!(workspace.ports.open_requests.len(), 1);
                    workspace.show_screens(window, cx);
                    assert!(workspace.ports.open_requests.is_empty());
                    assert!(workspace.ports.form.is_none());
                    assert!(
                        workspace.destination == Destination::Hosts,
                        "full queue prevents browsing release but still revokes browser consent"
                    );
                });
            });
            let queued = receiver.try_recv().unwrap();
            let (id, request) = match queued.kind() {
                Command::ManageForward {
                    request_id,
                    request,
                } => (*request_id, request.clone()),
                _ => panic!("unexpected command"),
            };
            assert!(receiver.try_recv().is_err());
            let token = queued.source_token().unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let late = ForwardResult::ui_test_fixture(
                        token,
                        id,
                        &request,
                        ForwardOutcome::Succeeded,
                        None,
                        Some(4173),
                    )
                    .unwrap();
                    let handle = late.eligible_open().unwrap().clone();
                    workspace.forward_result(late, window, cx);
                    assert!(!handle.is_active());
                    assert!(super::feedback_text(
                        workspace.ports.feedback.get(request.key()).unwrap()
                    )
                    .contains("browser did not open"));
                });
            });
        });
    }

    #[test]
    fn remote_screen_close_revokes_link_before_old_window_model_reconciles() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let mut model = WindowWorkspace::new(owner);
            let (screen, terminal) = model.ui_test_remote_terminal_fixture("vm1").unwrap();
            assert!(model.accepts_terminal(&terminal));
            let (commands, mut receiver) = mpsc::channel(1);
            window.update(|_window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.ports.set_owner(model.lifecycle_token());
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                    workspace.remote_link(
                        terminal.clone(),
                        WebUrl::parse("http://localhost:3000/path?mode=dev#run").unwrap(),
                        cx,
                    );
                });
            });
            let first = receiver.try_recv().unwrap();
            let (id, request) = match first.kind() {
                Command::ManageForward {
                    request_id,
                    request,
                } => (*request_id, request.clone()),
                _ => panic!("unexpected command"),
            };
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.request_close_screen(screen.clone(), window, cx);
                    assert!(
                        workspace
                            .model
                            .as_ref()
                            .unwrap()
                            .accepts_terminal(&terminal),
                        "old model remains until worker reconciliation"
                    );
                    assert!(
                        workspace.ports.open_requests.is_empty(),
                        "close revokes matching link immediately"
                    );
                });
            });
            assert!(matches!(
                receiver.try_recv().unwrap().kind(),
                Command::DetachRemoteScreen { .. }
            ));
            assert!(
                receiver.try_recv().is_err(),
                "best-effort cancel must not displace admitted Detach"
            );
            let owner = first.source_token().unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let result = ForwardResult::ui_test_fixture(
                        owner,
                        id,
                        &request,
                        ForwardOutcome::Succeeded,
                        None,
                        Some(4173),
                    )
                    .unwrap();
                    let handle = result.eligible_open().unwrap().clone();
                    workspace.forward_result(result, window, cx);
                    assert!(!handle.is_active());
                    assert_eq!(
                        workspace.ports.link_notice.as_deref(),
                        Some("Development link closed before it opened")
                    );
                });
            });
        });
    }

    #[test]
    fn admitted_remote_pane_close_revokes_link_without_waiting_for_reconcile() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let mut model = WindowWorkspace::new(owner);
            let (screen, terminal) = model.ui_test_remote_terminal_fixture("vm1").unwrap();
            let (commands, mut receiver) = mpsc::channel(1);
            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.ports.set_owner(model.lifecycle_token());
                    workspace.model = Some(model);
                    workspace.remote_link(
                        terminal.clone(),
                        WebUrl::parse("http://localhost:3000/").unwrap(),
                        cx,
                    );
                });
            });
            let first = receiver.try_recv().unwrap();
            let (id, request) = match first.kind() {
                Command::ManageForward {
                    request_id,
                    request,
                } => (*request_id, request.clone()),
                _ => panic!("unexpected command"),
            };
            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    let source = workspace
                        .model
                        .as_ref()
                        .unwrap()
                        .source(screen.source_id())
                        .unwrap();
                    let plan = source
                        .assess_close(
                            7,
                            CloseTarget::Pane {
                                screen: screen.screen_id().to_owned(),
                                pane: terminal.pane_id().to_owned(),
                            },
                        )
                        .unwrap();
                    workspace.submit_close(plan, true, cx);
                    assert!(workspace.pending_close.is_some());
                    assert!(workspace.ports.open_requests.is_empty());
                    assert!(workspace
                        .model
                        .as_ref()
                        .unwrap()
                        .accepts_terminal(&terminal));
                });
            });
            assert!(matches!(
                receiver.try_recv().unwrap().kind(),
                Command::RequestClose { .. }
            ));
            assert!(
                receiver.try_recv().is_err(),
                "best-effort cancel must not displace admitted close"
            );
            let owner = first.source_token().unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let late = ForwardResult::ui_test_fixture(
                        owner,
                        id,
                        &request,
                        ForwardOutcome::Succeeded,
                        None,
                        Some(4173),
                    )
                    .unwrap();
                    let handle = late.eligible_open().unwrap().clone();
                    workspace.forward_result(late, window, cx);
                    assert!(!handle.is_active());
                });
            });
        });
    }

    #[test]
    fn accepted_snapshot_keeps_saved_port_without_discovery_and_uncertain_feedback() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            window.update(|window, cx| {
            view.update(cx, |workspace, _cx| {
                workspace.model = Some(WindowWorkspace::new(owner));
                let token = workspace.model.as_ref().unwrap().local_source().source_token();
                workspace.ports.set_owner(token);
                workspace.destination = Destination::Hosts;
                let saved = r#"{"version":1,"forwards":[{"target":"vm1","remote":3000,"local":4173,"label":"App","enabled":false,"lastError":null}]}"#;
                let snapshot = Arc::new(ForwardSnapshot::ui_test_fixture(token, 1, None, Some(saved)).unwrap());
                assert!(workspace.ports.accept(snapshot));
                let rows = workspace.ports.rows(workspace.model.as_ref());
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].state(), ForwardState::Paused);
                assert!(rows[0].is_saved());
                let key = rows[0].key().clone();
                workspace.ports.record(key.clone(), PortFeedback { owner: token, request_id: Uuid::new_v4(), action: ForwardAction::Forget, outcome: ForwardOutcome::Uncertain, reason: Some("reply lost".into()), browser_opened: None, settle_after: 0 });
                assert!(workspace.ports.accept(Arc::new(ForwardSnapshot::ui_test_fixture(token, 2, Some("[]"), Some(r#"{"version":1,"forwards":[]}"#)).unwrap())));
                assert_eq!(workspace.ports.feedback[&key].outcome, ForwardOutcome::Uncertain);
            });
            window.draw(cx).clear(cx);
        });
            assert!(window.debug_bounds("hosts-detached-feedback").is_some());
        });
    }

    #[test]
    fn saved_port_settings_stop_without_starting() {
        let cases = [
            (
                r#"{"version":1,"forwards":[{"target":"vm1","remote":3000,"local":4173,"label":"App","enabled":true,"lastError":null}]}"#,
                ForwardState::Saved,
            ),
            (
                r#"{"version":1,"forwards":[{"target":"vm1","remote":3000,"local":4173,"label":"App","enabled":true,"lastError":{"code":"local_port_conflict","message":"occupied"}}]}"#,
                ForwardState::Failed,
            ),
        ];
        for (saved, expected_state) in cases {
            with_view(|window, view| {
                let owner = Uuid::new_v4();
                let (commands, mut receiver) = mpsc::channel(4);
                window.update(|window, cx| {
                    view.update(cx, |workspace, _cx| {
                        workspace.commands = commands;
                        workspace.model = Some(WindowWorkspace::new(owner));
                        let token = workspace
                            .model
                            .as_ref()
                            .unwrap()
                            .local_source()
                            .source_token();
                        workspace.ports.set_owner(token);
                        workspace.destination = Destination::Hosts;
                        workspace.hosts.section = super::super::hosts::HostSection::Ports;
                        assert!(workspace.ports.accept(Arc::new(
                            ForwardSnapshot::ui_test_fixture(token, 1, Some("[]"), Some(saved))
                                .unwrap()
                        )));
                        let row = workspace
                            .ports
                            .rows(workspace.model.as_ref())
                            .pop()
                            .unwrap();
                        assert_eq!(row.state(), expected_state);
                        assert!(row.is_saved());
                    });
                    window.draw(cx).clear(cx);
                });
                let more = window
                    .debug_bounds("port-settings:vm1:3000")
                    .expect("saved port row menu should be visible");
                window.simulate_click(more.center(), Modifiers::default());
                window.update(|window, cx| window.draw(cx).clear(cx));
                let stop = window.debug_bounds("port-stop").unwrap();
                window.simulate_click(stop.center(), Modifiers::default());
                let sent = receiver
                    .try_recv()
                    .expect("Pause saved port must submit Stop");
                assert!(matches!(
                    sent.kind(),
                    Command::ManageForward { request, .. }
                        if request.action() == ForwardAction::Stop
                            && request.key().target() == "vm1"
                            && request.key().remote() == 3000
                ));
                assert!(
                    receiver.try_recv().is_err(),
                    "Pause must not start forwarding"
                );
            });
        }
    }

    #[test]
    fn paused_port_edit_enter_submits_metadata_save_without_resuming() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let (commands, mut receiver) = mpsc::channel(4);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(WindowWorkspace::new(owner));
                    let token = workspace.model.as_ref().unwrap().local_source().source_token();
                    workspace.ports.set_owner(token);
                    workspace.destination = Destination::Hosts;
                    let saved = r#"{"version":1,"forwards":[{"target":"vm1","remote":3000,"local":4173,"label":"App","enabled":false,"lastError":null}]}"#;
                    workspace.ports.accept(Arc::new(ForwardSnapshot::ui_test_fixture(token, 1, Some("[]"), Some(saved)).unwrap()));
                    let row = workspace.ports.rows(workspace.model.as_ref()).pop().unwrap();
                    workspace.open_port_form(Some(row), window, cx);
                });
                window.draw(cx).clear(cx);
            });
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let form = workspace.ports.form.as_ref().unwrap();
                    assert!(form.edit_key.is_some());
                    assert!(!form.target.read(cx).is_editable());
                    assert!(!form.remote.read(cx).is_editable());
                    assert!(form.local.read(cx).focus_handle(cx).is_focused(window));
                    form.label
                        .update(cx, |input, cx| input.set_value("Changed", window, cx));
                });
            });
            window.simulate_keystrokes("enter");
            let sent = receiver.try_recv().expect("Enter should submit Save");
            assert!(
                matches!(sent.kind(), Command::ManageForward { request, .. } if request.action() == ForwardAction::Save && request.key().target() == "vm1" && request.key().remote() == 3000)
            );
            assert_eq!(sent.source_token().unwrap().connection_id(), owner);
        });
    }

    #[test]
    fn alternate_local_port_attempt_keeps_saved_rule_until_forward_succeeds() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let (commands, mut receiver) = mpsc::channel(4);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(WindowWorkspace::new(owner));
                    let token = workspace.model.as_ref().unwrap().local_source().source_token();
                    workspace.ports.set_owner(token);
                    workspace.destination = Destination::Hosts;
                    let saved = r#"{"version":1,"forwards":[{"target":"vm1","remote":3000,"local":4173,"label":"App","enabled":false,"lastError":null}]}"#;
                    workspace.ports.accept(Arc::new(ForwardSnapshot::ui_test_fixture(token, 1, Some("[]"), Some(saved)).unwrap()));
                    let row = workspace.ports.rows(workspace.model.as_ref()).pop().unwrap();
                    workspace.open_port_form(Some(row), window, cx);
                    let form = workspace.ports.form.as_ref().unwrap();
                    form.local.update(cx, |input, cx| input.set_value("4174", window, cx));
                    workspace.submit_port_form(ForwardAction::Start, window, cx);
                });
            });
            let sent = receiver
                .try_recv()
                .expect("alternate port must submit Start");
            let (id, request) = match sent.kind() {
                Command::ManageForward {
                    request_id,
                    request,
                } => (*request_id, request.clone()),
                _ => panic!("unexpected command"),
            };
            assert_eq!(request.action(), ForwardAction::Start);
            assert_eq!(request.local(), Some(4174));
            assert!(request.remember());
            assert!(
                receiver.try_recv().is_err(),
                "Save must not run before Start succeeds"
            );
            let token = sent.source_token().unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.forward_result(
                        ForwardResult::ui_test_fixture(
                            token,
                            id,
                            &request,
                            ForwardOutcome::Failed,
                            Some("local port conflict".into()),
                            None,
                        )
                        .unwrap(),
                        window,
                        cx,
                    );
                    assert!(workspace.ports.form.as_ref().unwrap().pending.is_none());
                    assert!(workspace
                        .ports
                        .form
                        .as_ref()
                        .unwrap()
                        .local
                        .read(cx)
                        .is_editable());
                    assert_eq!(
                        workspace.ports.rows(workspace.model.as_ref())[0].local(),
                        Some(4173),
                        "failed attempt retains accepted saved local port"
                    );
                });
            });
        });
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod drawer_tests;
