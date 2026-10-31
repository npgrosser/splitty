use super::*;
use splitty_core::pty::{
    manager::Manager,
    testing::{pty_available, wait_until},
    CreateSessionOpts, PtyError,
};
use tokio::sync::Notify;

struct Controlled {
    manager: Manager,
    block: AtomicBool,
    entered: Notify,
    release: Notify,
}
#[async_trait::async_trait]
impl Backend for Controlled {
    async fn create(
        &self,
        opts: CreateSessionOpts,
    ) -> std::result::Result<Arc<dyn SessionHandle>, PtyError> {
        self.manager.create(opts).await
    }
    async fn create_program(
        &self,
        opts: CreateSessionOpts,
        argv: Vec<String>,
    ) -> std::result::Result<Arc<dyn SessionHandle>, PtyError> {
        if self.block.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.manager.create_program(opts, argv).await
    }
    fn get(&self, id: &str) -> Option<Arc<dyn SessionHandle>> {
        self.manager.get(id)
    }
    fn close(&self, id: &str) -> std::result::Result<(), PtyError> {
        self.manager.close(id)
    }
    fn close_all(&self) {
        self.manager.close_all();
    }
    fn sessions(&self) -> Vec<Arc<dyn SessionHandle>> {
        self.manager.sessions()
    }
}
struct Fixture {
    jobs: Arc<Jobs>,
    backend: Arc<Controlled>,
    directory: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        let backend = Arc::new(Controlled {
            manager: Manager::new(),
            block: AtomicBool::new(false),
            entered: Notify::new(),
            release: Notify::new(),
        });
        let jobs = Jobs::new(
            Arc::new(Store::new()),
            backend.clone(),
            Arc::new(EffectRunner::new(backend.clone())),
            EnvContext {
                control_socket: directory
                    .path()
                    .join("owner.sock")
                    .to_string_lossy()
                    .into_owned(),
            },
            broadcast::channel(256).0,
        );
        Self {
            jobs,
            backend,
            directory,
        }
    }
    fn launch(&self, script: &str) -> JobLaunch {
        JobLaunch {
            request_id: JobId::new().to_string(),
            argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
            cwd: self.directory.path().to_string_lossy().into_owned(),
            name: Some("backend".into()),
            placement: JobPlacement::Screen {},
            new_screen_id: None,
            new_pane_id: None,
            background: true,
        }
    }
    fn grant(&self, name: &str) -> Actor {
        let id = AgentId::new();
        let grant = Grant::new(
            name.into(),
            self.directory
                .path()
                .join(format!("{id}.sock"))
                .to_string_lossy()
                .into_owned(),
            None,
            id.clone(),
        );
        self.jobs.register_grant(grant).expect("grant");
        Actor::Agent(id)
    }
    async fn start(&self, actor: Actor, script: &str) -> JobId {
        let (access, _) = self
            .jobs
            .launch(actor, self.launch(script))
            .await
            .expect("start");
        JobId::from(access.value["jobId"].as_str().expect("id"))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.backend.release.notify_one();
        self.backend.close_all();
    }
}

