//! Private gates around actual in-process Clients. These inject only setup/close
//! timing, never a public fake source registry or an alternate runtime model.
use super::*;
use crate::workspace::SourceStatus;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::oneshot;

// Real Clients share the process transport budget. Bound simultaneous fixture
// lifetimes instead of racing unrelated tests for production admission capacity.
static CLIENT_FIXTURES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

fn service() -> (OwnedService, std::path::PathBuf) {
    let (runtime, path) = crate::appearance::local::tests::fixture();
    (crate::EmbeddedService::from_runtime(runtime).into(), path)
}
fn admitted<'a>(service: &'a OwnedService, window: &mut WindowWorkspace, target: &str) -> Host<'a> {
    let id = window
        .admit_remote(window.lifecycle_token(), target)
        .unwrap();
    Host::new(service, window.host(id).unwrap().action_target().clone()).unwrap()
}
async fn poll_once(remotes: &mut Remotes<'_>) -> Option<Step> {
    let mut wait = Box::pin(remotes.next());
    poll_fn(|cx| {
        Poll::Ready(match wait.as_mut().poll(cx) {
            Poll::Ready(step) => Some(step),
            Poll::Pending => None,
        })
    })
    .await
}
async fn next(remotes: &mut Remotes<'_>) -> Step {
    tokio::time::timeout(Duration::from_secs(5), remotes.next())
        .await
        .expect("retained host future completes")
}

