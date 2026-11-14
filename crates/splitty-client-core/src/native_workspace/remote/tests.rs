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
    assert!(!host
        .observe_prelude(&serde_json::json!({"type":"remote.progress","futureField":1}))
        .unwrap());
    assert!(host.daemon_version.is_none());
    let ready = serde_json::json!({"type":"remote.ready","target":"prelude.test","daemon":{"version":"0.3.2","protocol":1,"started":false},"futureField":true});
    assert!(!host.observe_prelude(&ready).unwrap());
    assert!(host.was_ready);
    assert!(host.offer.is_none(), "a version alone offers no update");
    assert_eq!(
        host.daemon_version
            .as_deref()
            .map(|version| version.value.as_str()),
        Some("0.3.2")
    );
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

#[tokio::test]
async fn first_remote_screen_keeps_empty_browsed_host_alive_until_terminal_arrives() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local_client = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local_client.connection_id());
    window
        .ui_test_screen_order_fixture(&[("local", "Local")], None)
        .unwrap();
    let owner = window.lifecycle_token();
    let mut local = SourceRuntime::new(local_client, owner);
    let mut host = admitted(&service, &mut window, "empty.test");
    let action = host.action.clone();
    let id = action.source_id();
    let client = service.connect().await.unwrap();
    window.test_bind_remote_client(id, &client);
    let token = window.source(id).unwrap().source_token();
    window.confirm_ready(token).unwrap();
    let mut source = SourceRuntime::new(client, token);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            source.reconcile(&mut window).unwrap();
            let event = source.client.next_event().await.unwrap().unwrap();
            source.event(&mut window, Box::new(event)).unwrap();
            if window
                .source(id)
                .unwrap()
                .live_catalog()
                .is_some_and(|catalog| catalog.is_empty())
                && !source.observation_pending(&window)
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    window.host_mut(&action).unwrap().catalog = CatalogState::Ready;
    host.setup = None;
    host.source = Some(source);
    host.remote_ready = true;
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    assert!(window
        .opened()
        .iter()
        .all(|entry| entry.key().source_id() != id));
    assert!(window.host(id).unwrap().is_browsing());
    let request = window
        .prepare_remote_screen_creation(&action, token)
        .unwrap();
    let result = create_remote_screen(
        &mut window,
        &mut local,
        &mut remotes,
        &mut HostConnects::default(),
        request.clone(),
    );
    assert!(result.changed);
    assert!(result.updates.is_empty());
    assert!(!window.host(id).unwrap().is_browsing());
    assert!(
        !remotes.hosts[0].dispose,
        "the new reference must replace browse demand before it is released"
    );
    assert!(has_references(&window, id));
    assert!(request.is_follow_requested());
    let source = remotes.hosts[0].source.as_mut().unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            source.reconcile(&mut window).unwrap();
            let event = source.client.next_event().await.unwrap().unwrap();
            let effects = source.event(&mut window, Box::new(event)).unwrap();
            if let Some(request) = effects.follow_remote_creation {
                window.follow_remote_creation(&request).unwrap();
            }
            if window.shown_key().is_some_and(|key| {
                key.source_id() == id
                    && window
                        .source(id)
                        .unwrap()
                        .screen(key.screen_id())
                        .is_some_and(|screen| screen.snapshot().is_some())
            }) {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(window.opened().len(), 2);
    remotes.finish(&mut window).await.unwrap();
    local.client.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn a_host_restored_at_start_keeps_its_demand_until_it_settles() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (runtime, path) = crate::appearance::local::tests::fixture();
    std::fs::create_dir_all(&path).unwrap();
    runtime
        .state
        .hub
        .update_host_preference(serde_json::json!({"target":"known.test","alias":"Known"}))
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
    assert!(remotes
        .restore(&mut window, owner, "unknown.test", Vec::new(), &snapshot)
        .is_err());
    remotes
        .restore(
            &mut window,
            owner,
            "known.test",
            vec!["work".into()],
            &snapshot,
        )
        .unwrap();
    // A host that is already in the window is not restored a second time.
    remotes
        .restore(
            &mut window,
            owner,
            "known.test",
            vec!["other".into()],
            &snapshot,
        )
        .unwrap();
    assert_eq!(remotes.restoring.len(), 1);
    let action = remotes.hosts[0].action.clone();
    // No SSH is started: this attempt fails as an unreachable host does.
    remotes.hosts[0].setup = Some(Box::pin(async {
        Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "unreachable",
        ))
    }));

    // A panel that closes releases what it connected, not the restore.
    remotes.release_all_browse(&mut window);
    assert!(window.host(action.source_id()).unwrap().is_browsing());
    assert!(remotes.reconcile(&mut window).unwrap().updates.is_empty());

    let mut notices = Vec::new();
    for _ in 0..4 {
        if remotes.restoring.is_empty() {
            break;
        }
        let step = next(&mut remotes).await;
        remotes.handle(&mut window, step);
        notices.extend(remotes.reconcile(&mut window).unwrap().updates);
    }
    assert!(remotes.restoring.is_empty());
    assert!(
        matches!(notices.as_slice(), [Update::HostReconnectFailed { target }] if target == "known.test"),
        "an unreachable host leaves with one notice"
    );
    assert!(window
        .host(action.source_id())
        .is_none_or(|host| !host.is_browsing()));
    remotes.finish(&mut window).await.unwrap();
    hub.finish().await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn a_restored_host_opens_the_screens_it_still_has() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let owner = window.lifecycle_token();
    let used = window
        .ui_test_screen_order_fixture(&[("local", "Local")], None)
        .unwrap()
        .remove(0);
    let keys = window
        .ui_test_screen_order_fixture(&[("api", "api"), ("logs", "logs")], Some("back.test"))
        .unwrap();
    let id = keys[0].source_id();
    window.detach_all(owner, id).unwrap();
    window.select(owner, &used).unwrap();
    let action = window.host(id).unwrap().action_target().clone();
    window.host_mut(&action).unwrap().browsing = true;
    let mut host = Host::new(&service, action.clone()).unwrap();
    host.setup = None;
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    remotes.restoring.push((
        action.clone(),
        vec!["logs".into(), "ended".into(), "api".into()],
    ));
    assert!(remotes.reconcile(&mut window).unwrap().changed);
    assert!(remotes.restoring.is_empty());
    assert_eq!(
        window
            .source(id)
            .unwrap()
            .screens()
            .iter()
            .map(|screen| screen.id.as_str().to_owned())
            .collect::<Vec<_>>(),
        ["logs", "api"],
        "the stored order, without the Screen that ended"
    );
    assert_eq!(
        window.shown_key(),
        Some(used),
        "the Screen in use stays shown"
    );
    assert!(!window.host(id).unwrap().is_browsing());
    assert!(
        !remotes.hosts[0].dispose,
        "the opened Screens hold the host now"
    );
    remotes.hosts[0].setup = Some(Box::pin(async { Err(io::Error::other("test ended")) }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}
#[tokio::test]
async fn a_screen_that_ended_on_its_host_leaves_the_window() {
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let keys = window
        .ui_test_screen_order_fixture(&[("api", "api"), ("logs", "logs")], Some("back.test"))
        .unwrap();
    let id = keys[0].source_id();
    window.select(window.lifecycle_token(), &keys[1]).unwrap();
    let action = window.host(id).unwrap().action_target().clone();
    let mut host = Host::new(&service, action).unwrap();
    host.setup = None;
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);
    window.ui_test_remote_catalog(id, &["api"]).unwrap();
    assert!(window.source(id).unwrap().is_missing("logs"));
    assert!(remotes.reconcile(&mut window).unwrap().changed);
    assert_eq!(
        window
            .source(id)
            .unwrap()
            .screens()
            .iter()
            .map(|screen| screen.id.as_str().to_owned())
            .collect::<Vec<_>>(),
        ["api"]
    );
    assert_eq!(
        window.shown_key(),
        Some(keys[0].clone()),
        "the view moves to a Screen that still exists"
    );
    assert!(!remotes.hosts[0].dispose, "the other Screen holds the host");
    remotes.hosts[0].setup = Some(Box::pin(async { Err(io::Error::other("test ended")) }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

/// Drive the first host's bound source until `done` holds.
#[cfg(not(target_os = "windows"))]
async fn pump_first_host(
    remotes: &mut Remotes<'_>,
    window: &mut WindowWorkspace,
    done: impl Fn(&WindowWorkspace, &SourceRuntime) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let source = remotes.hosts[0].source.as_mut().expect("bound source");
            source.reconcile(window).unwrap();
            if done(window, source) {
                break;
            }
            let event = source.client.next_event().await.unwrap().unwrap();
            source.event(window, Box::new(event)).unwrap();
        }
    })
    .await
    .expect("the host's service answers");
}

