//! Native remote Screen creation action and its bounded window lifecycle projection.
use super::hosts::HostGroupTarget;
use super::*;
use splitty_client_core::workspace::{HostActionTarget, RemoteScreenCreateStatus};

pub(super) struct RemoteCreateNotice {
    owner: SourceToken,
    source: SourceToken,
    request_id: Uuid,
    message: String,
}

pub(super) struct RemoteCreateInterest {
    request: RemoteScreenCreateRequest,
    observed: bool,
}

impl RemoteCreateNotice {
    pub(super) fn message(&self) -> &str {
        &self.message
    }
}

fn bounded_notice(message: String) -> String {
    super::hosts::display(&message).chars().take(240).collect()
}

fn remote_create_binding_current(
    model: &WindowWorkspace,
    request: &RemoteScreenCreateRequest,
) -> bool {
    model.lifecycle_token() == request.target().local_owner()
        && model
            .host(request.target().source_id())
            .is_some_and(|host| host.action_target() == request.target())
        && model.execution_token(request.target().source_id()) == Some(request.source())
        && model
            .opened()
            .iter()
            .any(|entry| entry.key().source_id() == request.target().source_id())
}

impl Workspace {
    pub(super) fn cancel_remote_create_follow_for_detach(&mut self, screen: &ScreenKey) {
        let matches_current_request = self.remote_create_follow.as_ref().is_some_and(|request| {
            self.model.as_ref().is_some_and(|model| {
                model
                    .source(screen.source_id())
                    .and_then(SourceWorkspace::remote_screen_creation)
                    .is_some_and(|creation| {
                        creation.request_id() == request.request_id()
                            && creation.screen_id() == screen.screen_id()
                    })
            })
        });
        if matches_current_request {
            self.cancel_remote_create_follow();
        }
    }

    pub(super) fn cancel_remote_create_follow(&mut self) {
        if let Some(request) = self.remote_create_follow.take() {
            request.cancel_follow();
        }
    }

    pub(super) fn reconcile_remote_create(&mut self, model: &WindowWorkspace) {
        self.remote_create_enqueues.retain(|_, request| {
            if !remote_create_binding_current(model, request) {
                request.cancel_follow();
                return false;
            }
            !model
                .source(request.target().source_id())
                .and_then(SourceWorkspace::remote_screen_creation)
                .is_some_and(|creation| creation.request_id() == request.request_id())
        });
        if self
            .remote_create_follow
            .as_ref()
            .is_some_and(|request| !remote_create_binding_current(model, request))
        {
            self.cancel_remote_create_follow();
        }
        if let Some((request, observed)) = self
            .remote_create_latest
            .as_ref()
            .map(|interest| (interest.request.clone(), interest.observed))
        {
            let source_id = request.target().source_id();
            let same_group = model.lifecycle_token() == request.target().local_owner()
                && model.source_target(source_id) == Some(request.target().target());
            let creation = model
                .source(source_id)
                .and_then(SourceWorkspace::remote_screen_creation)
                .filter(|creation| creation.request_id() == request.request_id());
            if !same_group
                || (observed
                    && model.source(source_id).is_some()
                    && creation.is_none()
                    && !self.remote_create_enqueues.contains_key(&source_id))
            {
                self.remote_create_latest = None;
                if self
                    .remote_create_notice
                    .as_ref()
                    .is_some_and(|notice| notice.request_id == request.request_id())
                {
                    self.remote_create_notice = None;
                }
            } else if let Some(creation) = creation {
                if let Some(interest) = self.remote_create_latest.as_mut() {
                    interest.observed = true;
                }
                let message = match creation.status() {
                    RemoteScreenCreateStatus::Pending => None,
                    RemoteScreenCreateStatus::Uncertain => Some(format!(
                        "Screen creation on {} is unconfirmed. {}",
                        super::hosts::display(request.target().target()),
                        creation.reason().unwrap_or(
                            "The host may have created it; reconnect or detach the reference."
                        )
                    )),
                    RemoteScreenCreateStatus::Missing => Some(format!(
                        "Requested Screen is currently missing on {}. You can create another Screen or detach this reference.",
                        super::hosts::display(request.target().target())
                    )),
                    RemoteScreenCreateStatus::Rejected => Some(format!(
                        "New Screen on {} was rejected: {}",
                        super::hosts::display(request.target().target()),
                        creation.reason().unwrap_or("request was not accepted")
                    )),
                    RemoteScreenCreateStatus::Present => {
                        if self
                            .remote_create_notice
                            .as_ref()
                            .is_some_and(|notice| notice.request_id == request.request_id())
                        {
                            self.remote_create_notice = None;
                        }
                        None
                    }
                };
                if let Some(message) = message {
                    self.remote_create_notice = Some(RemoteCreateNotice {
                        owner: request.target().local_owner(),
                        source: request.source(),
                        request_id: request.request_id(),
                        message: bounded_notice(message),
                    });
                }
            }
        }
        if self.remote_create_notice.as_ref().is_some_and(|notice| {
            model.lifecycle_token() != notice.owner
                || model.source_target(notice.source.source_id()).is_none()
        }) {
            self.remote_create_notice = None;
        }
    }

