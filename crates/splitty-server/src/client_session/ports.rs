//! `ports.changed` (mux-protocol, Listening Ports): the TCP ports processes of
//! this server's sessions listen on, sent to a client right after its
//! `mux.welcome` and to every client whenever the list changes.
//!
//! The scan (`splitty_core::pty::ports`) runs every [`INTERVAL`] while at least
//! one client is connected, on a blocking thread since it reads `/proc`. A
//! client that connects gets the list the others hold, so all clients agree;
//! only when no scan runs yet is one made for it. The list is sent even when
//! it is empty, so a client knows the server reports ports. On systems
//! without the scan nothing is sent.

use std::sync::Arc;
use std::time::Duration;

use splitty_core::protocol::ServerEvent;
use splitty_core::pty::ports::{self, ListeningPort};
use splitty_core::pty::Backend;
use tokio::sync::Mutex;

use super::sender::ClientSender;
use super::ClientState;

/// How often the ports are checked while clients are connected.
pub const INTERVAL: Duration = Duration::from_secs(2);

/// How the ports are found: the scan over the sessions of the server's PTY
/// backend, or a list a test sets. Blocking.
pub type Scan = Arc<dyn Fn() -> Vec<ListeningPort> + Send + Sync>;

/// The listening ports of one server's sessions, as its clients know them.
pub struct PortWatch {
    /// Whether ports are reported at all.
    enabled: bool,
    catalog: Mutex<Catalog>,
    scan: Scan,
    state: Mutex<Watching>,
}

#[derive(Default)]
struct Watching {
    /// Whether the task that checks every [`INTERVAL`] runs.
    running: bool,
    /// The list every client holds.
    sent: Vec<ListeningPort>,
}

impl PortWatch {
    /// The ports of `backend`'s sessions, on systems with the scan.
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self::with_scan(
            ports::SUPPORTED,
            Arc::new(move || ports::listening(backend.as_ref())),
        )
    }

    pub fn with_scan(enabled: bool, scan: Scan) -> Self {
        Self {
            enabled,
            catalog: Mutex::new(Catalog::default()),
            scan,
            state: Mutex::default(),
        }
    }

    pub fn with_catalog(mut self, directory: std::path::PathBuf) -> Self {
        self.catalog = Mutex::new(Catalog::load(directory));
        self
    }

    async fn scan(&self) -> Vec<ListeningPort> {
        let scan = self.scan.clone();
        match tokio::task::spawn_blocking(move || scan()).await {
            Ok(ports) => ports,
            Err(error) => {
                tracing::warn!("the port scan failed: {error}");
                Vec::new()
            }
        }
    }
}

/// Send `sender`, a client that just got its `mux.welcome`, the current list,
/// and keep checking while clients are connected. The client must be
/// registered already, so the check that stops without clients sees it.
pub(super) async fn greet(client_state: &Arc<ClientState>, sender: &ClientSender) {
    let watch = &client_state.ports;
    // Discovery keeps its existing greeting order for older clients.
    if !watch.enabled {
        let _ = sender.send_event(&watch.catalog.lock().await.event()).await;
        return;
    }
    let mut state = watch.state.lock().await;
    if !state.running {
        state.sent = watch.scan().await;
        state.running = true;
        tokio::spawn(check(client_state.clone()));
    }
    let _ = sender
        .send_event(&ServerEvent::ports_changed(state.sent.clone()))
        .await;
    let _ = sender.send_event(&watch.catalog.lock().await.event()).await;
}

/// Check every [`INTERVAL`] until no client is connected, and send every
/// client the list when it changed.
async fn check(client_state: Arc<ClientState>) {
    let watch = &client_state.ports;
    loop {
        tokio::time::sleep(INTERVAL).await;
        // Under the watch's lock: a client that connects meanwhile finds this
        // task running, or starts the next one.
        let mut state = watch.state.lock().await;
        if client_state
            .clients
            .lock()
            .expect("client registry")
            .is_empty()
        {
            state.running = false;
            return;
        }
        let ports = watch.scan().await;
        if ports == state.sent {
            continue;
        }
        state.sent = ports;
        client_state.broadcast_events(&[ServerEvent::ports_changed(state.sent.clone())], None);
    }
}