/// session-scratchpad, Saving Through The Window: a save from a pane of a
/// host's Screen passes the remote command gate, reaches the host's service
/// and comes back in the Screen's snapshot. A save refused at the gate stays
/// in the window only, where the next snapshot takes its place.
#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn a_scratchpad_save_on_a_remote_screen_reaches_its_host() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let client = service.connect().await.unwrap();
    let ids = client.create_screen("Host screen").unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let id = window
        .admit_remote(window.lifecycle_token(), "pad.test")
        .unwrap();
    window.test_bind_remote_client(id, &client);
    let token = window.source(id).unwrap().source_token();
    window.confirm_ready(token).unwrap();
    let action = window.host(id).unwrap().action_target().clone();
    let mut host = Host::new(&service, action.clone()).unwrap();
    host.setup = None;
    host.source = Some(SourceRuntime::new(client, token));
    let mut remotes = Remotes::new(&service);
    remotes.hosts.push(host);

    pump_first_host(&mut remotes, &mut window, |window, source| {
        window
            .source(id)
            .unwrap()
            .live_catalog()
            .is_some_and(|catalog| catalog.len() == 1)
            && !source.observation_pending(window)
    })
    .await;
    window.host_mut(&action).unwrap().catalog = crate::workspace::CatalogState::Ready;
    let key = window.source(id).unwrap().screen_key(&ids.screen_id);
    window.open_existing(token, &key).unwrap();
    let screen_id = ids.screen_id.clone();
    pump_first_host(&mut remotes, &mut window, |window, source| {
        window
            .source(id)
            .unwrap()
            .screen(&screen_id)
            .and_then(|screen| screen.snapshot())
            .is_some_and(|snapshot| !snapshot.sessions.is_empty())
            && !source.observation_pending(window)
    })
    .await;
    let session = window
        .source(id)
        .unwrap()
        .screen(&ids.screen_id)
        .unwrap()
        .snapshot()
        .unwrap()
        .sessions
        .keys()
        .next()
        .unwrap()
        .to_string();

    let effects = remotes
        .command(
            &mut window,
            Command::new(
                token,
                CommandKind::SetScratchpad {
                    screen: ids.screen_id.clone(),
                    connection: token.connection_id(),
                    session: session.clone(),
                    text: "ask about the failing test".into(),
                    if_version: None,
                },
            ),
        )
        .unwrap();
    assert!(effects.updates.is_empty(), "the save is not rejected");
    assert!(
        window.host(id).unwrap().diagnostic().is_none(),
        "the save leaves no rejection on the host"
    );
    pump_first_host(&mut remotes, &mut window, |window, _| {
        window
            .source(id)
            .unwrap()
            .screen(&screen_id)
            .and_then(|screen| screen.snapshot())
            .is_some_and(|snapshot| {
                snapshot.sessions.iter().any(|(id, entry)| {
                    id.as_str() == session && entry.scratchpad == "ask about the failing test"
                })
            })
    })
    .await;

    remotes.hosts[0].setup = Some(Box::pin(async { Err(io::Error::other("test ended")) }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

/// A host served by `client`, with its Screen `screen` opened and shown in
/// `window`. The host keeps its place in the window without a Screen.
#[cfg(not(target_os = "windows"))]
async fn opened_host_screen<'a>(
    service: &'a OwnedService,
    window: &mut WindowWorkspace,
    client: Client,
    screen: &str,
) -> (Remotes<'a>, SourceId, SourceToken) {
    let id = window
        .admit_remote(window.lifecycle_token(), "close.test")
        .unwrap();
    window.test_bind_remote_client(id, &client);
    let token = window.source(id).unwrap().source_token();
    window.confirm_ready(token).unwrap();
    let action = window.host(id).unwrap().action_target().clone();
    let mut host = Host::new(service, action.clone()).unwrap();
    host.setup = None;
    host.source = Some(SourceRuntime::new(client, token));
    let mut remotes = Remotes::new(service);
    remotes.hosts.push(host);
    pump_first_host(&mut remotes, window, |window, source| {
        window
            .source(id)
            .unwrap()
            .live_catalog()
            .is_some_and(|catalog| catalog.len() == 1)
            && !source.observation_pending(window)
    })
    .await;
    let host = window.host_mut(&action).unwrap();
    host.catalog = crate::workspace::CatalogState::Ready;
    host.browsing = true;
    let key = window.source(id).unwrap().screen_key(screen);
    window.open_existing(token, &key).unwrap();
    pump_first_host(&mut remotes, window, |window, source| {
        window
            .source(id)
            .unwrap()
            .screen(screen)
            .is_some_and(|screen| screen.snapshot().is_some())
            && !source.observation_pending(window)
    })
    .await;
    (remotes, id, token)
}

/// The IDs of the Screens the service lists now.
#[cfg(not(target_os = "windows"))]
async fn listed_screens(client: &mut Client) -> Vec<String> {
    client.request_workspace().unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let event = client.next_event().await.unwrap().unwrap();
            if let Some(catalog) = event
                .json()
                .filter(|value| value["type"] == "mux.workspace_snapshot")
            {
                return catalog["screens"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|screen| screen["id"].as_str().unwrap().to_owned())
                    .collect();
            }
        }
    })
    .await
    .expect("the service lists its Screens")
}