    pub(super) fn remote_create_rejected(
        &mut self,
        owner: SourceToken,
        source: SourceToken,
        request_id: Uuid,
        reason: String,
        cx: &mut Context<Self>,
    ) {
        let matches_enqueued = self
            .remote_create_enqueues
            .get(&source.source_id())
            .is_some_and(|request| {
                request.target().local_owner() == owner
                    && request.source() == source
                    && request.request_id() == request_id
                    && self.model.as_ref().is_some_and(|model| {
                        model.lifecycle_token() == owner
                            && model.source_target(source.source_id())
                                == Some(request.target().target())
                    })
            });
        let same_interest = self.remote_create_latest.as_ref().is_some_and(|interest| {
            let request = &interest.request;
            request.target().local_owner() == owner
                && request.source() == source
                && request.request_id() == request_id
                && self.model.as_ref().is_some_and(|model| {
                    model.lifecycle_token() == owner
                        && model.source_target(source.source_id())
                            == Some(request.target().target())
                })
        });
        if !matches_enqueued && !same_interest {
            return;
        }
        if matches_enqueued {
            self.remote_create_enqueues.remove(&source.source_id());
        }
        if !same_interest {
            cx.notify();
            return;
        }
        if self
            .remote_create_follow
            .as_ref()
            .is_some_and(|request| request.request_id() == request_id)
        {
            self.cancel_remote_create_follow();
        }
        if self
            .remote_create_latest
            .as_ref()
            .is_some_and(|interest| interest.request.request_id() == request_id)
        {
            self.remote_create_latest = None;
        }
        self.remote_create_notice = Some(RemoteCreateNotice {
            owner,
            source,
            request_id,
            message: bounded_notice(format!("New Screen was not created: {reason}")),
        });
        cx.notify();
    }

    fn create_remote_screen(
        &mut self,
        group: &HostGroupTarget,
        target: &HostActionTarget,
        source: SourceToken,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.drawer_host_actions_available() || cx.has_active_drag() {
            return;
        }
        let Some(model) = self.model.as_ref().filter(|model| group.is_current(model)) else {
            return;
        };
        if self.remote_create_enqueues.contains_key(&group.source_id()) {
            return;
        }
        let request = match model.prepare_remote_screen_creation(target, source) {
            Ok(request) => request,
            Err(error) => {
                self.status = format!("New Screen unavailable: {error}");
                cx.notify();
                return;
            }
        };
        let owner = model.lifecycle_token();
        match self.commands.try_send(WorkerCommand::new(
            owner,
            Command::CreateRemoteScreen {
                request: request.clone(),
            },
        )) {
            Ok(()) => {
                self.cancel_remote_create_follow();
                self.remote_create_follow = Some(request.clone());
                self.remote_create_latest = Some(RemoteCreateInterest {
                    request: request.clone(),
                    observed: false,
                });
                self.remote_create_enqueues
                    .insert(group.source_id(), request);
                self.remote_create_notice = None;
                self.enter_screens_after_remote_create(window, cx);
            }
            Err(error) => {
                self.remote_create_notice = Some(RemoteCreateNotice {
                    owner,
                    source,
                    request_id: request.request_id(),
                    message: bounded_notice(format!("New Screen was not sent: {error}")),
                });
                cx.notify();
            }
        }
    }
    pub(super) fn drawer_host_actions_available(&self) -> bool {
        !self.hosts_navigation_blocked()
            && !self.hosts.has_open_form()
            && !self.ports.has_open_form()
    }

    pub(super) fn render_remote_create_action(
        &self,
        group: &HostGroupTarget,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let source = group.source_id();
        let available = self.drawer_host_actions_available();
        let queued = self.remote_create_enqueues.contains_key(&source);
        let creating = self
            .model
            .as_ref()
            .and_then(|model| model.source(source))
            .and_then(SourceWorkspace::remote_screen_creation)
            .is_some_and(|creation| creation.status() == RemoteScreenCreateStatus::Pending);
        let create_capture = self.model.as_ref().and_then(|model| {
            let target = model.host(source)?.action_target().clone();
            let execution = model.source(source)?.source_token();
            let unavailable = model
                .validate_remote_screen_creation(&target, execution)
                .err()
                .map(|error| error.to_string());
            Some((target, execution, unavailable))
        });
        let create_error = if !available {
            Some("Finish the current workspace action first".to_owned())
        } else if queued {
            Some("Screen creation is being submitted".to_owned())
        } else {
            create_capture
                .as_ref()
                .and_then(|(_, _, reason)| reason.clone())
                .or_else(|| {
                    create_capture
                        .is_none()
                        .then(|| "Host connection unavailable".into())
                })
        };
        let create_label = format!("New Screen on {}", super::hosts::display(group.target()));
        let create_tooltip = create_error.as_ref().map_or_else(
            || create_label.clone(),
            |reason| format!("{create_label}: {reason}"),
        );
        let create_group = group.clone();
        let create_weak = cx.entity().downgrade();
        Button::new(SharedString::from(format!("drawer-host-new-{source}")))
            .debug_selector(move || format!("drawer-host-new-{source}"))
            .accessibility_label(create_label)
            .tooltip(create_tooltip)
            .icon(Icon::new(IconName::Plus).size_3())
            .ghost()
            .compact()
            .small()
            .w(chrome_rems(26.))
            .h(chrome_rems(26.))
            .loading(creating || queued)
            .disabled(create_error.is_some())
            .on_click(move |_, window, cx| {
                if let Some((target, execution, _)) = &create_capture {
                    let _ = create_weak.update(cx, |this, cx| {
                        this.create_remote_screen(&create_group, target, *execution, window, cx);
                    });
                }
            })
            .into_any_element()
    }
}
