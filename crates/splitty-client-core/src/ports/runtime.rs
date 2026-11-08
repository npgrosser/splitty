//! Four independent mapping lanes and one passive reader. Futures retain setup,
//! request and close ownership even when the worker's select branch is cancelled.
use super::*;
use crate::owned_service::OwnedService;
use splitty_transport::{
    budget::{ConnectionBudget, Lease},
    queue,
};
use std::{
    collections::VecDeque,
    future::{poll_fn, Future},
    pin::Pin,
    sync::atomic::{AtomicU16, AtomicU64},
    task::Poll,
};

pub(crate) enum PortsEvent {
    Snapshot(Arc<ForwardSnapshot>),
    Result(Box<ForwardResult>),
}
pub(super) struct Consent {
    pub allocation: Option<Arc<Lease>>,
    pub active: Arc<AtomicBool>,
    pub expected: AtomicU16,
}
struct OpenConsent {
    id: Uuid,
    key: MappingKey,
    terminal: Option<TerminalToken>,
    consent: Arc<Consent>,
}
struct Job {
    id: Uuid,
    request: ForwardRequest,
    consent: Option<Arc<Consent>>,
    _lease: Arc<Lease>,
}
type Pending<'a> = Pin<Box<dyn Future<Output = (ForwardResult, io::Result<()>)> + Send + 'a>>;
struct Running<'a> {
    id: Uuid,
    key: MappingKey,
    context: queue::Context,
    future: Pending<'a>,
}
struct ReadResult {
    cleanup: io::Result<()>,
    active: io::Result<Charged<Vec<ActiveForward>>>,
    saved: io::Result<Charged<Preferences>>,
}
type Reading<'a> = Pin<Box<dyn Future<Output = ReadResult> + Send + 'a>>;
struct Passive<'a> {
    revision: u64,
    generation: u64,
    context: queue::Context,
    future: Reading<'a>,
}

