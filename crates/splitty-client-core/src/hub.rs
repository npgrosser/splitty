//! Owner-bound host directory reads and identified local metadata mutations.
use crate::api_stream::{decode, ApiStream};
use splitty_core::native_api::{directory::Directory, Operation};
use splitty_remote::ssh::SshTarget;
use splitty_transport::budget::Charged;
use std::{
    collections::{HashSet, VecDeque},
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
};
use tokio::sync::Notify;
use uuid::Uuid;

mod management;
use crate::workspace::SourceToken;
pub use management::{
    HostManagementAction, HostManagementOutcome, HostManagementRequest, HostManagementResult,
};

pub(crate) enum HubEvent {
    Directory(HubUpdate),
    Management(HostManagementResult),
}
type WritePending = Pin<Box<dyn Future<Output = (ApiStream, HostManagementResult, bool)> + Send>>;

#[derive(Debug)]
pub struct HostDirectorySnapshot {
    connection_id: Uuid,
    generation: u64,
    directory: Charged<Directory>,
}
impl HostDirectorySnapshot {
    #[cfg(feature = "ui-test-support")]
    #[doc(hidden)]
    pub fn ui_test_fixture(
        connection_id: Uuid,
        generation: u64,
        targets: &[&str],
    ) -> io::Result<Self> {
        let entries: Vec<_> = targets.iter().map(|target| (*target, None)).collect();
        Self::ui_test_fixture_with_aliases(connection_id, generation, &entries)
    }

    #[cfg(feature = "ui-test-support")]
    #[doc(hidden)]
    pub fn ui_test_fixture_with_aliases(
        connection_id: Uuid,
        generation: u64,
        entries: &[(&str, Option<&str>)],
    ) -> io::Result<Self> {
        let directory = Directory::new(
            Vec::new(),
            entries
                .iter()
                .map(|(target, alias)| {
                    let mut host = splitty_core::native_api::directory::DirectoryHost::new(*target);
                    host.alias = alias.map(str::to_owned);
                    host
                })
                .collect(),
        );
        let budget = splitty_transport::budget::ConnectionBudget::default();
        validate(&directory, &budget)?;
        let bytes = serde_json::to_vec(&directory).map_err(io::Error::other)?;
        if bytes.len() > splitty_core::native_api::MAX_API_BODY {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "host directory fixture exceeds native limit",
            ));
        }
        let lease = splitty_transport::codec::reserve_json_typed(&bytes, &budget, false)?;
        Ok(Self {
            connection_id,
            generation,
            directory: lease.charge(directory),
        })
    }
    pub fn connection_id(&self) -> Uuid {
        self.connection_id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn directory(&self) -> &Directory {
        &self.directory
    }
}

#[derive(Clone, Debug)]
pub enum HostDirectoryState {
    Unavailable,
    Loading {
        previous: Option<Arc<HostDirectorySnapshot>>,
    },
    Ready(Arc<HostDirectorySnapshot>),
    Failed {
        previous: Option<Arc<HostDirectorySnapshot>>,
        reason: String,
    },
}

/// Loading and its completion share a read generation. Consumers reject only
/// older generations and may accept a same-generation completion.
#[derive(Clone, Debug)]
pub struct HubUpdate {
    pub connection_id: Uuid,
    pub generation: u64,
    pub state: HostDirectoryState,
}
impl HubUpdate {
    pub fn unavailable(connection_id: Uuid) -> Self {
        Self {
            connection_id,
            generation: 0,
            state: HostDirectoryState::Unavailable,
        }
    }
}

type Pending =
    Pin<Box<dyn Future<Output = (ApiStream, io::Result<HostDirectorySnapshot>, bool)> + Send>>;

