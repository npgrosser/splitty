use super::*;
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

async fn next(updates: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(Duration::from_secs(10), updates.recv())
        .await
        .expect("hub worker timeout")
        .expect("hub worker alive")
}

async fn directory(updates: &mut mpsc::Receiver<Update>) -> HubUpdate {
    loop {
        if let Update::HostDirectory(update) = next(updates).await {
            if matches!(update.state, HostDirectoryState::Ready(_)) {
                return update;
            }
        }
    }
}

#[tokio::test]
async fn passive_directory_refresh_keeps_local_worker_and_cleanup_independent() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (runtime, path) = crate::appearance::local::tests::fixture();
    std::fs::create_dir_all(&path).unwrap();
    let state = runtime.state.clone();
    state
        .hub
        .update_host_preference(json!({"target":"vm1.example","alias":"Build"}))
        .unwrap();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let (commands, receiver) = mpsc::channel(64);
    let (sender, mut updates) = mpsc::channel(128);
    let stop = Arc::new(AtomicBool::new(false));
    let cleanup = Arc::new(AtomicBool::new(false));
    let worker = tokio::spawn(run_service(
        service,
        receiver,
        sender,
        stop.clone(),
        cleanup.clone(),
        ColorScheme::Dark,
    ));
    let initial = directory(&mut updates).await;
    let owner = initial.connection_id;
    let HostDirectoryState::Ready(snapshot) = initial.state else {
        unreachable!()
    };
    assert_eq!(snapshot.directory().hosts[0].target, "vm1.example");
    assert_eq!(
        snapshot.directory().hosts[0].alias.as_deref(),
        Some("Build")
    );
    commands
        .send(Command::RefreshHostDirectory {
            connection: Uuid::new_v4(),
        })
        .await
        .unwrap();
    commands
        .send(Command::RefreshHostDirectory { connection: owner })
        .await
        .unwrap();
    let refreshed = directory(&mut updates).await;
    assert_eq!(refreshed.connection_id, owner);
    assert!(refreshed.generation > initial.generation);
    state
        .hub
        .update_host_preference(json!({"target":"vm1.example","alias":"Changed"}))
        .unwrap();
    let changed = directory(&mut updates).await;
    let HostDirectoryState::Ready(changed_snapshot) = changed.state else {
        unreachable!()
    };
    assert_eq!(
        changed_snapshot.directory().hosts[0].alias.as_deref(),
        Some("Changed")
    );
    assert!(changed.generation > refreshed.generation);
    drop(snapshot);
    drop(changed_snapshot);
    stop.store(true, Ordering::Release);
    drop(commands);
    worker.await.unwrap().unwrap();
    assert!(cleanup.load(Ordering::Acquire));
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn runtime_hub_retains_setup_across_cancelled_receive_and_finishes() {
    use std::{future::Future, task::Poll};
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let owner = Uuid::new_v4();
    let mut hub = RuntimeHub::new(&service, owner);
    let mut pending = Box::pin(hub.next_update());
    std::future::poll_fn(|cx| {
        assert!(pending.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(pending);
    hub.refresh(owner).unwrap();
    hub.finish().await.unwrap();
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn prior_stream_cleanup_failure_is_not_reported_as_successful_shutdown() {
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let completed = AtomicBool::new(false);
    let stop = AtomicBool::new(true);
    let result = finish_owned_run(
        service,
        Ok(()),
        Err(anyhow::anyhow!("hub stream cleanup failed")),
        &completed,
        &stop,
    )
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("hub stream cleanup failed"));
    assert!(!completed.load(Ordering::Acquire));
    assert!(!path.exists());
}

#[tokio::test]
async fn cancelled_close_wait_retains_failure_through_final_finish() {
    use std::{future::Future, task::Poll};
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let mut hub = RuntimeHub::new(&service, Uuid::new_v4());
    let (release, gate) = tokio::sync::oneshot::channel::<()>();
    hub.dirty = false;
    hub.closing = Some(Box::pin(async move {
        gate.await.unwrap();
        Err(std::io::Error::other("gated close failed"))
    }));
    let mut wait = Box::pin(hub.next_update());
    std::future::poll_fn(|cx| {
        assert!(wait.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(wait);
    let mut finish = Box::pin(hub.finish());
    std::future::poll_fn(|cx| {
        assert!(finish.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    release.send(()).unwrap();
    assert!(finish
        .await
        .unwrap_err()
        .to_string()
        .contains("gated close failed"));
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn final_finish_revokes_retained_setup_before_joining_it() {
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let mut hub = RuntimeHub::new(&service, Uuid::new_v4());
    let context = splitty_transport::queue::Context::default();
    let check = context.clone();
    let mut closed = context.closing();
    hub.setup_context = Some(context);
    hub.setup = Some(Box::pin(async move {
        if !*closed.borrow() {
            closed.changed().await.unwrap();
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "setup revoked",
        ))
    }));
    hub.finish().await.unwrap();
    assert!(check.is_closed());
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn published_close_failure_remains_shutdown_failure() {
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let mut hub = RuntimeHub::new(&service, Uuid::new_v4());
    hub.dirty = false;
    hub.closing = Some(Box::pin(async {
        Err(std::io::Error::other("close failure"))
    }));
    let update = hub.next_update().await;
    assert!(matches!(update.state, HostDirectoryState::Failed { .. }));
    assert!(hub
        .finish()
        .await
        .unwrap_err()
        .to_string()
        .contains("close failure"));
    service.close().await.unwrap();
    assert!(!path.exists());
}