#[tokio::test]
async fn launch_survives_cancelled_caller_and_retries_join_one_attempt() {
    if !pty_available() {
        return;
    }
    let f = Fixture::new();
    let actor = f.grant("agent");
    let request = f.launch("printf READY; read line; exit 7");
    f.backend.block.store(true, Ordering::SeqCst);
    let jobs = f.jobs.clone();
    let first = request.clone();
    let creator = actor.clone();
    let caller = tokio::spawn(async move {
        jobs.launch(creator, first)
            .await
            .map(|(a, _)| a.value.clone())
    });
    f.backend.entered.notified().await;
    caller.abort();
    let _ = caller.await;
    f.backend.release.notify_one();
    let replay = f
        .jobs
        .launch(actor.clone(), request.clone())
        .await
        .expect("retry");
    assert!(replay.1);
    let id = JobId::from(replay.0.value["jobId"].as_str().expect("id"));
    drop(replay);
    assert_eq!(f.backend.sessions().len(), 1);
    let mut conflict = request;
    conflict.argv.push("changed".into());
    assert_eq!(
        f.jobs
            .launch(actor.clone(), conflict)
            .await
            .err()
            .expect("conflict")
            .status,
        409
    );
    f.jobs
        .input(&actor, &id, "done\n".into())
        .await
        .expect("input");
    assert!(
        wait_until(Duration::from_secs(5), || f
            .jobs
            .store
            .job(&id)
            .is_some_and(|j| j.state == JobState::Exited))
        .await
    );
    assert_eq!(
        f.jobs.inspect(&actor, &id).await.expect("inspect").value["exitCode"],
        7
    );
    let logs = f
        .jobs
        .logs(&actor, &id, OutputQuery::default(), Duration::ZERO)
        .await
        .expect("logs")
        .value;
    assert!(logs["data"].as_str().expect("text").contains("READY"));
    let mut missing = f.launch("never");
    missing.argv = vec!["/no/such/splitty-test-program".into()];
    let failure = f
        .jobs
        .launch(actor.clone(), missing.clone())
        .await
        .err()
        .expect("spawn failure");
    assert_eq!(failure.code, "spawn_failed");
    assert_eq!(
        f.jobs
            .launch(actor, missing)
            .await
            .err()
            .expect("failure replay")
            .details,
        failure.details
    );
    f.jobs.shutdown().await;
}

#[tokio::test]
async fn ownership_transfer_revocation_and_waiter_capacity_fail_closed() {
    if !pty_available() {
        return;
    }
    let f = Fixture::new();
    let a = f.grant("same-name");
    let b = f.grant("same-name");
    let id = f.start(a.clone(), "read line; printf FINISHED").await;
    assert_eq!(
        f.jobs.inspect(&b, &id).await.err().expect("private").status,
        404
    );
    assert_eq!(
        f.jobs
            .input(&b, &id, "bad\n".into())
            .await
            .err()
            .expect("private")
            .status,
        404
    );
    assert_eq!(
        f.jobs.stop(&b, &id).await.err().expect("private").status,
        404
    );
    assert_eq!(
        f.jobs.forget(&a, &id).await.err().expect("live").status,
        409
    );
    let cursor = f
        .jobs
        .logs(
            &a,
            &id,
            OutputQuery {
                tail: Some(0),
                ..Default::default()
            },
            Duration::ZERO,
        )
        .await
        .expect("cursor")
        .value["nextCursor"]
        .as_str()
        .expect("cursor")
        .to_owned();
    let mut readers = Vec::new();
    for _ in 0..8 {
        let jobs = f.jobs.clone();
        let actor = a.clone();
        let id = id.clone();
        let cursor = cursor.clone();
        readers.push(tokio::spawn(async move {
            jobs.logs(
                &actor,
                &id,
                OutputQuery {
                    cursor: Some(cursor),
                    ..Default::default()
                },
                Duration::from_secs(30),
            )
            .await
            .map(|a| a.value.clone())
        }));
    }
    assert!(
        wait_until(Duration::from_secs(5), || f
            .jobs
            .readers
            .available_permits()
            == 120)
        .await
    );
    assert_eq!(
        f.jobs
            .logs(&a, &id, OutputQuery::default(), Duration::from_secs(30))
            .await
            .err()
            .expect("capacity")
            .status,
        429
    );
    f.jobs.transfer(&id, b.owner()).await.expect("transfer");
    for reader in readers {
        assert_eq!(
            reader.await.expect("reader").expect_err("old owner").status,
            404
        );
    }
    assert_eq!(f.jobs.readers.available_permits(), 128);
    assert_eq!(
        f.jobs
            .inspect(&a, &id)
            .await
            .err()
            .expect("old owner")
            .status,
        404
    );
    let grant = f.jobs.grant(&b.owner().expect("grant")).expect("grant");
    let lane = f
        .jobs
        .live(&id)
        .expect("live")
        .lane
        .clone()
        .lock_owned()
        .await;
    let jobs = f.jobs.clone();
    let actor = b.clone();
    let target = id.clone();
    let queued = tokio::spawn(async move {
        jobs.input(&actor, &target, "SHOULD-NOT-RUN\n".into())
            .await
            .map(|a| a.value.clone())
    });
    assert!(wait_until(Duration::from_secs(5), || grant.gate.try_write().is_err()).await);
    let jobs = f.jobs.clone();
    let grant_id = grant.id.clone();
    let revoked = tokio::spawn(async move { jobs.revoke(&grant_id).await });
    assert!(
        wait_until(Duration::from_secs(5), || !grant
            .active
            .load(Ordering::SeqCst))
        .await
    );
    drop(lane);
    assert_eq!(
        queued.await.expect("queued").expect_err("denied").status,
        404
    );
    revoked.await.expect("revoker").expect("revoked");
    assert!(!f
        .jobs
        .logs(&Actor::User, &id, OutputQuery::default(), Duration::ZERO)
        .await
        .expect("owner logs")
        .value["data"]
        .as_str()
        .expect("text")
        .contains("SHOULD-NOT-RUN"));
    f.jobs.shutdown().await;
}

