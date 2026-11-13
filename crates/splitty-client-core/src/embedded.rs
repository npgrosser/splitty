//! App ownership of an isolated service, independently of its client streams.
use crate::connection::Client;
use splitty_remote::ssh::SshTarget;
use splitty_server::{control, native::client::Connector, service};
use splitty_transport::{budget::Charged, queue, wire};
use std::{io, sync::Arc};
use uuid::Uuid;

/// The embedded service names the app's executable as OpenSSH's askpass
/// program, so that executable is also the helper that hands a question to
/// the service. Call first in `main`: `Some` is the outcome of the helper's
/// run, after which the process ends; `None` is an ordinary start.
pub fn askpass_helper() -> Option<io::Result<()>> {
    (std::env::var_os("SPLITTY_BROWSER_ASKPASS").as_deref() == Some(std::ffi::OsStr::new("1")))
        .then(splitty_remote::askpass::helper)
}

pub struct EmbeddedService {
    runtime: Option<service::runtime::Runtime>,
    control: Option<control::ControlChannel>,
    connector: Connector,
    /// The private directory of an isolated service, removed after it.
    scratch: Option<std::path::PathBuf>,
}

impl EmbeddedService {
    #[cfg(test)]
    pub(crate) fn from_runtime(runtime: service::runtime::Runtime) -> Self {
        Self {
            runtime: Some(runtime),
            control: None,
            connector: connector(),
            scratch: None,
        }
    }

    /// A service for tests that keeps its settings, metadata and control
    /// socket in a new private directory instead of the user's, and whose
    /// hub never reaches a host. Call inside a Tokio runtime, as `start`.
    #[cfg(any(test, feature = "ui-test-support"))]
    #[doc(hidden)]
    pub fn isolated() -> Self {
        let scratch =
            std::env::temp_dir().join(format!("splitty-test-{}", Uuid::new_v4().simple()));
        let config_dir = scratch.join("config");
        std::fs::create_dir_all(&config_dir).expect("isolated service directory");
        let path = scratch.join("control.sock");
        let runtime = service::runtime::build_in(
            "embedded",
            None,
            Arc::new(splitty_core::pty::terminal::LocalTerminals),
            &path,
            service::runtime::Home {
                config_dir,
                ssh_dir: None,
                connector: Box::new(offline::NoHosts),
            },
        );
        let control = control::spawn_at(runtime.state.clone(), path);
        Self {
            runtime: Some(runtime),
            control: Some(control),
            connector: connector(),
            scratch: Some(scratch),
        }
    }

    /// A separate API stream owned by this service. It never consumes session
    /// frames. Bind returned settings to the workspace's session identity.
    pub async fn local_service_client(
        &self,
        connection_id: Uuid,
        scheme: crate::appearance::ColorScheme,
    ) -> io::Result<crate::local_service::LocalServiceClient> {
        let client = self.api_stream().await?;
        Ok(crate::local_service::LocalServiceClient::new(
            client,
            connection_id,
            scheme,
        ))
    }

    pub(crate) async fn api_stream(&self) -> io::Result<crate::api_stream::ApiStream> {
        self.api_stream_with_context(self.connector.context()).await
    }

