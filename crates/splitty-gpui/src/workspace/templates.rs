//! One modal for the service-owned saved-template library and apply review.
use super::*;
use splitty_client_core::templates::{
    TemplateApiOutcome, TemplateApiRequest, TemplateApiResult, TemplateCatalogSnapshot,
    TemplateDraft,
};
use splitty_client_core::workspace::{
    PendingTemplateProjection, TemplateApplyChoice, TemplateApplyOutcome, TemplateApplyPlan,
    TemplateApplyResult, TemplateApplyTarget,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetChoice {
    NewScreen,
    CurrentScreen,
}

enum TemplateMode {
    Library,
    Extract {
        screen_id: String,
        name: Entity<InputState>,
        root: Entity<InputState>,
    },
    /// The service names the file (`templates`, Saved Template Names).
    Save {
        draft: Arc<TemplateDraft>,
        name: Entity<InputState>,
        commands: HashSet<String>,
    },
    Overwrite {
        request: TemplateApiRequest,
        slug: String,
    },
    Apply {
        draft: Arc<TemplateDraft>,
        root: Entity<InputState>,
        target: TargetChoice,
    },
    Review {
        plan: Arc<TemplateApplyPlan>,
        /// What replacing the Screen ends, from its service; asked with
        /// `impact_request` (`templates`, Native Template Replacement Consent).
        impact: CloseImpact,
        impact_request: u64,
    },
    Delete {
        slug: String,
        name: String,
    },
}

pub(super) struct TemplateDialog {
    owner: Uuid,
    source: SourceToken,
    mode: TemplateMode,
    catalog: Option<Arc<TemplateCatalogSnapshot>>,
    pub(super) pending: Option<(Uuid, TemplateApiRequest)>,
    error: Option<String>,
    scroll: ScrollHandle,
}

impl TemplateDialog {
    fn accept(&mut self, owner: Uuid, request: Uuid) -> Option<TemplateApiRequest> {
        if self.owner != owner || !self.pending.as_ref().is_some_and(|(id, _)| *id == request) {
            return None;
        }
        self.pending.take().map(|(_, request)| request)
    }

    fn input_focused(&self, window: &Window, cx: &App) -> bool {
        let focused =
            |input: &Entity<InputState>| input.read(cx).focus_handle(cx).is_focused(window);
        match &self.mode {
            TemplateMode::Extract { name, root, .. } => focused(name) || focused(root),
            TemplateMode::Save { name, .. } => focused(name),
            TemplateMode::Apply { root, .. } => focused(root),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModalKey {
    Dismiss,
    PassToInput,
    Consume,
}

fn modal_key(key: &str, input_focused: bool, request_pending: bool) -> ModalKey {
    if key == "escape" {
        ModalKey::Dismiss
    } else if input_focused && !request_pending {
        ModalKey::PassToInput
    } else {
        ModalKey::Consume
    }
}

fn names_with_commands(draft: &TemplateDraft) -> Vec<String> {
    let mut names: Vec<_> = draft
        .template()
        .panes
        .iter()
        .filter(|(_, pane)| !pane.command.trim().is_empty())
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

impl Workspace {
    pub(super) fn toggle_templates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.template_dialog.is_some() {
            self.dismiss_templates(window, cx);
            return;
        }
        if self.rename.is_some()
            || self
                .pending_close
                .as_ref()
                .is_some_and(|close| close.prompting)
        {
            return;
        }
        let Some(source) = self
            .model
            .as_ref()
            .map(WindowWorkspace::local_source)
            .map(SourceWorkspace::source_token)
        else {
            self.status = "Templates are unavailable while the workspace starts".into();
            cx.notify();
            return;
        };
        if cx.has_active_drag() {
            cx.stop_active_drag(window);
        }
        if self.destination == Destination::Settings {
            self.close_settings(window, cx);
        } else if self.destination == Destination::Hosts {
            self.show_screens(window, cx);
            if self.destination == Destination::Hosts {
                return;
            }
        }
        if !self.drawer_visible {
            self.drawer_overlay_open = true;
        }
        self.drag = None;
        self.float_gesture = None;
        self.drop_target = None;
        self.tab_context_menu = None;
        self.profile_picker = None;
        for pane in &self.visible {
            if let Some(view) = self.panes.get(pane) {
                view.terminal
                    .update(cx, |terminal, _| terminal.cancel_pointer());
            }
        }
        self.template_dialog = Some(TemplateDialog {
            owner: source.connection_id(),
            source,
            mode: TemplateMode::Library,
            catalog: None,
            pending: None,
            error: None,
            scroll: ScrollHandle::new(),
        });
        self.focus.focus(window, cx);
        self.template_api(TemplateApiRequest::List, cx);
        cx.notify();
    }

    pub(super) fn dismiss_templates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.template_dialog.take().is_some() {
            self.focus_current(window, cx);
            cx.notify();
        }
    }

    pub(super) fn template_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.template_dialog.as_ref() else {
            return;
        };
        match modal_key(
            &key.key,
            dialog.input_focused(window, cx),
            dialog.pending.is_some(),
        ) {
            ModalKey::Dismiss => {
                cx.stop_propagation();
                self.dismiss_templates(window, cx);
            }
            ModalKey::Consume => cx.stop_propagation(),
            ModalKey::PassToInput => {}
        }
    }

    fn template_api(&mut self, request: TemplateApiRequest, cx: &mut Context<Self>) {
        let Some(dialog) = self.template_dialog.as_mut() else {
            return;
        };
        if dialog.pending.is_some() {
            return;
        }
        if !self
            .model
            .as_ref()
            .map(WindowWorkspace::local_source)
            .is_some_and(|model| model.source_token() == dialog.source)
        {
            return;
        }
        let request_id = Uuid::new_v4();
        match self.commands.try_send(WorkerCommand::new(
            dialog.source,
            Command::TemplateApi {
                connection: dialog.owner,
                request_id,
                request: request.clone(),
            },
        )) {
            Ok(()) => {
                dialog.pending = Some((request_id, request));
                dialog.error = None;
            }
            Err(error) => dialog.error = Some(format!("Request not sent: {error}")),
        }
        cx.notify();
    }

    pub(super) fn template_result(
        &mut self,
        result: TemplateApiResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.template_dialog.as_mut() else {
            return;
        };
        if !self
            .model
            .as_ref()
            .map(WindowWorkspace::local_source)
            .is_some_and(|model| {
                model.source_token() == dialog.source
                    && model.connection_id() == result.connection_id
            })
        {
            return;
        }
        let Some(request) = dialog.accept(result.connection_id, result.request_id) else {
            return;
        };
        let request = Some(request);
        match result.outcome {
            TemplateApiOutcome::Catalog(catalog) => {
                if !matches!(request.as_ref(), Some(TemplateApiRequest::List)) {
                    dialog.error = Some("Unexpected template list reply".into());
                    cx.notify();
                    return;
                }
                dialog.catalog = Some(catalog);
                dialog.error = None;
            }
            TemplateApiOutcome::Draft(draft) => {
                if !matches!(
                    request.as_ref(),
                    Some(TemplateApiRequest::Extract { .. } | TemplateApiRequest::Load { .. })
                ) {
                    dialog.error = Some("Unexpected template draft reply".into());
                    cx.notify();
                    return;
                }
                let name =
                    cx.new(|cx| InputState::new(window, cx).default_value(draft.name().to_owned()));
                let commands = names_with_commands(&draft).into_iter().collect();
                dialog.mode =
                    if matches!(request.as_ref(), Some(TemplateApiRequest::Extract { .. })) {
                        TemplateMode::Save {
                            draft,
                            name: name.clone(),
                            commands,
                        }
                    } else {
                        let root = cx.new(|cx| InputState::new(window, cx));
                        root.update(cx, |input, cx| input.focus(window, cx));
                        TemplateMode::Apply {
                            draft,
                            root,
                            target: TargetChoice::NewScreen,
                        }
                    };
                if let TemplateMode::Save { name, .. } = &dialog.mode {
                    name.update(cx, |name, cx| name.focus(window, cx));
                }
            }
            TemplateApiOutcome::Saved { slug } => {
                if !matches!(request.as_ref(), Some(TemplateApiRequest::Save { .. })) {
                    dialog.error = Some("Unexpected template save reply".into());
                    cx.notify();
                    return;
                }
                self.status = format!("Template saved: {slug}");
                dialog.mode = TemplateMode::Library;
                dialog.catalog = None;
                self.focus.focus(window, cx);
                self.template_api(TemplateApiRequest::List, cx);
            }
            TemplateApiOutcome::Deleted { slug } => {
                if !matches!(request.as_ref(), Some(TemplateApiRequest::Delete { .. })) {
                    dialog.error = Some("Unexpected template delete reply".into());
                    cx.notify();
                    return;
                }
                self.status = format!("Template deleted: {slug}");
                dialog.mode = TemplateMode::Library;
                dialog.catalog = None;
                self.focus.focus(window, cx);
                self.template_api(TemplateApiRequest::List, cx);
            }
            TemplateApiOutcome::OverwriteRequired { slug } => {
                if let Some(request) =
                    request.filter(|request| matches!(request, TemplateApiRequest::Save { .. }))
                {
                    dialog.mode = TemplateMode::Overwrite { request, slug };
                    dialog.error = None;
                    self.focus.focus(window, cx);
                } else {
                    dialog.error = Some("Overwrite target changed; save again".into());
                }
            }
            TemplateApiOutcome::Rejected { reason } | TemplateApiOutcome::Uncertain { reason } => {
                dialog.error = Some(reason);
            }
        }
        cx.notify();
    }

    fn template_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &mut self.template_dialog {
            dialog.mode = TemplateMode::Library;
            dialog.error = None;
        }
        self.focus.focus(window, cx);
        self.template_api(TemplateApiRequest::List, cx);
    }

    fn template_load(&mut self, slug: String, cx: &mut Context<Self>) {
        self.template_api(TemplateApiRequest::Load { slug }, cx);
    }

    fn template_extract(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The Screen shown is saved, local or on a host.
        let Some(model) = self.selected_model().or_else(|| self.local_model()) else {
            return;
        };
        let Some(screen) = model.shown_screen_id().map(str::to_owned) else {
            self.status = "Select a Screen to save".into();
            cx.notify();
            return;
        };
        let name_text = model
            .screens()
            .iter()
            .find(|info| info.id.as_str() == screen.as_str())
            .and_then(|info| info.name.as_deref())
            .unwrap_or("Screen")
            .to_owned();
        let name = cx.new(|cx| InputState::new(window, cx).default_value(name_text));
        let root = cx.new(|cx| InputState::new(window, cx));
        if let Some(dialog) = &mut self.template_dialog {
            dialog.mode = TemplateMode::Extract {
                screen_id: screen.to_owned(),
                name: name.clone(),
                root,
            };
            dialog.error = None;
        }
        name.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn template_extract_submit(&mut self, cx: &mut Context<Self>) {
        let Some((screen_id, name, root)) = self.template_dialog.as_ref().and_then(|dialog| {
            if let TemplateMode::Extract {
                screen_id,
                name,
                root,
            } = &dialog.mode
            {
                Some((
                    screen_id.clone(),
                    name.read(cx).value().trim().to_owned(),
                    root.read(cx).value().to_string(),
                ))
            } else {
                None
            }
        }) else {
            return;
        };
        let shown = self
            .selected_model()
            .or_else(|| self.local_model())
            .filter(|model| model.shown_screen_id() == Some(screen_id.as_str()))
            .map(SourceWorkspace::source_token);
        let Some(shown) = shown else {
            if let Some(dialog) = &mut self.template_dialog {
                dialog.error = Some("Source Screen changed; reopen Save".into());
            }
            cx.notify();
            return;
        };
        if name.is_empty() {
            if let Some(dialog) = &mut self.template_dialog {
                dialog.error = Some("Enter a template name".into());
            }
            cx.notify();
            return;
        }
        let Some(dialog) = self.template_dialog.as_mut() else {
            return;
        };
        if shown != dialog.source {
            // A host reads its own Screen; the local service keeps the result.
            if dialog.pending.is_some() {
                return;
            }
            let request_id = Uuid::new_v4();
            match self.commands.try_send(WorkerCommand::new(
                shown,
                Command::ExtractTemplate {
                    owner: dialog.owner,
                    request_id,
                    screen: screen_id.clone(),
                    name: name.clone(),
                    root: root.clone(),
                },
            )) {
                Ok(()) => {
                    dialog.pending = Some((
                        request_id,
                        TemplateApiRequest::Extract {
                            screen_id,
                            name,
                            root,
                        },
                    ));
                    dialog.error = None;
                }
                Err(error) => dialog.error = Some(format!("Request not sent: {error}")),
            }
            cx.notify();
            return;
        }
        self.template_api(
            TemplateApiRequest::Extract {
                screen_id,
                name,
                root,
            },
            cx,
        );
    }

    fn template_save(&mut self, cx: &mut Context<Self>) {
        let Some((draft, name, included_commands)) =
            self.template_dialog.as_ref().and_then(|dialog| {
                if let TemplateMode::Save {
                    draft,
                    name,
                    commands,
                } = &dialog.mode
                {
                    Some((
                        draft.clone(),
                        name.read(cx).value().trim().to_owned(),
                        commands.iter().cloned().collect(),
                    ))
                } else {
                    None
                }
            })
        else {
            return;
        };
        self.template_api(
            TemplateApiRequest::Save {
                draft,
                name,
                included_commands,
                overwrite_confirmed: false,
            },
            cx,
        );
    }

    fn template_overwrite(&mut self, cx: &mut Context<Self>) {
        let Some(TemplateApiRequest::Save {
            draft,
            name,
            included_commands,
            ..
        }) = self.template_dialog.as_ref().and_then(|dialog| {
            if let TemplateMode::Overwrite { request, .. } = &dialog.mode {
                Some(request.clone())
            } else {
                None
            }
        })
        else {
            return;
        };
        self.template_api(
            TemplateApiRequest::Save {
                draft,
                name,
                included_commands,
                overwrite_confirmed: true,
            },
            cx,
        );
    }

    fn template_delete(&mut self, slug: String, name: String, cx: &mut Context<Self>) {
        if let Some(dialog) = &mut self.template_dialog {
            dialog.mode = TemplateMode::Delete { slug, name };
            dialog.error = None;
        }
        cx.notify();
    }

    fn template_delete_submit(&mut self, cx: &mut Context<Self>) {
        let Some(slug) = self.template_dialog.as_ref().and_then(|dialog| {
            if let TemplateMode::Delete { slug, .. } = &dialog.mode {
                Some(slug.clone())
            } else {
                None
            }
        }) else {
            return;
        };
        self.template_api(TemplateApiRequest::Delete { slug }, cx);
    }

    fn template_toggle_command(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(TemplateDialog {
            mode: TemplateMode::Save { commands, .. },
            ..
        }) = &mut self.template_dialog
        {
            if !commands.insert(name.to_owned()) {
                commands.remove(name);
            }
            cx.notify();
        }
    }

    fn template_select_target(&mut self, target: TargetChoice, cx: &mut Context<Self>) {
        if let Some(dialog) = &mut self.template_dialog {
            if let TemplateMode::Apply {
                target: selected, ..
            } = &mut dialog.mode
            {
                *selected = target;
                cx.notify();
            }
        }
    }

    /// Asks the service what replacing `screen` ends, for the review's text.
    fn assess_template_replacement(&mut self, source: SourceToken, screen: String) {
        let request_id = self.next_close_id.saturating_add(1);
        self.next_close_id = request_id;
        let sent = self.commands.try_send(WorkerCommand::new(
            source,
            Command::AssessClose {
                request_id,
                scope: close_scope(&CloseTarget::Screen { screen }),
            },
        ));
        if let Some(TemplateMode::Review {
            impact,
            impact_request,
            ..
        }) = self.template_dialog.as_mut().map(|dialog| &mut dialog.mode)
        {
            *impact_request = request_id;
            if sent.is_err() {
                *impact = CloseImpact::Unknown;
            }
        }
    }

    /// The service's answer for the review's replacement; false when the
    /// answer is for something else.
    pub(super) fn template_replacement_assessed(
        &mut self,
        source: SourceToken,
        request_id: u64,
        answer: &CloseImpact,
    ) -> bool {
        let Some(TemplateMode::Review {
            plan,
            impact,
            impact_request,
        }) = self.template_dialog.as_mut().map(|dialog| &mut dialog.mode)
        else {
            return false;
        };
        if *impact_request != request_id || plan.source_token() != source {
            return false;
        }
        *impact = answer.clone();
        true
    }

    fn template_prepare_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &self.template_dialog else {
            return;
        };
        let TemplateMode::Apply {
            draft,
            root,
            target,
        } = &dialog.mode
        else {
            return;
        };
        // A template goes to the source of the Screen shown, local or a host.
        let Some(model) = self.selected_model().or_else(|| self.local_model()) else {
            return;
        };
        let target = match target {
            TargetChoice::NewScreen => TemplateApplyTarget::NewScreen,
            TargetChoice::CurrentScreen => {
                let Some(screen_id) = model.shown_screen_id() else {
                    return;
                };
                TemplateApplyTarget::Current {
                    screen_id: screen_id.to_owned(),
                }
            }
        };
        let result = model.prepare_template_apply(
            Uuid::new_v4(),
            draft.clone(),
            target,
            root.read(cx).value().to_string(),
        );
        let mut direct_layout = false;
        let mut assess = None;
        if let Some(dialog) = &mut self.template_dialog {
            match result {
                Ok(plan) => {
                    direct_layout = plan.replacement_impact().is_none()
                        && plan.projection().pane_names.iter().all(|name| {
                            plan.draft()
                                .template()
                                .panes
                                .get(name)
                                .is_none_or(|pane| pane.command.trim().is_empty())
                        });
                    let replaced = plan
                        .replacement_impact()
                        .map(|_| plan.screen_id().to_owned());
                    let source = plan.source_token();
                    dialog.mode = TemplateMode::Review {
                        plan: Arc::new(plan),
                        impact: CloseImpact::Pending,
                        impact_request: 0,
                    };
                    dialog.error = None;
                    if let Some(screen) = replaced {
                        assess = Some((source, screen));
                    }
                }
                Err(error) => dialog.error = Some(format!("Cannot apply: {error}")),
            }
        }
        if let Some((source, screen)) = assess {
            self.assess_template_replacement(source, screen);
        }
        if direct_layout {
            self.template_apply(TemplateApplyChoice::LayoutOnly, window, cx);
        } else if self
            .template_dialog
            .as_ref()
            .is_some_and(|dialog| matches!(&dialog.mode, TemplateMode::Review { .. }))
        {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    fn template_apply(
        &mut self,
        choice: TemplateApplyChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.template_dialog.as_mut() else {
            return;
        };
        let TemplateMode::Review { plan, .. } = &dialog.mode else {
            return;
        };
        if self.pending_template_apply.is_some() {
            return;
        }
        let plan = plan.clone();
        let current = self
            .model
            .as_ref()
            .and_then(|window| window.source(plan.source_token().source_id()))
            .is_some_and(|model| model.source_token() == plan.source_token());
        if !current {
            dialog.error = Some("Template review belongs to an earlier connection".into());
            cx.notify();
            return;
        }
        match self.commands.try_send(WorkerCommand::new(
            plan.source_token(),
            Command::ApplyTemplate {
                plan: plan.clone(),
                choice,
                replace_confirmed: plan.replacement_impact().is_some(),
            },
        )) {
            Ok(()) => {
                if plan.replacement_impact().is_none() {
                    self.cancel_remote_create_follow();
                }
                self.pending_template_apply = Some((plan.connection_id(), plan.request_id()));
                // The worker publishes a separate pending projection. Dismiss
                // the modal now so the intended layout is immediately visible.
                self.template_dialog = None;
                self.sync_visible(cx);
                self.focus.focus(window, cx);
            }
            Err(error) => dialog.error = Some(format!("Apply not sent: {error}")),
        }
        cx.notify();
    }

    pub(super) fn template_apply_result(
        &mut self,
        result: TemplateApplyResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending_template_apply != Some((result.connection_id, result.request_id)) {
            return;
        }
        self.pending_template_apply = None;
        self.sync_visible(cx);
        self.focus_current(window, cx);
        match result.outcome {
            TemplateApplyOutcome::Confirmed {
                launch_errors,
                profile_fallbacks,
            } => {
                let mut notices = vec!["Template applied".to_owned()];
                if !launch_errors.is_empty() {
                    notices.push(format!(
                        "Launch errors: {}",
                        launch_errors
                            .iter()
                            .map(|error| error.reason.as_str())
                            .collect::<Vec<_>>()
                            .join(" · ")
                    ));
                }
                if !profile_fallbacks.is_empty() {
                    notices.push(format!(
                        "Profile fallbacks: {}",
                        profile_fallbacks
                            .iter()
                            .map(|fallback| format!(
                                "{} → {} ({})",
                                fallback.requested_profile_id,
                                fallback.effective_profile_id,
                                fallback.reason
                            ))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    ));
                }
                self.status = notices.join(" · ");
                cx.notify();
            }
            TemplateApplyOutcome::Rejected { reason } => {
                self.status = format!("Template apply rejected: {reason}");
                cx.notify();
            }
            TemplateApplyOutcome::Unverified { reason } => {
                self.status = format!("Template result unverified: {reason}");
                cx.notify();
            }
        }
    }

    pub(super) fn template_overlay(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.template_dialog.as_ref()?;
        let colors = &self.appearance;
        let weak = cx.entity().downgrade();
        let mut body = div()
            .flex()
            .flex_col()
            .gap(chrome_rems(8.))
            .text_size(chrome_rems(12.))
            .line_height(relative(1.45));
        let title = match &dialog.mode {
            TemplateMode::Library => "Saved templates",
            TemplateMode::Extract { .. } => "Save current Screen",
            TemplateMode::Save { .. } => "Review saved template",
            TemplateMode::Overwrite { .. } => "Confirm template overwrite",
            TemplateMode::Apply { .. } => "Apply saved template",
            TemplateMode::Review { .. } => "Review template application",
            TemplateMode::Delete { .. } => "Delete saved template?",
        };
        if dialog.pending.is_some() {
            body = body.child("Waiting for the local service…");
        } else {
            match &dialog.mode {
                TemplateMode::Library => {
                    body =
                        body.child(
                            div()
                                .id("template-save-current")
                                .map(|button| action_style(button, colors, true))
                                .child("Save current Screen…")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.template_extract(window, cx)
                                })),
                        );
                    if let Some(catalog) = &dialog.catalog {
                        if catalog.entries().is_empty() {
                            body = body.child("No saved templates");
                        }
                        for entry in catalog.entries() {
                            let slug = entry.slug.clone();
                            let slug_for_delete = slug.clone();
                            let name = entry.display_name.clone();
                            let name_for_delete = name.clone();
                            body = body.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("template-load-{slug}")))
                                            .flex_1()
                                            .min_w_0()
                                            .map(|row| row_style(row, colors, false))
                                            .child(name)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.template_load(slug.clone(), cx)
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "template-delete-{slug_for_delete}"
                                            )))
                                            .map(|button| quiet_style(button, colors))
                                            .child("Delete")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.template_delete(
                                                    slug_for_delete.clone(),
                                                    name_for_delete.clone(),
                                                    cx,
                                                )
                                            })),
                                    ),
                            );
                        }
                    } else {
                        body = body.child("Loading saved templates…");
                    }
                }
                TemplateMode::Extract { name, root, .. } => {
                    body = body
                        .child(field_label(colors, "Name"))
                        .child(
                            div()
                                .id("template-extract-name")
                                .map(|field| field_box(field, colors))
                                .child(Input::new(name)),
                        )
                        .child(field_label(
                            colors,
                            "Relative root on the terminal host (optional)",
                        ))
                        .child(
                            div()
                                .id("template-extract-root")
                                .map(|field| field_box(field, colors))
                                .child(Input::new(root)),
                        )
                        .child(
                            div()
                                .id("template-extract-submit")
                                .map(|button| action_style(button, colors, true))
                                .child("Extract for review")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.template_extract_submit(cx)),
                                ),
                        );
                }
                TemplateMode::Save {
                    draft,
                    name,
                    commands,
                } => {
                    body = body.child(field_label(colors, "Name")).child(
                        div()
                            .id("template-save-name")
                            .map(|field| field_box(field, colors))
                            .child(Input::new(name)),
                    );
                    for command_name in names_with_commands(draft) {
                        let included = commands.contains(&command_name);
                        let command = draft
                            .template()
                            .panes
                            .get(&command_name)
                            .map(|pane| pane.command.clone())
                            .unwrap_or_default();
                        let label = format!(
                            "{} {}: {}",
                            if included { "☑" } else { "□" },
                            command_name,
                            command
                        );
                        body = body.child(
                            div()
                                .id(SharedString::from(format!(
                                    "template-command-{command_name}"
                                )))
                                .map(|row| row_style(row, colors, false))
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.template_toggle_command(&command_name, cx)
                                })),
                        );
                    }
                    body = body.child(
                        div()
                            .id("template-save-submit")
                            .map(|button| action_style(button, colors, true))
                            .child("Save template")
                            .on_click(cx.listener(|this, _, _, cx| this.template_save(cx))),
                    );
                }
                TemplateMode::Overwrite { request, slug } => {
                    let summary = if let TemplateApiRequest::Save {
                        name,
                        included_commands,
                        ..
                    } = request
                    {
                        format!(
                            "Replace saved template {slug} with {name}? {} commands included.",
                            included_commands.len()
                        )
                    } else {
                        String::new()
                    };
                    body = body.child(summary).child(
                        div()
                            .id("template-overwrite-confirm")
                            .map(|button| action_style(button, colors, true))
                            .child("Replace saved template")
                            .on_click(cx.listener(|this, _, _, cx| this.template_overwrite(cx))),
                    );
                }
                TemplateMode::Apply {
                    draft,
                    root,
                    target,
                } => {
                    let terminals = draft.template().panes.len();
                    let direct_layout = *target == TargetChoice::NewScreen
                        && draft
                            .template()
                            .panes
                            .values()
                            .all(|pane| pane.command.trim().is_empty());
                    body = body.child(format!("{} · {terminals} terminal panes", draft.name()));
                    let current = *target == TargetChoice::CurrentScreen;
                    body = body
                        .child(
                            div()
                                .id("template-target-new")
                                .map(|row| row_style(row, colors, !current))
                                .child("New Screen")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.template_select_target(TargetChoice::NewScreen, cx)
                                })),
                        )
                        .child(
                            div()
                                .id("template-target-current")
                                .map(|row| row_style(row, colors, current))
                                .child("Replace current Screen")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.template_select_target(TargetChoice::CurrentScreen, cx)
                                })),
                        )
                        .child(field_label(
                            colors,
                            "Root directory on the terminal host (optional)",
                        ))
                        .child(
                            div()
                                .id("template-apply-root")
                                .map(|field| field_box(field, colors))
                                .child(Input::new(root)),
                        )
                        .child(
                            div()
                                .id("template-prepare-review")
                                .map(|button| action_style(button, colors, true))
                                .child(if direct_layout {
                                    "Apply layout"
                                } else {
                                    "Review application"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.template_prepare_review(window, cx)
                                })),
                        );
                }
                TemplateMode::Review { plan, impact, .. } => {
                    body = body.child(format!(
                        "Target: {} · Root: {}",
                        plan.display_name(),
                        if plan.root_directory().is_empty() {
                            "service default"
                        } else {
                            plan.root_directory()
                        }
                    ));
                    if let Some(replaced) = plan.replacement_impact() {
                        let ends = match impact {
                            CloseImpact::Pending => "Checking what runs there…".to_owned(),
                            CloseImpact::Unknown => {
                                "This service does not say what runs there.".to_owned()
                            }
                            CloseImpact::Assessed(assessment) if !assessment.confirm => {
                                "Nothing runs there.".to_owned()
                            }
                            CloseImpact::Assessed(assessment) => impact_detail(assessment),
                        };
                        let loading = if replaced.unresolved_count == 0 {
                            ""
                        } else {
                            " Session state is still loading."
                        };
                        body = body.child(format!(
                            "Replacing this Screen ends all its terminal sessions (including hidden and floating). {ends}{loading} Layout only also ends them.",
                        ));
                    }
                    let commands: Vec<_> = plan
                        .projection()
                        .pane_names
                        .iter()
                        .filter_map(|name| {
                            plan.draft()
                                .template()
                                .panes
                                .get(name)
                                .filter(|pane| !pane.command.trim().is_empty())
                                .map(|pane| (name, pane))
                        })
                        .collect();
                    if commands.is_empty() {
                        body = body.child("No commands in this template.");
                    } else {
                        body = body.child("Commands to run if approved:");
                        for (name, pane) in commands {
                            body = body.child(div().px_2().py_1().bg(rgb(colors.overlay))
                            .child(format!("{name} · command: {} · directory: {} · profile: {} · restore: {}",
                                pane.command,
                                if pane.dir.is_empty() { "default" } else { &pane.dir },
                                if pane.shell_profile.is_empty() { "default" } else { &pane.shell_profile },
                                if pane.restore_mode.is_empty() { "replay (default)" } else { &pane.restore_mode })));
                        }
                    }
                    let run_enabled = !plan.projection().pane_names.iter().all(|name| {
                        plan.draft()
                            .template()
                            .panes
                            .get(name)
                            .is_none_or(|pane| pane.command.trim().is_empty())
                    });
                    body = body.child(
                        div()
                            .id("template-layout-only")
                            .map(|button| action_style(button, colors, true))
                            .child("Layout only (no commands)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.template_apply(TemplateApplyChoice::LayoutOnly, window, cx)
                            })),
                    );
                    if run_enabled {
                        body = body.child(
                            div()
                                .id("template-run-commands")
                                .map(|button| action_style(button, colors, false))
                                .child("Run displayed commands")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.template_apply(
                                        TemplateApplyChoice::RunCommands,
                                        window,
                                        cx,
                                    )
                                })),
                        );
                    }
                }
                TemplateMode::Delete { slug, name } => {
                    body = body
                        .child(format!("Delete saved template {name} ({slug})?"))
                        .child(
                            div()
                                .id("template-delete-confirm")
                                .map(|button| action_style(button, colors, true))
                                .child("Delete this template")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.template_delete_submit(cx)),
                                ),
                        );
                }
            }
        }
        if let Some(error) = &dialog.error {
            body = body.child(div().text_color(rgb(colors.danger)).child(error.clone()));
        }
        let weak_for_dismiss = weak.clone();
        let show_back = !matches!(&dialog.mode, TemplateMode::Library) && dialog.pending.is_none();
        let panel = div()
            .id("template-modal")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(colors.raised))
            .text_color(rgb(colors.text))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors.subtle_border))
                    .child(
                        div()
                            .text_size(chrome_rems(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(show_back, |header| {
                                header.child(
                                    Button::new("template-back")
                                        .focus_ring(focus_visible(cx))
                                        .label("Back")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.template_library(window, cx)
                                        })),
                                )
                            })
                            .child(
                                Button::new("template-cancel")
                                    .focus_ring(focus_visible(cx))
                                    .label("Close")
                                    .ghost()
                                    .small()
                                    .on_click(move |_, window, cx| {
                                        let _ = weak_for_dismiss.update(cx, |this, cx| {
                                            this.dismiss_templates(window, cx)
                                        });
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .id("template-modal-scroll")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .track_scroll(&dialog.scroll)
                    .p_4()
                    .child(body),
            );
        Some(panel.into_any_element())
    }

    pub(super) fn render_template_node(
        &self,
        node: &LayoutNode,
        preview: &PendingTemplateProjection,
    ) -> AnyElement {
        match node {
            LayoutNode::Slot { pane_ids, .. } => {
                let selected = preview.selected_pane(node);
                let mut header = div()
                    .flex()
                    .gap_1()
                    .h(self.tab_header_height())
                    .overflow_hidden()
                    .bg(rgb(self.appearance.raised));
                for id in pane_ids {
                    let name = preview.pane_name(id.as_str()).unwrap_or("Terminal");
                    header = header.child(
                        div()
                            .flex_none()
                            .px_2()
                            .py_1()
                            .bg(rgb(if selected == Some(id.as_str()) {
                                self.appearance.active
                            } else {
                                self.appearance.raised
                            }))
                            .child(name.to_owned()),
                    );
                }
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .border_1()
                    .border_color(rgb(self.appearance.border))
                    .child(header)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgb(self.appearance.surface))
                            .text_color(rgb(self.appearance.secondary))
                            .child(format!(
                                "Pending terminal · {}",
                                selected
                                    .and_then(|id| preview.pane_name(id))
                                    .unwrap_or("Starting")
                            )),
                    )
                    .into_any_element()
            }
            LayoutNode::Split {
                axis,
                children,
                sizes,
                ..
            } => {
                let horizontal = *axis == Axis::Horizontal;
                let mut split = div()
                    .flex()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .when(!horizontal, |view| view.flex_col());
                for (index, child) in children.iter().enumerate() {
                    let ratio = sizes
                        .get(index)
                        .copied()
                        .filter(|size| size.is_finite() && *size > 0.)
                        .unwrap_or(1.);
                    split = split.child(
                        div()
                            .flex_basis(px(0.))
                            .flex_grow(ratio as f32)
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.render_template_node(child, preview)),
                    );
                    if index + 1 < children.len() {
                        split = split.child(
                            div()
                                .flex_none()
                                .bg(rgb(self.appearance.border))
                                .when(horizontal, |view| view.w(chrome_rems(5.)).h_full())
                                .when(!horizontal, |view| view.h(chrome_rems(5.)).w_full()),
                        );
                    }
                }
                split.into_any_element()
            }
        }
    }

    pub(super) fn render_template_preview(
        &self,
        preview: &PendingTemplateProjection,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity();
        let mut viewport = div()
            .id("template-pending-viewport")
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .p_1()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, cx| {
                            if this.float_viewport != Some(bounds) {
                                this.float_viewport = Some(bounds);
                                cx.notify();
                            }
                        })
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(self.render_template_node(preview.root(), preview));
        if let Some(bounds) = self.float_viewport {
            for entry in preview.floating() {
                let projected =
                    floating::project(&entry.bounds, bounds, self.appearance.zoom.window_scale());
                viewport = viewport.child(
                    div()
                        .id(SharedString::from(format!(
                            "template-pending-float-{}",
                            entry.id
                        )))
                        .absolute()
                        .left(projected.left() - bounds.left())
                        .top(projected.top() - bounds.top())
                        .w(projected.size.width)
                        .h(projected.size.height)
                        .flex()
                        .flex_col()
                        .occlude()
                        .overflow_hidden()
                        .border_2()
                        .border_color(rgb(self.appearance.accent))
                        .bg(rgb(self.appearance.surface))
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .bg(rgb(self.appearance.raised))
                                .child("Pending floating group"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .min_w_0()
                                .child(self.render_template_node(&entry.root, preview)),
                        ),
                );
            }
        }
        viewport.into_any_element()
    }
}

