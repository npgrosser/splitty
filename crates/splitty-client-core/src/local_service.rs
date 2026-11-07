//! One owned API stream for local settings reads and ordered zoom intents, separate from PTY events.
use crate::{
    appearance::{AppearanceSnapshot, ColorScheme},
    shell_profiles::ShellProfilesSnapshot,
    zoom::{ZoomAction, ZoomResult},
};
use serde::de::DeserializeOwned;
use splitty_server::native::client::Client;
use splitty_transport::{budget::Charged, codec, wire};
use std::{collections::VecDeque, future::Future, io, pin::Pin, sync::Arc};
use tokio::sync::Notify;
use uuid::Uuid;

pub enum LocalServiceUpdate {
    Template(crate::templates::TemplateApiResult),
    Zoom(ZoomResult),
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
#[derive(Clone)]
enum Job {
    Read(Feature, u64),
    Zoom(Uuid, ZoomAction),
    Template(Uuid, crate::templates::TemplateApiRequest),
}
type Read = Pin<Box<dyn Future<Output = (Client, Job, LocalServiceUpdate)> + Send>>;

/// `next_update` may be cancelled without cancelling the retained request. Each
/// feature coalesces its own refreshes; completing even a superseded read gives
/// the other dirty feature priority. Read failures wait for another refresh;
/// mutations are submitted at most once and never automatically retried.
pub struct LocalServiceClient {
    client: Option<Client>,
    pending: Option<Read>,
    active_zoom: Option<Uuid>,
    template: Option<(Uuid, crate::templates::TemplateApiRequest)>,
    active_template: Option<Uuid>,
    last_template: bool,
    zoom: VecDeque<(Uuid, ZoomAction)>,
    prefer_write: bool,
    cancel: Arc<Notify>,
    connection_id: Uuid,
    generations: [u64; 2],
    dirty: [bool; 2],
    next: Feature,
    scheme: ColorScheme,
}
impl LocalServiceClient {
    pub(crate) fn new(client: Client, connection_id: Uuid, scheme: ColorScheme) -> Self {
        Self {
            client: Some(client),
            pending: None,
            active_zoom: None,
            template: None,
            active_template: None,
            last_template: false,
            zoom: VecDeque::new(),
            prefer_write: false,
            cancel: Arc::new(Notify::new()),
            connection_id,
            generations: [1; 2],
            dirty: [true; 2],
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
    pub fn refresh_settings(&mut self, owner: Uuid) -> io::Result<()> {
        self.check_owner(owner)?;
        let generations = [
            increment(self.generations[0])?,
            increment(self.generations[1])?,
        ];
        self.generations = generations;
        self.dirty = [true; 2];
        Ok(())
    }
    pub fn set_scheme(&mut self, owner: Uuid, scheme: ColorScheme) -> io::Result<()> {
        self.check_owner(owner)?;
        if scheme != self.scheme {
            self.generations[0] = increment(self.generations[0])?;
            self.scheme = scheme;
            self.dirty[0] = true;
        }
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
        loop {
            if self.pending.is_none() {
                let feature = if self.dirty[self.next.index()] {
                    Some(self.next)
                } else if self.dirty[self.next.other().index()] {
                    Some(self.next.other())
                } else {
                    None
                };
                let job = if self.template.is_some()
                    && (!self.last_template || (self.zoom.is_empty() && feature.is_none()))
                {
                    let (id, request) = self.template.take().expect("queued template");
                    self.active_template = Some(id);
                    Job::Template(id, request)
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
                self.pending = Some(Box::pin(async move {
                    let cancelled = || {
                        io::Error::new(io::ErrorKind::Interrupted, "local settings read cancelled")
                    };
                    let result = match job.clone() {
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
                                result = crate::appearance::local::read(&mut client, connection, generation, scheme) => result.map(Arc::new),
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
                    };
                    (client, job, result)
                }));
            }
            let (client, job, result) = self.pending.as_mut().expect("settings job started").await;
            self.pending = None;
            self.client = Some(client);
            self.last_template = matches!(job, Job::Template(..));
            match job {
                Job::Template(..) => {
                    self.active_template = None;
                }
                Job::Read(feature, generation) => {
                    self.next = feature.other();
                    self.prefer_write = true;
                    if generation != self.generations[feature.index()] {
                        continue;
                    }
                }
                Job::Zoom(..) => {
                    self.active_zoom = None;
                    self.prefer_write = false;
                    // There is no concurrent read: every later appearance starts
                    // after this outcome, even when a settings event raced it.
                    self.dirty[0] = true;
                }
            }
            return result;
        }
    }
    pub async fn finish(mut self) -> io::Result<()> {
        if let Some(pending) = self.pending.take() {
            self.cancel.notify_one();
            let (client, _, _) = pending.await;
            self.client = Some(client);
        }
        if let Some(client) = self.client.take() {
            client.finish().await?;
        }
        Ok(())
    }
}
fn increment(generation: u64) -> io::Result<u64> {
    generation
        .checked_add(1)
        .ok_or_else(|| io::Error::other("local settings generation exhausted"))
}

/// The reply lease remains alive during charged typed decoding; neither feature
/// copies an unadmitted JSON body. Error text never retains peer-controlled data.
pub(crate) fn decode<T: DeserializeOwned>(
    client: &Client,
    reply: &wire::Reply,
) -> io::Result<Charged<T>> {
    decode_with_budget(&client.sender().context().budget, reply)
}

pub(crate) fn decode_with_budget<T: DeserializeOwned>(
    budget: &splitty_transport::budget::ConnectionBudget,
    reply: &wire::Reply,
) -> io::Result<Charged<T>> {
    if !(200..300).contains(&reply.status) || reply.content_type != wire::ContentType::Json {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("local settings API returned status {}", reply.status),
        ));
    }
    let bytes = reply.body.as_bytes();
    if bytes.len() > 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local settings response exceeds native limit",
        ));
    }
    let lease = codec::reserve_json_typed(bytes, budget, false)?;
    let value = serde_json::from_slice(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid local settings response",
        )
    })?;
    Ok(lease.charge(value))
}

#[cfg(test)]
mod tests;

mod zoom;

#[cfg(all(test, unix))]
mod zoom_tests;