/// Close `pane` of the host's shown Screen and drive the host until the
/// close settled and the Screen left the window. `report` may replace an
/// event of the host's service by what another service would have sent.
#[cfg(not(target_os = "windows"))]
async fn close_last_pane(
    remotes: &mut Remotes<'_>,
    window: &mut WindowWorkspace,
    id: SourceId,
    token: SourceToken,
    screen: &str,
    pane: &str,
    report: impl Fn(&serde_json::Value) -> Option<serde_json::Value>,
) {
    let target = CloseTarget::Pane {
        screen: screen.to_owned(),
        pane: pane.to_owned(),
    };
    close_settled(remotes, window, id, token, target, report).await;
}

/// Close `target` on the host and drive the host until the close settled
/// and the host has no Screen left in the window.
#[cfg(not(target_os = "windows"))]
async fn close_settled(
    remotes: &mut Remotes<'_>,
    window: &mut WindowWorkspace,
    id: SourceId,
    token: SourceToken,
    target: CloseTarget,
    report: impl Fn(&serde_json::Value) -> Option<serde_json::Value>,
) {
    let plan = window.source(id).unwrap().assess_close(1, target).unwrap();
    let close = Command::new(
        token,
        CommandKind::RequestClose {
            plan,
            confirmed: true,
        },
    );
    let effects = remotes.command(window, close).unwrap();
    assert!(
        matches!(effects.updates.as_slice(), [Update::CloseSubmitted { .. }]),
        "a host's Screen or pane closes through the remote command gate"
    );
    let mut settled = false;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            remotes.reconcile(window).unwrap();
            if settled && window.source(id).unwrap().screens().is_empty() {
                break;
            }
            let source = remotes.hosts[0].source.as_mut().expect("bound source");
            source.reconcile(window).unwrap();
            let event = source.client.next_event().await.unwrap().unwrap();
            let event = match event.json().and_then(&report) {
                Some(value) => Event::ui_test_json(event.connection_id, &value)
                    .await
                    .unwrap(),
                None => event,
            };
            let effects = source.event(window, Box::new(event)).unwrap();
            if let Some(reason) = effects.updates.iter().find_map(|update| match update {
                Update::CloseRejected { reason, .. } => Some(reason),
                _ => None,
            }) {
                panic!("the close was not settled: {reason}");
            }
            settled |= effects
                .updates
                .iter()
                .any(|update| matches!(update, Update::CloseSettled { .. }));
        }
    })
    .await
    .expect("the closed Screen leaves the window");
}

