//! The one session driver shared by local and deliberately opened host sources.
//! Canonical models stay in WindowWorkspace; this owns only one Client's controls.
use super::*;
use crate::{
    workspace::{RemoteScreenCreateRequest, SourceId, SourceKind, WorkspaceDelta},
    Client,
};
use std::io;

#[derive(Default)]
pub(super) struct SourceEffects {
    pub observation_error: Option<String>,
    pub changed: bool,
    pub close_window: bool,
    pub select_created: Option<ScreenKey>,
    pub follow_remote_creation: Option<RemoteScreenCreateRequest>,
    pub updates: Vec<Update>,
}
impl SourceEffects {
    fn push(&mut self, update: Update) {
        self.updates.push(update);
    }
}
pub(super) struct SourceRuntime {
    pub client: Client,
    owner: SourceToken,
    pub welcomed: bool,
    workspace_refresh: WorkspaceRefresh,
    attached: HashSet<String>,
    visible: HashSet<String>,
    desired_visible: HashSet<String>,
    owned_sessions: HashSet<String>,
    closed_sessions: HashSet<String>,
    pending_hide: HashSet<String>,
    pending_resize: HashSet<String>,
    activity_pending: bool,
    sizes: HashMap<String, (u16, u16)>,
    refresh_error: Option<String>,
    control_error: Option<String>,
    last_close_request: u64,
    pending_close: Option<PendingClose>,
    select_created: Option<String>,
}
impl SourceRuntime {
    pub fn new(client: Client, owner: SourceToken) -> Self {
        Self {
            client,
            owner,
            welcomed: false,
            workspace_refresh: WorkspaceRefresh::default(),
            attached: HashSet::new(),
            visible: HashSet::new(),
            desired_visible: HashSet::new(),
            owned_sessions: HashSet::new(),
            closed_sessions: HashSet::new(),
            pending_hide: HashSet::new(),
            pending_resize: HashSet::new(),
            activity_pending: false,
            sizes: HashMap::new(),
            refresh_error: None,
            control_error: None,
            last_close_request: 0,
            pending_close: None,
            select_created: None,
        }
    }
    pub fn owner(&self) -> SourceToken {
        self.owner
    }
    pub fn id(&self) -> SourceId {
        self.owner.source_id()
    }
    pub fn withdraw_visibility(&mut self) {
        self.desired_visible.clear();
    }
    pub fn has_pending_close(&self) -> bool {
        self.pending_close.is_some()
    }
    pub fn cancel_created_follow(&mut self) {
        self.select_created = None;
    }
    pub fn begin_observation(&mut self, window: &mut WindowWorkspace) -> io::Result<()> {
        let mut model = window
            .source_mut(self.id())
            .ok_or_else(|| crate::connection::invalid("source is gone"))?;
        self.workspace_refresh.begin(&mut model)
    }
    pub fn observation_pending(&self, window: &WindowWorkspace) -> bool {
        window
            .source(self.id())
            .is_some_and(|model| self.workspace_refresh.observation_pending(model))
    }
    pub fn observation_active(&self) -> bool {
        self.workspace_refresh.observation_active()
    }
    pub fn abort_observation(
        &mut self,
        window: &mut WindowWorkspace,
        reason: &str,
    ) -> SourceEffects {
        let mut out = SourceEffects::default();
        if let Some(mut model) = window.source_mut(self.id()) {
            self.workspace_refresh
                .abort_observation(&mut model, reason.into());
            if let Some(close) = self.pending_close.take() {
                out.push(Update::CloseRejected {
                    source: close.source,
                    request_id: close.request_id,
                    reason: reason.into(),
                });
            }
            out.changed = true;
        }
        out
    }
    pub fn reject_pending_close_on_loss(&mut self) -> Option<Update> {
        self.pending_close.take().map(|close| Update::CloseRejected {
            source: close.source,
            request_id: close.request_id,
            reason: "Connection lost before close completion was confirmed; its outcome is uncertain".into(),
        })
    }
    pub fn command(
        &mut self,
        window: &mut WindowWorkspace,
        command: Command,
    ) -> anyhow::Result<SourceEffects> {
        let mut out = SourceEffects::default();
        if let CommandKind::CreateRemoteScreen { request } = &command.kind {
            // Identity precedes bounded deduplication; a retained UUID never
            // upgrades an old owner/attempt to the current connection.
            let identity = command.validate_source(self.owner).and_then(|()| {
                window.validate_remote_creation_owner(request.target(), request.source())
            });
            if identity.is_ok()
                && window
                    .source(self.id())
                    .is_some_and(|model| model.remote_creation_is_request(request))
            {
                out.changed = true;
                return Ok(out);
            }
            let validation = identity.and_then(|()| {
                if self.pending_close.is_some() {
                    return Err(crate::connection::invalid("A close operation is pending"));
                }
                window.validate_remote_screen_creation(request.target(), request.source())
            });
            if let Err(error) = validation {
                request.cancel_follow();
                out.push(rejected_command(
                    command.source,
                    &command.kind,
                    error.to_string(),
                ));
                return Ok(out);
            }
            // A new deliberate navigation supersedes prior creation follows across all sources.
            window.cancel_remote_creation_follow();
            let mut model = window.source_mut(self.id()).expect("validated source");
            match model.create_remote_screen(&self.client, request.clone()) {
                Ok(()) => {
                    if let Err(error) = self.workspace_refresh.begin(&mut model) {
                        model.remote_creation_uncertain(&error.to_string());
                    }
                    out.changed = true;
                }
                Err(error) => {
                    request.cancel_follow();
                    out.push(rejected_command(
                        command.source,
                        &command.kind,
                        error.to_string(),
                    ));
                }
            }
            return Ok(out);
        }
        let shown_action = matches!(
            &command.kind,
            CommandKind::Split { .. }
                | CommandKind::Tab { .. }
                | CommandKind::Select { .. }
                | CommandKind::Focus { .. }
                | CommandKind::ResizeSplit { .. }
                | CommandKind::CycleTab { .. }
                | CommandKind::CycleTabInSlot { .. }
                | CommandKind::SetMaximized { .. }
                | CommandKind::MovePane { .. }
                | CommandKind::PlaceTab { .. }
                | CommandKind::ProfileTab { .. }
                | CommandKind::NewTerminal { .. }
        );
        if shown_action
            && window
                .shown_key()
                .is_none_or(|key| key.source_id() != self.id())
        {
            out.push(rejected_command(
                command.source,
                &command.kind,
                "Command belongs to a source that is no longer shown".into(),
            ));
            return Ok(out);
        }
        // Input and geometry inspect immutable authority; terminal traffic must
        // not clone the window's bounded navigation order via SourceEdit.
        match &command.kind {
            CommandKind::Input { route, bytes } => {
                let result = if window.accepts_terminal(route)
                    && self.attached.contains(route.session_id())
                {
                    self.client.input_bytes(route.session_id(), bytes)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "Terminal is not attached",
                    ))
                };
                if let Err(error) = result {
                    out.push(Update::Status(error.to_string()));
                }
                return Ok(out);
            }
            CommandKind::ResizePane {
                route,
                screen,
                route_generation,
                cols,
                rows,
            } => {
                if let Some(model) = window.source(self.id()) {
                    if window.shown_key().as_ref() == Some(screen)
                        && model.route_generation() == *route_generation
                        && window.contains_terminal_binding(route)
                        && model.is_ready()
                        && model.screen_for(route.pane_id()) == Some(screen.screen_id())
                        && model.is_displayed_pane(route.pane_id())
                    {
                        self.sizes.insert(route.session_id().into(), (*cols, *rows));
                        self.pending_resize.insert(route.session_id().into());
                    }
                }
                return Ok(out);
            }
            CommandKind::Visible {
                screen,
                route_generation,
                panes,
            } => {
                if let Some(model) = window.source(self.id()) {
                    if screen == &window.shown_key()
                        && *route_generation == model.route_generation()
                    {
                        self.desired_visible = panes
                            .iter()
                            .filter(|pane| model.is_displayed_pane(pane))
                            .cloned()
                            .collect();
                    }
                }
                return Ok(out);
            }
            _ => {}
        }
        if matches!(
            &command.kind,
            CommandKind::CreateScreen(_) | CommandKind::ApplyTemplate { .. }
        ) {
            window.cancel_remote_creation_follow();
        }
        let mut close_window = false;
        let mut model = Some(
            window
                .source_mut(self.id())
                .ok_or_else(|| crate::connection::invalid("source is gone"))?,
        );
        let command_source = command.source;
        let command = command.kind;

        if let CommandKind::ApplyTemplate {
            plan,
            choice,
            replace_confirmed,
        } = command
        {
            let (connection_id, request_id) = (plan.connection_id(), plan.request_id());
            if model
                .as_ref()
                .is_some_and(|m| m.template_request_pending(request_id))
            {
                return Ok(out);
            }
            let mut post_failure = None;
            let result = if self.pending_close.is_some() {
                Err(std::io::Error::other("A close operation is pending"))
            } else {
                model
                    .as_mut()
                    .ok_or_else(|| std::io::Error::other("Workspace is not ready"))
                    .and_then(|m| {
                        m.apply_template(&self.client, plan, choice, replace_confirmed)
                            .map(|()| {
                                if let Err(error) = self.workspace_refresh.begin(m) {
                                    post_failure = m.fail_template_observation(error.to_string());
                                }
                            })
                    })
            };
            out.changed = true;
            if let Some(result) = post_failure {
                out.push(Update::TemplateApply(result));
            }
            if let Err(error) = result {
                out.push(Update::TemplateApply(TemplateApplyResult {
                    connection_id,
                    request_id,
                    outcome: TemplateApplyOutcome::Rejected {
                        reason: error.to_string(),
                    },
                }));
            }
            return Ok(out);
        }
        if let CommandKind::RequestClose {
            mut plan,
            confirmed,
        } = command
        {
            let response_source = command_source;
            if plan.source_token().is_none() {
                // The channel-owned startup prompt may race publication.
                if let Err(error) = validate_pre_discovery_close(&plan, confirmed) {
                    self.last_close_request = self.last_close_request.max(plan.request_id);
                    out.push(Update::CloseRejected {
                        source: response_source,
                        request_id: plan.request_id,
                        reason: error.to_string(),
                    });
                    return Ok(out);
                }
                if let Some(model) = &model {
                    plan = model.assess_close(plan.request_id, CloseTarget::Window)?;
                }
            }
            let request_id = plan.request_id;
            if self
                .pending_close
                .as_ref()
                .is_some_and(|close| close.is_duplicate(request_id))
            {
                // The original is still awaiting authoritative settlement.
                // Rejecting this duplicate would clear the UI's matching pending request.
                return Ok(out);
            }
            let result = if self.pending_close.is_some() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "previous close still settling",
                ))
            } else if request_id <= self.last_close_request {
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "stale close request",
                ))
            } else if let Some(model) = &mut model {
                model
                    .validate_close(&plan, confirmed)
                    .and_then(|()| match &plan.target {
                        CloseTarget::Pane { screen, pane } => {
                            model.close_pane_on_screen(&self.client, screen, pane)
                        }
                        CloseTarget::Screen { screen } => model.close_screen(&self.client, screen),
                        CloseTarget::Window => Ok(()),
                    })
            } else {
                validate_pre_discovery_close(&plan, confirmed)
            };
            self.last_close_request = self.last_close_request.max(request_id);
            match result {
                Ok(()) => {
                    if plan.target == CloseTarget::Window {
                        close_window = true;
                        out.push(Update::WindowCloseApproved {
                            request_id,
                            source: response_source,
                        });
                    } else {
                        let generation = model
                            .as_ref()
                            .map_or(0, |model| model.mutation_generation());
                        self.pending_close = Some(PendingClose {
                            source: response_source,
                            request_id,
                            target: plan.target,
                            barrier_generation: generation,
                            catalog_barrier_seen: false,
                            failure: None,
                        });
                        out.push(Update::CloseSubmitted {
                            request_id,
                            source: response_source,
                        });
                    }
                }
                Err(error) => {
                    out.push(Update::CloseRejected {
                        source: response_source,
                        request_id,
                        reason: error.to_string(),
                    });
                }
            }
            if close_window {
                drop(model);
                window.cancel_remote_creation_follow();
                out.close_window = true;
                return Ok(out);
            }
            return Ok(out);
        }
        let floating_request = match &command {
            CommandKind::FloatPane { request_id, source } => {
                Some((*request_id, source.connection_id))
            }
            CommandKind::DockFloat { request_id, target }
            | CommandKind::MoveFloat {
                request_id, target, ..
            } => Some((*request_id, target.connection_id())),
            _ => None,
        };
        if let Some((request_id, connection_id)) = floating_request {
            if model
                .as_ref()
                .is_some_and(|model| model.floating_request_pending(request_id))
            {
                return Ok(out);
            }
            let result = if self.pending_close.is_some() {
                Err(std::io::Error::other("A close operation is pending"))
            } else if let Some(model) = &mut model {
                match command {
                    CommandKind::FloatPane { source, .. } => {
                        model.float_pane(&self.client, request_id, source)
                    }
                    CommandKind::DockFloat { target, .. } => {
                        model.dock_float(&self.client, request_id, target)
                    }
                    CommandKind::MoveFloat { target, bounds, .. } => {
                        model.move_float(&self.client, request_id, target, bounds)
                    }
                    _ => unreachable!("floating command classified"),
                }
            } else {
                Err(std::io::Error::other("Workspace is not ready"))
            };
            match result {
                Err(error) => out.push(Update::FloatingAction(FloatingActionResult {
                    connection_id,
                    request_id,
                    outcome: FloatingActionOutcome::Rejected(error.to_string()),
                })),
                Ok(false) => out.push(Update::FloatingAction(FloatingActionResult {
                    connection_id,
                    request_id,
                    outcome: FloatingActionOutcome::Confirmed,
                })),
                Ok(true) => {
                    out.changed = true;
                }
            }
            return Ok(out);
        }
        if let CommandKind::TransferPane {
            request_id,
            source,
            target,
        } = command
        {
            if model
                .as_ref()
                .is_some_and(|model| model.transfer_request_pending(request_id))
            {
                return Ok(out);
            }
            let connection_id = source.connection_id;
            let result = if self.pending_close.is_some() {
                Err(std::io::Error::other("A close operation is pending"))
            } else if let Some(model) = &mut model {
                model
                    .transfer_pane(&self.client, request_id, source, target)
                    .and_then(|()| self.workspace_refresh.begin(model))
            } else {
                Err(std::io::Error::other("Workspace is not ready"))
            };
            if let Err(error) = result {
                out.push(Update::ScreenTransfer(ScreenTransferResult {
                    connection_id,
                    request_id,
                    outcome: ScreenTransferOutcome::Rejected(error.to_string()),
                }));
            } else {
                out.changed = true;
            }
            return Ok(out);
        }

        let Some(model) = &mut model else {
            return Ok(out);
        };
        if let Err(error) = validate_command_target(model, &command) {
            out.push(rejected_command(
                command_source,
                &command,
                error.to_string(),
            ));
            return Ok(out);
        }
        let result: std::io::Result<()> = match command {
            CommandKind::CreateRemoteScreen { .. }
            | CommandKind::ManageForward { .. }
            | CommandKind::RefreshForwards
            | CommandKind::CancelForwardOpen { .. }
            | CommandKind::ManageHost { .. }
            | CommandKind::WithdrawHostConnect { .. }
            | CommandKind::ReleaseHostBrowsing
            | CommandKind::ConnectHost { .. }
            | CommandKind::RetryHost { .. }
            | CommandKind::RequestHostUpdate { .. }
            | CommandKind::ConfirmHostUpdate { .. }
            | CommandKind::DismissHostUpdate { .. }
            | CommandKind::RefreshBackendVersion
            | CommandKind::CancelHost { .. }
            | CommandKind::ReleaseHostBrowse { .. }
            | CommandKind::OpenRemoteScreen { .. }
            | CommandKind::DetachRemoteScreen { .. }
            | CommandKind::SwitchScreen(_)
            | CommandKind::MoveScreen { .. }
            | CommandKind::CycleScreen { .. }
            | CommandKind::RefreshHostDirectory { .. }
            | CommandKind::TemplateApi { .. }
            | CommandKind::ApplyTemplate { .. }
            | CommandKind::FloatPane { .. }
            | CommandKind::DockFloat { .. }
            | CommandKind::MoveFloat { .. }
            | CommandKind::TransferPane { .. }
            | CommandKind::Zoom { .. }
            | CommandKind::UpdateSettings { .. }
            | CommandKind::SetDrawerVisibility { .. }
            | CommandKind::SetColorScheme { .. }
            | CommandKind::RequestClose { .. } => {
                unreachable!("close is handled before normal commands")
            }
            CommandKind::CreateScreen(name) => {
                let result = model.create_screen(&self.client, &name).map(|ids| {
                    self.select_created = Some(ids.screen_id);
                });
                if result.is_ok() {
                    out.changed = true;
                }
                result
            }
            CommandKind::RenameScreen {
                screen,
                connection,
                name,
            } => {
                if self.pending_close.is_some() {
                    Err(std::io::Error::other("A close operation is pending"))
                } else if model.connection_id() == connection {
                    model.rename_screen(&self.client, &screen, &name)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Rename belongs to a previous connection",
                    ))
                }
            }
            CommandKind::Split { pane, axis, .. } => {
                let result = model.split(&self.client, &pane, axis).map(|_| ());
                if result.is_ok() {
                    out.changed = true;
                }
                result
            }
            CommandKind::Tab { pane, .. } => model.new_tab(&self.client, &pane).map(|_| ()),
            CommandKind::ProfileTab {
                connection,
                screen,
                pane,
                session,
                profile_id,
            } => {
                if connection != model.connection_id()
                    || model.shown_screen_id() != Some(screen.as_str())
                    || model.screen_for(&pane) != Some(screen.as_str())
                    || model.session_for(&pane) != Some(session.as_str())
                    || !model.is_displayed_pane(&pane)
                {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Shell profile choice belongs to a previous terminal",
                    ))
                } else {
                    model
                        .new_tab_with_profile(&self.client, &pane, &profile_id)
                        .map(|_| ())
                }
            }
            CommandKind::PlaceTab {
                drag,
                target,
                before,
            } => {
                let result = model.place_tab(&self.client, drag, target, before);
                if result.as_ref().is_ok_and(|moved| *moved) {
                    out.changed = true;
                }
                result.map(|_| ())
            }
            CommandKind::CycleTab {
                connection,
                screen,
                slot,
                step,
            } => match model.cycle_tab(connection, &screen, &slot, step) {
                Ok(true) => {
                    let result = model.sync_selected_tabs(&self.client);
                    out.changed = true;
                    result
                }
                Ok(false) => Ok(()),
                Err(error) => Err(error),
            },
            CommandKind::CycleTabInSlot {
                connection,
                screen,
                slot,
                step,
            } => match model.cycle_tab_in_slot(connection, &screen, &slot, step) {
                Ok(true) => {
                    let result = model.sync_selected_tabs(&self.client);
                    out.changed = true;
                    result
                }
                Ok(false) => Ok(()),
                Err(error) => Err(error),
            },
            CommandKind::MovePane {
                drag,
                target,
                placement,
            } => {
                let result = model.move_pane(&self.client, drag, target, placement);
                if result.as_ref().is_ok_and(|moved| *moved) {
                    out.changed = true;
                }
                result.map(|_| ())
            }
            CommandKind::SetMaximized {
                screen,
                connection,
                slot,
                pane,
            } => {
                if model.connection_id() == connection
                    && model.shown_screen_id() == Some(screen.as_str())
                    && model.set_maximized_slot(slot.as_deref(), pane.as_deref())
                {
                    out.changed = true;
                }
                Ok(())
            }
            CommandKind::NewTerminal { screen, connection } => {
                if model.connection_id() == connection
                    && model.shown_screen_id() == Some(screen.as_str())
                {
                    model.new_terminal(&self.client).map(|_| ())
                } else {
                    Ok(())
                }
            }
            CommandKind::ResizeSplit { node, sizes, .. } => {
                model.resize_split(&self.client, &node, sizes)
            }
            CommandKind::Select { pane, .. } => {
                model.select_tab(&pane);
                let result = model.sync_selected_tabs(&self.client);
                out.changed = true;
                result
            }
            CommandKind::Focus { .. } => unreachable!("focus is owned by the window worker"),
            CommandKind::Input { .. }
            | CommandKind::ResizePane { .. }
            | CommandKind::Visible { .. } => {
                unreachable!("read-only controls handled before model mutation")
            }
        };
        if let Err(error) = result {
            out.push(Update::Status(error.to_string()));
        }

        Ok(out)
    }

    pub fn event(
        &mut self,
        window: &mut WindowWorkspace,
        event: Box<Event>,
    ) -> anyhow::Result<SourceEffects> {
        let mut out = SourceEffects::default();
        let source = self.owner;
        if event.connection_id != source.connection_id()
            || window
                .source(self.id())
                .is_none_or(|model| model.source_token() != source)
        {
            return Ok(out);
        }
        let local = window
            .source(self.id())
            .is_some_and(|source| source.kind() == SourceKind::Local);
        let mut model = Some(
            window
                .source_mut(self.id())
                .ok_or_else(|| crate::connection::invalid("source is gone"))?,
        );
        let mut delta = WorkspaceDelta::default();
        let mut accepted_close_snapshot = false;
        let mut terminated = None;
        if let Some(json) = event.json() {
            match json["type"].as_str() {
                Some("mux.welcome") if !self.welcomed => {
                    let workspace = model.as_mut().expect("owned local source");
                    let supports_tab_selection =
                        json["capabilities"].as_array().is_some_and(|capabilities| {
                            capabilities
                                .iter()
                                .any(|capability| capability.as_str() == Some("tab-select-v1"))
                        });
                    workspace.set_templates_supported(
                        json["capabilities"].as_array().is_some_and(|caps| {
                            caps.iter().any(|cap| cap == "server-templates-v1")
                        }),
                    );
                    workspace.set_tab_selection_supported(supports_tab_selection);
                    workspace.set_tab_placement_supported(
                        json["capabilities"].as_array().is_some_and(|capabilities| {
                            capabilities
                                .iter()
                                .any(|capability| capability.as_str() == Some("tab-placement-v1"))
                        }),
                    );
                    let supports_catalog =
                        json["capabilities"].as_array().is_some_and(|capabilities| {
                            capabilities.iter().any(|capability| {
                                capability.as_str() == Some("workspace-catalog-v1")
                            })
                        });
                    workspace.set_catalog_supported(supports_catalog);
                    workspace.set_screen_placement_supported(
                        json["capabilities"].as_array().is_some_and(|caps| {
                            caps.iter().any(|cap| {
                                cap == splitty_core::protocol::SCREEN_PLACEMENT_CAPABILITY
                            })
                        }),
                    );
                    if supports_catalog {
                        workspace.mark_catalog_dirty();
                    } else {
                        out.push(Update::Status(
                            "Local service does not support workspace-catalog-v1".into(),
                        ));
                    }
                    out.changed = true;
                    self.welcomed = true;
                }
                Some("ports.changed") => {
                    if let Some(model) = &mut model {
                        match model.observe_ports(
                            source,
                            json,
                            &self.client.sender().context().budget,
                        ) {
                            Ok(accepted) => out.changed |= accepted,
                            Err(error) => out.push(Update::Status(error.to_string())),
                        }
                    }
                }
                Some("mux.workspace_snapshot") => {
                    if let Some(model) = &mut model {
                        let response_generation = model.catalog_request_generation();
                        let first = !model.catalog_initialized();
                        match self
                            .workspace_refresh
                            .apply_catalog(model, event.connection_id, json)
                        {
                            Ok(applied) => {
                                delta = applied;
                                if let Some(close) = &mut self.pending_close {
                                    if close.observe_catalog(response_generation) {
                                        if let CloseTarget::Pane { screen, .. } = &close.target {
                                            if model.screen(screen).is_some() {
                                                model.mark_screen_dirty(screen);
                                            }
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                if event.connection_id == model.connection_id() {
                                    if let Some(close) = &mut self.pending_close {
                                        if close.observe_catalog(response_generation) {
                                            close.failure = Some(format!(
                                                "Close could not be verified: {error}"
                                            ));
                                        }
                                    }
                                }
                                out.changed = true;
                                let reason = format!("Workspace catalog rejected: {error}");
                                if local {
                                    out.push(Update::Status(reason));
                                } else {
                                    out.observation_error = Some(reason);
                                }
                            }
                        }
                        if local
                            && first
                            && model.catalog_initialized()
                            && model.screens().is_empty()
                        {
                            match model.create_screen(&self.client, "Screen 1") {
                                Ok(_) => delta.snapshot_changed = true,
                                Err(error) => out.push(Update::Status(format!(
                                    "Initial screen not created: {error}"
                                ))),
                            }
                        }
                    }
                }
                Some("mux.screen_snapshot") => {
                    if let Some(model) = &mut model {
                        match self.workspace_refresh.apply_snapshot(
                            model,
                            event.connection_id,
                            json,
                        ) {
                            Ok(applied) => {
                                delta = applied;
                                accepted_close_snapshot =
                                    delta.snapshot_changed && !model.routes_reconciling();
                            }
                            Err(error) => {
                                if event.connection_id == model.connection_id() {
                                    if let Some(close) = &mut self.pending_close {
                                        if close.catalog_barrier_seen
                                            && matches!(&close.target, CloseTarget::Pane { screen, .. } if json["screen"]["id"] == *screen)
                                        {
                                            close.failure = Some(format!(
                                                "Close could not be verified: {error}"
                                            ));
                                        }
                                    }
                                }
                                out.changed = true;
                                out.push(Update::Status(format!(
                                    "Screen snapshot rejected: {error}"
                                )))
                            }
                        }
                    }
                }
                Some("mux.pane_spawned") => {
                    if let (Some(model), Some(pane), Some(session)) = (
                        &mut model,
                        json["paneId"].as_str(),
                        json["sessionId"].as_str(),
                    ) {
                        model.observe_template_spawn(event.connection_id, pane, session, json);
                        delta = model.observe_spawn(event.connection_id, pane, session);
                        if let Some(message) =
                            profile_fallback_status(model, event.connection_id, pane, session, json)
                        {
                            out.push(Update::Status(message));
                        }
                    }
                }
                Some("mux.nudge") => {
                    if let Some(model) = &mut model {
                        let id = json["screenId"].as_str();
                        if matches!(json["reason"].as_str(), Some("mux_op" | "cli_op"))
                            || id.is_none_or(|id| model.screen(id).is_none())
                        {
                            model.mark_catalog_dirty();
                        }
                        if let Some(id) = id {
                            model.mark_screen_dirty(id);
                        }
                    }
                }
                Some("attached") => {
                    if let Some(session) = json["sessionId"].as_str() {
                        if model.as_mut().is_some_and(|model| {
                            model.observe_attached(event.connection_id, session)
                        }) {
                            self.attached.insert(session.to_owned());
                            self.activity_pending = true;
                            if model
                                .as_ref()
                                .and_then(|model| model.pane_for(session))
                                .is_some_and(|pane| self.desired_visible.contains(pane))
                            {
                                self.pending_resize.insert(session.to_owned());
                            } else {
                                self.pending_hide.insert(session.to_owned());
                            }
                            out.changed = true;
                            out.push(Update::Attached {
                                source,
                                session: session.to_owned(),
                            });
                        }
                    }
                }
                Some("closed") => {
                    if let Some(session) = json["sessionId"].as_str() {
                        self.attached.remove(session);
                        // Client already forgets this attachment on `closed`.
                        self.owned_sessions.remove(session);
                        self.closed_sessions.insert(session.to_owned());
                    }
                }
                Some("mux.op_failed") | Some("error") => {
                    if json["type"] == "mux.op_failed" {
                        if let Some(model) = &mut model {
                            model.observe_placement_failure(event.connection_id, json);
                            model.observe_transfer_failure(event.connection_id, json);
                            model.observe_floating_failure(event.connection_id, json);
                            model.observe_template_failure(event.connection_id, json);
                            model.observe_remote_creation_failure(event.connection_id, json);
                        }
                        if let Some(close) = &mut self.pending_close {
                            close.observe_failure(json);
                        }
                    }
                    if json["type"] == "error" {
                        if let Some(session) = json["sessionId"].as_str() {
                            if !self.attached.contains(session) {
                                // A server-rejected pending attach is removed by
                                // Client. Tombstone it until its route changes so
                                // an absent session does not hot-loop reattach.
                                self.owned_sessions.remove(session);
                                self.closed_sessions.insert(session.to_owned());
                                if let Some(model) = &mut model {
                                    if let Some(screen) = model
                                        .pane_for(session)
                                        .and_then(|pane| model.screen_for(pane))
                                        .map(str::to_owned)
                                    {
                                        model.mark_screen_dirty(&screen);
                                    }
                                }
                            }
                        }
                    }
                    if json["op"] == "move_pane" {
                        if let Some(model) = &mut model {
                            model.reject_move(event.connection_id);
                        }
                    }
                    if let (Some(model), Some(pane)) = (&mut model, json["paneId"].as_str()) {
                        delta = model.observe_spawn_failure(event.connection_id, pane);
                    }
                    out.push(Update::Status(format!("Operation rejected: {json}")));
                    if let Some(model) = &mut model {
                        // A rejected lifecycle or pane operation can change
                        // catalog membership as well as one screen's layout.
                        model.mark_catalog_dirty();
                        if let Some(id) = json["screenId"].as_str() {
                            model.mark_screen_dirty(id);
                        }
                    }
                }
                _ => {}
            }
            let settled = self.pending_close.as_ref().and_then(|close| {
                let model = model.as_ref()?;
                let (screen_exists, pane_exists, matching_snapshot) = match &close.target {
                    CloseTarget::Pane { screen, pane } => (
                        model.screen(screen).is_some(),
                        model
                            .screen(screen)
                            .and_then(|screen| screen.snapshot())
                            .is_some_and(|snapshot| {
                                snapshot
                                    .panes
                                    .contains_key(&splitty_core::ids::PaneId::from(pane.as_str()))
                            }),
                        (accepted_close_snapshot && json["screen"]["id"] == *screen)
                            || delta.coherent_refresh,
                    ),
                    CloseTarget::Screen { screen } => {
                        (model.screen(screen).is_some(), false, false)
                    }
                    CloseTarget::Window => (false, false, false),
                };
                close.settlement(screen_exists, matching_snapshot, pane_exists)
            });
            if let Some(settled) = settled {
                if let Some(close) = self.pending_close.take() {
                    if settled.is_ok() && !local {
                        if let CloseTarget::Screen { screen } = &close.target {
                            terminated = model.as_ref().map(|source| source.screen_key(screen));
                        }
                    }
                    let update = match settled {
                        Ok(()) => Update::CloseSettled {
                            source: close.source,
                            request_id: close.request_id,
                        },
                        Err(reason) => Update::CloseRejected {
                            source: close.source,
                            request_id: close.request_id,
                            reason,
                        },
                    };
                    out.push(update);
                }
            }
        }
        // Attachment changes are reconciled below. Queue pressure
        // must not discard a route or close the owned connection.
        if delta.snapshot_changed
            || delta.placement.is_some()
            || delta.transfer.is_some()
            || delta.floating.is_some()
            || delta.template.is_some()
            || !delta.added.is_empty()
            || !delta.removed.is_empty()
        {
            out.changed = true;
        }
        if delta.coherent_refresh {
            // Geometry belongs to the new route generation. The UI
            // must measure/report again, even when its grid is equal.
            self.desired_visible.clear();
            self.sizes.clear();
            self.pending_resize.clear();
            self.pending_hide.extend(self.attached.iter().cloned());
        }
        if let Some(result) = delta.template {
            out.push(Update::TemplateApply(result));
        }
        if let Some(result) = delta.floating {
            out.push(Update::FloatingAction(result));
        }
        if let Some(result) = delta.transfer {
            out.push(Update::ScreenTransfer(result));
        }
        if let Some(reason) = delta.reconciliation_error {
            if local {
                out.push(Update::Status(reason));
            } else {
                out.observation_error = Some(reason);
            }
        }
        if let Some(PlacementOutcome::Rejected(reason)) = delta.placement {
            out.push(Update::Status(reason));
        }
        // The original native accounting stays alive until UI consumption.
        out.push(Update::Event { source, event });

        if let Some(model) = &model {
            out.follow_remote_creation = model.remote_creation_follow();
        }
        if let Some(created) = &self.select_created {
            let source = model.as_ref().expect("runtime source");
            if source
                .screens()
                .iter()
                .any(|screen| screen.id.as_str() == created)
            {
                out.select_created = Some(source.screen_key(created));
                self.select_created = None;
            }
        }
        drop(model);
        if let Some(key) = terminated {
            window.detach(window.lifecycle_token(), &key)?;
            out.changed = true;
        }
        Ok(out)
    }
    pub fn reconcile(&mut self, window: &mut WindowWorkspace) -> anyhow::Result<SourceEffects> {
        let mut out = SourceEffects::default();
        let globally_shown = window
            .shown_key()
            .is_some_and(|key| key.source_id() == self.id());
        if !globally_shown {
            self.desired_visible.clear();
        }
        if let Some(model) = window.source(self.id()) {
            let desired_sessions: HashSet<String> = model
                .routes()
                .into_iter()
                .map(|route| route.session_id)
                .collect();
            self.sizes
                .retain(|session, _| desired_sessions.contains(session));
            self.pending_resize
                .retain(|session| desired_sessions.contains(session));
            self.desired_visible.retain(|pane| {
                model.screen_for(pane) == model.shown_screen_id()
                    && model.session_for(pane).is_some()
                    && model.is_displayed_pane(pane)
            });
            self.closed_sessions
                .retain(|session| desired_sessions.contains(session));
            self.closed_sessions
                .extend(model.failed_template_sessions().map(str::to_owned));
            let mut pending_error = None::<String>;
            for session in self.owned_sessions.clone() {
                if !desired_sessions.contains(&session) {
                    match self.client.detach(&session) {
                        Ok(()) => {
                            self.owned_sessions.remove(&session);
                            self.attached.remove(&session);
                            self.pending_hide.remove(&session);
                            self.pending_resize.remove(&session);
                            self.sizes.remove(&session);
                        }
                        Err(error) => {
                            pending_error.get_or_insert_with(|| format!("Detach pending: {error}"));
                        }
                    }
                }
            }
            self.visible
                .retain(|pane| model.session_for(pane).is_some());
            let missing: Vec<_> = desired_sessions
                .difference(&self.owned_sessions)
                .cloned()
                .collect();
            for session in missing {
                if !self.closed_sessions.contains(&session) {
                    match self.client.attach(&session) {
                        Ok(()) => {
                            self.owned_sessions.insert(session);
                        }
                        Err(error) => {
                            pending_error.get_or_insert_with(|| format!("Attach pending: {error}"));
                        }
                    }
                }
            }
            if self.activity_pending {
                match self.client.activity(true) {
                    Ok(()) => self.activity_pending = false,
                    Err(error) => {
                        pending_error.get_or_insert_with(|| format!("Activity pending: {error}"));
                    }
                }
            }
            for pane in self.visible.clone() {
                if !self.desired_visible.contains(&pane) {
                    if let Some(session) = model.session_for(&pane) {
                        match self.client.hide(session) {
                            Ok(()) => {
                                self.visible.remove(&pane);
                            }
                            Err(error) => {
                                pending_error
                                    .get_or_insert_with(|| format!("Hide pending: {error}"));
                            }
                        }
                    } else {
                        self.visible.remove(&pane);
                    }
                }
            }
            for session in self.pending_hide.clone() {
                if !self.attached.contains(&session) || !desired_sessions.contains(&session) {
                    self.pending_hide.remove(&session);
                } else if model
                    .pane_for(&session)
                    .is_some_and(|pane| self.desired_visible.contains(pane))
                {
                    self.pending_hide.remove(&session);
                    self.pending_resize.insert(session);
                } else {
                    match self.client.hide(&session) {
                        Ok(()) => {
                            self.pending_hide.remove(&session);
                        }
                        Err(error) => {
                            pending_error.get_or_insert_with(|| format!("Hide pending: {error}"));
                        }
                    }
                }
            }
            for pane in &self.desired_visible {
                if let Some(session) = model.session_for(pane) {
                    if self.attached.contains(session)
                        && (!self.visible.contains(pane) || self.pending_resize.contains(session))
                    {
                        let (cols, rows) = self.sizes.get(session).copied().unwrap_or((80, 24));
                        match self.client.resize(session, cols, rows, true) {
                            Ok(()) => {
                                self.visible.insert(pane.clone());
                                self.pending_resize.remove(session);
                            }
                            Err(error) => {
                                pending_error
                                    .get_or_insert_with(|| format!("Resize pending: {error}"));
                            }
                        }
                    }
                }
            }
            match pending_error {
                None => self.control_error = None,
                Some(status) if self.control_error.as_deref() == Some(status.as_str()) => {}
                Some(status) => {
                    out.push(Update::Status(status.clone()));
                    self.control_error = Some(status);
                }
            }
        }
        if window
            .source(self.id())
            .is_some_and(|model| self.workspace_refresh.needs_flush(model))
        {
            let mut model = window.source_mut(self.id()).expect("runtime source");
            match self.workspace_refresh.flush(&mut model, &self.client) {
                Ok(()) => self.refresh_error = None,
                Err(error) => {
                    // Refreshes are read-only and keep their dirty state on
                    // admission failure. Retry them without ending the owned
                    // service or replaying a mutation/input command.
                    let status = format!("Workspace refresh pending: {error}");
                    if self.refresh_error.as_deref() != Some(status.as_str()) {
                        out.push(Update::Status(status.clone()));
                        self.refresh_error = Some(status);
                    }
                }
            }
        }

        Ok(out)
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn remote_creation_real_stream_confirms_canonical_name_and_late_cancel_prevents_selection(
    ) {
        if !splitty_core::pty::testing::pty_available() {
            return;
        }
        let (runtime, path) = crate::appearance::local::tests::fixture();
        let service = crate::EmbeddedService::from_runtime(runtime);
        let client = service.connect().await.unwrap();
        let initial = client.create_screen("Screen 1").unwrap();
        let unopened = client.create_screen("Screen 3").unwrap();
        let mut window = WindowWorkspace::new(uuid::Uuid::new_v4());
        let local_key = window
            .ui_test_screen_order_fixture(&[(&initial.screen_id, "Local")], None)
            .unwrap()
            .remove(0);
        let id = window
            .admit_remote(window.lifecycle_token(), "create.test")
            .unwrap();
        window.test_bind_remote_client(id, &client);
        let token = window.source(id).unwrap().source_token();
        window.confirm_ready(token).unwrap();
        let target = window.host(id).unwrap().action_target().clone();
        let mut source = SourceRuntime::new(client, token);
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                source.reconcile(&mut window).unwrap();
                let event = source.client.next_event().await.unwrap().unwrap();
                source.event(&mut window, Box::new(event)).unwrap();
                if window
                    .source(id)
                    .unwrap()
                    .live_catalog()
                    .is_some_and(|catalog| catalog.len() == 2)
                    && !source.observation_pending(&window)
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        window.host_mut(&target).unwrap().catalog = crate::workspace::CatalogState::Ready;
        let old = window.source(id).unwrap().screen_key(&initial.screen_id);
        window.open_existing(token, &old).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                source.reconcile(&mut window).unwrap();
                let event = source.client.next_event().await.unwrap().unwrap();
                source.event(&mut window, Box::new(event)).unwrap();
                if window
                    .source(id)
                    .unwrap()
                    .screen(&initial.screen_id)
                    .unwrap()
                    .snapshot()
                    .is_some()
                    && !source.observation_pending(&window)
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        window.select(window.lifecycle_token(), &local_key).unwrap();
        let stale = source
            .command(
                &mut window,
                Command::new(
                    token,
                    CommandKind::RenameScreen {
                        screen: initial.screen_id.clone(),
                        connection: uuid::Uuid::new_v4(),
                        name: "Stale rename".into(),
                    },
                ),
            )
            .unwrap();
        assert!(
            matches!(stale.updates.as_slice(), [Update::Status(reason)] if reason.contains("previous connection"))
        );
        let rename = Command::new(
            token,
            CommandKind::RenameScreen {
                screen: initial.screen_id.clone(),
                connection: token.connection_id(),
                name: "Renamed on host".into(),
            },
        );
        // The live worker checks this envelope before reaching SourceRuntime.
        // Exercising only the driver would miss a local-only classification.
        validate_worker_command_source(&window, &rename)
            .expect("worker ingress admits the captured remote rename while local is selected");
        source.command(&mut window, rename).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                source.reconcile(&mut window).unwrap();
                let event = source.client.next_event().await.unwrap().unwrap();
                source.event(&mut window, Box::new(event)).unwrap();
                if window.source(id).unwrap().screens().iter().any(|screen| {
                    screen.id.as_str() == initial.screen_id
                        && screen.name.as_deref() == Some("Renamed on host")
                }) {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(window.shown_key(), Some(local_key.clone()));
        assert_eq!(
            window.local_source().screens()[0].name.as_deref(),
            Some("Local")
        );
        let blocked = window
            .prepare_remote_screen_creation(&target, token)
            .unwrap();
        source.pending_close = Some(PendingClose {
            source: Some(token),
            request_id: 8,
            target: CloseTarget::Pane {
                screen: initial.screen_id.clone(),
                pane: initial.pane_id.clone(),
            },
            barrier_generation: 0,
            catalog_barrier_seen: false,
            failure: None,
        });
        let closing = source
            .command(
                &mut window,
                Command::new(
                    token,
                    CommandKind::RenameScreen {
                        screen: initial.screen_id.clone(),
                        connection: token.connection_id(),
                        name: "Closing rename".into(),
                    },
                ),
            )
            .unwrap();
        assert!(
            matches!(closing.updates.as_slice(), [Update::Status(reason)] if reason.contains("close operation"))
        );
        let rejected = source
            .command(
                &mut window,
                Command::new(
                    token,
                    CommandKind::CreateRemoteScreen {
                        request: blocked.clone(),
                    },
                ),
            )
            .unwrap();
        assert!(matches!(
            rejected.updates.as_slice(),
            [Update::RemoteScreenCreateRejected { .. }]
        ));
        assert!(window
            .source(id)
            .unwrap()
            .remote_screen_creation()
            .is_none());
        source.pending_close = None;
        let request = window
            .prepare_remote_screen_creation(&target, token)
            .unwrap();
        let result = source
            .command(
                &mut window,
                Command::new(
                    token,
                    CommandKind::CreateRemoteScreen {
                        request: request.clone(),
                    },
                ),
            )
            .unwrap();
        assert!(result.changed);
        assert!(result.updates.is_empty());
        let duplicate = source
            .command(
                &mut window,
                Command::new(
                    token,
                    CommandKind::CreateRemoteScreen {
                        request: request.clone(),
                    },
                ),
            )
            .unwrap();
        assert!(duplicate.updates.is_empty());
        assert!(request.is_follow_requested());
        let created = window
            .source(id)
            .unwrap()
            .remote_screen_creation()
            .unwrap()
            .screen_id()
            .to_owned();
        assert!(window
            .source(id)
            .unwrap()
            .screen(&unopened.screen_id)
            .is_none());
        let effects = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                source.reconcile(&mut window).unwrap();
                let event = source.client.next_event().await.unwrap().unwrap();
                let effects = source.event(&mut window, Box::new(event)).unwrap();
                if effects.follow_remote_creation.is_some() {
                    break effects;
                }
            }
        })
        .await
        .unwrap();
        // Renaming the original frees its old suggested name for creation.
        assert_eq!(
            window
                .source(id)
                .unwrap()
                .screens()
                .iter()
                .find(|screen| screen.id.as_str() == created)
                .unwrap()
                .name
                .as_deref(),
            Some("Screen 1")
        );
        assert!(window
            .source(id)
            .unwrap()
            .screen(&created)
            .unwrap()
            .snapshot()
            .unwrap()
            .panes
            .values()
            .any(|pane| pane.session_id.is_some()));
        request.cancel_follow(); // Between event preparation and publication, as destination navigation can do.
        let (updates, mut receive) = mpsc::channel(16);
        publish_source_effects(&mut window, effects, &updates, &AtomicBool::new(false))
            .await
            .unwrap();
        while receive.try_recv().is_ok() {}
        assert_eq!(window.shown_key(), Some(local_key));
        let key = window.source(id).unwrap().screen_key(&created);
        window.detach(window.lifecycle_token(), &key).unwrap();
        source.reconcile(&mut window).unwrap();
        source.client.request_workspace().unwrap();
        let catalog = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let event = source.client.next_event().await.unwrap().unwrap();
                if event
                    .json()
                    .is_some_and(|value| value["type"] == "mux.workspace_snapshot")
                {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        let screens = catalog.json().unwrap()["screens"].as_array().unwrap();
        assert_eq!(
            screens.len(),
            3,
            "reconciliation never repeats creation or closes detached host work"
        );
        assert!(screens.iter().any(|screen| screen["id"] == created));
        drop(catalog);
        source.client.finish().await.unwrap();
        service.close().await.unwrap();
        let _ = std::fs::remove_dir_all(path);
    }

    #[tokio::test]
    async fn connection_loss_rejects_pending_close_once_with_original_owner_and_request() {
        let (runtime, path) = crate::appearance::local::tests::fixture();
        let service = crate::EmbeddedService::from_runtime(runtime);
        let client = service.connect().await.unwrap();
        let owner = SourceWorkspace::new(client.connection_id()).source_token();
        let mut source = SourceRuntime::new(client, owner);
        source.pending_close = Some(PendingClose {
            source: Some(owner),
            request_id: 41,
            target: CloseTarget::Screen {
                screen: "existing".into(),
            },
            barrier_generation: 7,
            catalog_barrier_seen: false,
            failure: None,
        });
        match source.reject_pending_close_on_loss().unwrap() {
            Update::CloseRejected {
                source: captured,
                request_id,
                reason,
            } => {
                assert_eq!(captured, Some(owner));
                assert_eq!(request_id, 41);
                assert!(reason.contains("uncertain"));
            }
            _ => panic!("close loss must reject the original assessment"),
        }
        assert!(source.reject_pending_close_on_loss().is_none());
        let mut window = WindowWorkspace::new(owner.connection_id());
        let (remote, _) = window
            .ui_test_remote_terminal_fixture("close.test")
            .unwrap();
        let target = window
            .host(remote.source_id())
            .unwrap()
            .action_target()
            .clone();
        let request = window
            .prepare_remote_screen_creation(
                &target,
                window.execution_token(remote.source_id()).unwrap(),
            )
            .unwrap();
        window
            .ui_test_remote_creation_fixture(&request, false)
            .unwrap();
        let plan = window
            .local_source()
            .assess_close(42, CloseTarget::Window)
            .unwrap();
        let approved = source
            .command(
                &mut window,
                Command::new(
                    owner,
                    CommandKind::RequestClose {
                        plan,
                        confirmed: true,
                    },
                ),
            )
            .unwrap();
        assert!(
            approved.close_window,
            "remote pending creation cannot block local assessed shutdown"
        );
        assert!(!request.is_follow_requested());
        assert!(window
            .source(remote.source_id())
            .unwrap()
            .remote_screen_creation()
            .unwrap()
            .is_unresolved());
        source.client.finish().await.unwrap();
        service.close().await.unwrap();
        let _ = std::fs::remove_dir_all(path);
    }
}
