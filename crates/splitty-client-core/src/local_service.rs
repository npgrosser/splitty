//! One owned API stream for local settings reads and bounded mutations, separate from PTY events.
use crate::api_stream::ApiStream as Client;
use crate::{
    appearance::{
        AppearanceSnapshot, ColorScheme, DrawerVisibilityResult, SettingsEdit, SettingsField,
    },
    shell_profiles::ShellProfilesSnapshot,
    zoom::{ZoomAction, ZoomResult},
};
use std::{collections::VecDeque, future::Future, io, pin::Pin, sync::Arc};
use tokio::sync::Notify;
use uuid::Uuid;

mod settings;
pub use settings::{SettingsReconciled, SettingsWriteOutcome, SettingsWriteResult};

pub enum LocalServiceUpdate {
    BackendVersion {
        connection: Uuid,
        result: io::Result<Arc<splitty_transport::budget::Charged<String>>>,
    },
    SettingsWrite(SettingsWriteResult),
    SettingsReconciled(SettingsReconciled),
    Template(crate::templates::TemplateApiResult),
    Zoom(ZoomResult),
    DrawerVisibility(DrawerVisibilityResult),
    Appearance(io::Result<Arc<AppearanceSnapshot>>),
    ShellProfiles(io::Result<Arc<ShellProfilesSnapshot>>),
}

#[derive(Clone, Copy)]
enum Feature {
    Appearance,
    Profiles,
}
impl Feature {
    fn index(self) -> usize {
        match self {
            Self::Appearance => 0,
            Self::Profiles => 1,
        }
    }
    fn other(self) -> Self {
        match self {
            Self::Appearance => Self::Profiles,
            Self::Profiles => Self::Appearance,
        }
    }
}
const MAX_QUEUED_ZOOM: usize = 32;
const MAX_QUEUED_SETTINGS: usize = 16;
#[derive(Clone)]
enum Job {
    BackendVersion,
    Settings(Uuid, SettingsEdit, u64),
    Read(Feature, u64),
    Zoom(Uuid, ZoomAction),
    DrawerVisibility(Uuid, bool),
    Template(Uuid, crate::templates::TemplateApiRequest),
}
type Read = Pin<Box<dyn Future<Output = (Client, Job, LocalServiceUpdate, bool)> + Send>>;