#[tokio::test]
async fn concurrent_duplicate_requests_each_finish_and_preserve_one_process() {
    if !pty_available() {
        return;
    }
    let f = Fixture::new();
    let request = f.launch("printf READY; read line");
    let call = |request| async {
        f.jobs
            .launch(Actor::User, request)
            .await
            .map(|(a, _)| a.value.clone())
    };
    let mut conflict = request.clone();
    conflict.argv.push("conflict".into());
    f.backend.block.store(true, Ordering::SeqCst);
    let jobs = f.jobs.clone();
    let first = request.clone();
    let accepted = tokio::spawn(async move {
        jobs.launch(Actor::User, first)
            .await
            .map(|(a, _)| a.value.clone())
    });
    f.backend.entered.notified().await;
    assert_eq!(call(conflict).await.expect_err("conflict").status, 409);
    f.backend.release.notify_one();
    let mut alternate = request.clone();
    alternate.request_id = alternate.request_id.to_uppercase();
    let (a, b) = tokio::join!(call(alternate), call(request));
    assert_eq!(a.expect("a")["jobId"], b.expect("b")["jobId"]);
    accepted.await.expect("first caller").expect("accepted");
    assert_eq!(f.backend.sessions().len(), 1);
    f.jobs.shutdown().await;
}