const CATALOG_FILE: &str = "port-catalog.json";
#[derive(Default)]
struct Catalog {
    revision: u64,
    directory: Option<std::path::PathBuf>,
    ports: Vec<splitty_core::protocol::CatalogPort>,
    error: Option<String>,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    ports: Vec<splitty_core::protocol::CatalogPort>,
}
impl Catalog {
    fn load(directory: std::path::PathBuf) -> Self {
        let mut result = Self {
            directory: Some(directory.clone()),
            ..Self::default()
        };
        let read = || -> Result<Vec<splitty_core::protocol::CatalogPort>, String> {
            use std::io::Read;
            let file = match std::fs::File::open(directory.join(CATALOG_FILE)) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(error) => return Err(error.to_string()),
            };
            let mut bytes = Vec::new();
            file.take(256 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 256 * 1024 {
                return Err("Port catalog is too large".into());
            }
            let mut document: Document =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            document.ports.sort_by_key(|row| row.port);
            if document.version != 1
                || document.ports.len() > 256
                || document.ports.iter().any(|row| !row.is_valid())
                || document
                    .ports
                    .windows(2)
                    .any(|pair| pair[0].port == pair[1].port)
            {
                return Err("Invalid port catalog".into());
            }
            Ok(document.ports)
        };
        match read() {
            Ok(ports) => result.ports = ports,
            Err(error) => result.error = Some(format!("Cannot read {CATALOG_FILE}: {error}")),
        }
        result
    }
    fn event(&self) -> ServerEvent {
        ServerEvent::port_catalog(self.ports.clone(), self.error.clone(), self.revision)
    }
    fn update(&mut self, port: u16, name: Option<String>) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let row = splitty_core::protocol::CatalogPort {
            port,
            name: name.clone().unwrap_or_default(),
        };
        if !row.is_valid() {
            return Err(
                "Port must be 1–65535; name at most 200 characters without control characters"
                    .into(),
            );
        }
        let mut next = self.ports.clone();
        next.retain(|row| row.port != port);
        if name.is_some() {
            next.push(row);
        }
        if next.len() > 256 {
            return Err("At most 256 saved ports are allowed on a host".into());
        }
        next.sort_by_key(|row| row.port);
        if let Some(directory) = &self.directory {
            let bytes = serde_json::to_vec_pretty(&Document {
                version: 1,
                ports: next.clone(),
            })
            .map_err(|e| e.to_string())?;
            splitty_remote::hosts::write_private(directory, CATALOG_FILE, &bytes)
                .map_err(|e| format!("Cannot save port catalog: {e}"))?;
        }
        self.ports = next;
        self.revision += 1;
        Ok(())
    }
}

/// Authenticated source connections manage only this daemon's catalog.
pub(super) async fn mutate(state: &ClientState, sender: &ClientSender, value: &serde_json::Value) {
    let Some(request_id) = value["requestId"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 128)
    else {
        return;
    };
    let mut catalog = state.ports.catalog.lock().await;
    let result = (|| {
        let port = value["port"]
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p != 0)
            .ok_or("Invalid port")?;
        let name = if value["type"] == "ports.save" {
            Some(
                value["name"]
                    .as_str()
                    .ok_or("Missing port name")?
                    .to_owned(),
            )
        } else {
            None
        };
        if value.as_object().is_none_or(|obj| {
            obj.keys()
                .any(|k| !matches!(k.as_str(), "type" | "requestId" | "port" | "name"))
        }) {
            return Err("Unknown catalog field".into());
        }
        catalog.update(port, name)
    })();
    if result.is_ok() {
        state.broadcast_events(&[catalog.event()], None);
    }
    let _ = sender.try_send_json(
        &serde_json::json!({"type":"ports.result", "requestId":request_id, "error":result.err()}),
    );
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    #[test]
    fn catalog_survives_restart_and_validates_without_losing_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut catalog = Catalog::load(dir.path().join("config"));
        catalog.update(3000, Some("Frontend".into())).unwrap();
        catalog.update(8000, Some("API".into())).unwrap();
        assert!(catalog.update(0, Some("bad".into())).is_err());
        assert!(catalog.update(3000, Some("bad\nname".into())).is_err());
        assert!(catalog.update(3000, Some("x".repeat(201))).is_err());
        let mut loaded = Catalog::load(dir.path().join("config"));
        assert_eq!(loaded.ports, catalog.ports);
        loaded.update(3000, Some("Web".into())).unwrap();
        loaded.update(8000, None).unwrap();
        assert_eq!(Catalog::load(dir.path().join("config")).ports, loaded.ports);
        assert_eq!(loaded.ports[0].name, "Web");
    }
    #[test]
    fn failed_write_and_invalid_file_preserve_prior_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        let mut catalog = Catalog::load(path.clone());
        catalog.update(3000, Some("App".into())).unwrap();
        std::fs::rename(&path, dir.path().join("backup")).unwrap();
        std::fs::write(&path, b"blocked").unwrap();
        assert!(catalog.update(3000, Some("Other".into())).is_err());
        assert_eq!(catalog.ports[0].name, "App");
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(dir.path().join("backup"), &path).unwrap();
        std::fs::write(path.join(CATALOG_FILE), b"broken").unwrap();
        let mut broken = Catalog::load(path.clone());
        assert!(broken.error.is_some());
        assert!(broken.update(3000, Some("New".into())).is_err());
        assert_eq!(std::fs::read(path.join(CATALOG_FILE)).unwrap(), b"broken");
    }
}