#[tokio::test]
async fn cancelled_setup_keeps_late_client_and_close_owned_until_observed_cleanup() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "late.test");
    let action = host.action.clone();
    let context = host.context.clone();
    let late = service.connect().await.unwrap();
    let (return_client, gate) = oneshot::channel();
    let polled = Arc::new(AtomicUsize::new(0));
    let setup_polled = polled.clone();
    host.setup = Some(Box::pin(async move {
        setup_polled.fetch_add(1, Ordering::SeqCst);
        gate.await.unwrap();
        Ok(late)
    }));
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    assert!(poll_once(&mut remotes).await.is_none());
    assert_eq!(polled.load(Ordering::SeqCst), 1);
    remotes.cancel(&mut window, &action).unwrap();
    assert!(context.is_closed());
    for _ in 0..3 {
        assert!(poll_once(&mut remotes).await.is_none());
    }
    assert!(remotes.hosts[0].setup.is_some());
    assert_eq!(
        window.source_status(action.source_id()),
        Some(SourceStatus::Closing)
    );
    assert!(window
        .admit_remote(window.lifecycle_token(), "late.test")
        .is_err());
    return_client.send(()).unwrap();
    let step = next(&mut remotes).await;
    assert!(matches!(step, Step::Prepared(_)));
    remotes.handle(&mut window, step);
    assert!(
        window.source(action.source_id()).is_none(),
        "a cancelled late Client is never bound or published"
    );
    let close = remotes.hosts[0].closing.take().unwrap();
    let (finish_close, close_gate) = oneshot::channel();
    remotes.hosts[0].closing = Some(Box::pin(async move {
        close_gate.await.unwrap();
        close.await
    }));
    for _ in 0..3 {
        assert!(poll_once(&mut remotes).await.is_none());
    }
    assert!(remotes.hosts[0].closing.is_some());
    assert!(window.host(action.source_id()).is_some());
    finish_close.send(()).unwrap();
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert!(window.host(action.source_id()).is_none());
    assert!(remotes.hosts.is_empty());
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn failed_initial_attempt_waits_for_explicit_retry_and_ignores_old_eof() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "failed.test");
    let old = host.action.clone();
    let starts = Arc::new(AtomicUsize::new(0));
    let count = starts.clone();
    host.setup = Some(Box::pin(async move {
        count.fetch_add(1, Ordering::SeqCst);
        Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "initial failure",
        ))
    }));
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert!(
        !window.host(old.source_id()).unwrap().can_retry(),
        "cleanup completion is a separate observed step"
    );
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert!(window.host(old.source_id()).unwrap().can_retry());
    assert_eq!(
        window
            .host(old.source_id())
            .unwrap()
            .diagnostic()
            .unwrap()
            .message(),
        "initial failure"
    );
    for _ in 0..3 {
        assert!(poll_once(&mut remotes).await.is_none());
    }
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    remotes.retry(&mut window, &old).unwrap();
    // No SSH future is polled: replace only this test attempt's backend timing.
    remotes.hosts[0].setup = Some(Box::pin(std::future::pending()));
    let current = remotes.hosts[0].action.clone();
    assert_eq!(current.source_id(), old.source_id());
    assert!(current.attempt() > old.attempt());
    assert!(remotes.cancel(&mut window, &old).is_err());
    assert!(remotes.retry(&mut window, &old).is_err());
    let stale = remotes.handle(&mut window, Step::Event(old, Ok(None)));
    assert!(!stale.changed);
    assert_eq!(
        window.host(current.source_id()).unwrap().action_target(),
        &current
    );
    assert_eq!(
        window.source_status(current.source_id()),
        Some(SourceStatus::Connecting)
    );
    remotes.hosts[0].setup = Some(Box::pin(async {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "cancelled test setup",
        ))
    }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn published_close_failure_remains_a_final_cleanup_failure() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "close.test");
    let action = host.action.clone();
    let late = service.connect().await.unwrap();
    host.setup = Some(Box::pin(async move { Ok(late) }));
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    remotes.cancel(&mut window, &action).unwrap();
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    let close = remotes.hosts[0].closing.take().unwrap();
    remotes.hosts[0].closing = Some(Box::pin(async move {
        close.await?;
        Err(io::Error::other("observed close sentinel"))
    }));
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert!(window.host(action.source_id()).is_none());
    assert_eq!(
        remotes.finish(&mut window).await.unwrap_err().to_string(),
        "observed close sentinel"
    );
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn progress_is_coalesced_charged_and_cannot_revive_cancelled_attempt() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "progress.test");
    let action = host.action.clone();
    let budget = host.context.budget.clone();
    host.setup = Some(Box::pin(std::future::pending()));
    for message in ["first", "latest"] {
        let progress = budget
            .charge(256, true, || wire::Progress {
                phase: "prepare".into(),
                message: message.into(),
            })
            .unwrap();
        *host.progress.lock().unwrap() = Some(progress);
    }
    assert_eq!(budget.used(), 256);
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert_eq!(
        window
            .host(action.source_id())
            .unwrap()
            .progress()
            .unwrap()
            .message,
        "latest"
    );
    let view = window.clone();
    remotes.cancel(&mut window, &action).unwrap();
    *remotes.hosts[0].progress.lock().unwrap() = Some(
        budget
            .charge(256, true, || wire::Progress {
                phase: "late".into(),
                message: "must be ignored".into(),
            })
            .unwrap(),
    );
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert_eq!(budget.used(), 256, "rejected progress drops its own charge");
    assert_eq!(
        window.source_status(action.source_id()),
        Some(SourceStatus::Closing)
    );
    remotes.hosts[0].setup = Some(Box::pin(async {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "cancelled test setup",
        ))
    }));
    remotes.finish(&mut window).await.unwrap();
    assert_eq!(budget.used(), 256, "last view still owns its progress");
    drop(view);
    assert_eq!(budget.used(), 0);
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn recovery_delays_grow_to_cap_without_extending_original_deadline() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "recovery.test");
    let id = host.action.source_id();
    // Model the loss boundary after application readiness; actual connection
    // adoption is covered by the service adapter and window tests.
    host.setup = None;
    host.was_ready = true;
    host.failure = Some(io::Error::new(io::ErrorKind::ConnectionReset, "lost"));
    host.close_source();
    let deadline = host.recovery_deadline.unwrap();
    window.disconnect(id).unwrap();
    host.closed_due = true;
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    for delay in [1, 2, 4, 8, 10, 10] {
        let before = Instant::now();
        let step = next(&mut remotes).await;
        remotes.handle(&mut window, step);
        assert_eq!(window.source_status(id), Some(SourceStatus::Reconnecting));
        assert!(!window.host(id).unwrap().can_retry());
        assert!(window.host(id).unwrap().can_cancel());
        assert!(remotes.hosts[0].retry_at.unwrap() >= before + Duration::from_secs(delay));
        assert_eq!(remotes.hosts[0].recovery_deadline, Some(deadline));
        let old = remotes.hosts[0].action.clone();
        // Advance only the private timer boundary, without sleeping or dialing.
        remotes.hosts[0].retry_at = Some(Instant::now());
        remotes.reconcile(&mut window).unwrap();
        assert!(remotes.hosts[0].action.attempt() > old.attempt());
        assert_eq!(remotes.hosts[0].recovery_deadline, Some(deadline));
        remotes.hosts[0].setup = Some(Box::pin(async {
            Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "still unavailable",
            ))
        }));
        let step = next(&mut remotes).await;
        remotes.handle(&mut window, step);
        assert!(remotes.cancel(&mut window, &old).is_err());
    }
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn recovery_deadline_revokes_gated_setup_but_retains_late_cleanup() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "deadline.test");
    let id = host.action.source_id();
    host.was_ready = true;
    host.recovery_deadline = Some(Instant::now() - Duration::from_secs(1));
    let context = host.context.clone();
    let late = service.connect().await.unwrap();
    let (release, gate) = oneshot::channel();
    host.setup = Some(Box::pin(async move {
        gate.await.unwrap();
        Ok(late)
    }));
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    remotes.reconcile(&mut window).unwrap();
    assert!(context.is_closed());
    assert!(!window.host(id).unwrap().can_retry());
    assert!(poll_once(&mut remotes).await.is_none());
    release.send(()).unwrap();
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert!(
        window.source(id).is_none(),
        "expired late success is never bound"
    );
    assert!(!window.host(id).unwrap().can_retry());
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert!(window.host(id).unwrap().can_retry());
    assert!(remotes.hosts[0].retry_at.is_none());
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn authoritative_nonretryable_and_malformed_close_stop_recovery() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    for (message, retryable) in [
        ("permission denied", false),
        (r#"{"type":"remote.error"}"#, true),
    ] {
        let mut window = WindowWorkspace::new(local.connection_id());
        let mut host = admitted(&service, &mut window, "denied.test");
        let id = host.action.source_id();
        host.setup = None;
        host.was_ready = true;
        host.failure = Some(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "transport lost",
        ));
        host.close_source();
        host.reason = Some(Arc::new(
            host.context
                .budget
                .charge(512, true, || wire::Closed {
                    code: wire::CloseCode::BackendLost,
                    message: message.into(),
                    retryable,
                })
                .unwrap(),
        ));
        host.closed_due = true;
        window.disconnect(id).unwrap();
        let mut remotes = Remotes::new(&service);
        remotes.hosts.push(host);
        let step = next(&mut remotes).await;
        remotes.handle(&mut window, step);
        assert_eq!(window.source_status(id), Some(SourceStatus::Offline));
        assert!(window.host(id).unwrap().can_retry());
        assert!(remotes.hosts[0].retry_at.is_none());
        assert!(poll_once(&mut remotes).await.is_none());
        remotes.finish(&mut window).await.unwrap();
    }
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn losing_last_demand_during_recovery_wait_never_starts_another_setup() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "withdraw.test");
    let id = host.action.source_id();
    host.setup = None;
    host.was_ready = true;
    host.failure = Some(io::Error::new(io::ErrorKind::ConnectionReset, "lost"));
    host.close_source();
    host.closed_due = true;
    window.disconnect(id).unwrap();
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    let action = remotes.hosts[0].action.clone();
    assert_eq!(window.source_status(id), Some(SourceStatus::Reconnecting));
    remotes.release_browse(&mut window, &action).unwrap();
    remotes.hosts[0].retry_at = Some(Instant::now());
    remotes.reconcile(&mut window).unwrap();
    assert!(remotes.hosts[0].setup.is_none());
    assert!(remotes.hosts[0].context.is_closed());
    assert_eq!(window.source_status(id), Some(SourceStatus::Closing));
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert!(window.host(id).is_none());
    assert!(remotes.hosts.is_empty());
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn catalog_failure_is_source_scoped_and_preserves_transport_diagnostic() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "catalog.test");
    let id = host.action.source_id();
    host.setup = None;
    host.observation_deadline = Some(Instant::now() + Duration::from_secs(15));
    let transport = plain_diagnostic(&host.context.budget, &"previous transport detail").unwrap();
    window.host_mut(&host.action).unwrap().diagnostic = Some(transport.clone());
    let mut out = SourceEffects::default();
    merge_scoped(
        &mut host,
        &mut window,
        &mut out,
        SourceEffects {
            observation_error: Some("invalid duplicate live catalog".into()),
            ..SourceEffects::default()
        },
    );
    assert!(out.changed);
    assert!(
        out.updates.is_empty(),
        "no unscoped status escapes the source"
    );
    assert_eq!(
        window.host(id).unwrap().catalog_state(),
        CatalogState::Failed
    );
    assert_eq!(
        window.host(id).unwrap().catalog_error().unwrap().message(),
        "invalid duplicate live catalog"
    );
    assert!(Arc::ptr_eq(
        window.host(id).unwrap().diagnostic.as_ref().unwrap(),
        &transport
    ));
    assert!(host.observation_deadline.is_none());
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn exhausted_diagnostic_budget_keeps_previous_value_and_delivers_other_effects() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "bounded.test");
    host.setup = None;
    host.context.budget = ConnectionBudget::new(
        splitty_transport::budget::ProcessBudget::new(16384, 4096),
        8192,
        1024,
    );
    let budget = host.context.budget.clone();
    let previous = plain_diagnostic(&budget, &"last good diagnostic").unwrap();
    window.host_mut(&host.action).unwrap().diagnostic = Some(previous.clone());
    let held = budget.reserve(8192 - 1024 - budget.used(), false).unwrap();
    assert!(plain_diagnostic(&budget, &"cannot allocate").is_err());
    let mut out = SourceEffects::default();
    let owner = window.lifecycle_token();
    merge_scoped(
        &mut host,
        &mut window,
        &mut out,
        SourceEffects {
            updates: vec![
                Update::Status("bounded replacement".into()),
                Update::CloseRejected {
                    source: Some(owner),
                    request_id: 73,
                    reason: "uncertain".into(),
                },
            ],
            ..SourceEffects::default()
        },
    );
    assert!(!host.context.is_closed());
    assert!(!out.changed);
    assert!(Arc::ptr_eq(
        window
            .host(host.action.source_id())
            .unwrap()
            .diagnostic
            .as_ref()
            .unwrap(),
        &previous
    ));
    assert!(matches!(
        out.updates.as_slice(),
        [Update::CloseRejected { request_id: 73, .. }]
    ));
    drop(held);
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    remotes.finish(&mut window).await.unwrap();
    drop(previous);
    assert_eq!(budget.used(), 0);
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn accepted_directory_and_owner_admit_one_deliberate_attempt_without_polling_ssh() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (runtime, path) = crate::appearance::local::tests::fixture();
    std::fs::create_dir_all(&path).unwrap();
    runtime
        .state
        .hub
        .update_host_preference(serde_json::json!({
            "target":"known.test","alias":"Known"
        }))
        .unwrap();
    let service: OwnedService = crate::EmbeddedService::from_runtime(runtime).into();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let owner = window.lifecycle_token();
    let mut hub = HubClient::new(service.api_stream().await.unwrap(), local.connection_id());
    let snapshot = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let HostDirectoryState::Ready(snapshot) = hub.next_update().await.state {
                break snapshot;
            }
        }
    })
    .await
    .unwrap();
    let mut remotes = Remotes::new(&service);
    assert!(
        window.hosts().next().is_none(),
        "passive API read creates no host source"
    );
    assert!(poll_once(&mut remotes).await.is_none());
    let foreign = SourceWorkspace::new(uuid::Uuid::new_v4()).source_token();
    assert!(remotes
        .connect(&mut window, foreign, "known.test", &snapshot)
        .is_err());
    assert!(remotes
        .connect(&mut window, owner, "unknown.test", &snapshot)
        .is_err());
    assert!(remotes
        .connect(&mut window, owner, " known.test", &snapshot)
        .is_err());
    assert!(remotes.hosts.is_empty());
    assert!(window.hosts().next().is_none());
    remotes
        .connect(&mut window, owner, "known.test", &snapshot)
        .unwrap();
    let first = remotes.hosts[0].action.clone();
    remotes
        .connect(&mut window, owner, "known.test", &snapshot)
        .unwrap();
    assert_eq!(remotes.hosts.len(), 1);
    assert_eq!(remotes.hosts[0].action, first);
    assert!(window.host(first.source_id()).unwrap().is_browsing());
    // Replace the never-polled backend future: this admission test intentionally
    // cannot invoke SSH. Cleanup still follows the real retained lifetime.
    remotes.hosts[0].setup = Some(Box::pin(async {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "test cancellation",
        ))
    }));
    remotes.release_browse(&mut window, &first).unwrap();
    assert!(remotes.hosts[0].context.is_closed());
    remotes.finish(&mut window).await.unwrap();
    hub.finish().await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn assigned_purpose_mismatch_is_cleaned_up_before_retry_without_binding() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "assigned.test");
    let id = host.action.source_id();
    let wrong = service.connect().await.unwrap(); // Actual Main purpose, not Host.
    host.setup = Some(Box::pin(async move { Ok(wrong) }));
    let context = host.context.clone();
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert!(context.is_closed());
    assert!(window.source(id).is_none());
    assert!(window.execution_token(id).is_none());
    assert!(!window.host(id).unwrap().can_retry());
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert!(window.host(id).unwrap().can_retry());
    assert!(remotes.hosts[0].retry_at.is_none());
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn application_prelude_requires_exact_target_then_catalog_capability() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "prelude.test");
    host.setup = Some(Box::pin(async {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "unpolled test setup",
        ))
    }));
    let welcome =
        serde_json::json!({"type":"mux.welcome", "capabilities":["workspace-catalog-v1"]});
    assert_eq!(
        host.observe_prelude(&welcome).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        host.observe_prelude(&offer_ready("other.test"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert!(!host.was_ready);
    assert!(host.offer.is_none());
    let mut malformed_offer = offer_ready("prelude.test");
    malformed_offer["daemon"]["version"] = serde_json::json!("x".repeat(65));
    assert!(host.observe_prelude(&malformed_offer).is_err());
    assert!(host.offer.is_none());
    assert!(!host
        .observe_prelude(&serde_json::json!({"type":"remote.progress"}))
        .unwrap());
    let ready = serde_json::json!({"type":"remote.ready","target":"prelude.test"});
    assert!(!host.observe_prelude(&ready).unwrap());
    assert!(host.was_ready);
    assert_eq!(
        host.observe_prelude(&ready).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    let unsupported = host
        .observe_prelude(&serde_json::json!({"type":"mux.welcome","capabilities":[]}))
        .unwrap_err();
    assert_eq!(unsupported.kind(), io::ErrorKind::Unsupported);
    host.failure = Some(unsupported);
    assert!(
        !host.retryable(None),
        "unsupported protocol cannot become transient loss"
    );
    assert!(host.observe_prelude(&welcome).unwrap());
    assert!(
        window.execution_token(host.action.source_id()).is_none(),
        "prelude validation alone never binds a transport"
    );
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn explicit_metadata_continuations_withdraw_exact_demand_and_keep_failed_unlisted_work() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let hub_state = runtime.state.hub.clone();
    let service: OwnedService = crate::EmbeddedService::from_runtime(runtime).into();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let owner = window.lifecycle_token();
    let mut remotes = Remotes::new(&service);
    let mut requests = HostConnects::default();
    let request = HostManagementRequest::connect("entered.test", None).unwrap();
    let id = uuid::Uuid::new_v4();
    let mut success =
        HostManagementResult::new(owner, id, &request, HostManagementOutcome::Applied, None);
    requests.insert(id, owner);
    assert!(requests.withdraw(id).is_none());
    assert!(!requests.apply(&mut success, &mut window, &mut remotes));
    assert!(window.hosts().next().is_none());
    // Confirmed preparation may still persist after withdrawal; it is not an
    // offline saved target and cannot manufacture connection demand.
    hub_state
        .update_host_preference(serde_json::json!({"target":"entered.test","hidden":false}))
        .unwrap();
    assert!(hub_state.directory().hosts.is_empty());
    requests.insert(id, owner);
    assert!(requests.apply(&mut success, &mut window, &mut remotes));
    let action = remotes.hosts[0].action.clone();
    remotes.hosts[0].setup = Some(Box::pin(async {
        Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "entered target failed",
        ))
    }));
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert!(window.host(action.source_id()).unwrap().can_retry());
    assert!(hub_state.directory().hosts.is_empty());
    assert_eq!(
        window.hosts().next().unwrap().action_target().target(),
        "entered.test"
    );
    // A separate explicit request must not acquire ownership of an already
    // browsed target, so cancelling its form preserves existing demand.
    let other = uuid::Uuid::new_v4();
    requests.insert(other, owner);
    let mut second =
        HostManagementResult::new(owner, other, &request, HostManagementOutcome::Applied, None);
    assert!(requests.apply(&mut second, &mut window, &mut remotes));
    assert!(requests.withdraw(other).is_none());
    assert!(window.host(action.source_id()).unwrap().is_browsing());
    // An attempt replacement makes the old form's cancellation powerless.
    remotes.retry(&mut window, &action).unwrap();
    remotes.hosts[0].setup = Some(Box::pin(async {
        Err(io::Error::new(io::ErrorKind::Interrupted, "test end"))
    }));
    let old = requests.withdraw(id).unwrap();
    assert!(remotes.release_browse(&mut window, &old).is_err());
    assert!(window.host(action.source_id()).unwrap().is_browsing());
    remotes.release_all_browse(&mut window);
    assert!(!window.host(action.source_id()).unwrap().is_browsing());
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn late_success_cancel_and_admission_failure_remain_identified() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let owner = window.lifecycle_token();
    let mut remotes = Remotes::new(&service);
    let mut requests = HostConnects::default();
    let id = uuid::Uuid::new_v4();
    let request = HostManagementRequest::connect("late.test", None).unwrap();
    let mut success =
        HostManagementResult::new(owner, id, &request, HostManagementOutcome::Applied, None);
    requests.insert(id, owner);
    assert!(requests.apply(&mut success, &mut window, &mut remotes));
    // Cancelling after source admission but before UI delivery still withdraws it.
    let action = requests.withdraw(id).unwrap();
    remotes.hosts[0].setup = Some(Box::pin(async {
        Err(io::Error::new(io::ErrorKind::Interrupted, "test end"))
    }));
    remotes.release_browse(&mut window, &action).unwrap();
    assert!(!window.host(action.source_id()).unwrap().is_browsing());
    assert!(remotes.hosts[0].context.is_closed());
    // The same target cannot restart while its previous owned attempt closes.
    let next_id = uuid::Uuid::new_v4();
    requests.insert(next_id, owner);
    let mut blocked = HostManagementResult::new(
        owner,
        next_id,
        &request,
        HostManagementOutcome::Applied,
        None,
    );
    assert!(!requests.apply(&mut blocked, &mut window, &mut remotes));
    assert_eq!(blocked.outcome(), HostManagementOutcome::Rejected);
    assert!(blocked.reason().unwrap().contains("metadata was saved"));
    assert_eq!(blocked.request_id(), next_id);
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

fn offer_ready(target: &str) -> serde_json::Value {
    serde_json::json!({"type":"remote.ready","target":target,"daemon":{"version":"0.1.0","keepsSessions":true},"update":{"available":true,"keepsSessions":true}})
}
fn settle_offer(host: &mut Host<'_>, window: &mut WindowWorkspace) {
    host.setup = None;
    host.observe_prelude(&offer_ready(host.action.target()))
        .unwrap();
    host.failure = Some(
        host.observe_prelude(&serde_json::json!({"type":"mux.welcome","capabilities":[]}))
            .unwrap_err(),
    );
    window.disconnect(host.action.source_id()).unwrap();
    host.closed_due = true;
    let budget = &host.context.budget;
    let version = budget
        .reserve(64, false)
        .unwrap()
        .charge("0.2.0".to_owned());
    window.set_backend_version(
        window.lifecycle_token().connection_id(),
        Ok(Arc::new(version)),
    );
}
#[tokio::test]
async fn capability_failure_preserves_offer_and_confirmation_is_revocable() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "update.test");
    let action = host.action.clone();
    settle_offer(&mut host, &mut window);
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert!(window
        .host(action.source_id())
        .unwrap()
        .is_update_available());
    let first = remotes
        .request_update(&window, uuid::Uuid::new_v4(), &action)
        .unwrap();
    assert_eq!(first.to_version(), "0.2.0");
    assert!(window.accepts_host_update_plan(&first));
    remotes.dismiss_update(first.request_id());
    assert!(remotes.confirm_update(&mut window, &first).is_err());
    let current = remotes
        .request_update(&window, uuid::Uuid::new_v4(), &action)
        .unwrap();
    assert!(!WindowWorkspace::new(uuid::Uuid::new_v4()).accepts_host_update_plan(&current));
    assert!(!window.set_backend_version(uuid::Uuid::new_v4(), Err(io::Error::other("old owner"))));
    assert!(window.accepts_host_update_plan(&current));
    // New requests supersede old captured plans; late dismissals cannot revoke
    // the newer dialog. A backend-read failure revokes ordinary confirmation.
    let superseding = remotes
        .request_update(&window, uuid::Uuid::new_v4(), &action)
        .unwrap();
    assert!(remotes.confirm_update(&mut window, &current).is_err());
    remotes.dismiss_update(current.request_id());
    assert!(window.accepts_host_update_plan(&superseding));
    window.set_backend_version(
        window.lifecycle_token().connection_id(),
        Err(io::Error::other("read failed")),
    );
    assert!(!window.accepts_host_update_plan(&superseding));
    assert!(remotes
        .request_update(&window, uuid::Uuid::new_v4(), &action)
        .is_err());
    let version = remotes.hosts[0]
        .context
        .budget
        .reserve(64, false)
        .unwrap()
        .charge("0.2.0".to_owned());
    window.set_backend_version(
        window.lifecycle_token().connection_id(),
        Ok(Arc::new(version)),
    );
    let current = remotes
        .request_update(&window, uuid::Uuid::new_v4(), &action)
        .unwrap();
    window
        .set_host_order(window.lifecycle_token(), &["update.test".into()])
        .unwrap();
    assert!(window.accepts_host_update_plan(&current));
    remotes.confirm_update(&mut window, &current).unwrap();
    assert!(remotes.confirm_update(&mut window, &current).is_err());
    assert!(remotes.hosts[0].update_pending);
    remotes.cancel(&mut window, &action).unwrap();
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    remotes.reconcile(&mut window).unwrap();
    assert!(!remotes.hosts[0].update_dispatched);
    assert!(remotes.hosts[0].setup.is_none());
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn update_waits_for_cleanup_and_failed_dispatch_never_replays() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "update.test");
    let action = host.action.clone();
    settle_offer(&mut host, &mut window);
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    let plan = remotes
        .request_update(&window, uuid::Uuid::new_v4(), &action)
        .unwrap();
    let (complete, gate) = oneshot::channel();
    remotes.hosts[0].closing = Some(Box::pin(async move {
        gate.await.unwrap();
        Ok(())
    }));
    remotes.confirm_update(&mut window, &plan).unwrap();
    remotes.reconcile(&mut window).unwrap();
    assert!(!remotes.hosts[0].update_dispatched);
    assert!(poll_once(&mut remotes).await.is_none());
    complete.send(()).unwrap();
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    remotes.reconcile(&mut window).unwrap();
    assert!(remotes.hosts[0].update_dispatched);
    assert_eq!(
        window
            .host(action.source_id())
            .unwrap()
            .action_target()
            .attempt(),
        action.attempt() + 1
    );
    assert!(remotes.confirm_update(&mut window, &plan).is_err());
    // Exercise the production prepared/closed state transitions without SSH.
    remotes.hosts[0].setup = Some(Box::pin(async {
        Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "update response lost",
        ))
    }));
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert_eq!(
        window.host(action.source_id()).unwrap().update_state(),
        HostUpdateState::Unconfirmed
    );
    assert!(remotes.hosts[0].retry_at.is_none());
    assert!(!window
        .host(action.source_id())
        .unwrap()
        .is_update_available());
    let next_action = window
        .host(action.source_id())
        .unwrap()
        .action_target()
        .clone();
    remotes.retry(&mut window, &next_action).unwrap();
    assert!(!remotes.hosts[0].update_dispatched);
    remotes.hosts[0].setup = Some(Box::pin(async {
        Err(io::Error::new(io::ErrorKind::Interrupted, "test done"))
    }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn update_cleanup_failure_and_lost_demand_never_dispatch() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    for lose_demand in [false, true] {
        let (service, path) = service();
        let local = service.connect().await.unwrap();
        let mut window = WindowWorkspace::new(local.connection_id());
        let mut host = admitted(&service, &mut window, "update.test");
        let action = host.action.clone();
        settle_offer(&mut host, &mut window);
        let mut remotes = Remotes::new(&service);
        remotes.hosts.push(host);
        let step = next(&mut remotes).await;
        remotes.handle(&mut window, step);
        let plan = remotes
            .request_update(&window, uuid::Uuid::new_v4(), &action)
            .unwrap();
        let (complete, gate) = oneshot::channel();
        remotes.hosts[0].closing = Some(Box::pin(async move {
            gate.await.unwrap();
            if lose_demand {
                Ok(())
            } else {
                Err(io::Error::other("cleanup failed"))
            }
        }));
        remotes.confirm_update(&mut window, &plan).unwrap();
        if lose_demand {
            remotes.release_browse(&mut window, &action).unwrap();
        }
        remotes.reconcile(&mut window).unwrap();
        assert!(poll_once(&mut remotes).await.is_none());
        complete.send(()).unwrap();
        let step = next(&mut remotes).await;
        remotes.handle(&mut window, step);
        remotes.reconcile(&mut window).unwrap();
        assert!(remotes
            .hosts
            .iter()
            .all(|host| !host.update_dispatched && host.setup.is_none()));
        let result = remotes.finish(&mut window).await;
        assert_eq!(result.is_ok(), lose_demand);
        local.finish().await.unwrap();
        service.close().await.unwrap();
        let _ = std::fs::remove_dir_all(path);
    }
}

#[tokio::test]
async fn cancellation_after_update_dispatch_retains_late_client_cleanup() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let mut host = admitted(&service, &mut window, "update.test");
    let action = host.action.clone();
    settle_offer(&mut host, &mut window);
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    let plan = remotes
        .request_update(&window, uuid::Uuid::new_v4(), &action)
        .unwrap();
    remotes.confirm_update(&mut window, &plan).unwrap();
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    remotes.reconcile(&mut window).unwrap();
    assert!(remotes.hosts[0].update_dispatched);
    let late = service.connect().await.unwrap();
    let (deliver, gate) = oneshot::channel();
    remotes.hosts[0].setup = Some(Box::pin(async move {
        gate.await.unwrap();
        Ok(late)
    }));
    let action = remotes.hosts[0].action.clone();
    assert!(poll_once(&mut remotes).await.is_none());
    remotes.cancel(&mut window, &action).unwrap();
    assert!(remotes.hosts[0].context.is_closed());
    assert!(poll_once(&mut remotes).await.is_none());
    deliver.send(()).unwrap();
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    assert!(window.source(action.source_id()).is_none());
    assert!(remotes.hosts[0].closing.is_some());
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    remotes.reconcile(&mut window).unwrap();
    assert_eq!(
        window.host(action.source_id()).unwrap().update_state(),
        HostUpdateState::Unconfirmed
    );
    assert!(remotes.hosts[0].setup.is_none());
    assert!(remotes.hosts[0].retry_at.is_none());
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn connected_update_revokes_execution_but_preserves_hidden_screen_references() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let (key, terminal) = window
        .ui_test_remote_terminal_fixture("update.test")
        .unwrap();
    let (other, _) = window
        .ui_test_remote_terminal_fixture("other.test")
        .unwrap();
    let other_owner = window.execution_token(other.source_id());
    let action = window
        .host(key.source_id())
        .unwrap()
        .action_target()
        .clone();
    let mut host = Host::new(&service, action.clone()).unwrap();
    host.setup = None;
    host.offer =
        HostUpdateOffer::from_ready(&offer_ready(action.target()), &host.context.budget).unwrap();
    window.host_mut(&action).unwrap().update_offer = host.offer.clone();
    let version = host
        .context
        .budget
        .reserve(64, false)
        .unwrap()
        .charge("0.2.0".to_owned());
    window.set_backend_version(
        window.lifecycle_token().connection_id(),
        Ok(Arc::new(version)),
    );
    let old_owner = window.execution_token(key.source_id()).unwrap();
    let mut source = SourceRuntime::new(service.connect().await.unwrap(), old_owner);
    source.welcomed = true;
    host.source = Some(source);
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    let plan = remotes
        .request_update(&window, uuid::Uuid::new_v4(), &action)
        .unwrap();
    remotes.confirm_update(&mut window, &plan).unwrap();
    assert!(window.execution_token(key.source_id()).is_none());
    assert_eq!(window.execution_token(other.source_id()), other_owner);
    assert!(has_references(&window, key.source_id()));
    remotes.release_browse(&mut window, &action).unwrap();
    assert!(
        !remotes.hosts[0].dispose,
        "hidden Screen references retain update demand"
    );
    let step = next(&mut remotes).await;
    remotes.handle(&mut window, step);
    remotes.reconcile(&mut window).unwrap();
    assert!(remotes.hosts[0].update_dispatched);
    assert!(has_references(&window, key.source_id()));
    assert_eq!(window.execution_token(other.source_id()), other_owner);
    assert_ne!(
        window.host(key.source_id()).unwrap().action_target(),
        &action
    );
    assert!(!window.accepts_host_update_plan(&plan));
    assert_eq!(terminal.source_token(), old_owner);
    remotes.hosts[0].setup = Some(Box::pin(async { Err(io::Error::other("test ended")) }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn remote_creation_navigation_withdraws_browsing_even_when_exact_connection_is_unavailable() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let client = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(client.connection_id());
    let owner = window.lifecycle_token();
    let mut local = SourceRuntime::new(client, owner);
    let (key, _) = window
        .ui_test_remote_terminal_fixture("create.test")
        .unwrap();
    let action = window
        .host(key.source_id())
        .unwrap()
        .action_target()
        .clone();
    let request = window
        .prepare_remote_screen_creation(&action, window.execution_token(key.source_id()).unwrap())
        .unwrap();
    let mut host = Host::new(&service, action.clone()).unwrap();
    host.setup = None; // A lost setup has no executable Client; never poll SSH.
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    let mut connects = HostConnects::default();
    let pending = uuid::Uuid::new_v4();
    connects.insert(pending, owner);
    assert!(window.host(key.source_id()).unwrap().is_browsing());
    let result = create_remote_screen(
        &mut window,
        &mut local,
        &mut remotes,
        &mut connects,
        request.clone(),
    );
    assert!(result.changed);
    assert!(
        matches!(result.updates.as_slice(), [Update::RemoteScreenCreateRejected { request_id, owner: captured, .. }] if *request_id == request.request_id() && *captured == owner)
    );
    assert!(!window.host(key.source_id()).unwrap().is_browsing());
    assert!(connects.withdraw(pending).is_none());
    assert!(!request.is_follow_requested());
    assert!(window
        .source(key.source_id())
        .unwrap()
        .remote_screen_creation()
        .is_none());
    assert!(remotes.hosts[0].setup.is_none());
    assert_eq!(
        window.opened().len(),
        1,
        "existing reference keeps host demand"
    );
    remotes.finish(&mut window).await.unwrap();
    local.client.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}