#[tokio::test]
async fn revoked_pending_start_finishes_and_transfer_blocks_replayed_and_queued_access() {
    if !pty_available() {
        return;
    }
    let f = Fixture::new();
    let actor = f.grant("first");
    let next = f.grant("next");
    let request = f.launch("printf RETAINED; read line");
    f.backend.block.store(true, Ordering::SeqCst);
    let jobs = f.jobs.clone();
    let who = actor.clone();
    let launch = request.clone();
    let caller =
        tokio::spawn(async move { jobs.launch(who, launch).await.map(|(a, _)| a.value.clone()) });
    f.backend.entered.notified().await;
    let record = f.jobs.store.jobs().remove(0);
    let id = record.job_id;
    f.jobs
        .revoke(&actor.owner().expect("grant"))
        .await
        .expect("revoke during spawn");
    f.backend.release.notify_one();
    assert_eq!(
        caller.await.expect("caller").expect_err("revoked").status,
        404
    );
    assert_eq!(
        f.jobs
            .inspect(&Actor::User, &id)
            .await
            .expect("owner")
            .value["state"],
        "running"
    );
    f.jobs.transfer(&id, next.owner()).await.expect("new grant");
    let cursor = f
        .jobs
        .logs(&next, &id, OutputQuery::default(), Duration::from_secs(5))
        .await
        .expect("retained")
        .value["nextCursor"]
        .as_str()
        .expect("cursor")
        .to_owned();
    let jobs = f.jobs.clone();
    let who = next.clone();
    let target = id.clone();
    let reader = tokio::spawn(async move {
        jobs.logs(
            &who,
            &target,
            OutputQuery {
                cursor: Some(cursor),
                ..Default::default()
            },
            Duration::from_secs(30),
        )
        .await
        .map(|a| a.value.clone())
    });
    assert!(
        wait_until(Duration::from_secs(5), || f
            .jobs
            .readers
            .available_permits()
            == 127)
        .await
    );
    reader.abort();
    let _ = reader.await;
    assert_eq!(f.jobs.readers.available_permits(), 128);
    let grant = f.jobs.grant(&next.owner().expect("grant")).expect("grant");
    let lane = f
        .jobs
        .live(&id)
        .expect("live")
        .lane
        .clone()
        .lock_owned()
        .await;
    let jobs = f.jobs.clone();
    let who = next.clone();
    let target = id.clone();
    let input = tokio::spawn(async move {
        jobs.input(&who, &target, "NEVER\n".into())
            .await
            .map(|a| a.value.clone())
    });
    assert!(wait_until(Duration::from_secs(5), || grant.gate.try_write().is_err()).await);
    let jobs = f.jobs.clone();
    let target = id.clone();
    let epoch = f.jobs.live(&id).expect("live").acl.load(Ordering::SeqCst);
    let transfer =
        tokio::spawn(async move { jobs.transfer(&target, None).await.map(|a| a.value.clone()) });
    assert!(
        wait_until(Duration::from_secs(5), || f
            .jobs
            .live(&id)
            .expect("live")
            .acl
            .load(Ordering::SeqCst)
            != epoch)
        .await
    );
    drop(lane);
    assert_eq!(
        input
            .await
            .expect("input")
            .expect_err("queued before transfer")
            .status,
        404
    );
    transfer.await.expect("transfer task").expect("transfer");
    let before = f
        .jobs
        .inspect(&Actor::User, &id)
        .await
        .expect("still running")
        .value;
    f.jobs.stop(&Actor::User, &id).await.expect("stop");
    let stopped = f
        .jobs
        .stop(&Actor::User, &id)
        .await
        .expect("second stop")
        .value;
    assert_eq!(stopped["sessionId"], before["sessionId"]);
    assert!(
        wait_until(Duration::from_secs(5), || f
            .jobs
            .live(&id)
            .expect("live")
            .session()
            .expect("session")
            .output_complete())
        .await
    );
    f.jobs.forget(&Actor::User, &id).await.expect("forget");
    assert_eq!(
        f.jobs
            .launch(actor, request)
            .await
            .err()
            .expect("old creator denied")
            .status,
        404
    );
    f.jobs.shutdown().await;
}

#[tokio::test]
async fn retries_survive_closed_anchors_and_forget_retains_the_request_tombstone() {
    if !pty_available() {
        return;
    }
    let f = Fixture::new();
    f.jobs
        .store
        .dispatch(Op::CreateScreen {
            name: None,
            screen_id: Some("anchor-screen".into()),
            new_pane_id: Some("anchor".into()),
            shell_profile_id: None,
        })
        .expect("anchor");
    let mut request = f.launch("read line");
    request.placement = JobPlacement::Tab {
        screen_id: Some("anchor-screen".into()),
        pane_id: Some("anchor".into()),
    };
    let (access, _) = f
        .jobs
        .launch(Actor::User, request.clone())
        .await
        .expect("start");
    let id = JobId::from(access.value["jobId"].as_str().expect("id"));
    drop(access);
    f.jobs
        .store
        .dispatch_screen(
            &"anchor-screen".into(),
            splitty_core::mux::ScreenOp::ClosePane {
                pane_id: "anchor".into(),
                resulting_sizes: None,
            },
        )
        .expect("close anchor");
    let (replay, repeated) = f
        .jobs
        .launch(Actor::User, request.clone())
        .await
        .expect("closed anchor retry");
    assert!(repeated);
    assert_eq!(replay.value["jobId"], json!(id));
    drop(replay);
    f.jobs.stop(&Actor::User, &id).await.expect("stop");
    assert!(
        wait_until(Duration::from_secs(5), || f
            .jobs
            .live(&id)
            .expect("live")
            .session()
            .expect("session")
            .output_complete())
        .await
    );
    f.jobs.forget(&Actor::User, &id).await.expect("forget");
    assert_eq!(
        f.jobs
            .launch(Actor::User, request)
            .await
            .err()
            .expect("gone")
            .status,
        410
    );
    assert!(f.backend.sessions().is_empty());
    f.jobs.shutdown().await;
}