    pub(crate) async fn api_stream_with_context(
        &self,
        context: splitty_transport::queue::Context,
    ) -> io::Result<crate::api_stream::ApiStream> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "service closed"))?;
        self.connector
            .embedded_admitted(
                runtime.state.clone(),
                service::catalog::Role::Main,
                self.connector.admit_context(context)?,
            )
            .await
            .map(Into::into)
    }

    /// Call inside the Tokio runtime that remains alive until close completes.
    /// No daemon discovery, workspace restoration or network listener is used.
    pub fn start() -> Self {
        let path = control::socket_path()
            .with_file_name(format!("spike-{}.sock", Uuid::new_v4().simple()));
        let runtime = service::runtime::build(
            "embedded",
            None,
            Arc::new(splitty_core::pty::terminal::LocalTerminals),
            &path,
        );
        let control = control::spawn_at(runtime.state.clone(), path);
        Self {
            runtime: Some(runtime),
            control: Some(control),
            connector: connector(),
            scratch: None,
        }
    }

    pub async fn connect(&self) -> io::Result<Client> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "service closed"))?;
        self.connector
            .embedded(runtime.state.clone(), service::catalog::Role::Main)
            .await
            .map(Client::from_service)
    }

    /// Open one explicit host stream through this service's existing hub.
    pub async fn host_stream(
        &self,
        target: SshTarget,
        update: bool,
        context: queue::Context,
        progress: impl FnMut(Charged<wire::Progress>),
    ) -> io::Result<Client> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "service closed"))?;
        self.connector
            .host_admitted(
                runtime.state.clone(),
                target,
                update,
                self.connector.admit_context(context)?,
                progress,
            )
            .await
            .map(Client::from_service)
    }

    /// Close active streams, then await all owned PTYs/jobs. Cancellation of
    /// this wait cannot cancel cleanup or release ownership prematurely.
    pub async fn close(mut self) -> io::Result<()> {
        let runtime = self.runtime.take();
        let control = self.control.take();
        let scratch = self.scratch.take();
        tokio::spawn(async move { shutdown(runtime, control, scratch).await })
            .await
            .map_err(io::Error::other)?
    }
}

/// A service's streams share the process-wide admission and byte budget. The
/// unit tests of this crate run many services concurrently in one process, so
/// there each service gets its own limits and cannot exhaust another test's.
fn connector() -> Connector {
    #[cfg(test)]
    {
        Connector::isolated()
    }
    #[cfg(not(test))]
    {
        Connector::default()
    }
}

async fn shutdown(
    runtime: Option<service::runtime::Runtime>,
    control: Option<control::ControlChannel>,
    scratch: Option<std::path::PathBuf>,
) -> io::Result<()> {
    let result = if let Some(runtime) = runtime {
        runtime.close().await
    } else {
        Ok(())
    };
    if let Some(control) = control {
        control.shutdown();
    }
    if let Some(scratch) = scratch {
        let _ = std::fs::remove_dir_all(scratch);
    }
    result
}

impl Drop for EmbeddedService {
    fn drop(&mut self) {
        if self.runtime.is_none() {
            return;
        }
        let runtime = self.runtime.take();
        let control = self.control.take();
        let scratch = self.scratch.take();
        if let Ok(executor) = tokio::runtime::Handle::try_current() {
            executor.spawn(async move {
                let _ = shutdown(runtime, control, scratch).await;
            });
        } else if let Some(scratch) = scratch {
            let _ = std::fs::remove_dir_all(scratch);
        }
    }
}

#[cfg(any(test, feature = "ui-test-support"))]
mod offline {
    //! A hub connector for isolated test services: every host is unreachable.
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

    type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    pub(super) struct NoHosts;

