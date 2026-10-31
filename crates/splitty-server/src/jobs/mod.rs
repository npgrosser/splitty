//! Server-owned managed jobs. The mux remains authoritative; this module owns
//! admitted work, live handles and authorization, independently of HTTP clients.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use splitty_core::ids::{AgentId, JobId, PaneId, ScreenId};
use splitty_core::mux::effect_runner::{EffectRunner, EnvContext};
use splitty_core::mux::jobs::{
    JobAdmission, JobLaunch, JobPlacement, JobRecord, JobResult, JobState, JobUpdate,
};
use splitty_core::mux::{Effect, Op, Store};
use splitty_core::protocol::ServerEvent;
use splitty_core::pty::output::{read_waiting, OutputError, OutputPage, OutputQuery};
use splitty_core::pty::{Backend, SessionHandle};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, Weak,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{
    broadcast, watch, Mutex as AsyncMutex, OwnedMutexGuard, OwnedRwLockReadGuard, RwLock, Semaphore,
};

pub const CAPABILITY: &str = "managed-jobs-v1";

#[derive(Clone, Debug)]
pub enum Actor {
    User,
    Agent(AgentId),
}
impl Actor {
    fn owner(&self) -> Option<AgentId> {
        match self {
            Self::User => None,
            Self::Agent(id) => Some(id.clone()),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobError {
    #[serde(skip)]
    pub status: u16,
    pub code: &'static str,
    pub error: String,
    #[serde(flatten)]
    pub details: serde_json::Map<String, Value>,
}
impl JobError {
    pub fn new(status: u16, code: &'static str, error: impl Into<String>) -> Self {
        Self {
            status,
            code,
            error: error.into(),
            details: Default::default(),
        }
    }
    fn missing() -> Self {
        Self::new(404, "not_found", "job or grant not found")
    }
    fn capacity() -> Self {
        Self::new(429, "capacity_exceeded", "managed-job capacity exceeded")
    }
    fn job(mut self, job: &JobRecord) -> Self {
        for (key, value) in [
            ("jobId", job.job_id.to_string()),
            ("requestId", job.launch.request_id.clone()),
            ("screenId", job.screen_id.to_string()),
            ("paneId", job.pane_id.to_string()),
            ("sessionId", job.session_id.to_string()),
        ] {
            self.details.insert(key.into(), Value::String(value));
        }
        self
    }
}
type Result<T> = std::result::Result<T, JobError>;

/// Holds authorization through response serialization, never through network I/O.
pub struct Access {
    pub value: Value,
    _actor: Option<OwnedRwLockReadGuard<()>>,
    _job: Option<OwnedMutexGuard<()>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Anchor {
    pub screen_id: ScreenId,
    pub pane_id: PaneId,
}

pub struct Grant {
    pub id: AgentId,
    pub name: String,
    pub socket: String,
    pub anchor: Option<Anchor>,
    active: AtomicBool,
    gate: Arc<RwLock<()>>,
    changed: watch::Sender<u64>,
    readers: Arc<Semaphore>,
    listener: Mutex<Option<tokio::task::AbortHandle>>,
}
impl Grant {
    pub fn new(name: String, socket: String, anchor: Option<Anchor>, id: AgentId) -> Arc<Self> {
        Arc::new(Self {
            id,
            name,
            socket,
            anchor,
            active: AtomicBool::new(true),
            gate: Arc::new(RwLock::new(())),
            changed: watch::channel(0).0,
            readers: Arc::new(Semaphore::new(8)),
            listener: Mutex::new(None),
        })
    }
    pub fn set_listener(&self, listener: tokio::task::AbortHandle) {
        *self.listener.lock().expect("listener") = Some(listener);
    }
    fn close_listener(&self) {
        if let Some(listener) = self.listener.lock().expect("listener").take() {
            listener.abort();
            let _ = std::fs::remove_file(&self.socket);
        }
    }
    fn view(&self) -> Value {
        json!({"agentId":self.id,"name":self.name,"socket":self.socket,"anchor":self.anchor,"revoked":!self.active.load(Ordering::SeqCst)})
    }
}
impl Drop for Grant {
    fn drop(&mut self) {
        self.close_listener();
    }
}

struct LiveJob {
    acl: AtomicU64,
    lane: Arc<AsyncMutex<()>>,
    ready: watch::Sender<bool>,
    changed: watch::Sender<u64>,
    session: Mutex<Option<Arc<dyn SessionHandle>>>,
    cancel: AtomicBool,
}
impl LiveJob {
    fn new(ready: bool) -> Arc<Self> {
        Arc::new(Self {
            acl: AtomicU64::new(0),
            lane: Arc::new(AsyncMutex::new(())),
            ready: watch::channel(ready).0,
            changed: watch::channel(0).0,
            session: Mutex::new(None),
            cancel: AtomicBool::new(false),
        })
    }
    fn session(&self) -> Option<Arc<dyn SessionHandle>> {
        self.session.lock().expect("job session").clone()
    }
    fn changed(&self) {
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
    }
}
#[derive(Default)]
struct Registry {
    jobs: HashMap<JobId, Arc<LiveJob>>,
    grants: HashMap<AgentId, Arc<Grant>>,
}

pub struct Jobs {
    pub store: Arc<Store>,
    backend: Arc<dyn Backend>,
    runner: Arc<EffectRunner>,
    env: EnvContext,
    registry: Mutex<Registry>,
    events: broadcast::Sender<String>,
    readers: Arc<Semaphore>,
    closing: AtomicBool,
    validation: Arc<Semaphore>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
fn view(job: &JobRecord) -> Value {
    json!({"jobId":job.job_id,"requestId":job.launch.request_id,"screenId":job.screen_id,"paneId":job.pane_id,
        "sessionId":job.session_id,"argv":job.launch.argv,"cwd":job.launch.cwd,"name":job.launch.name,
        "createdBy":job.created_by,"owner":job.owner,"createdAt":job.created_at,"startedAt":job.started_at,
        "endedAt":job.ended_at,"state":job.state,"exitCode":job.result.exit_code,"signal":job.result.signal,"error":job.error})
}

impl Jobs {
    #[cfg(test)]
    pub(crate) fn waiting_reads(&self) -> usize {
        128 - self.readers.available_permits()
    }
    pub fn owner_socket(&self) -> &str {
        &self.env.control_socket
    }
    pub fn new(
        store: Arc<Store>,
        backend: Arc<dyn Backend>,
        runner: Arc<EffectRunner>,
        env: EnvContext,
        events: broadcast::Sender<String>,
    ) -> Arc<Self> {
        let jobs = store
            .jobs()
            .into_iter()
            .map(|j| (j.job_id, LiveJob::new(true)))
            .collect();
        Arc::new(Self {
            store,
            backend,
            runner,
            env,
            events,
            registry: Mutex::new(Registry {
                jobs,
                grants: HashMap::new(),
            }),
            readers: Arc::new(Semaphore::new(128)),
            closing: AtomicBool::new(false),
            validation: Arc::new(Semaphore::new(64)),
        })
    }
    fn event(&self, event: ServerEvent) {
        if let Ok(text) = serde_json::to_string(&event) {
            let _ = self.events.send(text);
        }
    }
    fn nudge(&self, job: &JobRecord) {
        self.event(ServerEvent::mux_nudge(
            "cli_op".into(),
            Some(job.screen_id.to_string()),
        ));
    }
    fn update(&self, id: &JobId, update: JobUpdate) {
        if let Err(error) = self.store.dispatch(Op::UpdateJob {
            job_id: id.clone(),
            update,
        }) {
            tracing::warn!(%error, "job state update failed");
        }
        if let Some(job) = self.store.job(id) {
            self.nudge(&job);
        }
        if let Ok(live) = self.live(id) {
            live.changed();
        }
    }
    fn live(&self, id: &JobId) -> Result<Arc<LiveJob>> {
        self.registry
            .lock()
            .expect("jobs")
            .jobs
            .get(id)
            .cloned()
            .ok_or_else(JobError::missing)
    }
    fn grant(&self, id: &AgentId) -> Result<Arc<Grant>> {
        self.registry
            .lock()
            .expect("jobs")
            .grants
            .get(id)
            .cloned()
            .ok_or_else(JobError::missing)
    }
    async fn authority(&self, actor: &Actor) -> Result<Option<OwnedRwLockReadGuard<()>>> {
        if self.closing.load(Ordering::SeqCst) && matches!(actor, Actor::Agent(_)) {
            return Err(JobError::new(503, "shutting_down", "server is stopping"));
        }
        if let Actor::Agent(id) = actor {
            let grant = self.grant(id)?;
            let guard = grant.gate.clone().read_owned().await;
            if !grant.active.load(Ordering::SeqCst) {
                return Err(JobError::missing());
            }
            Ok(Some(guard))
        } else {
            Ok(None)
        }
    }
    fn owned(&self, actor: &Actor, id: &JobId) -> Result<JobRecord> {
        if let Actor::Agent(id) = actor {
            if !self.grant(id)?.active.load(Ordering::SeqCst) {
                return Err(JobError::missing());
            }
        }
        let job = self.store.job(id).ok_or_else(JobError::missing)?;
        if job.forgotten || matches!(actor, Actor::Agent(a) if job.owner.as_ref() != Some(a)) {
            return Err(JobError::missing());
        }
        Ok(job)
    }
    pub async fn list(&self, actor: &Actor) -> Result<Access> {
        let guard = self.authority(actor).await?;
        let mut jobs: Vec<_> = self
            .store
            .jobs()
            .into_iter()
            .filter(|j| {
                !j.forgotten && !matches!(actor, Actor::Agent(a) if j.owner.as_ref() != Some(a))
            })
            .collect();
        jobs.sort_by_key(|j| (j.created_at, j.job_id.to_string()));
        Ok(Access {
            value: json!({"capabilities":[CAPABILITY],"jobs":jobs.iter().map(view).collect::<Vec<_>>()}),
            _actor: guard,
            _job: None,
        })
    }
    pub async fn inspect(&self, actor: &Actor, id: &JobId) -> Result<Access> {
        let guard = self.authority(actor).await?;
        let lane = self.live(id)?.lane.clone().lock_owned().await;
        let job = self.owned(actor, id)?;
        Ok(Access {
            value: view(&job),
            _actor: guard,
            _job: Some(lane),
        })
    }
    pub fn register_grant(&self, grant: Arc<Grant>) -> Result<Value> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(JobError::new(503, "shutting_down", "server is stopping"));
        }
        if grant.name.chars().count() > 200 || grant.name.contains('\0') {
            return Err(JobError::new(400, "invalid_request", "invalid grant name"));
        }
        if let Some(anchor) = &grant.anchor {
            let screen = self
                .store
                .get_screen(&anchor.screen_id)
                .ok_or_else(JobError::missing)?;
            if screen.root.find_slot(&anchor.pane_id).is_none() {
                return Err(JobError::missing());
            }
        }
        let mut registry = self.registry.lock().expect("jobs");
        if self.closing.load(Ordering::SeqCst) {
            return Err(JobError::new(503, "shutting_down", "server is stopping"));
        }
        if registry
            .grants
            .values()
            .filter(|g| g.active.load(Ordering::SeqCst))
            .count()
            >= 64
        {
            return Err(JobError::capacity());
        }
        let value = grant.view();
        registry.grants.insert(grant.id.clone(), grant);
        Ok(value)
    }
    pub fn grants(&self) -> Value {
        let registry = self.registry.lock().expect("jobs");
        json!({"agents": registry.grants.values().map(|g|g.view()).collect::<Vec<_>>()})
    }
    pub fn grant_info(&self, id: &AgentId) -> Result<Value> {
        let grant = self.grant(id)?;
        if !grant.active.load(Ordering::SeqCst) {
            return Err(JobError::missing());
        }
        Ok(grant.view())
    }
    pub async fn revoke(&self, id: &AgentId) -> Result<Value> {
        let grant = self.grant(id)?;
        grant.active.store(false, Ordering::SeqCst);
        grant.changed.send_modify(|v| *v = v.wrapping_add(1));
        let _guard = grant.gate.write().await;
        grant.close_listener();
        self.registry.lock().expect("jobs").grants.remove(id);
        Ok(grant.view())
    }
    pub async fn transfer(&self, id: &JobId, owner: Option<AgentId>) -> Result<Access> {
        if let Some(new) = &owner {
            self.grant_info(new)?;
        }
        let live = self.live(id)?;
        live.acl.fetch_add(1, Ordering::SeqCst);
        live.changed();
        loop {
            let previous = self.owned(&Actor::User, id)?.owner;
            let grant = previous.as_ref().and_then(|id| self.grant(id).ok());
            let _old = match &grant {
                Some(g) => Some(g.gate.clone().write_owned().await),
                None => None,
            };
            let lane = self.live(id)?.lane.clone().lock_owned().await;
            if self.owned(&Actor::User, id)?.owner != previous {
                continue;
            }
            if let Some(new) = &owner {
                self.grant_info(new)?;
            }
            self.update(
                id,
                JobUpdate::Owner {
                    owner: owner.clone(),
                },
            );
            return Ok(Access {
                value: view(&self.owned(&Actor::User, id)?),
                _actor: None,
                _job: Some(lane),
            });
        }
    }

    pub async fn launch(
        self: &Arc<Self>,
        actor: Actor,
        mut launch: JobLaunch,
    ) -> Result<(Access, bool)> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(JobError::new(503, "shutting_down", "server is stopping"));
        }
        launch
            .normalize()
            .map_err(|e| JobError::new(400, "invalid_request", e))?;
        let guard = self.authority(&actor).await?;
        let owner = actor.owner();
        let existing = self
            .store
            .jobs()
            .iter()
            .any(|job| job.created_by == owner && job.launch.request_id == launch.request_id);
        drop(guard);
        let directory_valid = if existing {
            true
        } else {
            let permit = self
                .validation
                .clone()
                .try_acquire_owned()
                .map_err(|_| JobError::capacity())?;
            let cwd = launch.cwd.clone();
            tokio::time::timeout(
                Duration::from_secs(3),
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    std::path::Path::new(&cwd).is_dir()
                }),
            )
            .await
            .map_err(|_| {
                JobError::new(
                    503,
                    "directory_unavailable",
                    "directory validation timed out; no launch was admitted",
                )
            })?
            .map_err(|e| JobError::new(503, "directory_unavailable", e.to_string()))?
        };
        let guard = self.authority(&actor).await?;
        let (id, live, replay) = {
            let mut registry = self.registry.lock().expect("jobs");
            if self.closing.load(Ordering::SeqCst) {
                return Err(JobError::new(503, "shutting_down", "server is stopping"));
            }
            let records = self.store.jobs();
            if let Some(job) = records
                .iter()
                .find(|j| j.created_by == owner && j.launch.request_id == launch.request_id)
            {
                if matches!(&actor,Actor::Agent(id) if job.owner.as_ref()!=Some(id)) {
                    return Err(JobError::missing());
                }
                if job.forgotten {
                    return Err(
                        JobError::new(410, "job_forgotten", "request was forgotten").job(job)
                    );
                }
                if job.launch != launch {
                    return Err(JobError::new(
                        409,
                        "request_conflict",
                        "requestId names a different launch",
                    )
                    .job(job));
                }
                (
                    job.job_id.clone(),
                    registry
                        .jobs
                        .get(&job.job_id)
                        .cloned()
                        .ok_or_else(JobError::missing)?,
                    true,
                )
            } else {
                let active = |job: &&JobRecord| {
                    !job.state.is_terminal()
                        || registry
                            .jobs
                            .get(&job.job_id)
                            .and_then(|live| live.session())
                            .is_some_and(|s| !s.output_complete())
                };
                if records.len() >= 4096
                    || records.iter().filter(active).count() >= 64
                    || (owner.is_some()
                        && records
                            .iter()
                            .filter(|j| j.owner == owner)
                            .filter(active)
                            .count()
                            >= 16)
                {
                    return Err(JobError::capacity());
                }
                if !directory_valid {
                    return Err(JobError::new(
                        400,
                        "invalid_cwd",
                        "working directory does not exist on server",
                    ));
                }
                let grant = owner
                    .as_ref()
                    .and_then(|id| registry.grants.get(id))
                    .cloned();
                let mut placement = launch.placement.clone();
                match &mut placement {
                    JobPlacement::Screen {} => {}
                    JobPlacement::Tab { screen_id, pane_id }
                    | JobPlacement::Split {
                        screen_id, pane_id, ..
                    } => {
                        if screen_id.is_none() && pane_id.is_none() {
                            let anchor = grant
                                .as_ref()
                                .and_then(|g| g.anchor.as_ref())
                                .ok_or_else(|| {
                                    JobError::new(
                                        400,
                                        "missing_target",
                                        "tab/split requires a placement anchor",
                                    )
                                })?;
                            *screen_id = Some(anchor.screen_id.clone());
                            *pane_id = Some(anchor.pane_id.clone());
                        }
                    }
                }
                // An owner's shell whose pane moved still names its old screen
                // ($SPLITTY_SCREEN_ID): the placement follows the pane. An
                // agent's anchor does not (access-control, agent grants).
                if owner.is_none() {
                    if let JobPlacement::Tab {
                        screen_id: Some(screen),
                        pane_id: Some(pane),
                    }
                    | JobPlacement::Split {
                        screen_id: Some(screen),
                        pane_id: Some(pane),
                        ..
                    } = &mut placement
                    {
                        if let Some(actual) = self.store.screen_of_movable_pane(pane) {
                            *screen = actual;
                        }
                    }
                }
                if let (Some(grant), Some((screen, pane))) = (&grant, placement.anchor()) {
                    let configured = grant
                        .anchor
                        .as_ref()
                        .is_some_and(|a| &a.screen_id == screen && &a.pane_id == pane);
                    let owned = records.iter().any(|j| {
                        j.owner == owner
                            && !j.forgotten
                            && &j.screen_id == screen
                            && &j.pane_id == pane
                    });
                    if !configured && !owned {
                        return Err(JobError::missing());
                    }
                }
                let id = JobId::new();
                let effects = self
                    .store
                    .dispatch(Op::CreateJob(Box::new(JobAdmission {
                        job_id: id.clone(),
                        launch,
                        placement,
                        owner,
                        created_at: now(),
                    })))
                    .map_err(|e| match e {
                        splitty_core::mux::MuxError::ScreenNotFound(_)
                        | splitty_core::mux::MuxError::PaneNotFound(_) => JobError::missing(),
                        _ => JobError::new(400, "invalid_placement", e.to_string()),
                    })?;
                let live = LiveJob::new(false);
                registry.jobs.insert(id.clone(), live.clone());
                let this = self.clone();
                let worker_id = id.clone();
                let worker_live = live.clone();
                let env = grant
                    .map(|g| EnvContext {
                        control_socket: g.socket.clone(),
                    })
                    .unwrap_or_else(|| self.env.clone());
                tokio::spawn(async move {
                    this.spawn(worker_id, worker_live, effects, env).await;
                });
                (id, live, false)
            }
        };
        drop(guard); // Accepted work is server-owned; revocation need not wait for spawn.
        let mut ready = live.ready.subscribe();
        if tokio::time::timeout(Duration::from_secs(10), ready.wait_for(|r| *r))
            .await
            .is_err()
        {
            return Err(JobError::new(
                504,
                "launch_pending",
                "launch still pending; retry the same requestId",
            )
            .job(&self.owned(&actor, &id)?));
        }
        let access = self.inspect(&actor, &id).await?;
        let job = self.owned(&actor, &id)?;
        if job.state == JobState::Failed {
            return Err(
                JobError::new(500, "spawn_failed", job.error.clone().unwrap_or_default()).job(&job),
            );
        }
        Ok((access, replay))
    }

    async fn spawn(
        self: Arc<Self>,
        id: JobId,
        live: Arc<LiveJob>,
        effects: Vec<Effect>,
        env: EnvContext,
    ) {
        let runner = self.runner.clone();
        let runtime = tokio::runtime::Handle::current();
        let result =
            tokio::task::spawn_blocking(move || runtime.block_on(runner.execute(effects, &env)))
                .await;
        let error = match result {
            Ok(outcomes) => outcomes
                .into_iter()
                .find_map(|o| o.result.err().map(|e| e.to_string())),
            Err(e) => Some(e.to_string()),
        };
        if let Some(error) = error {
            self.update(&id, JobUpdate::Failed { at: now(), error });
            live.ready.send_replace(true);
            return;
        }
        let Some(job) = self.store.job(&id) else {
            live.ready.send_replace(true);
            return;
        };
        let Some(session) = self.backend.get(job.session_id.as_str()) else {
            self.update(
                &id,
                JobUpdate::Failed {
                    at: now(),
                    error: "spawn returned no session".into(),
                },
            );
            live.ready.send_replace(true);
            return;
        };
        *live.session.lock().expect("session") = Some(session.clone());
        let _ = self.store.dispatch(Op::UpdateJob {
            job_id: id.clone(),
            update: JobUpdate::Started { at: now() },
        });
        live.changed();
        let visible = self
            .store
            .get_screen(&job.screen_id)
            .is_some_and(|s| s.panes.contains_key(&job.pane_id));
        if visible && !live.cancel.load(Ordering::SeqCst) {
            self.event(ServerEvent::mux_pane_spawned(
                job.pane_id.to_string(),
                job.session_id.to_string(),
            ));
            self.nudge(&job);
        }
        if let Some(result) = session.process_result() {
            self.exited(&id, result);
        }
        live.ready.send_replace(true);
        if !visible || live.cancel.load(Ordering::SeqCst) || self.closing.load(Ordering::SeqCst) {
            let _ = session.stop().await;
        }
        self.watch_exit(id, session);
    }

    /// Record the job's result once its session's process has ended, without
    /// any client watching.
    fn watch_exit(self: &Arc<Self>, id: JobId, session: Arc<dyn SessionHandle>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut done = session.done();
            let _ = done.wait_for(|d| *d).await;
            if let (Some(this), Some(result)) = (weak.upgrade(), session.process_result()) {
                this.exited(&id, result);
            }
        });
    }

    /// Track again the unfinished jobs whose keepers a starting daemon
    /// adopted: their processes run on, under trusted administration, and
    /// their retained output is readable again.
    pub fn resume_adopted(self: &Arc<Self>) {
        for job in self.store.jobs() {
            if job.forgotten || job.state.is_terminal() {
                continue;
            }
            let Some(session) = self.backend.get(job.session_id.as_str()) else {
                continue;
            };
            let Ok(live) = self.live(&job.job_id) else {
                continue;
            };
            *live.session.lock().expect("session") = Some(session.clone());
            live.changed();
            match session.process_result() {
                Some(result) => self.exited(&job.job_id, result),
                None => self.watch_exit(job.job_id.clone(), session),
            }
        }
    }
    fn exited(&self, id: &JobId, result: splitty_core::pty::ProcessResult) {
        self.update(
            id,
            JobUpdate::Exited {
                at: now(),
                result: JobResult {
                    exit_code: result.exit_code,
                    signal: result.signal,
                },
            },
        );
    }
    pub async fn input(&self, actor: &Actor, id: &JobId, data: String) -> Result<Access> {
        if data.len() > 16 * 1024 {
            return Err(JobError::new(
                413,
                "input_too_large",
                "input exceeds 16 KiB",
            ));
        }
        let guard = self.authority(actor).await?;
        let live = self.live(id)?;
        let epoch = live.acl.load(Ordering::SeqCst);
        let lane = live.lane.clone().lock_owned().await;
        if epoch != live.acl.load(Ordering::SeqCst) {
            return Err(JobError::missing());
        }
        let job = self.owned(actor, id)?;
        if job.state.is_terminal() {
            return Err(JobError::new(409, "job_ended", "job has ended"));
        }
        let session = live
            .session()
            .ok_or_else(|| JobError::new(409, "job_pending", "job has not started"))?;
        session
            .write(data.as_bytes())
            .await
            .map_err(|e| JobError::new(500, "write_failed", e.to_string()))?;
        Ok(Access {
            value: json!({"ok":true}),
            _actor: guard,
            _job: Some(lane),
        })
    }
    pub async fn stop(&self, actor: &Actor, id: &JobId) -> Result<Access> {
        let guard = self.authority(actor).await?;
        let live = self.live(id)?;
        let epoch = live.acl.load(Ordering::SeqCst);
        let lane = live.lane.clone().lock_owned().await;
        if epoch != live.acl.load(Ordering::SeqCst) {
            return Err(JobError::missing());
        }
        let job = self.owned(actor, id)?;
        if !job.state.is_terminal() {
            live.cancel.store(true, Ordering::SeqCst);
            self.update(id, JobUpdate::Stop);
            let mut ready = live.ready.subscribe();
            tokio::time::timeout(Duration::from_secs(10), ready.wait_for(|r| *r))
                .await
                .map_err(|_| JobError::new(504, "stop_pending", "start is still pending"))?
                .map_err(|_| JobError::new(500, "stop_failed", "start observer ended"))?;
            if let Some(session) = live.session() {
                let result = session
                    .stop()
                    .await
                    .map_err(|e| JobError::new(500, "stop_failed", e.to_string()))?;
                self.exited(id, result);
            }
        }
        Ok(Access {
            value: view(&self.owned(actor, id)?),
            _actor: guard,
            _job: Some(lane),
        })
    }
    pub async fn forget(&self, actor: &Actor, id: &JobId) -> Result<Access> {
        let guard = self.authority(actor).await?;
        let live = self.live(id)?;
        let epoch = live.acl.load(Ordering::SeqCst);
        let lane = live.lane.clone().lock_owned().await;
        if epoch != live.acl.load(Ordering::SeqCst) {
            return Err(JobError::missing());
        }
        let job = self.owned(actor, id)?;
        if !job.state.is_terminal() || live.session().is_some_and(|s| !s.output_complete()) {
            return Err(JobError::new(
                409,
                "job_active",
                "job is active or still draining",
            ));
        }
        let effects = self
            .store
            .dispatch(Op::UpdateJob {
                job_id: id.clone(),
                update: JobUpdate::Forget,
            })
            .map_err(|e| JobError::new(409, "forget_failed", e.to_string()))?;
        self.runner.execute(effects, &self.env).await;
        live.session.lock().expect("session").take();
        live.changed();
        self.nudge(&job);
        Ok(Access {
            value: json!({"ok":true,"jobId":id,"screenId":job.screen_id}),
            _actor: guard,
            _job: Some(lane),
        })
    }
    pub async fn logs(
        &self,
        actor: &Actor,
        id: &JobId,
        query: OutputQuery,
        wait: Duration,
    ) -> Result<Access> {
        query.validate_options().map_err(output_error)?;
        if wait > Duration::from_secs(30) {
            return Err(JobError::new(
                400,
                "invalid_request",
                "wait exceeds 30 seconds",
            ));
        }
        let guard = self.authority(actor).await?;
        let live = self.live(id)?;
        self.owned(actor, id)?;
        let grant = match actor {
            Actor::Agent(id) => Some(self.grant(id)?),
            Actor::User => None,
        };
        let _global = self
            .readers
            .clone()
            .try_acquire_owned()
            .map_err(|_| JobError::capacity())?;
        let _agent = match &grant {
            Some(g) => Some(
                g.readers
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| JobError::capacity())?,
            ),
            None => None,
        };
        let mut changes = live.changed.subscribe();
        let mut revocation = grant.as_ref().map(|g| g.changed.subscribe());
        drop(guard);
        let deadline = tokio::time::Instant::now() + wait;
        if live.session().is_none()
            && !*live.ready.borrow()
            && query.cursor.is_none()
            && !wait.is_zero()
        {
            let mut ready = live.ready.subscribe();
            tokio::select! {
                _=ready.wait_for(|r|*r)=>{},
                _=changes.changed()=>{},
                _=tokio::time::sleep_until(deadline)=>{},
                _=async{match &mut revocation{Some(r)=>{let _=r.changed().await;},None=>std::future::pending::<()>().await}}=>return Err(JobError::missing()),
            }
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let page = if let Some(session) = live.session() {
            tokio::select! {
                page=read_waiting(session.as_ref(),id.as_str(),&query,remaining)=>page.map_err(output_error)?,
                _=changes.changed()=>{self.owned(actor,id)?;session.read_output(id.as_str(),&query).map_err(output_error)?},
                _=async {match &mut revocation {Some(r)=>{let _=r.changed().await;},None=>std::future::pending::<()>().await}}=>return Err(JobError::missing()),
            }
        } else {
            if query.cursor.is_some() {
                return Err(JobError::new(
                    400,
                    "invalid_cursor",
                    "old output is unavailable",
                ));
            }
            let mut page = OutputPage::unavailable(query.format);
            page.output_complete = self.owned(actor, id)?.state.is_terminal();
            page
        };
        let guard = self.authority(actor).await?;
        let lane = live.lane.clone().lock_owned().await;
        let job = self.owned(actor, id)?;
        let mut value = serde_json::to_value(page)
            .map_err(|e| JobError::new(500, "encode_failed", e.to_string()))?;
        value["state"] = json!(job.state);
        Ok(Access {
            value,
            _actor: guard,
            _job: Some(lane),
        })
    }
    /// Only accepted layout effects may stop jobs. Rejected ops have no effects.
    pub async fn stop_effects(&self, effects: &[Effect]) -> Result<()> {
        for effect in effects {
            if let Effect::KillSession { session_id } = effect {
                for job in self
                    .store
                    .jobs()
                    .into_iter()
                    .filter(|j| !j.forgotten && &j.session_id == session_id)
                {
                    self.stop(&Actor::User, &job.job_id).await?;
                }
            }
        }
        Ok(())
    }

    /// The raw session-close path has no layout operation.
    pub async fn close_jobs(
        &self,
        screen: Option<&str>,
        pane: Option<&str>,
        session: Option<&str>,
    ) {
        for job in self.store.jobs().into_iter().filter(|j| {
            !j.forgotten
                && (screen.is_some_and(|s| s == j.screen_id.as_str())
                    || pane.is_some_and(|p| p == j.pane_id.as_str())
                    || session.is_some_and(|s| s == j.session_id.as_str()))
        }) {
            if let Err(error) = self.stop(&Actor::User, &job.job_id).await {
                tracing::warn!(error=%error.error,"job close pending");
            }
        }
    }
    /// A daemon's shutdown: grants end with the server, job processes run on
    /// in their keepers for the next daemon.
    pub async fn release(&self) {
        {
            let _admission = self.registry.lock().expect("jobs");
            self.closing.store(true, Ordering::SeqCst);
        }
        let grants: Vec<_> = self
            .registry
            .lock()
            .expect("jobs")
            .grants
            .keys()
            .cloned()
            .collect();
        for id in grants {
            let _ = self.revoke(&id).await;
        }
    }

    pub async fn shutdown(&self) {
        {
            let _admission = self.registry.lock().expect("jobs");
            self.closing.store(true, Ordering::SeqCst);
        }
        let grants: Vec<_> = self
            .registry
            .lock()
            .expect("jobs")
            .grants
            .keys()
            .cloned()
            .collect();
        for id in grants {
            let _ = self.revoke(&id).await;
        }
        self.close_jobs_all().await;
        self.closing.store(true, Ordering::SeqCst);
    }
    async fn close_jobs_all(&self) {
        let ids: Vec<_> = self
            .store
            .jobs()
            .into_iter()
            .filter(|j| !j.forgotten && !j.state.is_terminal())
            .map(|j| j.job_id)
            .collect();
        futures::future::join_all(ids.iter().map(|id| self.stop(&Actor::User, id))).await;
        for id in ids {
            if self
                .store
                .job(&id)
                .is_some_and(|j| j.state != JobState::Failed)
            {
                self.update(&id, JobUpdate::Interrupted { at: now() });
            }
        }
    }

    pub async fn mux(self: &Arc<Self>, screen: &str, mut op: Value) -> Option<Result<Access>> {
        let kind = op.get("type").and_then(Value::as_str)?.to_owned();
        if !matches!(kind.as_str(), "create_job" | "stop_job" | "forget_job") {
            return None;
        }
        op.as_object_mut()?.remove("type");
        Some(
            async {
                let mut access = if kind == "create_job" {
                    let launch: JobLaunch = serde_json::from_value(op)
                        .map_err(|e| JobError::new(400, "invalid_request", e.to_string()))?;
                    let target = launch
                        .placement
                        .anchor()
                        .map(|(s, _)| s.as_str())
                        .or_else(|| launch.new_screen_id.as_ref().map(|s| s.as_str()));
                    if !screen.is_empty() && target != Some(screen) {
                        return Err(JobError::new(
                            400,
                            "invalid_target",
                            "conflicting envelope screen",
                        ));
                    }
                    self.launch(Actor::User, launch).await?.0
                } else {
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase", deny_unknown_fields)]
                    struct Target {
                        job_id: JobId,
                    }
                    let target: Target = serde_json::from_value(op)
                        .map_err(|e| JobError::new(400, "invalid_request", e.to_string()))?;
                    let job = self.owned(&Actor::User, &target.job_id)?;
                    if !screen.is_empty() && job.screen_id.as_str() != screen {
                        return Err(JobError::new(
                            400,
                            "invalid_target",
                            "conflicting envelope screen",
                        ));
                    }
                    if kind == "stop_job" {
                        self.stop(&Actor::User, &target.job_id).await?
                    } else {
                        self.forget(&Actor::User, &target.job_id).await?
                    }
                };
                access.value["ok"] = json!(true);
                access.value["created"] = if kind == "create_job" {
                    json!([{"paneId":access.value["paneId"],"sessionId":access.value["sessionId"]}])
                } else {
                    json!([])
                };
                access.value["revision"] = json!(access.value["screenId"]
                    .as_str()
                    .and_then(|s| self.store.get_screen(&ScreenId::from(s)))
                    .map(|s| s.revision)
                    .unwrap_or(0));
                Ok(access)
            }
            .await,
        )
    }
}
fn output_error(error: OutputError) -> JobError {
    let code = match error {
        OutputError::InvalidCursor => "invalid_cursor",
        OutputError::InvalidOptions(_) => "invalid_request",
        OutputError::Unavailable => "output_unavailable",
    };
    JobError::new(400, code, error.to_string())
}
impl Drop for Jobs {
    fn drop(&mut self) {
        if let Ok(registry) = self.registry.get_mut() {
            for grant in registry.grants.values() {
                grant.close_listener();
            }
            for live in registry.jobs.values() {
                if let Some(session) = live.session() {
                    let _ = session.close();
                }
            }
        }
    }
}

pub type WeakJobs = Weak<Jobs>;

#[cfg(test)]
mod tests;