#[tokio::test]
async fn draining_capture_prevents_forget_and_consumes_admission_until_eof() {
    if !pty_available() {
        return;
    }
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let f = Fixture::new();
    let actor = f.grant("capture");
    let gate = f.directory.path().join("gate");
    nix::unistd::mkfifo(
        &gate,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .expect("gate");
    let id = f
        .start(
            actor.clone(),
            "trap '' HUP; (read line < gate; printf FINAL) & exit 7",
        )
        .await;
    assert!(
        wait_until(Duration::from_secs(5), || f
            .jobs
            .store
            .job(&id)
            .is_some_and(|j| j.state == JobState::Exited))
        .await
    );
    let denied = f.jobs.forget(&actor, &id).await.err().expect("draining");
    let incomplete = f
        .jobs
        .logs(&actor, &id, OutputQuery::default(), Duration::ZERO)
        .await
        .expect("logs")
        .value;
    for _ in 0..15 {
        f.jobs
            .store
            .dispatch(Op::CreateJob(Box::new(JobAdmission {
                job_id: JobId::new(),
                launch: f.launch("fixture"),
                placement: JobPlacement::Screen {},
                owner: actor.owner(),
                created_at: 1,
            })))
            .expect("capacity fixture");
    }
    let full = f
        .jobs
        .launch(actor.clone(), f.launch("exit 0"))
        .await
        .err()
        .expect("draining counts");
    // Release the real descendant even when a subsequent assertion fails.
    assert!(
        wait_until(Duration::from_secs(5), || std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(nix::libc::O_NONBLOCK)
            .open(&gate)
            .and_then(|mut file| file.write_all(b"go\n"))
            .is_ok())
        .await
    );
    assert_eq!(denied.status, 409);
    assert_eq!(full.status, 429);
    assert_eq!(incomplete["outputComplete"], false);
    assert!(
        wait_until(Duration::from_secs(5), || f
            .jobs
            .live(&id)
            .expect("live")
            .session()
            .expect("session")
            .output_complete())
        .await
    );
    assert!(f
        .jobs
        .logs(&actor, &id, OutputQuery::default(), Duration::ZERO)
        .await
        .expect("final")
        .value["data"]
        .as_str()
        .expect("text")
        .contains("FINAL"));
    f.jobs.forget(&actor, &id).await.expect("now drained");
    f.jobs
        .launch(actor, f.launch("exit 0"))
        .await
        .expect("slot released");
    // Model-only capacity fixtures have no accepted tasks to stop.
    for job in f.jobs.store.jobs() {
        f.jobs.update(&job.job_id, JobUpdate::Interrupted { at: 1 });
    }
    f.jobs.shutdown().await;
}

#[tokio::test]
async fn response_commit_precedes_transfer_and_shutdown_wakes_waiting_reads() {
    if !pty_available() {
        return;
    }
    let f = Fixture::new();
    let actor = f.grant("reader");
    let next = f.grant("next");
    let id = f.start(actor.clone(), "read line").await;
    let response = f
        .jobs
        .inspect(&actor, &id)
        .await
        .expect("authorized response");
    let jobs = f.jobs.clone();
    let target = id.clone();
    let owner = next.owner();
    let transfer =
        tokio::spawn(async move { jobs.transfer(&target, owner).await.map(|a| a.value.clone()) });
    assert!(
        wait_until(Duration::from_secs(5), || f
            .jobs
            .live(&id)
            .expect("live")
            .acl
            .load(Ordering::SeqCst)
            > 0)
        .await
    );
    assert!(
        !transfer.is_finished(),
        "ack cannot overtake a response commit"
    );
    drop(response);
    transfer.await.expect("transfer task").expect("transfer");
    assert_eq!(
        f.jobs
            .inspect(&actor, &id)
            .await
            .err()
            .expect("old owner")
            .status,
        404
    );
    let jobs = f.jobs.clone();
    let target = id.clone();
    let read = tokio::spawn(async move {
        jobs.logs(
            &next,
            &target,
            OutputQuery::default(),
            Duration::from_secs(30),
        )
        .await
        .map(|a| a.value.clone())
    });
    assert!(wait_until(Duration::from_secs(5), || f.jobs.waiting_reads() == 1).await);
    f.jobs.shutdown().await;
    assert!(read.await.expect("read task").is_err());
    assert_eq!(f.jobs.waiting_reads(), 0);
    assert!(*f
        .jobs
        .live(&id)
        .expect("live")
        .session()
        .expect("session")
        .done()
        .borrow());
}

#[tokio::test]
async fn global_job_record_and_reader_caps_are_atomic_and_do_not_evict() {
    let f = Fixture::new();
    f.jobs
        .store
        .dispatch(Op::CreateJob(Box::new(JobAdmission {
            job_id: JobId::new(),
            launch: f.launch("fixture"),
            placement: JobPlacement::Screen {},
            owner: None,
            created_at: 1,
        })))
        .expect("record");
    let template = f.jobs.store.jobs().remove(0);
    for (count, state, forgotten) in [
        (64, JobState::Starting, false),
        (4096, JobState::Exited, true),
    ] {
        let mut workspace = splitty_core::mux::WorkspaceState::default();
        for _ in 0..count {
            let mut job = template.clone();
            job.job_id = JobId::new();
            job.launch.request_id = JobId::new().to_string();
            job.state = state;
            job.forgotten = forgotten;
            workspace.jobs.insert(job.job_id.clone(), job);
        }
        let service = Jobs::new(
            Arc::new(Store::with_state(workspace)),
            f.backend.clone(),
            f.jobs.runner.clone(),
            f.jobs.env.clone(),
            broadcast::channel(16).0,
        );
        let error = service
            .launch(Actor::User, f.launch("no process"))
            .await
            .err()
            .expect("limit");
        assert_eq!(error.status, 429);
        assert_eq!(service.store.jobs().len(), count);
        assert!(f.backend.sessions().is_empty());
    }
    // Reserve the bounded global waiter pool; rejecting an additional reader
    // must neither consume an agent permit nor allocate an output subscription.
    let record = f.jobs.store.jobs().remove(0);
    let live = LiveJob::new(true);
    f.jobs
        .registry
        .lock()
        .expect("registry")
        .jobs
        .insert(record.job_id.clone(), live);
    let permits = f
        .jobs
        .readers
        .clone()
        .acquire_many_owned(128)
        .await
        .expect("pool");
    assert_eq!(
        f.jobs
            .logs(
                &Actor::User,
                &record.job_id,
                OutputQuery::default(),
                Duration::ZERO
            )
            .await
            .err()
            .expect("read cap")
            .status,
        429
    );
    drop(permits);
    assert_eq!(f.jobs.waiting_reads(), 0);
}

#[tokio::test]
async fn pending_start_times_out_without_cancelling_or_duplicating_it() {
    if !pty_available() {
        return;
    }
    let f = Fixture::new();
    f.backend.block.store(true, Ordering::SeqCst);
    let request = f.launch("read line");
    let retry = request.clone();
    let jobs = f.jobs.clone();
    let caller = tokio::spawn(async move {
        jobs.launch(Actor::User, request)
            .await
            .map(|(a, _)| a.value.clone())
    });
    f.backend.entered.notified().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(11)).await;
    let pending = caller.await.expect("caller").expect_err("pending");
    assert_eq!(pending.code, "launch_pending");
    tokio::time::resume();
    f.backend.release.notify_one();
    let (response, replay) = f.jobs.launch(Actor::User, retry).await.expect("retry");
    assert!(replay);
    assert_eq!(response.value["jobId"], pending.details["jobId"]);
    drop(response);
    assert_eq!(f.backend.sessions().len(), 1);
    f.jobs.shutdown().await;
}