/// `next_update` may be cancelled without cancelling the retained request. Each
/// feature coalesces its own refreshes; completing even a superseded read gives
/// the other dirty feature priority. Read failures wait for another refresh;
/// mutations are submitted at most once and never automatically retried.
pub struct LocalServiceClient {
    client: Option<Client>,
    ready: VecDeque<(u64, LocalServiceUpdate)>,
    settings: VecDeque<(Uuid, SettingsEdit)>,
    active_settings: Option<(Uuid, SettingsField)>,
    uncertain_settings: Vec<(SettingsField, Uuid)>,
    uncertain_legacy: Vec<SettingsField>,
    last_appearance: Option<Arc<AppearanceSnapshot>>,
    terminal_generation: u64,
    pending: Option<Read>,
    active_zoom: Option<Uuid>,
    drawer_visibility: Option<(Uuid, bool)>,
    active_drawer_visibility: Option<Uuid>,
    prefer_drawer_visibility: bool,
    template: Option<(Uuid, crate::templates::TemplateApiRequest)>,
    active_template: Option<Uuid>,
    last_template: bool,
    zoom: VecDeque<(Uuid, ZoomAction)>,
    prefer_write: bool,
    cancel: Arc<Notify>,
    connection_id: Uuid,
    generations: [u64; 2],
    dirty: [bool; 2],
    build_dirty: bool,
    build_pending: bool,
    next: Feature,
    scheme: ColorScheme,
}
impl LocalServiceClient {
    pub(crate) fn new(client: impl Into<Client>, connection_id: Uuid, scheme: ColorScheme) -> Self {
        Self {
            client: Some(client.into()),
            ready: VecDeque::new(),
            settings: VecDeque::new(),
            active_settings: None,
            uncertain_settings: Vec::new(),
            uncertain_legacy: Vec::new(),
            last_appearance: None,
            terminal_generation: 1,
            pending: None,
            active_zoom: None,
            drawer_visibility: None,
            active_drawer_visibility: None,
            prefer_drawer_visibility: false,
            template: None,
            active_template: None,
            last_template: false,
            zoom: VecDeque::new(),
            prefer_write: false,
            cancel: Arc::new(Notify::new()),
            connection_id,
            generations: [1; 2],
            dirty: [true; 2],
            build_dirty: false,
            build_pending: false,
            next: Feature::Appearance,
            scheme,
        }
    }
    fn check_owner(&self, owner: Uuid) -> io::Result<()> {
        if owner != self.connection_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "local settings source changed",
            ));
        }
        Ok(())
    }
    pub fn refresh_backend_version(&mut self, owner: Uuid) -> io::Result<()> {
        self.check_owner(owner)?;
        if !self.build_pending {
            self.build_dirty = true;
        }
        Ok(())
    }
    pub fn refresh_settings(&mut self, owner: Uuid) -> io::Result<()> {
        self.check_owner(owner)?;
        let generations = [
            increment(self.generations[0])?,
            increment(self.generations[1])?,
        ];
        self.terminal_generation = increment(self.terminal_generation)?;
        self.generations = generations;
        self.dirty = [true; 2];
        Ok(())
    }
    pub fn set_scheme(&mut self, owner: Uuid, scheme: ColorScheme) -> io::Result<()> {
        self.check_owner(owner)?;
        if scheme != self.scheme {
            self.generations[0] = increment(self.generations[0])?;
            self.terminal_generation = increment(self.terminal_generation)?;
            self.scheme = scheme;
            self.dirty[0] = true;
        }
        Ok(())
    }
    fn field_pending(&self, field: SettingsField) -> bool {
        self.uncertain_legacy.contains(&field)
            || self
                .active_settings
                .is_some_and(|(_, active)| active == field)
            || self.settings.iter().any(|(_, edit)| edit.field() == field)
            || self
                .uncertain_settings
                .iter()
                .any(|(blocked, _)| *blocked == field)
    }
    pub fn enqueue_settings(
        &mut self,
        owner: Uuid,
        request: Uuid,
        edit: SettingsEdit,
    ) -> io::Result<()> {
        self.check_owner(owner)?;
        let field = edit.field();
        if self.active_settings.is_some_and(|(id, _)| id == request)
            || self.settings.iter().any(|(id, _)| *id == request)
            || self.uncertain_settings.iter().any(|(_, id)| *id == request)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Settings request is already pending",
            ));
        }
        if self.settings.len() + usize::from(self.active_settings.is_some()) >= MAX_QUEUED_SETTINGS
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Settings queue is full",
            ));
        }
        if self.field_pending(field)
            || (field == SettingsField::DrawerPinned
                && (self.drawer_visibility.is_some() || self.active_drawer_visibility.is_some()))
            || (field == SettingsField::Zoom
                && (!self.zoom.is_empty() || self.active_zoom.is_some()))
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "This setting is pending or awaiting reconciliation",
            ));
        }
        self.last_appearance
            .as_ref()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::WouldBlock, "Local settings are not ready")
            })?
            .validate_edit(&edit)?;
        self.settings.push_back((request, edit));
        Ok(())
    }
    /// Admit one action once. Queue rejection never starts an API request.
    pub fn enqueue_zoom(
        &mut self,
        owner: Uuid,
        request: Uuid,
        action: ZoomAction,
    ) -> io::Result<()> {
        self.check_owner(owner)?;
        if self.field_pending(SettingsField::Zoom) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Display zoom is pending or awaiting reconciliation",
            ));
        }
        if self.active_zoom == Some(request) || self.zoom.iter().any(|(id, _)| *id == request) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "zoom request is already pending",
            ));
        }
        if self.zoom.len() == MAX_QUEUED_ZOOM {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "zoom queue is full",
            ));
        }
        self.zoom.push_back((request, action));
        Ok(())
    }
    /// Admit one visibility change. A second intent waits for the first outcome.
    pub fn enqueue_drawer_visibility(
        &mut self,
        owner: Uuid,
        request: Uuid,
        pinned: bool,
    ) -> io::Result<()> {
        self.check_owner(owner)?;
        if self.field_pending(SettingsField::DrawerPinned) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Drawer visibility is pending or awaiting reconciliation",
            ));
        }
        if self.active_drawer_visibility == Some(request)
            || self.drawer_visibility.is_some_and(|(id, _)| id == request)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "drawer visibility request is already pending",
            ));
        }
        if self.drawer_visibility.is_some() || self.active_drawer_visibility.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "a drawer visibility update is pending",
            ));
        }
        self.drawer_visibility = Some((request, pinned));
        Ok(())
    }
    pub fn enqueue_template(
        &mut self,
        owner: Uuid,
        id: Uuid,
        request: crate::templates::TemplateApiRequest,
    ) -> io::Result<()> {
        self.check_owner(owner)?;
        if self.active_template == Some(id)
            || self
                .template
                .as_ref()
                .is_some_and(|(pending, _)| *pending == id)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Template request is already pending",
            ));
        }
        request.validate(owner)?;
        if self.template.is_some() || self.active_template.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "A template library operation is pending",
            ));
        }
        self.template = Some((id, request));
        Ok(())
    }
    pub async fn next_update(&mut self) -> LocalServiceUpdate {
        if let Some((generation, update)) = self.ready.pop_front() {
            if generation == self.generations[0] {
                return update;
            }
        }
        loop {
            if self.pending.is_none() {
                let feature = if self.dirty[self.next.index()] {
                    Some(self.next)
                } else if self.dirty[self.next.other().index()] {
                    Some(self.next.other())
                } else {
                    None
                };
                let job = if self.build_dirty {
                    self.build_dirty = false;
                    self.build_pending = true;
                    Job::BackendVersion
                } else if self.template.is_some()
                    && (!self.last_template
                        || (self.zoom.is_empty()
                            && self.settings.is_empty()
                            && self.drawer_visibility.is_none()
                            && feature.is_none()))
                {
                    let (id, request) = self.template.take().expect("queued template");
                    self.active_template = Some(id);
                    Job::Template(id, request)
                } else if !self.settings.is_empty() && (self.prefer_write || feature.is_none()) {
                    let (id, edit) = self.settings.pop_front().expect("queued settings");
                    let generation = match increment(self.generations[0]) {
                        Ok(generation) => generation,
                        Err(error) => {
                            return LocalServiceUpdate::SettingsWrite(
                                SettingsWriteResult::rejected(
                                    self.connection_id,
                                    id,
                                    edit.field(),
                                    error.to_string(),
                                ),
                            )
                        }
                    };
                    self.generations[0] = generation;
                    self.terminal_generation = generation.max(self.terminal_generation);
                    self.active_settings = Some((id, edit.field()));
                    Job::Settings(id, edit, generation)
                } else if self.drawer_visibility.is_some()
                    && (self.prefer_write || feature.is_none())
                    && (self.prefer_drawer_visibility || self.zoom.is_empty())
                {
                    let generation = match increment(self.generations[0]) {
                        Ok(generation) => generation,
                        Err(error) => {
                            let (id, _) = self
                                .drawer_visibility
                                .take()
                                .expect("queued drawer visibility");
                            return LocalServiceUpdate::DrawerVisibility(
                                DrawerVisibilityResult::rejected(
                                    self.connection_id,
                                    id,
                                    error.to_string(),
                                ),
                            );
                        }
                    };
                    self.generations[0] = generation;
                    let (id, pinned) = self
                        .drawer_visibility
                        .take()
                        .expect("queued drawer visibility");
                    self.active_drawer_visibility = Some(id);
                    Job::DrawerVisibility(id, pinned)
                } else if !self.zoom.is_empty() && (self.prefer_write || feature.is_none()) {
                    let (id, action) = self.zoom.pop_front().expect("queued zoom");
                    // Reserve a generation before starting this indivisible job.
                    // No read can publish it until after the mutation outcome.
                    let generation = match increment(self.generations[0]) {
                        Ok(generation) => generation,
                        Err(error) => {
                            return LocalServiceUpdate::Zoom(ZoomResult::rejected(
                                self.connection_id,
                                id,
                                error.to_string(),
                            ))
                        }
                    };
                    self.generations[0] = generation;
                    self.active_zoom = Some(id);
                    Job::Zoom(id, action)
                } else if let Some(feature) = feature {
                    self.dirty[feature.index()] = false;
                    Job::Read(feature, self.generations[feature.index()])
                } else {
                    std::future::pending::<Job>().await
                };
                let mut client = self
                    .client
                    .take()
                    .expect("local settings client owns its stream");
                let cancel = self.cancel.clone();
                let connection = self.connection_id;
                let scheme = self.scheme;
                let terminal_generation = self.terminal_generation;
                let previous = self.last_appearance.clone();
                self.pending = Some(Box::pin(async move {
                    let cancelled = || {
                        io::Error::new(io::ErrorKind::Interrupted, "local settings read cancelled")
                    };
                    let mut home_read = false;
                    let result = match job.clone() {
                        Job::BackendVersion => LocalServiceUpdate::BackendVersion {
                            connection,
                            result: tokio::select! { biased;
                                _ = cancel.notified() => Err(cancelled()),
                                result = read_backend_version(&mut client) => result,
                            },
                        },
                        Job::Settings(id, edit, generation) => LocalServiceUpdate::SettingsWrite(
                            settings::execute(
                                &mut client,
                                connection,
                                id,
                                edit,
                                generation,
                                &cancel,
                            )
                            .await,
                        ),
                        Job::Template(id, request) => LocalServiceUpdate::Template(
                            crate::templates::execute(
                                &mut client,
                                connection,
                                id,
                                request,
                                &cancel,
                            )
                            .await,
                        ),
                        Job::Read(Feature::Appearance, generation) => {
                            LocalServiceUpdate::Appearance(tokio::select! { biased;
                                _ = cancel.notified() => Err(cancelled()),
                                result = crate::appearance::local::read(&mut client, connection, generation, scheme, terminal_generation, previous, &mut home_read) => result.map(Arc::new),
                            })
                        }
                        Job::Read(Feature::Profiles, generation) => {
                            LocalServiceUpdate::ShellProfiles(tokio::select! { biased;
                                _ = cancel.notified() => Err(cancelled()),
                                result = crate::shell_profiles::read(&mut client, connection, generation) => result.map(Arc::new),
                            })
                        }
                        Job::Zoom(id, action) => LocalServiceUpdate::Zoom(
                            zoom::execute(&mut client, connection, id, action, &cancel).await,
                        ),
                        Job::DrawerVisibility(id, pinned) => LocalServiceUpdate::DrawerVisibility(
                            drawer_visibility::execute(
                                &mut client,
                                connection,
                                id,
                                pinned,
                                &cancel,
                            )
                            .await,
                        ),
                    };
                    (client, job, result, home_read)
                }));
            }
            let (client, job, mut result, home_read) =
                self.pending.as_mut().expect("settings job started").await;
            self.pending = None;
            self.client = Some(client);
            self.last_template = matches!(job, Job::Template(..));
            match job {
                Job::BackendVersion => {
                    self.build_pending = false;
                }
                Job::Settings(id, edit, _) => {
                    self.active_settings = None;
                    self.prefer_write = false;
                    self.dirty[0] = true;
                    if let LocalServiceUpdate::SettingsWrite(outcome) = &mut result {
                        // A same-service notification invalidates reads, not a
                        // matched successful write acknowledgement. The writer's
                        // own broadcast commonly arrives before its API reply.
                        // Reserve completion for the latest subsequent read; no
                        // effective values from the mutation reply are published.
                        outcome.require_refresh(self.generations[0]);
                        if matches!(outcome.outcome(), SettingsWriteOutcome::Uncertain { .. }) {
                            self.uncertain_settings.push((edit.field(), id));
                        }
                    }
                }
                Job::Template(..) => {
                    self.active_template = None;
                }
                Job::Read(feature, generation) => {
                    self.next = feature.other();
                    self.prefer_write = true;
                    if generation != self.generations[feature.index()] {
                        continue;
                    }
                    if let LocalServiceUpdate::Appearance(Ok(snapshot)) = &mut result {
                        self.last_appearance = Some(snapshot.clone());
                    }
                }
                Job::Zoom(..) => {
                    if matches!(&result, LocalServiceUpdate::Zoom(result) if matches!(result.outcome, crate::zoom::ZoomOutcome::Uncertain { .. }))
                        && !self.uncertain_legacy.contains(&SettingsField::Zoom)
                    {
                        self.uncertain_legacy.push(SettingsField::Zoom);
                    }
                    self.active_zoom = None;
                    self.prefer_write = false;
                    self.prefer_drawer_visibility = true;
                    // There is no concurrent read: every later appearance starts
                    // after this outcome, even when a settings event raced it.
                    self.dirty[0] = true;
                }
                Job::DrawerVisibility(..) => {
                    if matches!(&result, LocalServiceUpdate::DrawerVisibility(result) if matches!(result.outcome, crate::appearance::DrawerVisibilityOutcome::Uncertain { .. }))
                        && !self.uncertain_legacy.contains(&SettingsField::DrawerPinned)
                    {
                        self.uncertain_legacy.push(SettingsField::DrawerPinned);
                    }
                    self.active_drawer_visibility = None;
                    self.prefer_write = false;
                    self.prefer_drawer_visibility = false;
                    self.dirty[0] = true;
                }
            }
            if home_read {
                self.uncertain_legacy.clear();
            }
            if home_read && !self.uncertain_settings.is_empty() {
                let reconciled = SettingsReconciled {
                    connection_id: self.connection_id,
                    generation: self.generations[0],
                    requests: std::mem::take(&mut self.uncertain_settings),
                };
                self.ready.push_back((self.generations[0], result));
                return LocalServiceUpdate::SettingsReconciled(reconciled);
            }
            return result;
        }
    }
    pub async fn finish(self) -> io::Result<()> {
        self.finish_with_outcomes().await.map(|_| ())
    }
    /// Discard queued edits, join the owned stream, and retain the active write's
    /// uncertain close outcome for callers that still have a presentation sink.
    pub async fn finish_with_outcomes(mut self) -> io::Result<Vec<SettingsWriteResult>> {
        self.settings.clear();
        let mut outcomes = Vec::new();
        if let Some(pending) = self.pending.take() {
            self.cancel.notify_one();
            let (client, _, result, _) = pending.await;
            if let LocalServiceUpdate::SettingsWrite(result) = result {
                outcomes.push(result);
            }
            self.client = Some(client);
        }
        if let Some(client) = self.client.take() {
            client.finish().await?;
        }
        Ok(outcomes)
    }
}
fn increment(generation: u64) -> io::Result<u64> {
    generation
        .checked_add(1)
        .ok_or_else(|| io::Error::other("local settings generation exhausted"))
}

pub(crate) use crate::api_stream::decode;
#[cfg(test)]
pub(crate) use crate::api_stream::decode_with_budget;

#[cfg(all(test, not(target_os = "windows")))]
mod tests;

mod drawer_visibility;
mod zoom;

#[cfg(all(test, unix))]
mod zoom_tests;

async fn read_backend_version(
    client: &mut Client,
) -> io::Result<Arc<splitty_transport::budget::Charged<String>>> {
    #[derive(serde::Deserialize)]
    struct Build {
        version: String,
    }
    let reply = client
        .request(
            splitty_core::native_api::Operation::BuildGet,
            serde_json::Map::new(),
        )
        .await?;
    let parsed = decode::<Build>(client, &reply)?;
    if parsed.version.is_empty()
        || parsed.version.len() > 64
        || parsed.version.chars().any(char::is_control)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid backend build version",
        ));
    }
    Ok(Arc::new(parsed.map(|build| build.version)))
}
