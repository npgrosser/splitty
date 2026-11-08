//! Real worker/session/API composition with a gated service forwarding adapter.
use super::*;
use crate::ports::MappingKey;
use splitty_remote::{
    bridge::NativeBridge,
    handshake::{Handshake, RunningDaemon},
    install::{Prepared, Server},
    ssh::SshTarget,
    tunnel::{exposure::Share, PortReservation},
};
use splitty_server::hub::{Connector, Forwarding, HubError};
use splitty_transport::{admission, budget::Charged, queue};
use std::{future::Future, pin::Pin};
use tokio::sync::Notify;
use uuid::Uuid;
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
struct GatedForward {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    completed: Arc<Notify>,
}
impl Connector for GatedForward {
    fn prepare<'a>(
        &'a self,
        _: &'a SshTarget,
        _: Box<dyn FnOnce(&str) + Send + 'a>,
        _: &'a queue::Context,
    ) -> BoxFuture<'a, Result<Prepared, HubError>> {
        unreachable!("Ports must not request a Screen transport")
    }
    fn handshake<'a>(
        &'a self,
        _: &'a SshTarget,
        _: &'a Prepared,
        _: &'a queue::Context,
    ) -> BoxFuture<'a, Result<Charged<Handshake>, HubError>> {
        unreachable!("no Screen handshake")
    }
    fn stop_daemon<'a>(
        &'a self,
        _: &'a SshTarget,
        _: &'a Server,
        _: &'a RunningDaemon,
        _: &'a queue::Context,
    ) -> BoxFuture<'a, Result<(), HubError>> {
        unreachable!("no daemon mutation")
    }
    fn migrate_legacy<'a>(
        &'a self,
        _: &'a SshTarget,
        _: &'a Server,
        _: &'a RunningDaemon,
        _: &'a queue::Context,
    ) -> BoxFuture<'a, Result<(), HubError>> {
        unreachable!("no daemon migration")
    }
    fn bridge<'a>(
        &'a self,
        _: &'a SshTarget,
        _: &'a Server,
        _: &'a RunningDaemon,
        _: (admission::Carrier, admission::Opening),
        _: &'a queue::Context,
    ) -> BoxFuture<'a, Result<NativeBridge, HubError>> {
        unreachable!("no Screen bridge")
    }
    fn forward<'a>(
        &'a self,
        target: &'a SshTarget,
        _local: PortReservation,
        _remote: u16,
    ) -> BoxFuture<'a, Result<Box<dyn Forwarding>, HubError>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            self.completed.notify_one();
            Err(HubError::NoDaemon {
                host: target.as_str().into(),
            })
        })
    }
    fn reserve_share(&self, _: u16) -> BoxFuture<'_, Result<Option<Share>, HubError>> {
        unreachable!("outbound only")
    }
    fn reverse<'a>(
        &'a self,
        _: &'a SshTarget,
        _: u16,
        _: u16,
        _: Option<&'a Share>,
    ) -> BoxFuture<'a, Result<Box<dyn Forwarding>, HubError>> {
        unreachable!("outbound only")
    }
}
async fn next(updates: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(Duration::from_secs(10), updates.recv())
        .await
        .expect("Ports worker deadline")
        .expect("worker alive")
}
#[tokio::test]
async fn ports_slow_start_keeps_terminal_host_writes_and_passive_reads_responsive() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (runtime, path) =
        crate::appearance::local::tests::fixture_with_connector(Box::new(GatedForward {
            entered: entered.clone(),
            release: release.clone(),
            completed: Arc::new(Notify::new()),
        }));
    std::fs::create_dir_all(&path).unwrap();
    let (commands, receiver) = mpsc::channel(32);
    let (sender, mut updates) = mpsc::channel(128);
    let stop = Arc::new(AtomicBool::new(false));
    let cleanup = Arc::new(AtomicBool::new(false));
    let worker = tokio::spawn(run_service(
        crate::EmbeddedService::from_runtime(runtime),
        receiver,
        sender,
        stop.clone(),
        cleanup.clone(),
        ColorScheme::Dark,
    ));
    let model = loop {
        if let Update::Workspace(window) = next(&mut updates).await {
            let model = local_projection(&window);
            if model.routes().len() == 1 && model.session_ready(&model.routes()[0].session_id) {
                break model;
            }
        }
    };
    let owner = model.source_token();
    let terminal = model.terminal_token(&model.routes()[0].pane_id).unwrap();
    let opening = Uuid::new_v4();
    let key = MappingKey::new("slow.invalid", 3000).unwrap();
    commands
        .send(Command::new(
            owner,
            CommandKind::ManageForward {
                request_id: opening,
                request: ForwardRequest::open(key.clone(), None, false, None).unwrap(),
            },
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("forward entered real service");
    let stopping = Uuid::new_v4();
    let saving = Uuid::new_v4();
    let host = Uuid::new_v4();
    for kind in [
        CommandKind::ManageForward {
            request_id: stopping,
            request: ForwardRequest::stop(key),
        },
        CommandKind::ManageForward {
            request_id: saving,
            request: ForwardRequest::save(
                MappingKey::new("other.invalid", 3001).unwrap(),
                Some(4174),
                None,
            )
            .unwrap(),
        },
        CommandKind::ManageHost {
            request_id: host,
            request: HostManagementRequest::set_alias("other.invalid", "Build").unwrap(),
        },
        CommandKind::RefreshHostDirectory {
            connection: owner.connection_id(),
        },
        CommandKind::Input {
            route: terminal.clone(),
            bytes: b"printf 'PORTS_%s\\n' 'TERMINAL_ALIVE'\n".to_vec(),
        },
    ] {
        commands.send(Command::new(owner, kind)).await.unwrap();
    }
    let mut saved = false;
    let mut edited = false;
    let mut directory = false;
    let mut passive = false;
    let mut output = Vec::new();
    while !saved
        || !edited
        || !directory
        || !passive
        || !output
            .windows(b"PORTS_TERMINAL_ALIVE".len())
            .any(|part| part == b"PORTS_TERMINAL_ALIVE")
    {
        match next(&mut updates).await {
            Update::Forward(result) if result.outcome() != ForwardOutcome::Pending => {
                assert_ne!(
                    result.request_id(),
                    opening,
                    "slow fixture completed without release"
                );
                assert_ne!(result.request_id(), stopping, "Stop overtook pending Start");
                if result.request_id() == saving {
                    assert_eq!(result.outcome(), ForwardOutcome::Succeeded);
                    saved = true;
                }
            }
            Update::HostManagement(result) if result.request_id() == host => {
                if result.outcome() == HostManagementOutcome::Applied {
                    edited = true;
                }
            }
            Update::HostDirectory(HubUpdate {
                state: HostDirectoryState::Ready(snapshot),
                ..
            }) => {
                directory |= snapshot.directory().hosts.iter().any(|host| {
                    host.target == "other.invalid" && host.alias.as_deref() == Some("Build")
                });
            }
            Update::Forwards(snapshot) if !snapshot.is_loading() => {
                passive |= snapshot
                    .rows(&WindowWorkspace::new(owner.connection_id()))
                    .iter()
                    .any(|row| row.key().target() == "other.invalid");
            }
            Update::Event { event, .. } => {
                if let Some(chunk) = event.output() {
                    if chunk.session_id == terminal.session_id() {
                        output.extend_from_slice(chunk.bytes);
                    }
                }
            }
            _ => {}
        }
    }
    release.notify_one();
    let mut settled = std::collections::HashSet::new();
    while settled.len() < 2 {
        if let Update::Forward(result) = next(&mut updates).await {
            if [opening, stopping].contains(&result.request_id())
                && result.outcome() != ForwardOutcome::Pending
            {
                assert!(result.eligible_open().is_none());
                settled.insert(result.request_id());
            }
        }
    }
    stop.store(true, Ordering::Release);
    drop(commands);
    worker.await.unwrap().unwrap();
    assert!(cleanup.load(Ordering::Acquire));
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn owner_shutdown_joins_pending_forward_carrier_without_replaying_or_saving() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let completed = Arc::new(Notify::new());
    let (runtime, path) =
        crate::appearance::local::tests::fixture_with_connector(Box::new(GatedForward {
            entered: entered.clone(),
            release: release.clone(),
            completed: completed.clone(),
        }));
    let state = runtime.state.clone();
    let (commands, receiver) = mpsc::channel(32);
    let (sender, mut updates) = mpsc::channel(128);
    let stop = Arc::new(AtomicBool::new(false));
    let cleanup = Arc::new(AtomicBool::new(false));
    let mut worker = tokio::spawn(run_service(
        crate::EmbeddedService::from_runtime(runtime),
        receiver,
        sender,
        stop.clone(),
        cleanup.clone(),
        ColorScheme::Dark,
    ));
    let owner = loop {
        if let Update::Forwards(snapshot) = next(&mut updates).await {
            if snapshot.is_available() {
                break snapshot.owner();
            }
        }
    };
    let request = Uuid::new_v4();
    commands
        .send(Command::new(
            owner,
            CommandKind::ManageForward {
                request_id: request,
                request: ForwardRequest::open(
                    MappingKey::new("slow.invalid", 3000).unwrap(),
                    None,
                    true,
                    Some("Must not save".into()),
                )
                .unwrap(),
            },
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    stop.store(true, Ordering::Release);
    drop(commands);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut worker)
            .await
            .is_err(),
        "owned cleanup must retain the admitted service mutation"
    );
    assert!(!cleanup.load(Ordering::Acquire));
    // Existing native carriers join admitted mutations even after the caller
    // closes. Release the owned fixture operation, then require observed cleanup.
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .expect("released carrier cleanup must finish")
        .unwrap()
        .unwrap();
    assert!(cleanup.load(Ordering::Acquire));
    while let Ok(update) = updates.try_recv() {
        if let Update::Forward(result) = update {
            assert!(result.eligible_open().is_none());
        }
    }
    assert!(state.hub.forward_preferences().forwards.is_empty());
    tokio::time::timeout(Duration::from_secs(5), completed.notified())
        .await
        .unwrap();
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}