#[tokio::test]
async fn admission_caps_and_stale_anchors_do_not_start_processes() {
    let f = Fixture::new();
    let actor = f.grant("agent");
    let owner = actor.owner();
    for _ in 0..16 {
        let launch = f.launch("not executed");
        f.jobs
            .store
            .dispatch(Op::CreateJob(Box::new(JobAdmission {
                job_id: JobId::new(),
                launch,
                placement: JobPlacement::Screen {},
                owner: owner.clone(),
                created_at: 1,
            })))
            .expect("model fixture");
    }
    let (a, b) = tokio::join!(
        f.jobs.launch(actor.clone(), f.launch("not executed")),
        f.jobs.launch(actor, f.launch("not executed"))
    );
    assert_eq!(a.err().expect("capacity").status, 429);
    assert_eq!(b.err().expect("capacity").status, 429);
    assert!(f.backend.sessions().is_empty());
    let mut bad = f.launch("not executed");
    bad.placement = JobPlacement::Tab {
        screen_id: Some("missing".into()),
        pane_id: Some("missing".into()),
    };
    assert!(f.jobs.launch(Actor::User, bad).await.is_err());
    assert!(f.backend.sessions().is_empty());
    for _ in 1..64 {
        f.grant("limit");
    }
    assert_eq!(
        f.jobs
            .register_grant(Grant::new(
                "overflow".into(),
                "unused".into(),
                None,
                AgentId::new()
            ))
            .expect_err("grant limit")
            .status,
        429
    );
}