/// workspace-model, Closing a Pane (Close the last pane of a screen), seen
/// from a window: the host removes the Screen with its last pane, the Screen
/// leaves the window, and the window shows another Screen.
#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn closing_the_last_pane_of_a_host_screen_takes_the_screen_out_of_the_window() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let mut local = service.connect().await.unwrap();
    let client = service.connect().await.unwrap();
    let ids = client.create_screen("Host screen").unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let other = window
        .ui_test_screen_order_fixture(&[("local", "Local")], None)
        .unwrap()
        .remove(0);
    let (mut remotes, id, token) =
        opened_host_screen(&service, &mut window, client, &ids.screen_id).await;
    assert_eq!(
        window.shown_key(),
        Some(window.source(id).unwrap().screen_key(&ids.screen_id))
    );

    close_last_pane(
        &mut remotes,
        &mut window,
        id,
        token,
        &ids.screen_id,
        &ids.pane_id,
        |_| None,
    )
    .await;

    assert_eq!(
        window.shown_key(),
        Some(other),
        "the window shows a Screen that still exists"
    );
    assert!(
        listed_screens(&mut local).await.is_empty(),
        "the host removed the Screen with its last pane"
    );
    remotes.hosts[0].setup = Some(Box::pin(async { Err(io::Error::other("test ended")) }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

/// A host whose service keeps a Screen without panes, as an earlier version
/// does: once the close of the last pane settled, the window closes the
/// Screen there with `close_screen`. The fixture's service removes an
/// emptied Screen itself, so the Screen keeps a second pane the window is
/// not told about; its end shows that `close_screen` reached the host.
#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn a_host_that_keeps_an_emptied_screen_gets_it_closed() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let mut local = service.connect().await.unwrap();
    let client = service.connect().await.unwrap();
    let ids = client.create_screen("Host screen").unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let (mut remotes, id, token) =
        opened_host_screen(&service, &mut window, client, &ids.screen_id).await;
    let key = window.source(id).unwrap().screen_key(&ids.screen_id);
    let split = Command::new(
        token,
        CommandKind::Split {
            screen: key,
            pane: ids.pane_id.clone(),
            axis: Axis::Horizontal,
        },
    );
    assert!(remotes
        .command(&mut window, split)
        .unwrap()
        .updates
        .is_empty());
    let screen_id = ids.screen_id.clone();
    pump_first_host(&mut remotes, &mut window, |window, source| {
        window
            .source(id)
            .unwrap()
            .screen(&screen_id)
            .and_then(|screen| screen.snapshot())
            .is_some_and(|snapshot| snapshot.sessions.len() == 2)
            && !source.observation_pending(window)
    })
    .await;

    let reported = ids.screen_id.clone();
    close_last_pane(
        &mut remotes,
        &mut window,
        id,
        token,
        &ids.screen_id,
        &ids.pane_id,
        move |event| {
            (event["type"] == "mux.screen_snapshot" && event["screen"]["id"] == reported.as_str())
                .then(|| {
                    let mut event = event.clone();
                    let screen = &mut event["screen"];
                    screen["panes"] = serde_json::json!({});
                    screen["sessions"] = serde_json::json!({});
                    screen["floating"] = serde_json::json!([]);
                    screen["root"] =
                        serde_json::json!({"type": "slot", "id": "root", "paneIds": []});
                    event
                })
        },
    )
    .await;

    assert!(
        listed_screens(&mut local).await.is_empty(),
        "the window closed the Screen its host had kept"
    );
    remotes.hosts[0].setup = Some(Box::pin(async { Err(io::Error::other("test ended")) }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

/// A Screen without panes on a host, as a host of an earlier version keeps
/// it: a new terminal passes the worker's source admission and the remote
/// command gate, and the host starts it in that Screen, which keeps its name.
#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn an_empty_host_screen_takes_a_new_terminal() {
    use splitty_core::{
        ids::ScreenId,
        mux::{ScreenState, WorkspaceState},
    };
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let mut workspace = WorkspaceState::default();
    let kept = ScreenId::from("kept");
    let mut screen = ScreenState::new(kept.clone(), Some("Kept".into()));
    screen.revision = 1;
    workspace.screens.insert(kept.clone(), screen);
    workspace.screen_order.push(kept.clone());
    workspace.active_screen_id = Some(kept);
    let (runtime, path) = crate::appearance::local::tests::fixture_with_workspace(workspace);
    let service: OwnedService = crate::EmbeddedService::from_runtime(runtime).into();
    let local = service.connect().await.unwrap();
    let client = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let (mut remotes, id, token) = opened_host_screen(&service, &mut window, client, "kept").await;
    assert!(window.source(id).unwrap().screen_ran_empty("kept"));

    let command = Command::new(
        token,
        CommandKind::NewTerminal {
            screen: "kept".into(),
            connection: token.connection_id(),
        },
    );
    crate::native_workspace::validate_worker_command_source(&window, &command)
        .expect("worker ingress admits a new terminal on a host's Screen");
    let effects = remotes.command(&mut window, command).unwrap();
    assert!(effects.updates.is_empty(), "the terminal is not rejected");
    assert!(
        window.host(id).unwrap().diagnostic().is_none(),
        "the terminal leaves no rejection on the host"
    );
    pump_first_host(&mut remotes, &mut window, |window, source| {
        window
            .source(id)
            .unwrap()
            .screen("kept")
            .and_then(|screen| screen.snapshot())
            .is_some_and(|snapshot| snapshot.panes.len() == 1 && snapshot.sessions.len() == 1)
            && !source.observation_pending(window)
    })
    .await;
    let source = window.source(id).unwrap();
    assert_eq!(source.screens().len(), 1);
    assert_eq!(source.screens()[0].name.as_deref(), Some("Kept"));

    remotes.hosts[0].setup = Some(Box::pin(async { Err(io::Error::other("test ended")) }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

/// mux-protocol, Exact Tab Placement, for a host's Screen: the host's
/// welcome announces `tab-placement-v1` to the window as the local service's
/// does, and a tab dropped before another one is placed there on the host.
#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn a_tab_of_a_host_screen_is_placed_exactly() {
    use splitty_core::mux::LayoutNode;
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let client = service.connect().await.unwrap();
    let ids = client.create_screen("Host screen").unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let (mut remotes, id, token) =
        opened_host_screen(&service, &mut window, client, &ids.screen_id).await;
    assert!(
        window.source(id).unwrap().tab_placement_supported(),
        "the host's welcome reached the window's source"
    );
    let key = window.source(id).unwrap().screen_key(&ids.screen_id);
    let tab = Command::new(
        token,
        CommandKind::Tab {
            screen: key,
            pane: ids.pane_id.clone(),
        },
    );
    assert!(remotes
        .command(&mut window, tab)
        .unwrap()
        .updates
        .is_empty());
    let screen_id = ids.screen_id.clone();
    let tabs = |window: &WindowWorkspace| -> Option<(String, Vec<String>)> {
        let screen = window.source(id)?.screen(&screen_id)?.server_snapshot()?;
        match &screen.root {
            LayoutNode::Slot { id, pane_ids, .. } => Some((
                id.to_string(),
                pane_ids.iter().map(ToString::to_string).collect(),
            )),
            LayoutNode::Split { .. } => None,
        }
    };
    pump_first_host(&mut remotes, &mut window, |window, source| {
        tabs(window).is_some_and(|(_, panes)| panes.len() == 2)
            && window
                .source(id)
                .unwrap()
                .screen(&screen_id)
                .and_then(|screen| screen.snapshot())
                .is_some_and(|snapshot| snapshot.sessions.len() == 2)
            && !source.observation_pending(window)
    })
    .await;
    let (slot, panes) = tabs(&window).unwrap();
    assert_eq!(panes[0], ids.pane_id);
    let source = window.source(id).unwrap();
    let drag = source.capture_drag(&panes[1]).expect("a tab to drag");
    assert!(source.can_place_tab(&drag, &slot, Some(&panes[0])));

    let place = Command::new(
        token,
        CommandKind::PlaceTab {
            drag,
            target: slot,
            before: Some(panes[0].clone()),
        },
    );
    assert!(remotes
        .command(&mut window, place)
        .unwrap()
        .updates
        .is_empty());
    let moved = [panes[1].clone(), panes[0].clone()];
    pump_first_host(&mut remotes, &mut window, |window, _| {
        !window.source(id).unwrap().placement_pending()
            && tabs(window).is_some_and(|(_, panes)| panes == moved)
    })
    .await;

    remotes.hosts[0].setup = Some(Box::pin(async { Err(io::Error::other("test ended")) }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

/// workspace-model, Native Tab Group Move: a host's Screen takes a dragged
/// group as one `arrange_panes`, through the same command gate as a local
/// one. While the host's daemon is taken not to announce
/// `layout-arrange-v1`, the drop sends nothing and the window is told the
/// daemon needs an update.
#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn a_tab_group_of_a_host_screen_moves_by_drag_unless_its_daemon_is_older() {
    use splitty_core::mux::LayoutNode;
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let client = service.connect().await.unwrap();
    let ids = client.create_screen("Host screen").unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let (mut remotes, id, token) =
        opened_host_screen(&service, &mut window, client, &ids.screen_id).await;
    let key = window.source(id).unwrap().screen_key(&ids.screen_id);
    let split = Command::new(
        token,
        CommandKind::Split {
            screen: key,
            pane: ids.pane_id.clone(),
            axis: Axis::Horizontal,
        },
    );
    assert!(remotes
        .command(&mut window, split)
        .unwrap()
        .updates
        .is_empty());
    let screen_id = ids.screen_id.clone();
    let groups = |window: &WindowWorkspace| -> Vec<(String, Vec<String>)> {
        let Some(screen) = window
            .source(id)
            .and_then(|source| source.screen(&screen_id))
            .and_then(|screen| screen.server_snapshot())
        else {
            return Vec::new();
        };
        let mut groups = Vec::new();
        screen.root.walk(&mut |node| {
            if let LayoutNode::Slot { id, pane_ids, .. } = node {
                groups.push((
                    id.to_string(),
                    pane_ids.iter().map(ToString::to_string).collect(),
                ));
            }
            true
        });
        groups
    };
    pump_first_host(&mut remotes, &mut window, |window, source| {
        groups(window).len() == 2
            && window
                .source(id)
                .unwrap()
                .screen(&screen_id)
                .and_then(|screen| screen.snapshot())
                .is_some_and(|snapshot| snapshot.sessions.len() == 2)
            && !source.observation_pending(window)
    })
    .await;
    let before = groups(&window);
    let (target, _) = before[0].clone();
    let (dragged, panes) = before[1].clone();
    let move_group = |window: &WindowWorkspace| {
        let source = window.source(id).unwrap();
        let drag = source
            .capture_group_drag(&dragged)
            .expect("a group to drag");
        assert!(source.can_move_group(&drag, &target, MovePlacement::Tab, None));
        Command::new(
            token,
            CommandKind::MoveGroup {
                drag,
                target: target.clone(),
                placement: MovePlacement::Tab,
                before: None,
            },
        )
    };

    // An older daemon: nothing is sent, one status says why.
    remotes.hosts[0]
        .source
        .as_mut()
        .unwrap()
        .announce_only(&[splitty_core::protocol::capability::TAB_PLACEMENT]);
    let command = move_group(&window);
    let refused = remotes.command(&mut window, command).unwrap().updates;
    assert!(
        matches!(refused.as_slice(), [Update::ServerOutdated(status)]
            if status.contains("host's Splitty daemon is older")),
        "{} updates",
        refused.len()
    );
    assert_eq!(groups(&window), before, "the layout stays as it was");
    let shown_groups = window
        .source(id)
        .unwrap()
        .screen(&screen_id)
        .and_then(|screen| screen.snapshot())
        .map(|screen| matches!(&screen.root, LayoutNode::Split { children, .. } if children.len() == 2));
    assert_eq!(shown_groups, Some(true), "no move is shown either");

    // The daemon as it is: the group joins the other one.
    remotes.hosts[0]
        .source
        .as_mut()
        .unwrap()
        .announce_only(&[splitty_core::protocol::capability::LAYOUT_ARRANGE]);
    let command = move_group(&window);
    assert!(remotes
        .command(&mut window, command)
        .unwrap()
        .updates
        .is_empty());
    let mut joined = vec![ids.pane_id.clone()];
    joined.extend(panes);
    pump_first_host(&mut remotes, &mut window, |window, _| {
        let groups = groups(window);
        groups.len() == 1 && groups[0].1 == joined
    })
    .await;

    remotes.hosts[0].setup = Some(Box::pin(async { Err(io::Error::other("test ended")) }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

/// workspace-model, Native Tab Group Transfer: a host's tab group moves to a
/// new Screen of that host as one `move_tab_group_to_screen`, through the
/// same command gate as a local one. While the host's daemon is taken not to
/// announce `tab-group-transfer-v1`, nothing is sent: the transfer is
/// rejected and the window is told the daemon needs an update.
#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn a_tab_group_of_a_host_screen_moves_to_another_screen_unless_its_daemon_is_older() {
    use splitty_core::mux::LayoutNode;
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (service, path) = service();
    let local = service.connect().await.unwrap();
    let client = service.connect().await.unwrap();
    let ids = client.create_screen("Host screen").unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let (mut remotes, id, token) =
        opened_host_screen(&service, &mut window, client, &ids.screen_id).await;
    let key = window.source(id).unwrap().screen_key(&ids.screen_id);
    let tab = Command::new(
        token,
        CommandKind::Tab {
            screen: key,
            pane: ids.pane_id.clone(),
        },
    );
    assert!(remotes
        .command(&mut window, tab)
        .unwrap()
        .updates
        .is_empty());
    let screen_id = ids.screen_id.clone();
    let tabs_on = |window: &WindowWorkspace, screen: &str| -> Option<Vec<String>> {
        let snapshot = window
            .source(id)
            .and_then(|source| source.screen(screen))
            .and_then(|screen| screen.server_snapshot())?;
        match &snapshot.root {
            LayoutNode::Slot { pane_ids, .. } => {
                Some(pane_ids.iter().map(ToString::to_string).collect())
            }
            LayoutNode::Split { .. } => None,
        }
    };
    pump_first_host(&mut remotes, &mut window, |window, source| {
        tabs_on(window, &screen_id).is_some_and(|tabs| tabs.len() == 2)
            && window
                .source(id)
                .unwrap()
                .screen(&screen_id)
                .and_then(|screen| screen.snapshot())
                .is_some_and(|snapshot| snapshot.sessions.len() == 2)
            && !source.observation_pending(window)
    })
    .await;
    let tabs = tabs_on(&window, &screen_id).unwrap();
    let new_screen = uuid::Uuid::new_v4().to_string();
    let transfer = |window: &WindowWorkspace, request_id| {
        let source = window.source(id).unwrap();
        let group = source.capture_group_action(&tabs[1]).expect("a group");
        assert_eq!(group.tab_count(), 2);
        assert!(source.can_transfer_group(&group));
        Command::new(
            token,
            CommandKind::TransferGroup {
                request_id,
                source: group,
                target: ScreenTransferTarget::New {
                    screen_id: new_screen.clone(),
                },
            },
        )
    };

    // An older daemon: nothing is sent; the transfer settles as rejected
    // and one status says why.
    remotes.hosts[0]
        .source
        .as_mut()
        .unwrap()
        .announce_only(&[splitty_core::protocol::capability::LAYOUT_ARRANGE]);
    let request = uuid::Uuid::new_v4();
    let command = transfer(&window, request);
    let refused = remotes.command(&mut window, command).unwrap().updates;
    assert!(
        matches!(refused.as_slice(), [
            Update::ScreenTransfer(ScreenTransferResult { request_id, outcome: ScreenTransferOutcome::Rejected(_), .. }),
            Update::ServerOutdated(status),
        ] if *request_id == request && status.contains("host's Splitty daemon is older")),
        "{} updates",
        refused.len()
    );
    assert_eq!(tabs_on(&window, &screen_id), Some(tabs.clone()));
    assert!(!window.source(id).unwrap().transfer_pending());

    // The daemon as it is: the group moves whole and its Screen goes along.
    remotes.hosts[0]
        .source
        .as_mut()
        .unwrap()
        .announce_only(&[splitty_core::protocol::capability::TAB_GROUP_TRANSFER]);
    let command = transfer(&window, uuid::Uuid::new_v4());
    assert!(remotes
        .command(&mut window, command)
        .unwrap()
        .updates
        .is_empty());
    pump_first_host(&mut remotes, &mut window, |window, _| {
        tabs_on(window, &new_screen).as_ref() == Some(&tabs)
            && window.source(id).unwrap().screen(&screen_id).is_none()
            && !window.source(id).unwrap().transfer_pending()
    })
    .await;

    remotes.hosts[0].setup = Some(Box::pin(async { Err(io::Error::other("test ended")) }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

/// remote-screens, Host Screen Directory Cache (A Screen that left a
/// connected host): once the host confirmed the terminate of a Screen, the
/// window reports the host's remaining Screens over its local connection and
/// the local service stores them, so the host's next connection does not
/// list the Screen while it is not ready yet.
#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn a_terminated_host_screen_leaves_the_stored_directory() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let _fixture_slot = CLIENT_FIXTURES.acquire().await.unwrap();
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let hub = runtime.state.hub.clone();
    let service: OwnedService = crate::EmbeddedService::from_runtime(runtime).into();
    let mut local = service.connect().await.unwrap();
    let client = service.connect().await.unwrap();
    let ids = client.create_screen("Host screen").unwrap();
    // What the service stored when the host's connection became ready.
    hub.update_catalog(serde_json::json!({"target":"close.test","screens":[
        {"id":ids.screen_id,"name":"Host screen","panes":1}
    ]}))
    .unwrap();
    let stored = |hub: &splitty_server::hub::Hub| {
        hub.directory()
            .hosts
            .into_iter()
            .find(|host| host.target == "close.test")
            .map(|host| host.screens)
            .unwrap_or_default()
    };
    let mut window = WindowWorkspace::new(local.connection_id());
    window
        .ui_test_screen_order_fixture(&[("local", "Local")], None)
        .unwrap();
    let (mut remotes, id, token) =
        opened_host_screen(&service, &mut window, client, &ids.screen_id).await;
    // The service keeps stored pane counts (`hub-host-state-v1`).
    let mut reports = super::super::host_catalog::HostCatalogReports::new(true);
    let mut sent = 0;
    reports.sync(&window, Some(&hub.directory()), |_, _| {
        sent += 1;
        Ok(())
    });
    assert_eq!(sent, 0, "the stored Screen is live");

    let screen = CloseTarget::Screen {
        screen: ids.screen_id.clone(),
    };
    close_settled(&mut remotes, &mut window, id, token, screen, |_| None).await;
    assert!(
        listed_screens(&mut local).await.is_empty(),
        "the host ended the Screen"
    );
    assert_eq!(stored(&hub).len(), 1, "nothing but the window reports it");
    reports.sync(&window, Some(&hub.directory()), |target, screens| {
        sent += 1;
        local.report_host_screens(target, screens)
    });
    assert_eq!(sent, 1);
    tokio::time::timeout(Duration::from_secs(15), async {
        while !stored(&hub).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the local service stores the report");

    remotes.hosts[0].setup = Some(Box::pin(async { Err(io::Error::other("test ended")) }));
    remotes.finish(&mut window).await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}