pub(crate) struct PortsClient<'a> {
    revision: Arc<AtomicU64>,
    service: &'a OwnedService,
    owner: SourceToken,
    snapshot: ForwardSnapshot,
    generation: u64,
    dirty: bool,
    loading_due: bool,
    reading: Option<Passive<'a>>,
    running: Vec<Running<'a>>,
    queued: VecDeque<Job>,
    identities: VecDeque<Uuid>,
    consents: VecDeque<OpenConsent>,
    cleanup_error: Option<io::Error>,
    budget: ConnectionBudget,
    closing: Arc<AtomicBool>,
}
impl<'a> PortsClient<'a> {
    pub(crate) fn new(service: &'a OwnedService, owner: SourceToken, available: bool) -> Self {
        Self {
            revision: Arc::new(AtomicU64::new(0)),
            service,
            owner,
            snapshot: ForwardSnapshot::new(owner, available),
            generation: 1,
            dirty: available,
            loading_due: true,
            reading: None,
            running: Vec::new(),
            queued: VecDeque::new(),
            identities: VecDeque::new(),
            consents: VecDeque::new(),
            cleanup_error: None,
            budget: ConnectionBudget::default(),
            closing: Arc::new(AtomicBool::new(false)),
        }
    }
    pub(crate) fn refresh(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.dirty = self.snapshot.available;
    }
    pub(crate) fn enqueue(
        &mut self,
        owner: SourceToken,
        id: Uuid,
        request: ForwardRequest,
        window: &WindowWorkspace,
    ) -> io::Result<ForwardResult> {
        if owner != self.owner || !request.valid_origin(window) {
            return Err(invalid("Forwarding source changed"));
        }
        if self.identities.contains(&id)
            || self.running.iter().any(|job| job.id == id)
            || self.queued.iter().any(|job| job.id == id)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Forwarding request identity was already admitted",
            ));
        }
        // Deliberate supersession revokes consent even when capacity refuses the
        // new operation. A rejected Stop never claims that a tunnel was stopped.
        for open in &self.consents {
            if open.key == request.key {
                open.consent.active.store(false, Ordering::Release);
            }
        }
        if !self.snapshot.available {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "This service does not support native forwarding updates",
            ));
        }
        if self.queued.len() >= 16 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Forwarding queue is full; action was not submitted",
            ));
        }
        self.consents
            .retain(|open| open.consent.active.load(Ordering::Acquire));
        let lease = self
            .budget
            .reserve(
                8192 + 4 * request.key.target.len()
                    + request.link.as_ref().map_or(0, |(terminal, url)| {
                        2 * url.as_str().len()
                            + 4 * (terminal.pane_id().len() + terminal.session_id().len())
                    }),
                false,
            )
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "Forwarding request capacity exhausted",
                )
            })?;
        let lease = Arc::new(lease);
        let consent = (request.action == ForwardAction::Open).then(|| {
            Arc::new(Consent {
                allocation: Some(lease.clone()),
                active: Arc::new(AtomicBool::new(true)),
                expected: AtomicU16::new(0),
            })
        });
        if let Some(consent) = &consent {
            self.consents.push_back(OpenConsent {
                id,
                key: request.key.clone(),
                terminal: request.link.as_ref().map(|(terminal, _)| terminal.clone()),
                consent: consent.clone(),
            });
        }
        self.identities.push_back(id);
        // Settled history is bounded; running and queued identities are checked independently.
        if self.identities.len() > 256 {
            self.identities.pop_front();
        }
        while self.consents.len() > 256 {
            if let Some(open) = self.consents.pop_front() {
                open.consent.active.store(false, Ordering::Release);
            }
        }
        let mut result = ForwardResult::new(owner, id, &request, ForwardOutcome::Pending, None);
        result._allocation = Some(lease.clone());
        self.queued.push_back(Job {
            id,
            request,
            consent,
            _lease: lease,
        });
        self.refresh();
        Ok(result)
    }
    pub(crate) fn cancel_open(&mut self, id: Uuid) {
        for open in &self.consents {
            if open.id == id {
                open.consent.active.store(false, Ordering::Release);
            }
        }
    }
    pub(crate) fn revalidate(&self, window: &WindowWorkspace) {
        for open in &self.consents {
            if self.owner != window.lifecycle_token()
                || open.terminal.as_ref().is_some_and(|terminal| {
                    !window.accepts_terminal(terminal)
                        || window.source_target(terminal.source_token().source_id())
                            != Some(open.key.target())
                })
            {
                open.consent.active.store(false, Ordering::Release);
            }
        }
    }
    fn launch(&mut self) {
        while self.running.len() < 4 {
            let Some(ix) = self.queued.iter().position(|job| {
                !self
                    .running
                    .iter()
                    .any(|running| running.key == job.request.key)
            }) else {
                break;
            };
            let job = self.queued.remove(ix).expect("queued job index");
            let key = job.request.key.clone();
            let id = job.id;
            let service = self.service;
            let owner = self.owner;
            let context = queue::Context::default();
            let opening = context.clone();
            let closing = self.closing.clone();
            let revision = self.revision.clone();
            let future = Box::pin(async move {
                let allocation = job._lease;
                let mut client = match service.api_stream_with_context(opening).await {
                    Ok(client) => client,
                    Err(_) => {
                        let mut result = ForwardResult::new(
                            owner,
                            job.id,
                            &job.request,
                            ForwardOutcome::Rejected,
                            Some("Forwarding API setup failed; action was not submitted".into()),
                        );
                        result._allocation = Some(allocation);
                        return (result, Ok(()));
                    }
                };
                let mut result = if closing.load(Ordering::Acquire) {
                    ForwardResult::new(
                        owner,
                        job.id,
                        &job.request,
                        ForwardOutcome::Rejected,
                        Some("Forwarding owner closed before submission".into()),
                    )
                } else {
                    api::mutate(
                        &mut client,
                        owner,
                        job.id,
                        &job.request,
                        job.consent.as_deref(),
                        &revision,
                    )
                    .await
                };
                result._allocation = Some(allocation);
                let cleanup = client.finish().await;
                if cleanup.is_err() {
                    if let Some(consent) = &job.consent {
                        consent.active.store(false, Ordering::Release);
                    }
                }
                (result, cleanup)
            });
            self.running.push(Running {
                id,
                key,
                context,
                future,
            });
        }
        if self.reading.is_none() && self.dirty {
            self.dirty = false;
            self.snapshot.loading = true;
            self.loading_due = true;
            let service = self.service;
            let context = queue::Context::default();
            let opening = context.clone();
            let future = Box::pin(async move {
                let mut client = match service.api_stream_with_context(opening).await {
                    Ok(client) => client,
                    Err(_) => {
                        return ReadResult {
                            cleanup: Ok(()),
                            active: Err(io::Error::other("Active forwards unavailable")),
                            saved: Err(io::Error::other("Saved forwards unavailable")),
                        }
                    }
                };
                let active = api::read_active(&mut client).await;
                let saved = api::read_saved(&mut client).await;
                let cleanup = client.finish().await;
                ReadResult {
                    cleanup,
                    active,
                    saved,
                }
            });
            self.reading = Some(Passive {
                revision: self.revision.load(Ordering::Acquire),
                generation: self.generation,
                context,
                future,
            });
        }
    }
    pub(crate) async fn next_event(&mut self) -> PortsEvent {
        loop {
            self.launch();
            if self.loading_due {
                self.loading_due = false;
                self.snapshot.generation = self.generation;
                return PortsEvent::Snapshot(Arc::new(self.snapshot.clone()));
            }
            enum Completed {
                Job(usize, (ForwardResult, io::Result<()>)),
                Read(ReadResult),
            }
            let completed = poll_fn(|cx| {
                for (ix, running) in self.running.iter_mut().enumerate() {
                    if let Poll::Ready(result) = running.future.as_mut().poll(cx) {
                        return Poll::Ready(Completed::Job(ix, result));
                    }
                }
                if let Some(reading) = &mut self.reading {
                    if let Poll::Ready(result) = reading.future.as_mut().poll(cx) {
                        return Poll::Ready(Completed::Read(result));
                    }
                }
                Poll::Pending
            })
            .await;
            match completed {
                Completed::Job(ix, (result, cleanup)) => {
                    self.running.remove(ix);
                    // Only a published browser continuation needs revocation
                    // ownership after settlement. Failed/setup-rejected Opens
                    // must not retain their request allocation indefinitely.
                    if result.eligible_open().is_none() {
                        self.cancel_open(result.request_id());
                    }
                    self.consents
                        .retain(|open| open.consent.active.load(Ordering::Acquire));
                    self.record_cleanup(cleanup);
                    self.refresh();
                    return PortsEvent::Result(Box::new(result));
                }
                Completed::Read(result) => {
                    let reading = self.reading.take().expect("completed passive reader");
                    self.record_cleanup(result.cleanup);
                    if reading.revision != self.revision.load(Ordering::Acquire) {
                        self.refresh();
                    }
                    if reading.generation != self.generation {
                        continue;
                    }
                    self.snapshot.loading = false;
                    self.snapshot.generation = self.generation;
                    match result.active {
                        Ok(active) => {
                            for open in &self.consents {
                                let expected = open.consent.expected.load(Ordering::Acquire);
                                if expected != 0
                                    && !active.iter().any(|row| {
                                        row.target == open.key.target
                                            && row.remote == open.key.remote
                                            && row.local == expected
                                            && row.state == ActiveState::Up
                                    })
                                {
                                    open.consent.active.store(false, Ordering::Release);
                                }
                            }
                            self.snapshot.active = Some(Arc::new(active));
                            self.snapshot.active_error = None;
                        }
                        Err(_) => self.snapshot.active_error = Some(
                            "Could not refresh active forwards; last observed state is retained"
                                .into(),
                        ),
                    }
                    match result.saved {
                        Ok(saved) => {
                            self.snapshot.saved = Some(Arc::new(saved));
                            self.snapshot.saved_error = None;
                        }
                        Err(_) => {
                            self.snapshot.saved_error = Some(
                                "Could not refresh saved forwards; last observed state is retained"
                                    .into(),
                            )
                        }
                    }
                    return PortsEvent::Snapshot(Arc::new(self.snapshot.clone()));
                }
            }
        }
    }
    fn record_cleanup(&mut self, result: io::Result<()>) {
        if let Err(error) = result {
            if self.cleanup_error.is_none() {
                self.cleanup_error = Some(error);
            }
        }
    }
    pub(crate) async fn finish(mut self) -> io::Result<()> {
        self.closing.store(true, Ordering::Release);
        self.queued.clear();
        for open in &self.consents {
            open.consent.active.store(false, Ordering::Release);
        }
        for running in &self.running {
            running.context.close();
        }
        if let Some(reading) = &self.reading {
            reading.context.close();
        }
        // Poll all retained carriers concurrently. A slow request cannot prevent
        // another carrier from observing its cancellation and joining cleanup.
        poll_fn(|cx| {
            let mut ix = 0;
            while ix < self.running.len() {
                if let Poll::Ready((_, cleanup)) = self.running[ix].future.as_mut().poll(cx) {
                    self.running.remove(ix);
                    self.record_cleanup(cleanup);
                } else {
                    ix += 1;
                }
            }
            if let Some(reading) = &mut self.reading {
                if let Poll::Ready(result) = reading.future.as_mut().poll(cx) {
                    self.reading = None;
                    self.record_cleanup(result.cleanup);
                }
            }
            if self.running.is_empty() && self.reading.is_none() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        self.cleanup_error.map_or(Ok(()), Err)
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;
    use crate::{appearance::local::tests::fixture, EmbeddedService};
    use std::time::Duration;
    fn setup() -> (OwnedService, std::path::PathBuf, WindowWorkspace) {
        let (runtime, path) = fixture();
        (
            EmbeddedService::from_runtime(runtime).into(),
            path,
            WindowWorkspace::new(Uuid::new_v4()),
        )
    }
    async fn next(client: &mut PortsClient<'_>) -> PortsEvent {
        tokio::time::timeout(Duration::from_secs(5), client.next_event())
            .await
            .expect("ports event deadline")
    }
    fn save(target: &str) -> ForwardRequest {
        ForwardRequest::save(
            MappingKey::new(target, 3000).unwrap(),
            Some(4173),
            Some("App".into()),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn slow_mapping_orders_stop_but_other_jobs_and_passive_reads_progress() {
        let (service, path, window) = setup();
        let owner = window.lifecycle_token();
        let mut client = PortsClient::new(&service, owner, true);
        let slow = ForwardRequest::start(MappingKey::new("slow", 3000).unwrap(), None, false, None)
            .unwrap();
        let id = Uuid::new_v4();
        let (release, gate) = tokio::sync::oneshot::channel();
        let slow_key = slow.key.clone();
        client.running.push(Running {
            id,
            key: slow_key.clone(),
            context: queue::Context::default(),
            future: Box::pin(async move {
                gate.await.unwrap();
                (
                    ForwardResult::new(owner, id, &slow, ForwardOutcome::Failed, None),
                    Ok(()),
                )
            }),
        });
        let stop = Uuid::new_v4();
        client
            .enqueue(owner, stop, ForwardRequest::stop(slow_key), &window)
            .unwrap();
        let other = Uuid::new_v4();
        client
            .enqueue(owner, other, save("other"), &window)
            .unwrap();
        let mut saved = false;
        let mut observed = false;
        while !saved || !observed {
            match next(&mut client).await {
                PortsEvent::Result(result) => {
                    assert_ne!(
                        result.request_id(),
                        stop,
                        "same mapping Stop overtook opening"
                    );
                    if result.request_id() == other {
                        assert_eq!(result.outcome(), ForwardOutcome::Succeeded);
                        saved = true;
                    }
                }
                PortsEvent::Snapshot(snapshot) => {
                    if !snapshot.is_loading() {
                        assert!(snapshot
                            .rows(&window)
                            .iter()
                            .any(|row| row.key().target() == "other"));
                        observed = true;
                    }
                }
            }
        }
        assert_eq!(client.queued.len(), 1);
        release.send(()).unwrap();
        loop {
            if let PortsEvent::Result(result) = next(&mut client).await {
                if result.request_id() == stop {
                    assert_eq!(result.outcome(), ForwardOutcome::Failed);
                    break;
                }
            }
        }
        client.finish().await.unwrap();
        service.close().await.unwrap();
        let _ = std::fs::remove_dir_all(path);
    }
    #[tokio::test]
    async fn metadata_save_stop_label_and_forget_need_no_screen_or_ssh() {
        let (service, path, window) = setup();
        let owner = window.lifecycle_token();
        let mut client = PortsClient::new(&service, owner, true);
        let key = MappingKey::new("not-connected.invalid", 3000).unwrap();
        for (request, state, label) in [
            (
                ForwardRequest::save(key.clone(), Some(4173), Some("App".into())).unwrap(),
                Some(ForwardState::Saved),
                Some("App"),
            ),
            (
                ForwardRequest::stop(key.clone()),
                Some(ForwardState::Paused),
                Some("App"),
            ),
            (
                ForwardRequest::save(key.clone(), None, None).unwrap(),
                Some(ForwardState::Paused),
                Some("App"),
            ),
            (
                ForwardRequest::save(key.clone(), None, Some(String::new())).unwrap(),
                Some(ForwardState::Paused),
                Some(""),
            ),
            (ForwardRequest::forget(key.clone()), None, None),
        ] {
            let id = Uuid::new_v4();
            client.enqueue(owner, id, request, &window).unwrap();
            let mut completed = false;
            loop {
                match next(&mut client).await {
                    PortsEvent::Result(result) => {
                        assert_eq!(result.request_id(), id);
                        assert_eq!(
                            result.outcome(),
                            ForwardOutcome::Succeeded,
                            "{:?}",
                            result.reason()
                        );
                        completed = true;
                    }
                    PortsEvent::Snapshot(snapshot) if completed && !snapshot.is_loading() => {
                        let rows = snapshot.rows(&window);
                        assert_eq!(rows.first().map(ForwardRow::state), state);
                        assert_eq!(rows.first().and_then(ForwardRow::label), label);
                        break;
                    }
                    _ => {}
                }
            }
        }
        client.finish().await.unwrap();
        service.close().await.unwrap();
        std::fs::remove_dir_all(path).unwrap();
    }
    #[tokio::test]
    async fn admission_is_bounded_duplicate_safe_and_revoke_precedes_refusal() {
        let (service, path, window) = setup();
        let owner = window.lifecycle_token();
        let mut client = PortsClient::new(&service, owner, true);
        let request =
            ForwardRequest::open(MappingKey::new("same", 3000).unwrap(), None, false, None)
                .unwrap();
        let id = Uuid::new_v4();
        client.enqueue(owner, id, request.clone(), &window).unwrap();
        let consent = client.consents[0].consent.clone();
        assert_eq!(
            client
                .enqueue(owner, id, request.clone(), &window)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(consent.active.load(Ordering::Acquire));
        for ix in 0..15 {
            client
                .enqueue(owner, Uuid::new_v4(), save(&format!("host{ix}")), &window)
                .unwrap();
        }
        assert_eq!(
            client
                .enqueue(
                    owner,
                    Uuid::new_v4(),
                    ForwardRequest::stop(request.key.clone()),
                    &window
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(!consent.active.load(Ordering::Acquire));
        assert_eq!(client.queued.len(), 16);
        client.identities.clear(); // Settled history eviction never permits an active duplicate.
        assert_eq!(
            client
                .enqueue(owner, id, request, &window)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            client
                .enqueue(
                    crate::workspace::SourceWorkspace::new(Uuid::new_v4()).source_token(),
                    Uuid::new_v4(),
                    save("other"),
                    &window
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        client.launch();
        assert_eq!(client.running.len(), 4);
        assert_eq!(client.queued.len(), 12);
        client.finish().await.unwrap();
        service.close().await.unwrap();
        assert!(
            !path.join("remote-forwards.json").exists(),
            "closing before setup must discard all metadata writes"
        );
        let _ = std::fs::remove_dir_all(path);
    }
    #[tokio::test]
    async fn joined_cleanup_failure_is_not_reported_as_success() {
        let (service, path, window) = setup();
        let owner = window.lifecycle_token();
        let mut client = PortsClient::new(&service, owner, true);
        let request = save("host");
        client.running.push(Running {
            id: Uuid::new_v4(),
            key: request.key.clone(),
            context: queue::Context::default(),
            future: Box::pin(async move {
                (
                    ForwardResult::new(
                        owner,
                        Uuid::new_v4(),
                        &request,
                        ForwardOutcome::Uncertain,
                        None,
                    ),
                    Err(io::Error::other("owned cleanup failed")),
                )
            }),
        });
        assert!(client.finish().await.is_err());
        service.close().await.unwrap();
        let _ = std::fs::remove_dir_all(path);
    }
    fn charged<T: serde::de::DeserializeOwned>(raw: &str) -> Charged<T> {
        crate::api_stream::decode_with_budget(
            &ConnectionBudget::default(),
            &splitty_transport::wire::Reply {
                id: 1,
                status: 200,
                content_type: splitty_transport::wire::ContentType::Json,
                body: raw.into(),
                uncertain: false,
            },
        )
        .unwrap()
    }
    #[tokio::test]
    async fn stale_reads_cannot_overwrite_settlement_and_failures_preserve_each_resource() {
        let (service, path, window) = setup();
        let mut client = PortsClient::new(&service, window.lifecycle_token(), true);
        client.snapshot.active = Some(Arc::new(charged(
            r#"[{"target":"host","remote":3000,"local":4173,"state":"up"}]"#,
        )));
        client.snapshot.saved = Some(Arc::new(charged(
            r#"{"version":1,"forwards":[{"target":"host","remote":3000,"local":4173,"label":"old","enabled":true}]}"#,
        )));
        client.loading_due = false;
        client.dirty = false;
        client.reading = Some(Passive {
            revision: 0,
            generation: 0,
            context: queue::Context::default(),
            future: Box::pin(async {
                ReadResult {
                    cleanup: Ok(()),
                    active: Ok(charged("[]")),
                    saved: Ok(charged(r#"{"version":1,"forwards":[]}"#)),
                }
            }),
        });
        let mut poll = Box::pin(client.next_event());
        poll_fn(|cx| {
            assert!(poll.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(poll);
        assert_eq!(
            client.snapshot.active.as_ref().unwrap().len(),
            1,
            "stale empty list overwrote active state"
        );
        client.reading = Some(Passive {
            revision: 0,
            generation: client.generation,
            context: queue::Context::default(),
            future: Box::pin(async {
                ReadResult {
                    cleanup: Ok(()),
                    active: Err(io::Error::other("failed active read")),
                    saved: Ok(charged(
                        r#"{"version":1,"forwards":[{"target":"host","remote":3000,"local":4173,"label":"updated","enabled":false}]}"#,
                    )),
                }
            }),
        });
        let PortsEvent::Snapshot(snapshot) = next(&mut client).await else {
            panic!("snapshot")
        };
        assert!(snapshot.active_error().is_some());
        assert!(snapshot.saved_error().is_none());
        assert_eq!(snapshot.rows(&window)[0].label(), Some("updated"));
        assert_eq!(snapshot.rows(&window)[0].state(), ForwardState::Up);
        client.finish().await.unwrap();
        service.close().await.unwrap();
        let _ = std::fs::remove_dir_all(path);
    }
    #[tokio::test]
    async fn pre_ack_empty_read_does_not_revoke_but_fresh_absent_read_does() {
        let (service, path, window) = setup();
        let owner = window.lifecycle_token();
        let mut client = PortsClient::new(&service, owner, true);
        let consent = Arc::new(Consent {
            allocation: None,
            active: Arc::new(AtomicBool::new(true)),
            expected: AtomicU16::new(4173),
        });
        client.consents.push_back(OpenConsent {
            id: Uuid::new_v4(),
            key: MappingKey::new("host", 3000).unwrap(),
            terminal: None,
            consent: consent.clone(),
        });
        client.loading_due = false;
        client.dirty = false;
        // The reader was begun before the acknowledged Start; release its old
        // empty result only after the mutation's revision has advanced.
        client.reading = Some(Passive {
            revision: 0,
            generation: client.generation,
            context: queue::Context::default(),
            future: Box::pin(async {
                ReadResult {
                    cleanup: Ok(()),
                    active: Ok(charged("[]")),
                    saved: Ok(charged(r#"{"version":1,"forwards":[]}"#)),
                }
            }),
        });
        client.revision.store(1, Ordering::Release);
        let PortsEvent::Snapshot(snapshot) = next(&mut client).await else {
            panic!("fresh read loading")
        };
        assert!(snapshot.is_loading());
        assert!(
            consent.active.load(Ordering::Acquire),
            "pre-ack observation revoked a newer opening"
        );
        loop {
            if let PortsEvent::Snapshot(snapshot) = next(&mut client).await {
                if !snapshot.is_loading() {
                    break;
                }
            }
        }
        assert!(
            !consent.active.load(Ordering::Acquire),
            "fresh absence must revoke the old browser consent"
        );
        client.finish().await.unwrap();
        service.close().await.unwrap();
        let _ = std::fs::remove_dir_all(path);
    }
    #[tokio::test]
    async fn captured_remote_origin_survives_selection_but_not_detach_disconnect_or_replacement() {
        for revoke in ["detach", "disconnect", "binding", "reconnect", "owner"] {
            let (service, path, mut window) = setup();
            let owner = window.lifecycle_token();
            let (screen, terminal) =
                crate::workspace::remote_terminal_fixture(&mut window, "original");
            let request = ForwardRequest::open_link(
                MappingKey::new("original", 3000).unwrap(),
                terminal.clone(),
                WebUrl::parse("https://localhost:3000/app?q=one#two").unwrap(),
            )
            .unwrap();
            assert!(request.valid_origin(&window));
            let wrong = ForwardRequest::open_link(
                MappingKey::new("other", 3000).unwrap(),
                terminal.clone(),
                WebUrl::parse("http://localhost:3000").unwrap(),
            )
            .unwrap();
            assert!(
                !wrong.valid_origin(&window),
                "terminal origin must not authorize another target"
            );
            let mut client = PortsClient::new(&service, owner, true);
            let id = Uuid::new_v4();
            client.enqueue(owner, id, request.clone(), &window).unwrap();
            let consent = client.consents.back().unwrap().consent.clone();
            let eligible = ForwardOpen {
                _allocation: consent.allocation.clone(),
                url: rewritten_url(&request, 4173).unwrap(),
                terminal: Some(terminal.clone()),
                active: consent.active.clone(),
            };
            let (other, _) = crate::workspace::remote_terminal_fixture(&mut window, "other");
            assert_eq!(window.shown_key(), Some(other));
            assert!(request.valid_origin(&window));
            client.revalidate(&window);
            assert!(
                eligible.is_active(),
                "unrelated selection must not retarget or revoke consent"
            );
            match revoke {
                "detach" => {
                    window.detach(owner, &screen).unwrap();
                }
                "disconnect" => {
                    window
                        .disconnect(terminal.source_token().source_id())
                        .unwrap();
                }
                "binding" => {
                    let mut source = window
                        .source_mut(terminal.source_token().source_id())
                        .unwrap();
                    let connection = source.connection_id();
                    source.apply_snapshot(connection,&serde_json::json!({"type":"mux.screen_snapshot","screen":{"id":"work","revision":2,"root":{"type":"slot","id":"slot","paneIds":["p0"],"activePaneId":"p0"},"panes":{"p0":{"id":"p0","sessionId":"replacement"}},"sessions":{"replacement":{"id":"replacement","status":"running"}}}})).unwrap();
                }
                "reconnect" => {
                    let (_, replacement) =
                        crate::workspace::remote_terminal_fixture(&mut window, "original");
                    assert_eq!(replacement.pane_id(), terminal.pane_id());
                    assert_eq!(replacement.session_id(), terminal.session_id());
                    assert_ne!(replacement.source_token(), terminal.source_token());
                }
                "owner" => window = WindowWorkspace::new(Uuid::new_v4()),
                _ => unreachable!(),
            }
            client.revalidate(&window);
            assert!(!request.valid_origin(&window));
            assert!(
                !eligible.consume(),
                "obsolete terminal must not dispatch a queued browser result"
            );
            client.finish().await.unwrap();
            service.close().await.unwrap();
            let _ = std::fs::remove_dir_all(path);
        }
    }
    #[tokio::test]
    async fn settled_open_releases_failed_consent_but_keeps_published_open_revocable() {
        let (service, path, window) = setup();
        let owner = window.lifecycle_token();
        for outcome in [
            ForwardOutcome::Failed,
            ForwardOutcome::Uncertain,
            ForwardOutcome::Partial,
            ForwardOutcome::Succeeded,
        ] {
            let mut client = PortsClient::new(&service, owner, true);
            let budget = client.budget.clone();
            let id = Uuid::new_v4();
            let request =
                ForwardRequest::open(MappingKey::new("host", 3000).unwrap(), None, false, None)
                    .unwrap();
            let pending = client.enqueue(owner, id, request, &window).unwrap();
            let job = client.queued.pop_front().unwrap();
            let consent = job.consent.unwrap();
            let active = consent.active.clone();
            let mut settled = ForwardResult::new(owner, id, &job.request, outcome, None);
            settled._allocation = Some(job._lease);
            if outcome == ForwardOutcome::Succeeded {
                settled.open = Some(ForwardOpen {
                    _allocation: consent.allocation.clone(),
                    url: rewritten_url(&job.request, 4173).unwrap(),
                    terminal: None,
                    active: active.clone(),
                });
            }
            drop(consent);
            client.running.push(Running {
                id,
                key: job.request.key,
                context: queue::Context::default(),
                future: Box::pin(async move { (settled, Ok(())) }),
            });
            client.loading_due = false;
            client.dirty = false;
            let PortsEvent::Result(result) = next(&mut client).await else {
                panic!("settled result")
            };
            drop(pending);
            assert!(budget.used() > 0, "published result owns its allocation");
            if outcome == ForwardOutcome::Succeeded {
                assert!(result.eligible_open().is_some());
                client.cancel_open(id);
                assert!(result.eligible_open().is_none());
                drop(result);
            } else {
                assert!(!active.load(Ordering::Acquire));
                assert!(client.consents.is_empty());
                drop(result);
                assert_eq!(budget.used(), 0, "settled failure retained request budget");
            }
            client.finish().await.unwrap();
            assert_eq!(budget.used(), 0);
        }
        service.close().await.unwrap();
        let _ = std::fs::remove_dir_all(path);
    }
    #[tokio::test]
    async fn admitted_allocation_follows_result_and_open_consent_after_owner_cleanup() {
        let (service, path, window) = setup();
        let owner = window.lifecycle_token();
        let mut client = PortsClient::new(&service, owner, true);
        let budget = client.budget.clone();
        let request =
            ForwardRequest::open(MappingKey::new("host", 3000).unwrap(), None, false, None)
                .unwrap();
        let pending = client
            .enqueue(owner, Uuid::new_v4(), request, &window)
            .unwrap();
        let consent = client.consents.back().unwrap().consent.clone();
        assert!(budget.used() > 0);
        client.finish().await.unwrap();
        assert!(budget.used() > 0);
        drop(pending);
        assert!(
            budget.used() > 0,
            "the revocable continuation still retains its allocation"
        );
        drop(consent);
        assert_eq!(budget.used(), 0);
        service.close().await.unwrap();
        let _ = std::fs::remove_dir_all(path);
    }
}