    fn unreachable<'a, T: Send + 'a>(target: &SshTarget) -> BoxFuture<'a, Result<T, HubError>> {
        let host = target.as_str().to_owned();
        Box::pin(async move { Err(HubError::NoDaemon { host }) })
    }

    impl Connector for NoHosts {
        fn prepare<'a>(
            &'a self,
            target: &'a SshTarget,
            _: Box<dyn FnOnce(&str) + Send + 'a>,
            _: &'a queue::Context,
        ) -> BoxFuture<'a, Result<Prepared, HubError>> {
            unreachable(target)
        }
        fn handshake<'a>(
            &'a self,
            target: &'a SshTarget,
            _: &'a Prepared,
            _: &'a queue::Context,
        ) -> BoxFuture<'a, Result<Charged<Handshake>, HubError>> {
            unreachable(target)
        }
        fn stop_daemon<'a>(
            &'a self,
            target: &'a SshTarget,
            _: &'a Server,
            _: &'a RunningDaemon,
            _: &'a queue::Context,
        ) -> BoxFuture<'a, Result<(), HubError>> {
            unreachable(target)
        }
        fn migrate_legacy<'a>(
            &'a self,
            target: &'a SshTarget,
            _: &'a Server,
            _: &'a RunningDaemon,
            _: &'a queue::Context,
        ) -> BoxFuture<'a, Result<(), HubError>> {
            unreachable(target)
        }
        fn bridge<'a>(
            &'a self,
            target: &'a SshTarget,
            _: &'a Server,
            _: &'a RunningDaemon,
            _: (admission::Carrier, admission::Opening),
            _: &'a queue::Context,
        ) -> BoxFuture<'a, Result<NativeBridge, HubError>> {
            unreachable(target)
        }
        fn forward<'a>(
            &'a self,
            target: &'a SshTarget,
            _: PortReservation,
            _: u16,
        ) -> BoxFuture<'a, Result<Box<dyn Forwarding>, HubError>> {
            unreachable(target)
        }
        fn reserve_share(&self, _: u16) -> BoxFuture<'_, Result<Option<Share>, HubError>> {
            Box::pin(async { Ok(None) })
        }
        fn reverse<'a>(
            &'a self,
            target: &'a SshTarget,
            _: u16,
            _: u16,
            _: Option<&'a Share>,
        ) -> BoxFuture<'a, Result<Box<dyn Forwarding>, HubError>> {
            unreachable(target)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnnotationDraft, Event, ScreenIds};
    use serde_json::json;
    use splitty_core::pty::Backend as _;
    use std::time::Duration;

    fn service() -> EmbeddedService {
        let service = EmbeddedService::isolated();
        // An explicit in-memory profile avoids depending on the user's shell
        // startup files. This profile override is memory-only; the production
        // constructor above still loads configuration and may persist migrations.
        *service
            .runtime
            .as_ref()
            .unwrap()
            .state
            .shell_profiles
            .write()
            .unwrap() = serde_json::from_value(json!({"defaultProfileId":"test-sh", "profiles":[{
            "id":"test-sh", "name":"Test shell", "command":"/bin/sh", "args":[],
            "enabled":true, "available":true, "target":"local", "source":"custom"
        }]}))
        .unwrap();
        service
    }

    async fn event(client: &mut Client, predicate: impl Fn(&Event) -> bool) -> Event {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = client.next_event().await.unwrap().expect("live stream");
                if predicate(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("expected native event")
    }

    async fn kind(client: &mut Client, name: &str) -> Event {
        event(client, |event| {
            event.json().is_some_and(|value| value["type"] == name)
        })
        .await
    }

    async fn screen(client: &mut Client) -> (ScreenIds, String) {
        kind(client, "mux.welcome").await;
        let ids = client.create_screen("Native adapter test").unwrap();
        let spawned = kind(client, "mux.pane_spawned").await;
        let value = spawned.json().unwrap();
        assert_eq!(value["paneId"], ids.pane_id);
        (ids, value["sessionId"].as_str().unwrap().into())
    }

    async fn attach(client: &mut Client, session: &str) {
        client.activity(true).unwrap();
        client.attach(session).unwrap();
        assert_eq!(
            client.input(session, "early").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            client.input_bytes(session, b"\x80").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        kind(client, "attached").await;
        client.resize(session, 80, 24, true).unwrap();
    }

    async fn output(client: &mut Client, needle: &[u8]) -> Vec<u8> {
        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = client.next_event().await.unwrap().expect("live stream");
                if let Some(output) = event.output() {
                    assert!(!output.replay);
                    received.extend_from_slice(output.bytes);
                    if received
                        .windows(needle.len())
                        .any(|window| window == needle)
                    {
                        break;
                    }
                }
            }
        })
        .await
        .expect("terminal output");
        received
    }

    /// Real PTYs through the real native service, within this process: the
    /// required evidence is session behavior and ownership, not binary startup.
    #[tokio::test]
    async fn real_service_input_resize_reconnect_documents_and_independent_cleanup() {
        if !splitty_core::pty::testing::pty_available() {
            return;
        }
        let other_service = service();
        let service = service();
        let mut client = service.connect().await.unwrap();
        let mut other = other_service.connect().await.unwrap();
        let (ids, session) = screen(&mut client).await;
        let (_, other_session) = screen(&mut other).await;
        let pty = service.runtime.as_ref().unwrap().pty.get(&session).unwrap();
        let other_pty = other_service
            .runtime
            .as_ref()
            .unwrap()
            .pty
            .get(&other_session)
            .unwrap();
        let pid = pty.shell_pid();
        assert!(pid.is_some());
        attach(&mut client, &session).await;
        client
            .input(&session, "printf 'CORE_'; printf 'MARKER\\n'\n")
            .unwrap();
        output(&mut client, b"CORE_MARKER").await;
        // The raw reader makes corruption by UTF-8 conversion observable. A
        // whole bracketed paste follows the binary report on the same stream.
        client.input(&session, "stty raw -echo; printf 'RAW_'; printf READY; dd bs=1 count=19 2>/dev/null | od -An -tx1; stty sane; printf 'RAW_DONE\\n'\n").unwrap();
        output(&mut client, b"RAW_READY").await;
        client.input_bytes(&session, b"\x00\x80\xff\x1b").unwrap();
        client
            .input_bytes(&session, b"\x1b[200~abc\x1b[201~")
            .unwrap();
        let raw = output(&mut client, b"RAW_DONE").await;
        let raw = std::str::from_utf8(&raw)
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            raw.contains("00 80 ff 1b 1b 5b 32 30 30 7e 61 62 63 1b 5b 32 30 31 7e"),
            "{raw:?}"
        );
        // A welcome without the extension must revoke it, including after
        // this connection has previously used binary input successfully.
        service
            .runtime
            .as_ref()
            .unwrap()
            .state
            .client_state
            .broadcast_events(
                &[splitty_core::protocol::ServerEvent::mux_welcome(
                    "legacy-peer".into(),
                    vec![],
                    None,
                )],
                None,
            );
        kind(&mut client, "mux.welcome").await;
        assert_eq!(
            client.input_bytes(&session, b"\x80").unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        client
            .input_bytes(&session, b"printf 'LEGACY_'; printf 'TEXT_OK\\n'\n")
            .unwrap();
        output(&mut client, b"LEGACY_TEXT_OK").await;
        client.resize(&session, 111, 37, false).unwrap();
        event(&mut client, |event| {
            event.json().is_some_and(|value| {
                value["type"] == "size" && value["cols"] == 111 && value["rows"] == 37
            })
        })
        .await;
        client.input(&session, "stty size\n").unwrap();
        output(&mut client, b"37 111").await;

        let text = "# Native document\n\nGrüße 世界 — selected passage.\n";
        let pane = client
            .open_markdown(&ids.screen_id, &ids.pane_id, text, "Native probe")
            .unwrap();
        event(&mut client, |event| {
            event.json().is_some_and(|value| {
                value["type"] == "mux.screen_snapshot"
                    && value["screen"]["panes"][&pane]["viewer"].is_object()
            })
        })
        .await;
        let mut read = client.read_document(&pane).unwrap();
        let document = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = client.next_event().await.unwrap().unwrap();
                if let Some(document) = read.accept(&client, &event).unwrap() {
                    break document;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(document.bytes(), text.as_bytes());
        assert_eq!(document.connection_id, client.connection_id());
        let annotation = client
            .put_annotation(
                &ids.screen_id,
                &pane,
                AnnotationDraft {
                    quote: "Grüße 世界".into(),
                    prefix: "\n\n".into(),
                    suffix: " — selected passage.".into(),
                    start_line: 3,
                    end_line: 3,
                    body: "Unicode survives the native path".into(),
                },
            )
            .unwrap();
        event(&mut client, |event| {
            event.json().is_some_and(|value| {
                value["type"] == "mux.screen_snapshot"
                    && value["screen"]["panes"][&pane]["viewer"]["annotations"][0]["id"]
                        == annotation
            })
        })
        .await;

        let first_connection = client.connection_id();
        client.finish().await.unwrap();
        assert!(
            !*pty.done().borrow(),
            "stream closure must leave the session running"
        );
        let mut reconnected = service.connect().await.unwrap();
        assert_ne!(first_connection, reconnected.connection_id());
        let welcome = kind(&mut reconnected, "mux.welcome").await;
        assert!(
            read.accept(&reconnected, &welcome).is_err(),
            "pending reads cannot cross connection identities"
        );
        reconnected.resync(&ids.screen_id).unwrap();
        let snapshot = kind(&mut reconnected, "mux.screen_snapshot").await;
        let snapshot = snapshot.json().unwrap();
        assert_eq!(
            snapshot["screen"]["panes"][&ids.pane_id]["sessionId"],
            session
        );
        assert_eq!(
            snapshot["screen"]["panes"][&pane]["viewer"]["annotations"][0]["number"],
            1
        );
        assert_eq!(
            snapshot["screen"]["panes"][&pane]["viewer"]["annotations"][0]["target"]["quote"],
            "Grüße 世界"
        );
        assert_eq!(service.runtime.as_ref().unwrap().pty.sessions().len(), 1);
        assert_eq!(pty.shell_pid(), pid, "reconnect never respawns a process");
        reconnected.attach(&session).unwrap();
        let mut replayed = Vec::new();
        loop {
            let event = reconnected.next_event().await.unwrap().unwrap();
            if let Some(output) = event.output() {
                assert!(output.replay);
                replayed.extend_from_slice(output.bytes);
            }
            if event
                .json()
                .is_some_and(|value| value["type"] == "attached")
            {
                break;
            }
        }
        assert!(replayed
            .windows(b"CORE_MARKER".len())
            .any(|bytes| bytes == b"CORE_MARKER"));
        reconnected
            .input(&session, "printf 'AFTER_'; printf 'RECONNECT\\n'\n")
            .unwrap();
        output(&mut reconnected, b"AFTER_RECONNECT").await;
        drop((document, read));
        reconnected.finish().await.unwrap();
        service.close().await.unwrap();
        assert!(
            *pty.done().borrow(),
            "owned service close awaits terminal exit"
        );
        assert!(
            !*other_pty.done().borrow(),
            "another service remains independent"
        );
        other.finish().await.unwrap();
        other_service.close().await.unwrap();
        assert!(*other_pty.done().borrow());
    }

    #[tokio::test]
    async fn workspace_catalog_uses_the_ordered_native_stream_around_live_session_output() {
        if !splitty_core::pty::testing::pty_available() {
            return;
        }
        let service = service();
        let mut client = service.connect().await.unwrap();
        let welcome = kind(&mut client, "mux.welcome").await;
        assert!(welcome.json().unwrap()["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == "workspace-catalog-v1"));

        client.request_workspace().unwrap();
        let empty = kind(&mut client, "mux.workspace_snapshot").await;
        assert_eq!(empty.json().unwrap()["screens"], json!([]));
        assert!(empty.json().unwrap()["activeScreenId"].is_null());

        let ids = ScreenIds::new();
        client.create_screen_with_ids(&ids, "First").unwrap();
        client.request_workspace().unwrap();
        let spawned = kind(&mut client, "mux.pane_spawned").await;
        assert_eq!(spawned.json().unwrap()["paneId"], ids.pane_id);
        let session = spawned.json().unwrap()["sessionId"]
            .as_str()
            .unwrap()
            .to_owned();
        let created = kind(&mut client, "mux.workspace_snapshot").await;
        assert_eq!(
            created.json().unwrap()["screens"],
            json!([{"id":ids.screen_id,"name":"First"}])
        );
        assert_eq!(created.json().unwrap()["activeScreenId"], ids.screen_id);

        attach(&mut client, &session).await;
        client
            .input(&session, "printf 'CATALOG_BEFORE_OK\\n'\n")
            .unwrap();
        output(&mut client, b"CATALOG_BEFORE_OK").await;
        client.rename_screen(&ids.screen_id, "Renamed").unwrap();
        client.request_workspace().unwrap();
        let renamed = kind(&mut client, "mux.workspace_snapshot").await;
        assert_eq!(renamed.json().unwrap()["screens"][0]["name"], "Renamed");
        client
            .input(&session, "printf 'CATALOG_AFTER_OK\\n'\n")
            .unwrap();
        output(&mut client, b"CATALOG_AFTER_OK").await;

        client.close_screen(&ids.screen_id).unwrap();
        client.request_workspace().unwrap();
        let closed = kind(&mut client, "mux.workspace_snapshot").await;
        assert_eq!(closed.json().unwrap()["screens"], json!([]));
        assert!(closed.json().unwrap()["activeScreenId"].is_null());
        // Two actual clients submit default creations without sharing a name
        // reservation. The server's application order assigns unique gaps.
        let named = ScreenIds::new();
        client.create_screen_with_ids(&named, "Screen 2").unwrap();
        client.request_workspace().unwrap();
        kind(&mut client, "mux.workspace_snapshot").await;
        let mut other = service.connect().await.unwrap();
        kind(&mut other, "mux.welcome").await;
        let first = ScreenIds::new();
        let second = ScreenIds::new();
        client.create_default_screen_with_ids(&first).unwrap();
        other.create_default_screen_with_ids(&second).unwrap();
        let names = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                other.request_workspace().unwrap();
                let event = kind(&mut other, "mux.workspace_snapshot").await;
                let entries = event.json().unwrap()["screens"].as_array().unwrap();
                if entries.len() == 3 {
                    assert_eq!(
                        entries
                            .iter()
                            .find(|entry| entry["id"] == named.screen_id)
                            .unwrap()["name"],
                        "Screen 2"
                    );
                    break entries
                        .iter()
                        .filter(|entry| {
                            entry["id"] == first.screen_id || entry["id"] == second.screen_id
                        })
                        .map(|entry| entry["name"].as_str().unwrap().to_owned())
                        .collect::<std::collections::BTreeSet<_>>();
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            names,
            ["Screen 1".into(), "Screen 3".into()].into_iter().collect()
        );
        other.finish().await.unwrap();
        client.finish().await.unwrap();
        service.close().await.unwrap();
    }

    #[tokio::test]
    async fn retained_events_and_pending_reads_share_the_native_budget() {
        let service = service();
        let mut client = service.connect().await.unwrap();
        let budget = client.sender().context().budget;
        let welcome = kind(&mut client, "mux.welcome").await;
        assert_eq!(welcome.connection_id, client.connection_id());
        let held = budget.used();
        assert!(held > 0);
        // Saturation is explicit and does not allocate a pending read or DOM.
        let available = splitty_transport::wire::CONNECTION_BYTES
            - splitty_transport::wire::CONNECTION_CONTROL_BYTES
            - budget.data.used();
        let saturation = budget.reserve(available, false).unwrap();
        assert!(client.read_document("a-pane").is_err());
        assert!(client.create_screen("cannot allocate").is_err());
        drop(saturation);
        drop(welcome);
        client.finish().await.unwrap();
        service.close().await.unwrap();
        assert_eq!(budget.used(), 0);
    }
}