#[tokio::test]
async fn concurrent_admission_claims_the_last_slot_only_once() {
    if !pty_available() {
        return;
    }
    let f = Fixture::new();
    let actor = f.grant("agent");
    for _ in 0..15 {
        let launch = f.launch("fixture only");
        f.jobs
            .store
            .dispatch(Op::CreateJob(Box::new(JobAdmission {
                job_id: JobId::new(),
                launch,
                placement: JobPlacement::Screen {},
                owner: actor.owner(),
                created_at: 1,
            })))
            .expect("fixture");
    }
    let call = || async {
        f.jobs
            .launch(actor.clone(), f.launch("read line"))
            .await
            .map(|(a, _)| a.value.clone())
    };
    let (a, b) = tokio::join!(call(), call());
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        a.err().or_else(|| b.err()).expect("one refused").status,
        429
    );
    assert_eq!(f.backend.sessions().len(), 1);
    f.jobs.shutdown().await;
}

#[tokio::test]
async fn closing_during_spawn_stops_the_late_process_without_resurrecting_the_pane() {
    if !pty_available() {
        return;
    }
    let f = Fixture::new();
    f.backend.block.store(true, Ordering::SeqCst);
    let jobs = f.jobs.clone();
    let request = f.launch("read line");
    let mut events = f.jobs.events.subscribe();
    let caller = tokio::spawn(async move {
        jobs.launch(Actor::User, request)
            .await
            .map(|(a, _)| a.value.clone())
    });
    f.backend.entered.notified().await;
    let job = f.jobs.store.jobs().remove(0);
    let jobs = f.jobs.clone();
    let id = job.job_id.clone();
    let stop =
        tokio::spawn(async move { jobs.stop(&Actor::User, &id).await.map(|a| a.value.clone()) });
    assert!(
        wait_until(Duration::from_secs(5), || f
            .jobs
            .store
            .job(&job.job_id)
            .is_some_and(|j| j.state == JobState::Stopping))
        .await
    );
    f.jobs
        .store
        .dispatch_screen(
            &job.screen_id,
            splitty_core::mux::ScreenOp::ClosePane {
                pane_id: job.pane_id.clone(),
                resulting_sizes: None,
            },
        )
        .expect("close model");
    f.backend.release.notify_one();
    stop.await.expect("stop task").expect("stopped");
    caller.await.expect("caller").expect("accepted attempt");
    assert!(!f
        .jobs
        .store
        .get_screen(&job.screen_id)
        .expect("screen")
        .panes
        .contains_key(&job.pane_id));
    assert!(f
        .jobs
        .live(&job.job_id)
        .expect("live")
        .session()
        .expect("session")
        .done()
        .borrow()
        .to_owned());
    while let Ok(event) = events.try_recv() {
        assert_ne!(
            serde_json::from_str::<Value>(&event).expect("event")["type"],
            "mux.pane_spawned"
        );
    }
    f.jobs.shutdown().await;
}