/// One API stream serializes bounded metadata writes and coalesced directory reads.
/// SSH connection demand remains in the workspace worker.
pub struct HubClient {
    owner: Uuid,
    generation: u64,
    dirty: bool,
    client: Option<ApiStream>,
    pending: Option<Pending>,
    pending_generation: u64,
    loading_due: bool,
    last_good: Option<Arc<HostDirectorySnapshot>>,
    broken: bool,
    reopen_due: bool,
    cancel: Arc<Notify>,
    writes: VecDeque<management::Job>,
    writing: Option<WritePending>,
    active_write: Option<(Uuid, String)>,
    write_due: bool,
    management_owner: Option<SourceToken>,
}
impl HubClient {
    #[cfg(test)]
    pub(crate) fn new(client: impl Into<ApiStream>, owner: Uuid) -> Self {
        Self::with_previous(client, owner, 1, None)
    }
    pub(crate) fn with_previous(
        client: impl Into<ApiStream>,
        owner: Uuid,
        generation: u64,
        last_good: Option<Arc<HostDirectorySnapshot>>,
    ) -> Self {
        Self {
            owner,
            generation,
            dirty: true,
            client: Some(client.into()),
            pending: None,
            pending_generation: 0,
            loading_due: false,
            last_good: last_good.filter(|snapshot| snapshot.connection_id == owner),
            broken: false,
            reopen_due: false,
            cancel: Arc::new(Notify::new()),
            writes: VecDeque::new(),
            writing: None,
            active_write: None,
            write_due: true,
            management_owner: None,
        }
    }
    pub fn connection_id(&self) -> Uuid {
        self.owner
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn is_broken(&self) -> bool {
        self.broken
    }
    pub(crate) fn take_reopen_due(&mut self) -> bool {
        std::mem::take(&mut self.reopen_due)
    }
    pub fn last_good(&self) -> Option<&Arc<HostDirectorySnapshot>> {
        self.last_good.as_ref()
    }
    pub fn refresh(&mut self, owner: Uuid) -> io::Result<()> {
        if owner != self.owner {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "hub directory source changed",
            ));
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("hub directory generation exhausted"))?;
        self.dirty = true;
        Ok(())
    }
    pub(crate) fn enqueue(
        &mut self,
        owner: SourceToken,
        id: Uuid,
        request: HostManagementRequest,
    ) -> io::Result<HostManagementResult> {
        if owner.connection_id() != self.owner
            || self
                .management_owner
                .is_some_and(|expected| expected != owner)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Host metadata owner changed",
            ));
        }
        if self.broken {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Host metadata stream unavailable",
            ));
        }
        if self.writes.len() + usize::from(self.active_write.is_some()) >= 16 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Host metadata queue is full",
            ));
        }
        if self
            .active_write
            .as_ref()
            .is_some_and(|(active, _)| *active == id)
            || self.writes.iter().any(|job| job.id == id)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Host request is already pending",
            ));
        }
        if self
            .active_write
            .as_ref()
            .is_some_and(|(_, target)| target == request.target())
            || self
                .writes
                .iter()
                .any(|job| job.request.target() == request.target())
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Another action for this host is pending",
            ));
        }
        self.refresh(self.owner)?;
        self.management_owner = Some(owner);
        let job = management::Job { owner, id, request };
        let result = job.result(HostManagementOutcome::Pending, None);
        self.writes.push_back(job);
        Ok(result)
    }
    #[cfg(test)]
    pub async fn next_update(&mut self) -> HubUpdate {
        match self.next_event().await {
            HubEvent::Directory(update) => update,
            HubEvent::Management(_) => panic!("unexpected metadata update"),
        }
    }
    pub(crate) async fn next_event(&mut self) -> HubEvent {
        loop {
            if self.writing.is_none()
                && self.pending.is_none()
                && !self.writes.is_empty()
                && (self.write_due || !self.dirty || self.broken)
            {
                let job = self.writes.pop_front().expect("queued write");
                if self.broken {
                    return HubEvent::Management(job.result(
                        HostManagementOutcome::Rejected,
                        Some("Host metadata stream unavailable; action was not submitted".into()),
                    ));
                }
                self.active_write = Some((job.id, job.request.target().into()));
                let mut client = self.client.take().expect("hub stream retained");
                self.writing = Some(Box::pin(async move {
                    let (result, broken) = management::write(&mut client, &job).await;
                    (client, result, broken)
                }));
            }
            if let Some(writing) = &mut self.writing {
                let (client, result, broken) = writing.await;
                self.writing = None;
                self.active_write = None;
                self.client = Some(client);
                self.broken = broken;
                self.refresh(self.owner).expect("same owner refresh");
                self.write_due = false;
                return HubEvent::Management(result);
            }
            if self.pending.is_none() {
                if !self.dirty {
                    std::future::pending::<()>().await;
                }
                if self.broken {
                    self.reopen_due = true;
                    return HubEvent::Directory(HubUpdate {
                        connection_id: self.owner,
                        generation: self.generation,
                        state: HostDirectoryState::Loading {
                            previous: self.last_good.clone(),
                        },
                    });
                }
                if !self.dirty {
                    std::future::pending::<()>().await;
                }
                self.dirty = false;
                let generation = self.generation;
                let owner = self.owner;
                let mut client = self.client.take().expect("hub API stream retained");
                let cancel = self.cancel.clone();
                self.pending_generation = generation;
                self.loading_due = true;
                self.pending = Some(Box::pin(async move {
                    let read = tokio::select! { biased;
                        _ = cancel.notified() => Err(io::Error::new(io::ErrorKind::Interrupted, "hub read cancelled")),
                        result = read(&mut client, owner, generation) => result,
                    };
                    let broken = read
                        .as_ref()
                        .err()
                        .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe);
                    (client, read, broken)
                }));
            }
            if self.loading_due {
                self.loading_due = false;
                return HubEvent::Directory(HubUpdate {
                    connection_id: self.owner,
                    generation: self.pending_generation,
                    state: HostDirectoryState::Loading {
                        previous: self.last_good.clone(),
                    },
                });
            }
            let (client, result, broken) = self.pending.as_mut().expect("hub read started").await;
            self.pending = None;
            self.write_due = true;
            self.client = Some(client);
            self.broken = broken;
            if self.pending_generation != self.generation {
                if broken && self.writes.is_empty() {
                    // A refresh arrived during EOF. The old stream cannot
                    // service it; the runtime must close and reopen once.
                    self.reopen_due = true;
                    return HubEvent::Directory(HubUpdate {
                        connection_id: self.owner,
                        generation: self.generation,
                        state: HostDirectoryState::Loading {
                            previous: self.last_good.clone(),
                        },
                    });
                }
                continue;
            }
            let state = match result {
                Ok(snapshot) => {
                    let snapshot = Arc::new(snapshot);
                    self.last_good = Some(snapshot.clone());
                    HostDirectoryState::Ready(snapshot)
                }
                Err(error) => HostDirectoryState::Failed {
                    previous: self.last_good.clone(),
                    reason: classify(&error),
                },
            };
            return HubEvent::Directory(HubUpdate {
                connection_id: self.owner,
                generation: self.generation,
                state,
            });
        }
    }
    pub async fn finish(mut self) -> io::Result<()> {
        self.writes.clear();
        if let Some(writing) = self.writing.take() {
            let (client, _, _) = writing.await;
            self.client = Some(client);
        }
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

fn classify(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::OutOfMemory | io::ErrorKind::WouldBlock => {
            "Host directory exceeds local capacity".into()
        }
        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset | io::ErrorKind::TimedOut => {
            "Host directory connection unavailable".into()
        }
        io::ErrorKind::Interrupted => "Host directory read cancelled".into(),
        _ => "Could not read a valid host directory".into(),
    }
}

