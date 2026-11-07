//! App ownership of an isolated service, independently of its client streams.
use crate::connection::Client;
use splitty_server::{control, native::client::Connector, service};
use std::{io, sync::Arc};
use uuid::Uuid;

pub struct EmbeddedService {
    runtime: Option<service::runtime::Runtime>,
    control: Option<control::ControlChannel>,
    connector: Connector,
}

impl EmbeddedService {
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
        let control = control::spawn_service(runtime.state.clone(), path);
        Self {
            runtime: Some(runtime),
            control: Some(control),
            connector: Connector::default(),
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

    /// Close active streams, then await all owned PTYs/jobs. Cancellation of
    /// this wait cannot cancel cleanup or release ownership prematurely.
    pub async fn close(mut self) -> io::Result<()> {
        let runtime = self.runtime.take();
        let control = self.control.take();
        tokio::spawn(async move { shutdown(runtime, control).await })
            .await
            .map_err(io::Error::other)?
    }
}

async fn shutdown(
    runtime: Option<service::runtime::Runtime>,
    control: Option<control::ControlChannel>,
) -> io::Result<()> {
    let result = if let Some(runtime) = runtime {
        for client in runtime
            .state
            .client_state
            .clients
            .lock()
            .expect("service clients")
            .values()
        {
            client.sender.close();
        }
        runtime.close().await
    } else {
        Ok(())
    };
    if let Some(control) = control {
        control.shutdown();
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
        if let Ok(executor) = tokio::runtime::Handle::try_current() {
            executor.spawn(async move {
                let _ = shutdown(runtime, control).await;
            });
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
        let service = EmbeddedService::start();
        // An explicit in-memory profile avoids depending on the user's shell
        // startup files. This does not write settings or touch saved workspaces.
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
        kind(client, "attached").await;
        client.resize(session, 80, 24, true).unwrap();
    }

    async fn output(client: &mut Client, needle: &[u8]) {
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
