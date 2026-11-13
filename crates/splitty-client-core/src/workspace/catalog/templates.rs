//! Captured template replacement policy and session-free pending presentation.
use super::*;
use crate::templates::TemplateDraft;
use splitty_transport::budget::Lease;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug)]
pub enum TemplateApplyTarget {
    Current { screen_id: String },
    NewScreen,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateApplyChoice {
    RunCommands,
    LayoutOnly,
}
#[derive(Debug)]
pub struct TemplateApplyPlan {
    source_id: SourceId,
    connection: Uuid,
    request_id: Uuid,
    screen_id: String,
    display_name: String,
    draft: Arc<TemplateDraft>,
    request: mux::ApplyTemplate,
    projection: mux::TemplateProjection,
    impact: Option<ClosePlan>,
    baseline_revision: Option<u64>,
    consumed: AtomicBool,
    _lease: Lease,
}
impl TemplateApplyPlan {
    pub fn source_token(&self) -> SourceToken {
        SourceToken::new(self.source_id, self.connection)
    }
    pub fn connection_id(&self) -> Uuid {
        self.connection
    }
    pub fn request_id(&self) -> Uuid {
        self.request_id
    }
    pub fn screen_id(&self) -> &str {
        &self.screen_id
    }
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
    pub fn draft(&self) -> &TemplateDraft {
        &self.draft
    }
    pub fn replacement_impact(&self) -> Option<&ClosePlan> {
        self.impact.as_ref()
    }
    pub fn root_directory(&self) -> &str {
        &self.request.root
    }
    pub fn projection(&self) -> &mux::TemplateProjection {
        &self.projection
    }
    pub fn pane_name(&self, id: &str) -> Option<&str> {
        self.request
            .pane_ids
            .iter()
            .find_map(|(name, pane)| (pane.as_str() == id).then_some(name.as_str()))
    }
}
#[derive(Clone, Debug)]
pub struct PendingTemplateProjection {
    plan: Arc<TemplateApplyPlan>,
}
impl PendingTemplateProjection {
    pub fn connection_id(&self) -> Uuid {
        self.plan.connection_id()
    }
    pub fn request_id(&self) -> Uuid {
        self.plan.request_id()
    }
    pub fn screen_id(&self) -> &str {
        self.plan.screen_id()
    }
    pub fn display_name(&self) -> &str {
        self.plan.display_name()
    }
    pub fn root(&self) -> &LayoutNode {
        &self.plan.projection.root
    }
    pub fn floating(&self) -> &[FloatingEntry] {
        &self.plan.projection.floating
    }
    pub fn pane_name(&self, id: &str) -> Option<&str> {
        self.plan.pane_name(id)
    }
    pub fn selected_pane<'a>(&self, node: &'a LayoutNode) -> Option<&'a str> {
        if let LayoutNode::Slot {
            pane_ids,
            active_pane_id,
            ..
        } = node
        {
            active_pane_id
                .as_ref()
                .or_else(|| pane_ids.first())
                .map(PaneId::as_str)
        } else {
            None
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLaunchError {
    pub pane_id: Option<String>,
    pub reason: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateProfileFallback {
    pub pane_id: String,
    pub requested_profile_id: String,
    pub effective_profile_id: String,
    pub reason: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateApplyResult {
    pub connection_id: Uuid,
    pub request_id: Uuid,
    pub outcome: TemplateApplyOutcome,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateApplyOutcome {
    Confirmed {
        launch_errors: Vec<TemplateLaunchError>,
        profile_fallbacks: Vec<TemplateProfileFallback>,
    },
    Rejected {
        reason: String,
    },
    Unverified {
        reason: String,
    },
}
#[derive(Clone)]
pub(super) struct PendingTemplate {
    pub(super) preview: PendingTemplateProjection,
    mutation: u64,
    follow: bool,
    errors: Vec<TemplateLaunchError>,
    spawned: HashMap<String, String>,
    fallbacks: HashMap<String, (String, TemplateProfileFallback)>,
}
impl SourceWorkspace {
    pub(super) fn template_reserved_admission(&self) -> Option<(&str, usize)> {
        let pending = self.pending_template.as_ref()?;
        Some((
            pending.preview.screen_id(),
            pending.preview.plan.projection.pane_ids().count(),
        ))
    }
    pub fn templates_supported(&self) -> bool {
        self.templates_supported
    }
    pub(crate) fn set_templates_supported(&mut self, value: bool) {
        self.templates_supported = value;
    }
    /// Whether this connection's service opens viewer panes (`viewer-v1`).
    pub fn viewers_supported(&self) -> bool {
        self.viewers_supported
    }
    pub(crate) fn set_viewers_supported(&mut self, value: bool) {
        self.viewers_supported = value;
    }
    /// Whether this connection's service keeps annotations and anchors and
    /// reviews them (`viewer-annotations-v1` and `viewer-reviews-v1`).
    pub fn reviews_supported(&self) -> bool {
        self.reviews_supported
    }
    pub(crate) fn set_reviews_supported(&mut self, value: bool) {
        self.reviews_supported = value;
    }
    pub fn template_apply_pending(&self) -> bool {
        self.pending_template.is_some()
    }
    pub fn pending_template(&self) -> Option<&PendingTemplateProjection> {
        self.pending_template
            .as_ref()
            .filter(|p| p.follow)
            .map(|p| &p.preview)
    }
    pub(super) fn cancel_template_follow(&mut self) {
        if let Some(p) = &mut self.pending_template {
            p.follow = false;
        }
    }
    pub(crate) fn template_request_pending(&self, id: Uuid) -> bool {
        self.pending_template
            .as_ref()
            .is_some_and(|p| p.preview.request_id() == id)
    }
    fn require_template_idle(&self) -> io::Result<()> {
        self.require_catalog()?;
        self.require_routes_settled()?;
        if !self.templates_supported {
            return Err(invalid("This service does not support templates"));
        }
        if !self.catalog_initialized
            || self.pending_template.is_some()
            || self.pending_placement.is_some()
            || self.pending_transfer.is_some()
            || !self.pending_screens.is_empty()
            || self
                .models
                .values()
                .any(|m| m.pending_move.is_some() || !m.pending_panes.is_empty())
        {
            return Err(invalid("A conflicting workspace operation is pending"));
        }
        Ok(())
    }
    pub fn prepare_template_apply(
        &self,
        request_id: Uuid,
        draft: Arc<TemplateDraft>,
        target: TemplateApplyTarget,
        root: String,
    ) -> io::Result<TemplateApplyPlan> {
        self.require_template_idle()?;
        // Templates are kept by the local service; a host is sent the whole
        // template with the request.
        if (self.kind() == SourceKind::Local && draft.connection_id() != self.connection_id)
            || root.len() > 4096
        {
            return Err(invalid("Template source changed or root exceeds limit"));
        }
        let lease = draft.reserve_copy()?;
        let (screen_id, impact, create_screen) = match target {
            TemplateApplyTarget::Current { screen_id } => {
                if self.shown_screen_id() != Some(screen_id.as_str()) {
                    return Err(invalid("Template replacement Screen is no longer shown"));
                }
                let impact = self.assess_close(
                    0,
                    CloseTarget::Screen {
                        screen: screen_id.clone(),
                    },
                )?;
                (screen_id, Some(impact), false)
            }
            TemplateApplyTarget::NewScreen => (Uuid::new_v4().to_string(), None, true),
        };
        let pane_ids = draft
            .template()
            .panes
            .keys()
            .map(|name| (name.clone(), PaneId::from(Uuid::new_v4().to_string())))
            .collect();
        let request = mux::ApplyTemplate {
            template: draft.template().clone(),
            pane_ids,
            root,
            create_screen,
            replace_confirmed: false,
        };
        let projection = mux::project_template(&request).map_err(io::Error::other)?;
        let display_name = if create_screen {
            draft.name().to_owned()
        } else {
            self.screen(&screen_id)
                .and_then(ScreenModel::snapshot)
                .and_then(|s| s.name.clone())
                .unwrap_or_else(|| draft.name().to_owned())
        };
        let baseline_revision = self
            .screen(&screen_id)
            .and_then(ScreenModel::snapshot)
            .map(|s| s.revision);
        let plan = TemplateApplyPlan {
            source_id: self.source_id,
            connection: self.connection_id,
            request_id,
            screen_id,
            display_name,
            draft,
            request,
            projection,
            baseline_revision,
            impact,
            consumed: AtomicBool::new(false),
            _lease: lease,
        };
        self.check_template_capacity(&plan)?;
        Ok(plan)
    }
    fn check_template_capacity(&self, plan: &TemplateApplyPlan) -> io::Result<()> {
        if plan.request.create_screen && self.screen_count() >= self.limits.screens {
            return Err(capacity("Native Screen limit reached"));
        }
        let other: usize = self
            .models
            .iter()
            .filter(|(id, _)| plan.request.create_screen || id.as_str() != plan.screen_id)
            .map(|(_, model)| {
                model
                    .snapshot()
                    .map_or(0, |s| s.panes.len().max(s.sessions.len()))
                    + model.pending_panes.len()
            })
            .sum();
        if other + plan.projection.pane_ids().count() > self.limits.panes {
            return Err(capacity("Template exceeds workspace terminal capacity"));
        }
        Ok(())
    }
    pub(crate) fn apply_template(
        &mut self,
        client: &Client,
        plan: Arc<TemplateApplyPlan>,
        choice: TemplateApplyChoice,
        replace_confirmed: bool,
    ) -> io::Result<()> {
        self.check_connection(client)?;
        self.apply_template_with(plan, choice, replace_confirmed, |screen, request| {
            client.submit_template(screen, request)
        })
    }
    fn apply_template_with(
        &mut self,
        plan: Arc<TemplateApplyPlan>,
        choice: TemplateApplyChoice,
        replace_confirmed: bool,
        send: impl FnOnce(&str, &mux::ApplyTemplate) -> io::Result<()>,
    ) -> io::Result<()> {
        self.require_template_idle()?;
        if plan.source_token() != self.source_token() || plan.consumed.load(Ordering::Acquire) {
            return Err(invalid(
                "Template review was already used or belongs to a previous connection",
            ));
        }
        if let Some(impact) = &plan.impact {
            if !replace_confirmed || self.shown_screen_id() != Some(plan.screen_id()) {
                return Err(invalid(
                    "Replacing terminals requires current explicit confirmation",
                ));
            }
            self.validate_close(impact, true)?;
        } else if self.models.contains_key(plan.screen_id()) {
            return Err(invalid("Template destination already exists"));
        }
        self.check_template_capacity(&plan)?;
        let mutation = self.next_mutation()?;
        mutation
            .checked_add(2)
            .ok_or_else(|| invalid("Workspace generation exhausted"))?;
        // A transient charged command copy keeps the captured review immutable.
        let _scratch = plan.draft.reserve_copy()?;
        let mut request = plan.request.clone();
        // Checked above: an existing Screen is replaced only after the review.
        request.replace_confirmed = plan.impact.is_some();
        if choice == TemplateApplyChoice::LayoutOnly {
            request.template.retain_commands(|_| false);
        }
        send(plan.screen_id(), &request)?;
        plan.consumed.store(true, Ordering::Release);
        self.mutation = mutation;
        self.catalog_dirty = true;
        if self.kind() == SourceKind::Remote && plan.request.create_screen {
            // A window shows only the Screens it opened: hold a place for the
            // one the host is about to make, so its refresh reads it too.
            self.catalog.push(ScreenInfo {
                id: plan.screen_id().into(),
                name: None,
            });
        }
        self.pending_template = Some(PendingTemplate {
            preview: PendingTemplateProjection { plan },
            mutation,
            follow: true,
            errors: Vec::new(),
            spawned: HashMap::new(),
            fallbacks: HashMap::new(),
        });
        Ok(())
    }
    pub(crate) fn observe_template_spawn(
        &mut self,
        connection: Uuid,
        pane: &str,
        session: &str,
        event: &Value,
    ) {
        if connection != self.connection_id {
            return;
        }
        let Some(p) = &mut self.pending_template else {
            return;
        };
        let Some(name) = p.preview.pane_name(pane) else {
            return;
        };
        let requested = p.preview.plan.request.template.panes[name]
            .shell_profile
            .clone();
        if crate::connection::identifier(session).is_err() {
            return;
        }
        p.spawned.insert(pane.to_owned(), session.to_owned());
        let (Some(reason), Some(effective)) = (
            event["shellProfileFallbackReason"].as_str(),
            event["shellProfileId"].as_str(),
        ) else {
            return;
        };
        if !matches!(reason, "missing" | "disabled" | "unavailable")
            || effective.len() > crate::shell_profiles::MAX_PROFILE_ID_BYTES
        {
            return;
        }
        p.fallbacks.insert(
            pane.to_owned(),
            (
                session.to_owned(),
                TemplateProfileFallback {
                    pane_id: pane.into(),
                    requested_profile_id: requested,
                    effective_profile_id: effective.into(),
                    reason: reason.into(),
                },
            ),
        );
    }
    pub fn template_launch_error(&self, pane: &str) -> Option<&str> {
        self.template_failures
            .get(pane)
            .filter(|(session, _)| self.session_for(pane) == Some(session.as_str()))
            .map(|(_, reason)| reason.as_str())
    }
    pub(crate) fn failed_template_sessions(&self) -> impl Iterator<Item = &str> {
        self.template_failures
            .iter()
            .filter(|(pane, (session, _))| self.session_for(pane) == Some(session.as_str()))
            .map(|(_, (session, _))| session.as_str())
    }
    pub(crate) fn observe_template_failure(&mut self, connection: Uuid, event: &Value) {
        if connection != self.connection_id
            || !MessageType::MuxOpFailed.is(event)
            || event["op"] != "apply_template"
        {
            return;
        }
        let Some(p) = &mut self.pending_template else {
            return;
        };
        if event["screenId"] != p.preview.screen_id() {
            return;
        }
        let pane = event["paneId"].as_str();
        if pane.is_some_and(|id| p.preview.pane_name(id).is_none()) {
            return;
        }
        if p.errors.len() > MAX_ATTACHMENTS
            || p.errors
                .iter()
                .any(|error| error.pane_id.as_deref() == pane)
        {
            return;
        }
        let reason = event["error"]
            .as_str()
            .unwrap_or("Template launch or effect failed");
        let reason: String = reason.chars().take(512).collect();
        p.errors.push(TemplateLaunchError {
            pane_id: pane.map(str::to_owned),
            reason,
        });
    }
    pub(super) fn settle_template_after_refresh(
        &mut self,
        watermark: u64,
    ) -> Option<TemplateApplyResult> {
        if watermark < self.pending_template.as_ref()?.mutation {
            return None;
        }
        self.finish_template(None)
    }
    pub(crate) fn fail_template_observation(
        &mut self,
        reason: String,
    ) -> Option<TemplateApplyResult> {
        self.finish_template(Some(reason))
    }
    pub(super) fn finish_template(&mut self, error: Option<String>) -> Option<TemplateApplyResult> {
        let mut pending = self.pending_template.take()?;
        let plan = &pending.preview.plan;
        let actual = self
            .models
            .get(plan.screen_id())
            .and_then(ScreenModel::snapshot);
        let confirmed = actual.is_some_and(|screen| {
            same_layout(&screen.root, &plan.projection.root)
                && screen.floating.len() == plan.projection.floating.len()
                && screen
                    .floating
                    .iter()
                    .zip(&plan.projection.floating)
                    .all(|(a, b)| {
                        a.id == b.id && a.bounds == b.bounds && same_layout(&a.root, &b.root)
                    })
                && plan
                    .projection
                    .pane_ids()
                    .all(|pane| screen.panes.contains_key(&pane))
        });
        let unchanged = actual
            .is_some_and(|screen| Some(screen.revision) == plan.baseline_revision)
            && plan.impact.as_ref().is_some_and(|before| {
                self.assess_close(0, before.target.clone())
                    .is_ok_and(|now| {
                        now.affected
                            .iter()
                            .map(|a| &a.identity)
                            .eq(before.affected.iter().map(|a| &a.identity))
                    })
            });
        let routes = self.route_map();
        self.template_failures
            .retain(|pane, (session, _)| routes.get(pane) == Some(session));
        if confirmed && error.is_none() {
            for pane in plan.projection.pane_ids() {
                if pending.spawned.get(pane.as_str()).map(String::as_str)
                    != self.session_for(pane.as_str())
                {
                    let reason = pending
                        .errors
                        .iter()
                        .find(|e| e.pane_id.as_deref() == Some(pane.as_str()))
                        .map(|e| e.reason.clone())
                        .unwrap_or_else(|| {
                            "Terminal launch was not confirmed by the service".into()
                        });
                    if !pending
                        .errors
                        .iter()
                        .any(|e| e.pane_id.as_deref() == Some(pane.as_str()))
                    {
                        pending.errors.push(TemplateLaunchError {
                            pane_id: Some(pane.to_string()),
                            reason: reason.clone(),
                        });
                    }
                    if let Some(session) = self.session_for(pane.as_str()) {
                        self.template_failures
                            .insert(pane.to_string(), (session.to_owned(), reason));
                    }
                }
            }
        }
        let outcome = if let Some(reason) = error {
            TemplateApplyOutcome::Unverified { reason }
        } else if confirmed {
            if pending.follow {
                self.shown = Some(plan.screen_id.clone());
                if let Some(model) = self.models.get_mut(plan.screen_id()) {
                    model.maximized_slot = None;
                    if let Some(pane) = plan.projection.pane_ids().next() {
                        model.select_tab(pane.as_str());
                    }
                }
            }
            TemplateApplyOutcome::Confirmed {
                launch_errors: pending.errors,
                profile_fallbacks: pending
                    .fallbacks
                    .into_iter()
                    .filter(|(pane, (session, _))| self.session_for(pane) == Some(session.as_str()))
                    .map(|(_, (_, notice))| notice)
                    .collect(),
            }
        } else if unchanged && pending.errors.iter().any(|e| e.pane_id.is_none()) {
            TemplateApplyOutcome::Rejected {
                reason: "Service rejected the template and the captured Screen remains unchanged"
                    .into(),
            }
        } else {
            TemplateApplyOutcome::Unverified{reason:"Expected template layout was not observed; it may have changed after application. No retry was sent".into()}
        };
        if self.kind() == SourceKind::Remote
            && plan.request.create_screen
            && !matches!(outcome, TemplateApplyOutcome::Confirmed { .. })
            && self
                .live_catalog()
                .is_none_or(|live| !live.iter().any(|info| info.id.as_str() == plan.screen_id()))
        {
            // The host never made the Screen; its place goes again.
            self.catalog
                .retain(|info| info.id.as_str() != plan.screen_id());
            self.models.remove(plan.screen_id());
        }
        Some(TemplateApplyResult {
            connection_id: self.connection_id,
            request_id: plan.request_id,
            outcome,
        })
    }
}
#[cfg(test)]
mod tests;