async fn read(
    client: &mut ApiStream,
    owner: Uuid,
    generation: u64,
) -> io::Result<HostDirectorySnapshot> {
    let reply = client
        .request(Operation::HostDirectory, serde_json::Map::new())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "directory reply unavailable"))?;
    if reply.uncertain {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "uncertain directory read",
        ));
    }
    let directory = decode::<Directory>(client, &reply)?;
    validate(&directory, &client.sender().context().budget)?;
    Ok(HostDirectorySnapshot {
        connection_id: owner,
        generation,
        directory,
    })
}

fn validate(
    directory: &Directory,
    budget: &splitty_transport::budget::ConnectionBudget,
) -> io::Result<()> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid host directory response",
        )
    };
    if directory.version != 1 || directory.order.len() > 256 {
        return Err(invalid());
    }
    // Hashing host identities prevents quadratic work on a legal 1 MiB reply.
    let scratch_bytes = directory.hosts.len().checked_mul(64).ok_or_else(invalid)?;
    let _scratch = budget.reserve(scratch_bytes, false).map_err(|_| {
        io::Error::new(
            io::ErrorKind::OutOfMemory,
            "host directory validation budget exhausted",
        )
    })?;
    let mut seen = HashSet::new();
    seen.try_reserve(directory.hosts.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::OutOfMemory,
            "host directory validation allocation failed",
        )
    })?;
    for (index, target) in directory.order.iter().enumerate() {
        valid_target(target)?;
        if directory.order[..index].contains(target) {
            return Err(invalid());
        }
    }
    for host in &directory.hosts {
        valid_target(&host.target)?;
        if !seen.insert(host.target.as_str()) || host.screens.len() > 200 {
            return Err(invalid());
        }
        if host
            .alias
            .as_ref()
            .is_some_and(|alias| alias.chars().count() > 200 || alias.chars().any(char::is_control))
        {
            return Err(invalid());
        }
        for (index, screen) in host.screens.iter().enumerate() {
            let id_chars = screen.id.chars().count();
            if id_chars == 0
                || id_chars > 256
                || screen
                    .name
                    .as_ref()
                    .is_some_and(|name| name.chars().count() > 200)
                || host.screens[..index]
                    .iter()
                    .any(|prior| prior.id == screen.id)
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

fn valid_target(target: &str) -> io::Result<()> {
    match SshTarget::parse(target) {
        Ok(parsed) if parsed.as_str() == target => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid host target in directory",
        )),
    }
}

#[cfg(test)]
pub(crate) mod tests;
