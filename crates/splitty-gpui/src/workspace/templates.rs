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
    Save {
        draft: Arc<TemplateDraft>,
        name: Entity<InputState>,
        slug: Entity<InputState>,
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
    },
    Delete {
        slug: String,
        name: String,
    },
}

pub(super) struct TemplateDialog {
    owner: Uuid,
    mode: TemplateMode,
    catalog: Option<Arc<TemplateCatalogSnapshot>>,
    pending: Option<(Uuid, TemplateApiRequest)>,
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
            TemplateMode::Extract { name, root, .. }
            | TemplateMode::Save {
                name, slug: root, ..
            } => focused(name) || focused(root),
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

fn slug_from_name(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if matches!(character, ' ' | '-' | '_') && !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() == 128 {
            break;
        }
    }
    slug.trim_end_matches('-').to_owned()
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
        let Some(owner) = self.model.as_ref().map(LocalWorkspace::connection_id) else {
            self.status = "Templates are unavailable while the workspace starts".into();
            cx.notify();
            return;
        };
        if cx.has_active_drag() {
            cx.stop_active_drag(window);
        }
        self.drag = None;
        self.float_gesture = None;
        self.drop_target = None;
        self.transfer_menu = None;
        self.profile_picker = None;
        for pane in &self.visible {
            if let Some(view) = self.panes.get(pane) {
                view.terminal
                    .update(cx, |terminal, _| terminal.cancel_pointer());
            }
        }
        self.template_dialog = Some(TemplateDialog {
            owner,
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
        let request_id = Uuid::new_v4();
        match self.commands.try_send(Command::TemplateApi {
            connection: dialog.owner,
            request_id,
            request: request.clone(),
        }) {
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
            .is_some_and(|model| model.connection_id() == result.connection_id)
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
                if !matches!(request.as_ref(), Some(TemplateApiRequest::Extract { .. } | TemplateApiRequest::Load { .. })) {
                    dialog.error = Some("Unexpected template draft reply".into());
                    cx.notify();
                    return;
                }
                let name = cx.new(|cx| InputState::new(window, cx).default_value(draft.name().to_owned()));
                let slug_text = slug_from_name(draft.name());
                let slug = cx.new(|cx| InputState::new(window, cx).default_value(slug_text));
                let commands = names_with_commands(&draft).into_iter().collect();
                dialog.mode = if matches!(request.as_ref(), Some(TemplateApiRequest::Extract { .. })) {
                    TemplateMode::Save {
                        draft,
                        name: name.clone(),
                        slug,
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
                if let Some(request) = request.filter(|request| {
                    matches!(request, TemplateApiRequest::Save { slug: original, .. } if original == &slug)
                }) {
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
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let Some(screen) = model.shown_screen_id() else {
            self.status = "Select a Screen to save".into();
            cx.notify();
            return;
        };
        let name_text = model
            .screens()
            .iter()
            .find(|info| info.id.as_str() == screen)
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
        if !self
            .model
            .as_ref()
            .is_some_and(|model| model.shown_screen_id() == Some(screen_id.as_str()))
        {
            if let Some(dialog) = &mut self.template_dialog {
                dialog.error = Some("Source Screen changed; reopen Save".into());
            }
            cx.notify();
            return;
        }
        if name.is_empty() {
            if let Some(dialog) = &mut self.template_dialog {
                dialog.error = Some("Enter a template name".into());
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
        let Some((draft, slug, name, included_commands)) =
            self.template_dialog.as_ref().and_then(|dialog| {
                if let TemplateMode::Save {
                    draft,
                    name,
                    slug,
                    commands,
                } = &dialog.mode
                {
                    Some((
                        draft.clone(),
                        slug.read(cx).value().trim().to_owned(),
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
                slug,
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
            slug,
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
                slug,
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
        let Some(model) = &self.model else {
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
        if let Some(dialog) = &mut self.template_dialog {
            match result {
                Ok(plan) => {
                    direct_layout = plan.replacement_impact().is_none()
                        && plan.projection().skipped_web_count == 0
                        && plan.projection().pane_names.iter().all(|name| {
                            plan.draft()
                                .template()
                                .panes
                                .get(name)
                                .is_none_or(|pane| pane.command.trim().is_empty())
                        });
                    dialog.mode = TemplateMode::Review {
                        plan: Arc::new(plan),
                    };
                    dialog.error = None;
                }
                Err(error) => dialog.error = Some(format!("Cannot apply: {error}")),
            }
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
        let TemplateMode::Review { plan } = &dialog.mode else {
            return;
        };
        if self.pending_template_apply.is_some() {
            return;
        }
        let plan = plan.clone();
        if !self
            .model
            .as_ref()
            .is_some_and(|model| model.connection_id() == plan.connection_id())
        {
            dialog.error = Some("Template review belongs to an earlier connection".into());
            cx.notify();
            return;
        }
        match self.commands.try_send(Command::ApplyTemplate {
            plan: plan.clone(),
            choice,
            replace_confirmed: plan.replacement_impact().is_some(),
        }) {
            Ok(()) => {
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.template_dialog.as_ref()?;
        let colors = &self.appearance;
        let weak = cx.entity().downgrade();
        let mut body = div().flex().flex_col().gap_2();
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
                                .px_3()
                                .py_2()
                                .bg(rgb(colors.active))
                                .cursor_pointer()
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
                                            .px_2()
                                            .py_1()
                                            .cursor_pointer()
                                            .bg(rgb(colors.overlay))
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
                                            .px_2()
                                            .py_1()
                                            .cursor_pointer()
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
                        .child("Name")
                        .child(
                            div()
                                .id("template-extract-name")
                                .h(chrome_rems(34.))
                                .child(Input::new(name)),
                        )
                        .child("Relative root on the terminal host (optional)")
                        .child(
                            div()
                                .id("template-extract-root")
                                .h(chrome_rems(34.))
                                .child(Input::new(root)),
                        )
                        .child(
                            div()
                                .id("template-extract-submit")
                                .px_3()
                                .py_2()
                                .bg(rgb(colors.active))
                                .cursor_pointer()
                                .child("Extract for review")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.template_extract_submit(cx)),
                                ),
                        );
                }
                TemplateMode::Save {
                    draft,
                    name,
                    slug,
                    commands,
                } => {
                    body = body
                        .child("Name")
                        .child(
                            div()
                                .id("template-save-name")
                                .h(chrome_rems(34.))
                                .child(Input::new(name)),
                        )
                        .child("File slug")
                        .child(
                            div()
                                .id("template-save-slug")
                                .h(chrome_rems(34.))
                                .child(Input::new(slug)),
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
                                .px_2()
                                .py_1()
                                .cursor_pointer()
                                .bg(rgb(colors.overlay))
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.template_toggle_command(&command_name, cx)
                                })),
                        );
                    }
                    body = body.child(
                        div()
                            .id("template-save-submit")
                            .px_3()
                            .py_2()
                            .bg(rgb(colors.active))
                            .cursor_pointer()
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
                            .px_3()
                            .py_2()
                            .bg(rgb(colors.active))
                            .cursor_pointer()
                            .child("Replace saved template")
                            .on_click(cx.listener(|this, _, _, cx| this.template_overwrite(cx))),
                    );
                }
                TemplateMode::Apply {
                    draft,
                    root,
                    target,
                } => {
                    let terminals = draft
                        .template()
                        .panes
                        .values()
                        .filter(|pane| pane.pane_type != "webview")
                        .count();
                    let direct_layout = *target == TargetChoice::NewScreen
                        && draft
                            .template()
                            .panes
                            .values()
                            .all(|pane| pane.command.trim().is_empty())
                        && draft
                            .template()
                            .panes
                            .values()
                            .all(|pane| pane.pane_type != "webview");
                    body = body.child(format!("{} · {terminals} terminal panes", draft.name()));
                    let current = *target == TargetChoice::CurrentScreen;
                    body = body
                        .child(
                            div()
                                .id("template-target-new")
                                .px_2()
                                .py_1()
                                .cursor_pointer()
                                .bg(rgb(if current {
                                    colors.overlay
                                } else {
                                    colors.active
                                }))
                                .child("New Screen")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.template_select_target(TargetChoice::NewScreen, cx)
                                })),
                        )
                        .child(
                            div()
                                .id("template-target-current")
                                .px_2()
                                .py_1()
                                .cursor_pointer()
                                .bg(rgb(if current {
                                    colors.active
                                } else {
                                    colors.overlay
                                }))
                                .child("Replace current Screen")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.template_select_target(TargetChoice::CurrentScreen, cx)
                                })),
                        )
                        .child("Root directory on the terminal host (optional)")
                        .child(
                            div()
                                .id("template-apply-root")
                                .h(chrome_rems(34.))
                                .child(Input::new(root)),
                        )
                        .child(
                            div()
                                .id("template-prepare-review")
                                .px_3()
                                .py_2()
                                .bg(rgb(colors.active))
                                .cursor_pointer()
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
                TemplateMode::Review { plan } => {
                    body = body.child(format!(
                        "Target: {} · Root: {}",
                        plan.display_name(),
                        if plan.root_directory().is_empty() {
                            "service default"
                        } else {
                            plan.root_directory()
                        }
                    ));
                    if let Some(impact) = plan.replacement_impact() {
                        body = body.child(format!(
                        "Replacing this Screen ends all its terminal sessions (including hidden and floating). Running: {} · Unresolved: {}. Layout only also ends them.",
                        impact.busy_count, impact.unresolved_count,
                    ));
                    }
                    if plan.projection().skipped_web_count != 0 {
                        body = body.child(format!(
                            "{} web panes will be skipped",
                            plan.projection().skipped_web_count
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
                            .px_3()
                            .py_2()
                            .bg(rgb(colors.active))
                            .cursor_pointer()
                            .child("Layout only (no commands)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.template_apply(TemplateApplyChoice::LayoutOnly, window, cx)
                            })),
                    );
                    if run_enabled {
                        body = body.child(
                            div()
                                .id("template-run-commands")
                                .px_3()
                                .py_2()
                                .bg(rgb(colors.overlay))
                                .cursor_pointer()
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
                                .px_3()
                                .py_2()
                                .bg(rgb(colors.active))
                                .cursor_pointer()
                                .child("Delete this template")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.template_delete_submit(cx)),
                                ),
                        );
                }
            }
        }
        if let Some(error) = &dialog.error {
            body = body.child(div().text_color(rgb(colors.accent)).child(error.clone()));
        }
        let weak_for_dismiss = weak.clone();
        let show_back = !matches!(&dialog.mode, TemplateMode::Library) && dialog.pending.is_none();
        let overlay = div()
            .id("template-modal")
            .absolute()
            .size_full()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000aa))
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .w(chrome_rems(700.))
                    .max_w(relative(0.94))
                    .max_h(px(f32::from(window.viewport_size().height) * 0.82))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .rounded_md()
                    .bg(rgb(colors.raised))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().font_weight(FontWeight::BOLD).child(title))
                            .when(show_back, |header| {
                                header.child(
                                    div()
                                        .id("template-back")
                                        .px_2()
                                        .py_1()
                                        .cursor_pointer()
                                        .child("Back to library")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.template_library(window, cx)
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .id("template-cancel")
                                    .px_2()
                                    .py_1()
                                    .cursor_pointer()
                                    .child("Cancel")
                                    .on_click(move |_, window, cx| {
                                        let _ = weak_for_dismiss.update(cx, |this, cx| {
                                            this.dismiss_templates(window, cx)
                                        });
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("template-modal-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&dialog.scroll)
                            .child(body),
                    ),
            );
        Some(overlay.into_any_element())
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

#[cfg(test)]
mod tests {
    use super::{modal_key, slug_from_name, ModalKey, TemplateDialog, TemplateMode};
    use gpui_kit::ScrollHandle;
    use splitty_client_core::templates::TemplateApiRequest;
    use uuid::Uuid;
    #[test]
    fn slug_is_bounded_and_stable() {
        assert_eq!(slug_from_name("My Session  2"), "my-session-2");
        assert_eq!(slug_from_name("  /  "), "");
        assert!(slug_from_name(&"a".repeat(512)).len() <= 128);
    }

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
