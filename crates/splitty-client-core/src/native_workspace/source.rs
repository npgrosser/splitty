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
    /// Sessions whose next size report follows their attach: only that
    /// report asks the server to redraw at an unchanged size. Any later one
    /// would make full-screen programs repaint for a layout change elsewhere.
    redraw_pending: HashSet<String>,
    activity_pending: bool,
    /// The size the window last reported for each session's pane, in the
    /// current route generation. A session without one is not reported.
    sizes: HashMap<String, (u16, u16)>,
    refresh_error: Option<String>,
    control_error: Option<String>,
    last_close_request: u64,
    pending_close: Option<PendingClose>,
    select_created: Option<String>,
    files: files::FileTransfers,
    viewers: viewers::Viewers,
    editors: editors::EditorRequests,
    /// Template extractions under way: the request's wire ID with the
    /// connection that keeps templates and the window's request.
    extractions: HashMap<String, (uuid::Uuid, uuid::Uuid)>,
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
            redraw_pending: HashSet::new(),
            activity_pending: false,
            sizes: HashMap::new(),
            refresh_error: None,
            control_error: None,
            last_close_request: 0,
            pending_close: None,
            select_created: None,
            files: files::FileTransfers::default(),
            viewers: viewers::Viewers::default(),
            editors: editors::EditorRequests::default(),
            extractions: HashMap::new(),
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
                | CommandKind::OpenTextViewer { .. }
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
            CommandKind::AnswerEditorRequest { .. } => {
                if let CommandKind::AnswerEditorRequest {
                    request_id,
                    outcome,
                } = command.kind
                {
                    self.editors.answer(&self.client, &request_id, outcome);
                }
                return Ok(out);
            }
            CommandKind::StoreFiles { .. }
            | CommandKind::CancelFileStore { .. }
            | CommandKind::AnswerFileOffer { .. } => {
                let source = self.owner;
                match command.kind {
                    CommandKind::StoreFiles {
                        request_id,
                        items,
                        dropped,
                        directory,
                    } => self.files.store(
                        &self.client,
                        source,
                        request_id,
                        items,
                        dropped,
                        directory,
                        &mut out.updates,
                    ),
                    CommandKind::CancelFileStore { request_id } => {
                        self.files
                            .cancel(&self.client, source, request_id, &mut out.updates)
                    }
                    CommandKind::AnswerFileOffer { offer_id, save } => {
                        self.files
                            .answer(&self.client, source, &offer_id, save, &mut out.updates)
                    }
                    _ => unreachable!("file commands"),
                }
                return Ok(out);
            }
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
            CommandKind::ReadViewer { pane, if_version } => {
                // Only a pane of this connection's Screens with a viewer record.
                let known = window.source(self.id()).is_some_and(|model| {
                    model.is_ready()
                        && model
                            .screen_for(pane)
                            .and_then(|screen| model.screen(screen))
                            .and_then(|screen| screen.snapshot())
                            .is_some_and(|screen| {
                                screen.panes.values().any(|entry| {
                                    entry.id.as_str() == pane && entry.viewer.is_some()
                                })
                            })
                });
                if known {
                    self.viewers.read(
                        &self.client,
                        self.owner,
                        pane,
                        if_version.as_deref(),
                        &mut out.updates,
                    );
                } else {
                    out.push(Update::Viewer(viewers::ViewerUpdate {
                        source: self.owner,
                        pane: pane.clone(),
                        content: viewers::ViewerContent::Failed {
                            code: "gone".into(),
                            error: "This document is no longer on its Screen".into(),
                        },
                    }));
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
        if let CommandKind::ExtractTemplate {
            owner,
            request_id,
            screen,
            name,
            root,
        } = command
        {
            let wire = request_id.to_string();
            let result = if self.extractions.len() >= 8 {
                Err(std::io::Error::other("Too many template extractions"))
            } else if !model
                .as_ref()
                .is_some_and(|model| model.templates_supported())
            {
                Err(std::io::Error::other(
                    "Update this host's Splitty to save its Screens as templates",
                ))
            } else {
                self.client.extract_template(&wire, &screen, &name, &root)
            };
            match result {
                Ok(()) => {
                    self.extractions.insert(wire, (owner, request_id));
                }
                Err(error) => out.push(Update::TemplateApi(TemplateApiResult::rejected(
                    owner,
                    request_id,
                    error.to_string(),
                ))),
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
            | CommandKind::ManageShare { .. }
            | CommandKind::RefreshShares
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
            | CommandKind::CloseHost { .. }
            | CommandKind::OpenRemoteScreen { .. }
            | CommandKind::OpenRemoteScreens { .. }
            | CommandKind::DetachRemoteScreen { .. }
            | CommandKind::SwitchScreen(_)
            | CommandKind::MoveScreen { .. }
            | CommandKind::CycleScreen { .. }
            | CommandKind::RefreshHostDirectory { .. }
            | CommandKind::TemplateApi { .. }
            | CommandKind::ApplyTemplate { .. }
            | CommandKind::ExtractTemplate { .. }
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
            CommandKind::SetScratchpad {
                screen,
                connection,
                session,
                text,
            } => {
                if self.pending_close.is_some() {
                    Err(std::io::Error::other("A close operation is pending"))
                } else if model.connection_id() == connection {
                    model.set_scratchpad(&self.client, &screen, &session, text)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Scratchpad edit belongs to a previous connection",
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
            CommandKind::Annotate { screen, pane, op } => {
                if model.screen_for(&pane) != Some(screen.as_str()) {
                    Err(crate::connection::invalid(
                        "The document is no longer on its Screen",
                    ))
                } else {
                    model.annotation_op(&self.client, &screen, &op.screen_op(&pane))
                }
            }
            CommandKind::AnswerReview {
                screen,
                pane,
                wait_id,
                verdict,
                review,
            } => {
                if model.screen_for(&pane) != Some(screen.as_str()) {
                    Err(crate::connection::invalid(
                        "The document is no longer on its Screen",
                    ))
                } else {
                    let result = self.client.answer_review(
                        &screen,
                        &pane,
                        &wait_id,
                        verdict,
                        review.as_deref(),
                    );
                    if result.is_ok() {
                        model.mark_screen_dirty(&screen);
                    }
                    result
                }
            }
            CommandKind::OpenTextViewer {
                screen,
                pane,
                title,
                text,
                cwd,
            } => {
                if !model.viewers_supported() {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "This host does not open viewers",
                    ))
                } else if model.shown_screen_id() != Some(screen.as_str())
                    || model.screen_for(&pane) != Some(screen.as_str())
                {
                    Err(crate::connection::invalid(
                        "The terminal is no longer on the shown Screen",
                    ))
                } else {
                    let result =
                        self.client
                            .open_text_viewer(&screen, &pane, &title, &text, cwd.as_deref());
                    if result.is_ok() {
                        model.mark_screen_dirty(&screen);
                    }
                    result.map(|_| ())
                }
            }
            CommandKind::ResizeSplit { node, sizes, .. } => {
                let result = model.resize_split(&self.client, &node, sizes);
                if result.is_ok() {
                    out.changed = true;
                }
                result
            }
            CommandKind::Select { pane, .. } => {
                model.select_tab(&pane);
                let result = model.sync_selected_tabs(&self.client);
                out.changed = true;
                result
            }
            CommandKind::Focus { .. } => unreachable!("focus is owned by the window worker"),
            CommandKind::Input { .. }
            | CommandKind::StoreFiles { .. }
            | CommandKind::CancelFileStore { .. }
            | CommandKind::AnswerFileOffer { .. }
            | CommandKind::AnswerEditorRequest { .. }
            | CommandKind::ResizePane { .. }
            | CommandKind::ReadViewer { .. }
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
        self.viewers
            .event(&self.client, source, &event, &mut out.updates);
        self.template_extracted(&event, &mut out.updates);
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
                    workspace.set_viewers_supported(
                        json["capabilities"]
                            .as_array()
                            .is_some_and(|caps| caps.iter().any(|cap| cap == "viewer-v1")),
                    );
                    workspace.set_tab_selection_supported(supports_tab_selection);
                    workspace.set_program_status_supported(
                        json["capabilities"].as_array().is_some_and(|caps| {
                            caps.iter()
                                .any(|cap| cap == crate::workspace::PROGRAM_STATUS_CAPABILITY)
                        }),
                    );
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
                    self.files.welcome(&self.client, json);
                    self.editors.welcome(&self.client, json);
                }
                Some("editor.request" | "editor.cancelled") => {
                    self.editors.event(
                        &self.client,
                        source,
                        &self.attached,
                        json,
                        &mut out.updates,
                    );
                }
                Some(
                    "file.offer" | "file.chunk" | "file.closed" | "image_saved" | "image_failed"
                    | "upload_progress" | "upload_saved" | "upload_failed",
                ) => {
                    self.files
                        .event(&self.client, source, &self.attached, json, &mut out.updates);
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
                Some("program_status.changed") => {
                    if let Some(model) = &mut model {
                        match model.observe_program_status(
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
                            self.redraw_pending.insert(session.to_owned());
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
                        self.redraw_pending.remove(session);
                        self.editors.session_closed(
                            &self.client,
                            source,
                            session,
                            &mut out.updates,
                        );
                        // Client already forgets this attachment on `closed`.
                        self.owned_sessions.remove(session);
                        self.closed_sessions.insert(session.to_owned());
                    }
                }
                Some("mux.op_failed") | Some("error") => {
                    let file_answer = self.files.event(
                        &self.client,
                        source,
                        &self.attached,
                        json,
                        &mut out.updates,
                    );
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
                    if json["op"] == "resize_split" {
                        if let Some(model) = &mut model {
                            model.reject_resize(event.connection_id, json);
                        }
                    }
                    if let (Some(model), Some(pane)) = (&mut model, json["paneId"].as_str()) {
                        delta = model.observe_spawn_failure(event.connection_id, pane);
                    }
                    if !file_answer {
                        out.push(Update::Status(format!("Operation rejected: {json}")));
                    }
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
                    if settled.is_ok() {
                        if let (CloseTarget::Pane { screen, .. }, Some(model)) =
                            (&close.target, &mut model)
                        {
                            // A service removes a Screen with its last pane. One
                            // of an earlier version keeps it without panes, and
                            // the window closes it there with a second operation.
                            if model.screen_ran_empty(screen) {
                                if let Err(error) = model.close_screen(&self.client, screen) {
                                    out.push(Update::Status(format!(
                                        "Empty Screen was not closed: {error}"
                                    )));
                                }
                            }
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
            self.sizes.clear();
            self.pending_resize.clear();
            // A shown pane that keeps its session keeps this connection's
            // size report until the window's next visibility says otherwise:
            // a host refreshes for every title change of a program, and a
            // `hide` here would hand the PTY to another connection's size
            // and back. Panes no longer displayed are hidden by `reconcile`;
            // a pane whose route changed hides the session it showed.
            for route in &delta.removed {
                if self.visible.remove(&route.pane_id) {
                    self.pending_hide.insert(route.session_id.clone());
                }
                self.desired_visible.remove(&route.pane_id);
            }
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
        self.files
            .expire(&self.client, self.owner, &mut out.updates);
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
            self.redraw_pending
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
                            self.redraw_pending.remove(&session);
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
                        // A pane is reported at its measured size only. After a
                        // route refresh the window measures every shown pane
                        // again; a placeholder sent before that would resize
                        // the PTY there and back.
                        let Some(&(cols, rows)) = self.sizes.get(session) else {
                            continue;
                        };
                        let redraw = self.redraw_pending.contains(session);
                        match self.client.resize(session, cols, rows, redraw) {
                            Ok(()) => {
                                self.visible.insert(pane.clone());
                                self.pending_resize.remove(session);
                                self.redraw_pending.remove(session);
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

    /// Drive the source as the worker does (reconcile, then one event) until
    /// `done`, collecting each session's terminal output.
    async fn pump_until(
        source: &mut SourceRuntime,
        window: &mut WindowWorkspace,
        output: &mut HashMap<String, Vec<u8>>,
        what: &str,
        mut done: impl FnMut(&SourceRuntime, &WindowWorkspace, &HashMap<String, Vec<u8>>) -> bool,
    ) {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                source.reconcile(window).unwrap();
                if done(source, window, output) {
                    break;
                }
                let event = source.client.next_event().await.unwrap().unwrap();
                if let Some(chunk) = event.output() {
                    output
                        .entry(chunk.session_id.to_owned())
                        .or_default()
                        .extend_from_slice(chunk.bytes);
                }
                source.event(window, Box::new(event)).unwrap();
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|part| part == needle)
    }

    /// What the window sends after a route refresh: the shown panes, then
    /// each pane's measured size, as separate commands.
    fn report_layout(
        source: &mut SourceRuntime,
        window: &mut WindowWorkspace,
        id: SourceId,
        screen: &ScreenKey,
        sizes: &[(&str, u16, u16)],
    ) {
        let model = window.source(id).unwrap();
        let token = model.source_token();
        let generation = model.route_generation();
        let routes: Vec<_> = sizes
            .iter()
            .map(|(pane, cols, rows)| {
                let session = model.session_for(pane).unwrap().to_owned();
                (TerminalToken::new(token, pane, &session), *cols, *rows)
            })
            .collect();
        source
            .command(
                window,
                Command::new(
                    token,
                    CommandKind::Visible {
                        screen: Some(screen.clone()),
                        route_generation: generation,
                        panes: sizes.iter().map(|(pane, ..)| (*pane).to_owned()).collect(),
                    },
                ),
            )
            .unwrap();
        source.reconcile(window).unwrap();
        for (route, cols, rows) in routes {
            source
                .command(
                    window,
                    Command::new(
                        token,
                        CommandKind::ResizePane {
                            route,
                            screen: screen.clone(),
                            route_generation: generation,
                            cols,
                            rows,
                        },
                    ),
                )
                .unwrap();
            source.reconcile(window).unwrap();
        }
    }

    #[tokio::test]
    async fn host_layout_refresh_leaves_a_pane_of_unchanged_size_alone() {
        if !splitty_core::pty::testing::pty_available() {
            return;
        }
        let (runtime, path) = crate::appearance::local::tests::fixture();
        let service = crate::EmbeddedService::from_runtime(runtime);
        let client = service.connect().await.unwrap();
        let initial = client.create_screen("Screen 1").unwrap();
        let mut window = WindowWorkspace::new(uuid::Uuid::new_v4());
        window
            .ui_test_screen_order_fixture(&[(&initial.screen_id, "Local")], None)
            .unwrap();
        let id = window
            .admit_remote(window.lifecycle_token(), "layout.test")
            .unwrap();
        window.test_bind_remote_client(id, &client);
        let token = window.source(id).unwrap().source_token();
        window.confirm_ready(token).unwrap();
        let target = window.host(id).unwrap().action_target().clone();
        let mut source = SourceRuntime::new(client, token);
        let mut output = HashMap::new();
        pump_until(
            &mut source,
            &mut window,
            &mut output,
            "the host catalog",
            |source, window, _| {
                window
                    .source(id)
                    .unwrap()
                    .live_catalog()
                    .is_some_and(|catalog| !catalog.is_empty())
                    && !source.observation_pending(window)
            },
        )
        .await;
        window.host_mut(&target).unwrap().catalog = crate::workspace::CatalogState::Ready;
        let screen = window.source(id).unwrap().screen_key(&initial.screen_id);
        window.open_existing(token, &screen).unwrap();
        if window.shown_key().as_ref() != Some(&screen) {
            window.select(window.lifecycle_token(), &screen).unwrap();
        }
        assert_eq!(
            window.source(id).unwrap().kind(),
            SourceKind::Remote,
            "every layout operation refreshes a host source's routes"
        );
        let attached = |count: usize| {
            move |source: &SourceRuntime, window: &WindowWorkspace, _: &HashMap<_, _>| {
                let model = window.source(id).unwrap();
                let routes = model.routes();
                routes.len() == count
                    && routes
                        .iter()
                        .all(|route| source.attached.contains(&route.session_id))
                    && !source.observation_pending(window)
            }
        };
        pump_until(
            &mut source,
            &mut window,
            &mut output,
            "the first pane",
            attached(1),
        )
        .await;
        let pane_a = initial.pane_id.clone();
        source
            .command(
                &mut window,
                Command::new(
                    token,
                    CommandKind::Split {
                        screen: screen.clone(),
                        pane: pane_a.clone(),
                        axis: Axis::Horizontal,
                    },
                ),
            )
            .unwrap();
        pump_until(
            &mut source,
            &mut window,
            &mut output,
            "the split",
            attached(2),
        )
        .await;
        let model = window.source(id).unwrap();
        let pane_b = model
            .routes()
            .into_iter()
            .find(|route| route.pane_id != pane_a)
            .unwrap();
        report_layout(
            &mut source,
            &mut window,
            id,
            &screen,
            &[(&pane_a, 50, 30), (&pane_b.pane_id, 50, 30)],
        );
        // A program that prints each SIGWINCH it gets, then each input line.
        let watcher = br#"exec sh -c 'trap "printf \"W%sH_%s\\n\" INC \"\$(stty size)\"" WINCH; printf "RE%sY\\n" AD; while :; do read -r line && printf "GOT_%s\\n" "$line"; done'"#;
        let input = |bytes: Vec<u8>| {
            Command::new(
                token,
                CommandKind::Input {
                    route: TerminalToken::new(token, &pane_b.pane_id, &pane_b.session_id),
                    bytes,
                },
            )
        };
        source
            .command(&mut window, input([&watcher[..], b"\n"].concat()))
            .unwrap();
        let session_b = pane_b.session_id.clone();
        pump_until(
            &mut source,
            &mut window,
            &mut output,
            "the watcher",
            |_, _, output| {
                output
                    .get(&session_b)
                    .is_some_and(|bytes| contains(bytes, b"READY"))
            },
        )
        .await;
        output.remove(&session_b);

        // Splitting the other pane changes nothing about this one's size.
        source
            .command(
                &mut window,
                Command::new(
                    token,
                    CommandKind::Split {
                        screen: screen.clone(),
                        pane: pane_a.clone(),
                        axis: Axis::Vertical,
                    },
                ),
            )
            .unwrap();
        pump_until(
            &mut source,
            &mut window,
            &mut output,
            "the second split",
            attached(3),
        )
        .await;
        let model = window.source(id).unwrap();
        let pane_c = model
            .routes()
            .into_iter()
            .find(|route| route.pane_id != pane_a && route.pane_id != pane_b.pane_id)
            .unwrap();
        report_layout(
            &mut source,
            &mut window,
            id,
            &screen,
            &[
                (&pane_a, 50, 15),
                (&pane_c.pane_id, 50, 14),
                (&pane_b.pane_id, 50, 30),
            ],
        );
        source
            .command(&mut window, input(b"FIRST\n".to_vec()))
            .unwrap();
        pump_until(
            &mut source,
            &mut window,
            &mut output,
            "the first line",
            |_, _, output| {
                output
                    .get(&session_b)
                    .is_some_and(|bytes| contains(bytes, b"GOT_FIRST"))
            },
        )
        .await;
        let seen = String::from_utf8_lossy(&output[&session_b]).into_owned();
        assert!(
            !seen.contains("WINCH_"),
            "a pane whose size did not change got SIGWINCH: {seen:?}"
        );

        // A real change of its size still reaches the PTY.
        report_layout(
            &mut source,
            &mut window,
            id,
            &screen,
            &[
                (&pane_a, 50, 15),
                (&pane_c.pane_id, 50, 14),
                (&pane_b.pane_id, 50, 29),
            ],
        );
        source
            .command(&mut window, input(b"SECOND\n".to_vec()))
            .unwrap();
        pump_until(
            &mut source,
            &mut window,
            &mut output,
            "the second line",
            |_, _, output| {
                output
                    .get(&session_b)
                    .is_some_and(|bytes| contains(bytes, b"GOT_SECOND"))
            },
        )
        .await;
        let seen = String::from_utf8_lossy(&output[&session_b]).into_owned();
        assert!(seen.contains("WINCH_29 50"), "{seen:?}");
        source.client.finish().await.unwrap();
        service.close().await.unwrap();
        let _ = std::fs::remove_dir_all(path);
    }

    /// [`pump_until`], reporting `pane` (its id and size) as shown again
    /// after every route refresh, as the window measures its panes then.
    #[allow(clippy::too_many_arguments)]
    async fn pump_reporting(
        source: &mut SourceRuntime,
        window: &mut WindowWorkspace,
        output: &mut HashMap<String, Vec<u8>>,
        id: SourceId,
        screen: &ScreenKey,
        pane: (&str, u16, u16),
        what: &str,
        mut done: impl FnMut(&SourceRuntime, &WindowWorkspace, &HashMap<String, Vec<u8>>) -> bool,
    ) {
        let mut reported = window.source(id).unwrap().route_generation();
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                source.reconcile(window).unwrap();
                if done(source, window, output) {
                    break;
                }
                let event = source.client.next_event().await.unwrap().unwrap();
                if let Some(chunk) = event.output() {
                    output
                        .entry(chunk.session_id.to_owned())
                        .or_default()
                        .extend_from_slice(chunk.bytes);
                }
                source.event(window, Box::new(event)).unwrap();
                let generation = window.source(id).unwrap().route_generation();
                if generation != reported && !source.observation_pending(window) {
                    // The worker acts on the refresh before the window's
                    // measurement of the new routes reaches it.
                    source.reconcile(window).unwrap();
                    report_layout(source, window, id, screen, &[pane]);
                    reported = generation;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    /// A program's title change makes the host publish the pane's metadata,
    /// and the host source refreshes its routes for it while nobody touches
    /// the window. With `other_viewer`, a second connection shows the session
    /// at another size and is idle, so the PTY follows this window's report
    /// only as long as the window keeps it.
    async fn host_metadata_refresh_leaves_pane_sizes_alone(other_viewer: bool) {
        if !splitty_core::pty::testing::pty_available() {
            return;
        }
        let (runtime, path) = crate::appearance::local::tests::fixture();
        let host = runtime.state.clone();
        let service = crate::EmbeddedService::from_runtime(runtime);
        let client = service.connect().await.unwrap();
        let initial = client.create_screen("Screen 1").unwrap();
        let mut window = WindowWorkspace::new(uuid::Uuid::new_v4());
        window
            .ui_test_screen_order_fixture(&[(&initial.screen_id, "Local")], None)
            .unwrap();
        let id = window
            .admit_remote(window.lifecycle_token(), "metadata.test")
            .unwrap();
        window.test_bind_remote_client(id, &client);
        let token = window.source(id).unwrap().source_token();
        window.confirm_ready(token).unwrap();
        let target = window.host(id).unwrap().action_target().clone();
        let mut source = SourceRuntime::new(client, token);
        let mut output = HashMap::new();
        pump_until(
            &mut source,
            &mut window,
            &mut output,
            "the host catalog",
            |source, window, _| {
                window
                    .source(id)
                    .unwrap()
                    .live_catalog()
                    .is_some_and(|catalog| !catalog.is_empty())
                    && !source.observation_pending(window)
            },
        )
        .await;
        window.host_mut(&target).unwrap().catalog = crate::workspace::CatalogState::Ready;
        let screen = window.source(id).unwrap().screen_key(&initial.screen_id);
        window.open_existing(token, &screen).unwrap();
        if window.shown_key().as_ref() != Some(&screen) {
            window.select(window.lifecycle_token(), &screen).unwrap();
        }
        let pane = initial.pane_id.clone();
        pump_until(
            &mut source,
            &mut window,
            &mut output,
            "the pane",
            |source, window, _| {
                let model = window.source(id).unwrap();
                model.routes().len() == 1
                    && source.attached.contains(&model.routes()[0].session_id)
                    && !source.observation_pending(window)
            },
        )
        .await;
        let session = window.source(id).unwrap().routes()[0].session_id.clone();
        report_layout(&mut source, &mut window, id, &screen, &[(&pane, 50, 30)]);

        let mut other = None;
        if other_viewer {
            let mut viewer = service.connect().await.unwrap();
            viewer.attach(&session).unwrap();
            let mut sizes = 0;
            tokio::time::timeout(std::time::Duration::from_secs(15), async {
                while sizes < 2 {
                    let event = viewer.next_event().await.unwrap().unwrap();
                    match event.json().and_then(|json| json["type"].as_str()) {
                        Some("attached") => {
                            viewer.resize(&session, 60, 20, false).unwrap();
                            viewer.idle().unwrap();
                            // Answered once the idle before it counts.
                            viewer.resize(&session, 60, 20, false).unwrap();
                        }
                        Some("size") => sizes += 1,
                        _ => {}
                    }
                }
            })
            .await
            .expect("the other viewer's size");
            other = Some(viewer);
        }

        // A program that prints each SIGWINCH it gets, then each input line.
        let watcher = br#"exec sh -c 'trap "printf \"W%sH_%s\\n\" INC \"\$(stty size)\"" WINCH; printf "RE%sY\\n" AD; while :; do read -r line && printf "GOT_%s\\n" "$line"; done'"#;
        let input = |bytes: Vec<u8>| {
            Command::new(
                token,
                CommandKind::Input {
                    route: TerminalToken::new(token, &pane, &session),
                    bytes,
                },
            )
        };
        source
            .command(&mut window, input([&watcher[..], b"\n"].concat()))
            .unwrap();
        let seen = |needle: &'static [u8]| {
            let session = session.clone();
            move |_: &SourceRuntime, _: &WindowWorkspace, output: &HashMap<String, Vec<u8>>| {
                output
                    .get(&session)
                    .is_some_and(|bytes| contains(bytes, needle))
            }
        };
        pump_reporting(
            &mut source,
            &mut window,
            &mut output,
            id,
            &screen,
            (&pane, 50, 30),
            "the watcher",
            seen(b"READY"),
        )
        .await;
        output.remove(&session);
        let before = window.source(id).unwrap().route_generation();

        // What the host's pane observer does when the program changes the
        // terminal title, as such programs do in bursts while they wait.
        for title in ["Working", "Working.", "Working.."] {
            host.mux_store
                .dispatch(splitty_core::mux::Op::UpdateSessionMetadata {
                    session_id: session.as_str().into(),
                    display_title: title.into(),
                    cwd: String::new(),
                    busy: true,
                    exited: false,
                    exit_code: None,
                    close: false,
                })
                .unwrap();
            host.client_state
                .notify_tx
                .send(&splitty_core::protocol::ServerEvent::mux_nudge(
                    "session_metadata".into(),
                    Some(initial.screen_id.clone()),
                ));
        }
        pump_reporting(
            &mut source,
            &mut window,
            &mut output,
            id,
            &screen,
            (&pane, 50, 30),
            "the title's refresh",
            |source, window, _| {
                window.source(id).unwrap().route_generation() > before
                    && !source.observation_pending(window)
            },
        )
        .await;
        source
            .command(&mut window, input(b"PING\n".to_vec()))
            .unwrap();
        pump_reporting(
            &mut source,
            &mut window,
            &mut output,
            id,
            &screen,
            (&pane, 50, 30),
            "the next line",
            seen(b"GOT_PING"),
        )
        .await;
        let seen = String::from_utf8_lossy(&output[&session]).into_owned();
        assert!(
            !seen.contains("WINCH_"),
            "a refresh for a title change resized the PTY: {seen:?}"
        );
        drop(other);
        source.client.finish().await.unwrap();
        service.close().await.unwrap();
        let _ = std::fs::remove_dir_all(path);
    }

    #[tokio::test]
    async fn host_metadata_refresh_leaves_a_pane_of_unchanged_size_alone() {
        host_metadata_refresh_leaves_pane_sizes_alone(false).await;
    }

    #[tokio::test]
    async fn host_metadata_refresh_keeps_the_size_report_of_a_shown_pane() {
        host_metadata_refresh_leaves_pane_sizes_alone(true).await;
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
impl SourceRuntime {
    /// The service's answer to a template extraction, as the draft the
    /// window reviews before the local service stores it.
    fn template_extracted(&mut self, event: &Event, out: &mut Vec<Update>) {
        let Some(value) = event
            .json()
            .filter(|value| value["type"] == "mux.template_extracted")
        else {
            return;
        };
        let Some((owner, request_id)) = value["requestId"]
            .as_str()
            .and_then(|id| self.extractions.remove(id))
        else {
            return;
        };
        let budget = self.client.budget();
        let draft = match value["error"].as_str() {
            Some(error) => Err(io::Error::other(
                error.chars().take(512).collect::<String>(),
            )),
            None => serde_json::to_vec(&value["template"])
                .map_err(io::Error::other)
                .and_then(|bytes| {
                    let lease = budget
                        .reserve(bytes.len().saturating_mul(8), false)
                        .map_err(io::Error::other)?;
                    let template = serde_json::from_slice(&bytes).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "invalid template")
                    })?;
                    crate::templates::TemplateDraft::new(
                        owner,
                        request_id,
                        lease.charge(template),
                        budget.clone(),
                    )
                }),
        };
        out.push(Update::TemplateApi(match draft {
            Ok(draft) => TemplateApiResult {
                connection_id: owner,
                request_id,
                outcome: crate::templates::TemplateApiOutcome::Draft(Arc::new(draft)),
            },
            Err(error) => TemplateApiResult::rejected(owner, request_id, error.to_string()),
        }));
    }
}