/// A button of the template panel; the primary one carries the accent.
fn action_style<E: Styled + StatefulInteractiveElement>(
    button: E,
    colors: &NativeAppearance,
    primary: bool,
) -> E {
    let hover: Hsla = rgba(colors.hover_overlay).into();
    button
        .h(chrome_rems(28.))
        .px(chrome_rems(12.))
        .mt(chrome_rems(4.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(chrome_rems(5.))
        .border_1()
        .border_color(rgb(if primary {
            colors.accent
        } else {
            colors.subtle_border
        }))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(if primary {
            colors.accent
        } else {
            colors.secondary
        }))
        .cursor_pointer()
        .hover(move |button| button.bg(hover))
}

/// A row to choose: a saved template, a target or a command.
fn row_style<E: Styled + StatefulInteractiveElement>(
    row: E,
    colors: &NativeAppearance,
    chosen: bool,
) -> E {
    let hover: Hsla = rgb(colors.hover).into();
    row.px(chrome_rems(10.))
        .py(chrome_rems(7.))
        .rounded(chrome_rems(4.))
        .cursor_pointer()
        .bg(if chosen {
            rgb(colors.active).into()
        } else {
            gpui::transparent_black()
        })
        .text_color(rgb(if chosen {
            colors.text
        } else {
            colors.secondary
        }))
        .hover(move |row| row.bg(hover))
}

/// A quiet action beside a row.
fn quiet_style<E: Styled + StatefulInteractiveElement>(button: E, colors: &NativeAppearance) -> E {
    let danger: Hsla = rgb(colors.danger).into();
    button
        .px(chrome_rems(8.))
        .py(chrome_rems(5.))
        .rounded(chrome_rems(4.))
        .text_size(chrome_rems(11.))
        .text_color(rgb(colors.muted))
        .cursor_pointer()
        .hover(move |button| button.text_color(danger))
}

/// The frame of a text field.
fn field_box<E: Styled>(field: E, colors: &NativeAppearance) -> E {
    field
        .h(chrome_rems(30.))
        .px(chrome_rems(4.))
        .flex()
        .items_center()
        .rounded(chrome_rems(5.))
        .border_1()
        .border_color(rgb(colors.subtle_border))
        .bg(rgb(colors.surface))
}

fn field_label(colors: &NativeAppearance, text: &'static str) -> Div {
    div()
        .mt(chrome_rems(4.))
        .text_size(chrome_rems(11.))
        .text_color(rgb(colors.secondary))
        .child(text)
}

#[cfg(test)]
mod tests {
    use super::{modal_key, ModalKey, TemplateDialog, TemplateMode};
    use gpui_kit::ScrollHandle;
    use splitty_client_core::templates::TemplateApiRequest;
    use uuid::Uuid;

    #[test]
    fn modal_consumes_review_and_pending_keys_but_passes_editor_text() {
        assert_eq!(modal_key("enter", false, false), ModalKey::Consume);
        assert_eq!(modal_key("p", false, false), ModalKey::Consume);
        assert_eq!(modal_key("p", true, false), ModalKey::PassToInput);
        assert_eq!(modal_key("p", true, true), ModalKey::Consume);
        assert_eq!(modal_key("escape", true, true), ModalKey::Dismiss);
    }

    #[test]
    fn dismissed_or_superseded_library_reply_cannot_change_dialog() {
        let owner = Uuid::new_v4();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut dialog = TemplateDialog {
            owner,
            source: splitty_client_core::workspace::SourceWorkspace::new(owner).source_token(),
            mode: TemplateMode::Library,
            catalog: None,
            pending: Some((second, TemplateApiRequest::List)),
            error: None,
            scroll: ScrollHandle::new(),
        };
        assert!(dialog.accept(owner, first).is_none());
        assert!(dialog.accept(Uuid::new_v4(), second).is_none());
        assert!(matches!(
            dialog.accept(owner, second),
            Some(TemplateApiRequest::List)
        ));
        assert!(dialog.accept(owner, second).is_none());
    }
}
