//! Routes protocol requests to the PTY backend and manages per-connection state.
//!
//! `Dispatcher` holds the backend reference, the optional notification engine
//! and the arbiter of PTY sizes. `Connection` represents a single client: it
//! handles create, attach, detach, input, resize, close, save_image, upload,
//! activity, idle and hide messages, forwards subscriptions to session output and size to
//! the client, and polls process info for title updates.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

use super::messages::*;
use super::notify::NotificationEngine;
use super::paste::{self, PasteError};
use super::sizing::{self, ClientKey, PtySize, SizeArbiter, SizeConfig, SizeState};
use super::upload::{self, Upload, UploadError};
use super::{Sender, ServerEvent};
use crate::mux::effect_runner::EnvContext;
use crate::pty::{Backend, CreateSessionOpts, SessionHandle, Subscription};

/// Largest binary frame used to replay buffered output.
const REPLAY_CHUNK: usize = 32 * 1024;

/// How long a stream waits, once a session's output has ended or its process
/// has exited, for the other of the two before it sends `closed`. EOF often
/// arrives before the exit status is known, and a process that has exited can
/// leave output still on its way.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// Callback type that resolves a pane ID to its mux session ID.
pub type PaneIdResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Resolves retained job identities as well as currently visible panes, so a stale
/// client's fallback cannot recreate a closed/failed/interrupted managed pane.
pub type JobPaneResolver =
    Arc<dyn Fn(&str) -> Option<(String, crate::mux::jobs::JobState)> + Send + Sync>;

/// Dispatcher routes protocol requests to the PTY backend.
pub struct Dispatcher {
    backend: Arc<dyn Backend>,
    /// Notification engine. `None` if notifications are not enabled.
    pub notify: Option<Arc<Mutex<NotificationEngine>>>,
    /// Callback to resolve a pane ID to its mux session ID.
    /// Set by the WS layer when mux is enabled.
    pub session_id_for_pane: Option<PaneIdResolver>,
    job_for_pane: RwLock<Option<JobPaneResolver>>,
    /// Environment context injected into spawned sessions (server addr, token).
    pub env: EnvContext,
    /// Where `save_image` stores pasted images: `splitty/paste/` of the
    /// user's cache directory, `None` without one. Tests point it elsewhere.
    pub paste_dir: Option<PathBuf>,
    /// The PTY size of sessions, from what the connections report.
    sizes: SizeArbiter,
    next_client: AtomicU64,
    /// Set once the tick that finds idle connections runs.
    size_timer: OnceLock<()>,
}

impl Dispatcher {
    /// Create a new dispatcher for the given backend.
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        let resizing = Arc::clone(&backend);
        let resize = move |id: &str, size: PtySize| match resizing.get(id) {
            Some(session) => session.resize(size.cols, size.rows),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "session not found",
            )),
        };
        Self {
            backend,
            notify: None,
            session_id_for_pane: None,
            job_for_pane: RwLock::new(None),
            env: EnvContext::default(),
            paste_dir: paste::default_dir(),
            sizes: SizeArbiter::new(SizeConfig::default(), Box::new(resize)),
            next_client: AtomicU64::new(1),
            size_timer: OnceLock::new(),
        }
    }

    /// Apply the size policy and idle time of the settings to every session.
    pub fn configure_sizes(&self, config: SizeConfig) {
        self.sizes.configure(config, Instant::now());
    }

    pub fn size_config(&self) -> SizeConfig {
        self.sizes.config()
    }

    /// Return a reference to the underlying backend.
    pub fn backend(&self) -> &Arc<dyn Backend> {
        &self.backend
    }

    pub fn set_job_resolver(&self, resolver: JobPaneResolver) {
        *self.job_for_pane.write().expect("job resolver lock") = Some(resolver);
    }

    /// Create a new [`Connection`] that sends responses via the given [`Sender`].
    pub fn new_connection(self: &Arc<Self>, sender: Arc<dyn Sender>) -> Connection {
        self.start_size_timer();
        Connection {
            dispatcher: Arc::clone(self),
            sender,
            streams: Mutex::new(HashMap::new()),
            uploads: Mutex::new(HashMap::new()),
            mux_managed: false,
            client: ClientKey(self.next_client.fetch_add(1, Ordering::Relaxed)),
        }
    }

    /// Look for idle connections every [`sizing::TICK`] while the dispatcher
    /// lives; started with the first connection on a Tokio runtime.
    fn start_size_timer(self: &Arc<Self>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self.size_timer.set(()).is_err() {
            return;
        }
        let dispatcher = Arc::downgrade(self);
        runtime.spawn(async move {
            let mut interval = tokio::time::interval(sizing::TICK);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(dispatcher) = dispatcher.upgrade() else {
                    break;
                };
                dispatcher.sizes.tick(Instant::now());
            }
        });
    }
}

/// Tracks a cancel token and join handle for a session's output stream.
struct StreamEntry {
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

impl StreamEntry {
    /// Stop the stream and wait until its task has ended. A cancelled task
    /// may still be forwarding a chunk; awaiting it guarantees that nothing
    /// from it reaches the client after whatever the caller sends next.
    async fn stop(self) {
        self.cancel.cancel();
        self.handle.abort();
        let _ = self.handle.await;
    }
}

/// Connection represents a single client connection's protocol state.
pub struct Connection {
    dispatcher: Arc<Dispatcher>,
    sender: Arc<dyn Sender>,
    streams: Mutex<HashMap<String, StreamEntry>>,
    /// The connection's uploads under way, by request id. One that is dropped
    /// removes what it wrote.
    uploads: Mutex<HashMap<String, Upload>>,
    /// When true, PTY lifecycle is handled by the mux layer.
    /// `close()` will only cancel output streams but NOT kill PTY sessions.
    pub mux_managed: bool,
    /// This connection, for the arbiter of PTY sizes.
    client: ClientKey,
}

impl Connection {
    /// Sessions actively streamed by this connection, for request routing.
    pub fn attached_sessions(&self) -> std::collections::HashSet<String> {
        self.streams
            .lock()
            .expect("session streams")
            .keys()
            .cloned()
            .collect()
    }

    /// Route a request to the appropriate handler.
    pub async fn handle(&self, req: &Request) {
        match req.msg_type.as_str() {
            MSG_CREATE => self.handle_create(req).await,
            MSG_ATTACH => self.handle_attach(req).await,
            MSG_DETACH => self.handle_detach(req).await,
            MSG_INPUT => self.handle_input(req).await,
            MSG_RESIZE => self.handle_resize(req),
            MSG_CLOSE => self.handle_close(req).await,
            MSG_SAVE_IMAGE => self.handle_save_image(req).await,
            MSG_UPLOAD => self.handle_upload(req).await,
            MSG_ACTIVITY => {
                self.dispatcher
                    .sizes
                    .activity(self.client, req.takeover, Instant::now())
            }
            MSG_IDLE => self.dispatcher.sizes.idle(self.client, Instant::now()),
            MSG_HIDE => self
                .dispatcher
                .sizes
                .withdraw(&req.id, self.client, Instant::now()),
            _ => {
                let _ = self.sender.send(&ServerEvent::error(
                    None,
                    format!("unknown message type: {}", req.msg_type),
                ));
            }
        }
    }

    /// Clean up on disconnect: cancel all output streams and optionally close
    /// sessions; the connection's sizes count no more.
    pub fn close(&self) {
        self.dispatcher
            .sizes
            .disconnect(self.client, Instant::now());
        // Unfinished uploads remove their directories as they are dropped.
        self.uploads.lock().expect("uploads").clear();
        let streams = self.streams.lock().unwrap();
        for (id, entry) in streams.iter() {
            entry.cancel.cancel();
            entry.handle.abort();
            if let Some(ref notify) = self.dispatcher.notify {
                let engine = notify.lock().unwrap();
                engine.untrack_session(id);
            }
            if !self.mux_managed {
                let _ = self.dispatcher.backend.close(id);
            }
        }
    }

    async fn handle_create(&self, req: &Request) {
        let managed = self
            .dispatcher
            .job_for_pane
            .read()
            .expect("job resolver lock")
            .as_ref()
            .and_then(|resolve| resolve(&req.pane_id));
        let existing_job = if let Some((session_id, state)) = managed {
            match self.dispatcher.backend.get(&session_id) {
                Some(session) if session.is_managed() => Some(session),
                _ => {
                    let _ = self.sender.send(&ServerEvent::error(
                        Some(session_id),
                        format!("managed job is {state:?}; no replacement shell will be started"),
                    ));
                    return;
                }
            }
        } else {
            None
        };
        // Resolve working directory
        let mut dir = req.dir.clone();
        if !dir.is_empty() && !req.root.is_empty() && !PathBuf::from(&dir).is_absolute() {
            dir = PathBuf::from(&req.root)
                .join(&dir)
                .to_string_lossy()
                .into_owned();
        }

        // Build extra env vars
        let mut extra_env = Vec::new();
        if !req.pane_id.is_empty() {
            extra_env.push(format!("SPLITTY_PANE_ID={}", req.pane_id));
        }
        if !req.screen_id.is_empty() {
            extra_env.push(format!("SPLITTY_SCREEN_ID={}", req.screen_id));
        }
        // See EnvContext: the control socket path replaces the former
        // SPLITTY_SERVER + SPLITTY_TOKEN pair (SR-05).
        if !self.dispatcher.env.control_socket.is_empty() {
            extra_env.push(format!(
                "SPLITTY_SOCK={}",
                self.dispatcher.env.control_socket
            ));
        }

        // Resolve mux session ID for this pane
        let mut session_id = String::new();
        if !req.pane_id.is_empty() {
            if let Some(ref resolver) = self.dispatcher.session_id_for_pane {
                if let Some(id) = resolver(&req.pane_id) {
                    session_id = id;
                } else {
                    warn!(
                        pane_id = %req.pane_id,
                        "SessionIDForPane returned None -- mux pane not found"
                    );
                }
            }
        }

        let opts = CreateSessionOpts {
            id: session_id,
            shell_profile_id: req.shell_profile_id.clone(),
            shell: req.shell.clone(),
            shell_args: vec![],
            cols: req.cols,
            rows: req.rows,
            dir,
            command: req.command.clone(),
            restore_mode: req.restore_mode.clone(),
            extra_env,
        };

        let result = match existing_job {
            Some(session) => Ok(session),
            None => self.dispatcher.backend.create(opts).await,
        };
        let session: Arc<dyn SessionHandle> = match result {
            Ok(s) => s,
            Err(e) => {
                error!(err = %e, "create session failed");
                let _ = self.sender.send(&ServerEvent::error(
                    None,
                    format!("failed to create session: {e}"),
                ));
                return;
            }
        };

        let sid = session.session_id().to_string();
        let cancel = CancellationToken::new();
        // A get-or-create may return a session this connection already
        // streams; its old stream must be gone before the new snapshot.
        self.stop_stream(&sid).await;
        // The session's reader runs from spawn, so the shell may already have
        // printed: the snapshot carries that, the receiver everything later.
        let subscription = session.subscribe();
        // The creating connection shows the session at the size it asked for.
        let mut sizes = self.dispatcher.sizes.subscribe(&sid);
        if req.cols > 0 && req.rows > 0 {
            let size = PtySize {
                cols: req.cols,
                rows: req.rows,
            };
            let reported =
                self.dispatcher
                    .sizes
                    .report(&sid, self.client, size, false, Instant::now());
            if let Err(error) = reported.result {
                warn!(session = %sid, %error, "PTY resize failed");
            }
        }
        let size = *sizes.borrow_and_update();

        // Track in notification engine
        if let Some(ref notify) = self.dispatcher.notify {
            let engine = notify.lock().unwrap();
            engine.track_session(&sid, &req.command);
        }

        // Send created response
        let home_dir = dirs::home_dir().map(|p| p.to_string_lossy().into_owned());

        let _ = self.sender.send(&ServerEvent::session_created_with_profile(
            sid.clone(),
            home_dir,
            Some("unknown".to_string()),
            session.shell_profile_id().map(str::to_string),
        ));
        if let Some(size) = size {
            let _ = self.sender.send(&size_event(&sid, size));
        }

        self.start_stream(&session, subscription, cancel.clone());
        self.watch_size(&session, sizes, cancel.clone());

        // Spawn proc-info poller (1s interval for /proc data)
        let notify_clone = self.dispatcher.notify.clone();
        let sender_clone = Arc::clone(&self.sender);
        let session_clone = Arc::clone(&session);
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            watch_title(session_clone, sender_clone, cancel_clone, notify_clone).await;
        });

        // Spawn event-driven OSC 0/2 title watcher (for tab titles)
        {
            let sender_clone = Arc::clone(&self.sender);
            let session_clone = Arc::clone(&session);
            let cancel_clone = cancel.clone();
            tokio::spawn(async move {
                watch_osc_title(session_clone, sender_clone, cancel_clone).await;
            });
        }

        // Spawn semantic event watcher (OSC 133)
        {
            let notify_clone = self.dispatcher.notify.clone();
            let sender_clone = Arc::clone(&self.sender);
            let session_clone = Arc::clone(&session);
            let cancel_clone = cancel.clone();
            tokio::spawn(async move {
                watch_semantic(session_clone, sender_clone, cancel_clone, notify_clone).await;
            });
        }

        // Spawn bell watcher
        {
            let sender_clone = Arc::clone(&self.sender);
            let session_clone = Arc::clone(&session);
            let cancel_clone = cancel.clone();
            tokio::spawn(async move {
                watch_bell(session_clone, sender_clone, cancel_clone).await;
            });
        }
    }

    async fn handle_attach(&self, req: &Request) {
        let session = match self.dispatcher.backend.get(&req.id) {
            Some(s) => s,
            None => {
                let _ = self.sender.send(&ServerEvent::error(
                    Some(req.id.clone()),
                    "session not found",
                ));
                return;
            }
        };

        let sid = session.session_id().to_string();
        let cancel = CancellationToken::new();
        // Re-attach of a session this connection already streams (e.g. a pane
        // remounted while its first attach was still in flight): the old
        // stream is stopped first, so none of its output follows this replay.
        self.stop_stream(&sid).await;
        let mut subscription = session.subscribe();

        // The size first, so the client's terminal has it before the replay.
        let mut sizes = self.dispatcher.sizes.subscribe(&sid);
        let size = *sizes.borrow_and_update();
        if let Some(size) = size {
            let _ = self.sender.send(&size_event(&sid, size));
        }

        // Replay what was buffered; live output waits in the receiver until
        // the stream starts after `attached`.
        self.send_snapshot(&sid, std::mem::take(&mut subscription.snapshot));

        // Track in notification engine
        if let Some(ref notify) = self.dispatcher.notify {
            let engine = notify.lock().unwrap();
            engine.track_session(&sid, "");
        }

        let home_dir = dirs::home_dir().map(|p| p.to_string_lossy().into_owned());

        // Include semantic state snapshot for reconnection
        let sem = session.semantic_state();
        let _ = self.sender.send(&ServerEvent::session_attached(
            sid.clone(),
            home_dir,
            Some(sem.phase.as_str().to_string()),
            sem.last_exit_code,
            Some(sem.command_count),
        ));

        self.start_stream(&session, subscription, cancel.clone());
        self.watch_size(&session, sizes, cancel.clone());

        // Spawn proc-info poller
        let notify_clone = self.dispatcher.notify.clone();
        let sender_clone = Arc::clone(&self.sender);
        let session_clone = Arc::clone(&session);
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            watch_title(session_clone, sender_clone, cancel_clone, notify_clone).await;
        });

        // Spawn event-driven OSC 0/2 title watcher (for tab titles)
        {
            let sender_clone = Arc::clone(&self.sender);
            let session_clone = Arc::clone(&session);
            let cancel_clone = cancel.clone();
            tokio::spawn(async move {
                watch_osc_title(session_clone, sender_clone, cancel_clone).await;
            });
        }

        // Spawn semantic event watcher (OSC 133)
        {
            let notify_clone = self.dispatcher.notify.clone();
            let sender_clone = Arc::clone(&self.sender);
            let session_clone = Arc::clone(&session);
            let cancel_clone = cancel.clone();
            tokio::spawn(async move {
                watch_semantic(session_clone, sender_clone, cancel_clone, notify_clone).await;
            });
        }

        // Spawn bell watcher
        {
            let sender_clone = Arc::clone(&self.sender);
            let session_clone = Arc::clone(&session);
            let cancel_clone = cancel.clone();
            tokio::spawn(async move {
                watch_bell(session_clone, sender_clone, cancel_clone).await;
            });
        }
    }

    async fn handle_detach(&self, req: &Request) {
        self.stop_stream(&req.id).await;
        self.dispatcher
            .sizes
            .withdraw(&req.id, self.client, Instant::now());
    }

    /// Stop this connection's stream of a session, if it has one, and wait
    /// until it can send nothing more.
    async fn stop_stream(&self, sid: &str) {
        let entry = self.streams.lock().unwrap().remove(sid);
        if let Some(entry) = entry {
            entry.stop().await;
        }
    }

    async fn handle_input(&self, req: &Request) {
        let session = match self.dispatcher.backend.get(&req.id) {
            Some(s) => s,
            None => {
                let _ = self.sender.send(&ServerEvent::error(
                    Some(req.id.clone()),
                    "session not found",
                ));
                return;
            }
        };
        if let Err(e) = session.write(&req.data).await {
            let _ = self.sender.send(&ServerEvent::error(
                Some(req.id.clone()),
                format!("write failed: {e}"),
            ));
        }
    }

    /// Record the size at which this connection shows the session; the size
    /// policy sets the PTY. Answered with the session's size, which reaches
    /// this connection's stream on its own when it changed.
    fn handle_resize(&self, req: &Request) {
        if self.dispatcher.backend.get(&req.id).is_none() {
            let _ = self.sender.send(&ServerEvent::error(
                Some(req.id.clone()),
                "session not found",
            ));
            return;
        }
        if req.cols == 0 || req.rows == 0 {
            let _ = self
                .sender
                .send(&ServerEvent::error(Some(req.id.clone()), "invalid size"));
            return;
        }
        let size = PtySize {
            cols: req.cols,
            rows: req.rows,
        };
        let reported =
            self.dispatcher
                .sizes
                .report(&req.id, self.client, size, req.redraw, Instant::now());
        if let Err(e) = reported.result {
            let _ = self.sender.send(&ServerEvent::error(
                Some(req.id.clone()),
                format!("resize failed: {e}"),
            ));
        }
        let streamed = self.streams.lock().unwrap().contains_key(&req.id);
        if !reported.changed || !streamed {
            let _ = self.sender.send(&size_event(&req.id, reported.state));
        }
    }

    async fn handle_close(&self, req: &Request) {
        if let Some(ref notify) = self.dispatcher.notify {
            let engine = notify.lock().unwrap();
            engine.mark_user_initiated(&req.id);
        }

        self.stop_stream(&req.id).await;
        self.dispatcher
            .sizes
            .withdraw(&req.id, self.client, Instant::now());

        if let Err(e) = self.dispatcher.backend.close(&req.id) {
            let _ = self
                .sender
                .send(&ServerEvent::error(Some(req.id.clone()), e.to_string()));
            return;
        }
        let _ = self
            .sender
            .send(&ServerEvent::session_closed(req.id.clone(), None));
    }

    /// Store a pasted image and answer with its path (`image_saved`) or why it
    /// was not stored (`image_failed`), echoing the request's id.
    async fn handle_save_image(&self, req: &Request) {
        let result = match self.dispatcher.paste_dir.clone() {
            // Refused before the copy below, so a huge message is not copied.
            _ if req.data.len() > paste::MAX_ENCODED_LEN => Err(PasteError::TooLarge),
            None => Err(PasteError::NoDirectory),
            Some(dir) => {
                let encoded = req.data.clone();
                tokio::task::spawn_blocking(move || paste::save_base64(&dir, &encoded))
                    .await
                    .unwrap_or_else(|error| Err(PasteError::Io(std::io::Error::other(error))))
            }
        };
        let event = match result {
            Ok(path) => ServerEvent::image_saved(&req.request_id, path.to_string_lossy()),
            Err(error) => {
                warn!(request_id = %req.request_id, %error, "a pasted image was not stored");
                ServerEvent::image_failed(&req.request_id, error.to_string())
            }
        };
        let _ = self.sender.send(&event);
    }

    /// Store one chunk of an upload and answer with the bytes received
    /// (`upload_progress`), the stored file's path (`upload_saved`) or why the
    /// upload ended (`upload_failed`), echoing the request's id. A failure
    /// removes what the upload wrote.
    async fn handle_upload(&self, req: &Request) {
        let id = req.request_id.clone();
        let event = match self.upload_chunk(req).await {
            Ok(Some(path)) => ServerEvent::upload_saved(&id, path.to_string_lossy()),
            Ok(None) => {
                let received = self
                    .uploads
                    .lock()
                    .expect("uploads")
                    .get(&id)
                    .map_or(0, Upload::received);
                ServerEvent::upload_progress(&id, received)
            }
            Err(error) => {
                warn!(request_id = %id, %error, "an upload failed");
                ServerEvent::upload_failed(&id, error.to_string())
            }
        };
        let _ = self.sender.send(&event);
    }

    /// Write `req`'s chunk; the path once the file is complete.
    async fn upload_chunk(&self, req: &Request) -> Result<Option<PathBuf>, UploadError> {
        // Taken out while it is written; put back unless it failed or ended.
        let current = self
            .uploads
            .lock()
            .expect("uploads")
            .remove(&req.request_id);
        let bytes = upload::decode_chunk(&req.data)?;
        let mut upload = match current {
            Some(upload) => upload,
            None if req.offset != 0 => return Err(UploadError::UnknownUpload),
            None => {
                if self.uploads.lock().expect("uploads").len() >= upload::MAX_IN_FLIGHT {
                    return Err(UploadError::TooMany);
                }
                let size = req.size.ok_or(UploadError::NoSize)?;
                if size > upload::MAX_FILE_BYTES {
                    return Err(UploadError::TooLarge);
                }
                let store = self
                    .dispatcher
                    .paste_dir
                    .clone()
                    .ok_or(UploadError::NoDirectory)?;
                let name = req.name.clone();
                tokio::task::spawn_blocking(move || Upload::begin(&store, &name, size))
                    .await
                    .map_err(|error| UploadError::Io(std::io::Error::other(error)))??
            }
        };
        let offset = req.offset;
        let upload =
            tokio::task::spawn_blocking(move || upload.append(offset, &bytes).map(|()| upload))
                .await
                .map_err(|error| UploadError::Io(std::io::Error::other(error)))??;
        if upload.is_complete() {
            return Ok(Some(upload.finish()));
        }
        self.uploads
            .lock()
            .expect("uploads")
            .insert(req.request_id.clone(), upload);
        Ok(None)
    }

    /// Replay buffered output in binary frames of at most [`REPLAY_CHUNK`].
    fn send_snapshot(&self, sid: &str, snapshot: Vec<u8>) {
        for chunk in snapshot.chunks(REPLAY_CHUNK) {
            let _ = self.sender.send_binary(sid, chunk);
        }
    }

    /// Send the subscription's snapshot, if any is left, then forward its live
    /// output until the session ends or the stream is cancelled. Replaces a
    /// stream this connection already had for the session.
    fn start_stream(
        &self,
        session: &Arc<dyn SessionHandle>,
        mut subscription: Subscription,
        cancel: CancellationToken,
    ) {
        let sid = session.session_id().to_string();
        self.send_snapshot(&sid, std::mem::take(&mut subscription.snapshot));

        let sender = Arc::clone(&self.sender);
        let notify = self.dispatcher.notify.clone();
        let session = Arc::clone(session);
        let task_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            stream_output(session, subscription, sender, task_cancel, notify).await;
        });

        let mut streams = self.streams.lock().unwrap();
        if let Some(prev) = streams.insert(sid, StreamEntry { cancel, handle }) {
            // Callers stop the previous stream before subscribing, and one
            // connection handles one message at a time; should one remain,
            // keep only the newer stream, or output would arrive twice.
            prev.cancel.cancel();
            prev.handle.abort();
        }
    }

    /// Send the session's later sizes to this connection while it streams it.
    fn watch_size(
        &self,
        session: &Arc<dyn SessionHandle>,
        mut sizes: tokio::sync::watch::Receiver<Option<SizeState>>,
        cancel: CancellationToken,
    ) {
        let sender = Arc::clone(&self.sender);
        let sid = session.session_id().to_string();
        let mut done_rx = session.done();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = done_rx.changed() => break,
                    result = sizes.changed() => {
                        if result.is_err() { break; }
                        let size = *sizes.borrow_and_update();
                        if let Some(size) = size {
                            let _ = sender.send(&size_event(&sid, size));
                        }
                    }
                }
            }
        });
    }
}

fn size_event(session_id: &str, state: SizeState) -> ServerEvent {
    ServerEvent::size(
        session_id,
        state.size.cols,
        state.size.rows,
        state.policy.as_str(),
    )
}

/// Background task that forwards a session's output subscription to the
/// client and sends `closed` once the session has ended.
///
/// Cancelling it (detach, close, disconnect, a replacing stream) ends only
/// this subscription; the session keeps reading its output.
async fn stream_output(
    session: Arc<dyn SessionHandle>,
    subscription: Subscription,
    sender: Arc<dyn Sender>,
    cancel: CancellationToken,
    notify: Option<Arc<Mutex<NotificationEngine>>>,
) {
    let mut receiver = subscription.receiver;
    let mut done_rx = session.done();
    let mut output_ended = false;
    let mut exited = false;
    let mut deadline = None;

    while !(output_ended && exited) {
        tokio::select! {
            _ = cancel.cancelled() => return,
            chunk = receiver.recv(), if !output_ended => match chunk {
                Some(chunk) => {
                    let _ = sender.send_binary(session.session_id(), &chunk);
                }
                None => {
                    output_ended = true;
                    deadline.get_or_insert_with(|| tokio::time::Instant::now() + EXIT_GRACE);
                }
            },
            // An error means the exit watcher is gone, which it only is once
            // the process has exited.
            _ = done_rx.wait_for(|done| *done), if !exited => {
                exited = true;
                deadline.get_or_insert_with(|| tokio::time::Instant::now() + EXIT_GRACE);
            }
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)),
                if deadline.is_some() => break,
        }
    }

    let exit_code = session.exit_code();
    if let Some(ref notify) = notify {
        if session.claim_exit_report() {
            let engine = notify.lock().unwrap();
            engine.on_session_exit(session.session_id(), exit_code);
        }
    }
    let result = session.process_result();
    let _ = sender.send(&ServerEvent::session_closed_result(
        session.session_id(),
        result
            .as_ref()
            .and_then(|r| r.exit_code)
            .and_then(|code| i32::try_from(code).ok()),
        result.and_then(|r| r.signal),
    ));
}

/// Background task that reacts to OSC 0/2 title changes and forwards them to clients.
/// Event-driven via the session's watch channel — no polling.
async fn watch_osc_title(
    session: Arc<dyn SessionHandle>,
    sender: Arc<dyn Sender>,
    cancel: CancellationToken,
) {
    let mut rx = session.osc_title_watch();
    let mut done_rx = session.done();
    let sid = session.session_id().to_string();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = done_rx.changed() => break,
            result = rx.changed() => {
                if result.is_err() { break; }
                let event = rx.borrow_and_update().clone();

                // Send OSC title update to frontend
                let info = session.title();
                let cmdline = if info.cmdline.is_empty() { None } else { Some(info.cmdline) };
                let osc = if event.title.is_empty() { None } else { Some(event.title) };
                let _ = sender.send(&ServerEvent::title_changed(
                    sid.clone(), info.proc, info.cwd,
                    cmdline, info.base_shell, osc,
                ));
            }
        }
    }
}

/// Background task that polls session proc info and sends title updates on change.
/// Only handles /proc-based data (proc name, cwd, cmdline, base_shell).
/// OSC 133 semantic events are handled by watch_semantic.
async fn watch_title(
    session: Arc<dyn SessionHandle>,
    sender: Arc<dyn Sender>,
    cancel: CancellationToken,
    notify: Option<Arc<Mutex<NotificationEngine>>>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut prev_title = String::new();
    let mut prev_cwd = String::new();
    let mut prev_cmdline = String::new();
    let mut prev_base_shell = false;
    let mut done_rx = session.done();

    // Send initial title if available
    {
        let info = session.title();
        let osc_rx = session.osc_title_watch();
        let osc = osc_rx.borrow().title.clone();
        if !info.proc.is_empty() {
            prev_title = info.proc.clone();
            prev_cwd = info.cwd.clone();
            prev_cmdline = info.cmdline.clone();
            prev_base_shell = info.base_shell;
            let cmdline = if info.cmdline.is_empty() {
                None
            } else {
                Some(info.cmdline)
            };
            let osc_title = if osc.is_empty() { None } else { Some(osc) };
            let _ = sender.send(&ServerEvent::title_changed(
                session.session_id(),
                info.proc,
                info.cwd,
                cmdline,
                info.base_shell,
                osc_title,
            ));
        }
    }

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = done_rx.changed() => break,
            _ = interval.tick() => {
                let info = session.title();
                let osc_rx = session.osc_title_watch();
                let osc = osc_rx.borrow().title.clone();

                if info.proc != prev_title
                    || info.cwd != prev_cwd
                    || info.cmdline != prev_cmdline
                    || info.base_shell != prev_base_shell
                {
                    let cmdline = if info.cmdline.is_empty() { None } else { Some(info.cmdline.clone()) };
                    let osc_title = if osc.is_empty() { None } else { Some(osc) };
                    let _ = sender.send(&ServerEvent::title_changed(
                        session.session_id(), info.proc.clone(), info.cwd.clone(),
                        cmdline, info.base_shell, osc_title,
                    ));

                    prev_title = info.proc.clone();
                    prev_cwd = info.cwd.clone();
                    prev_cmdline = info.cmdline.clone();
                    prev_base_shell = info.base_shell;
                }

                // Notify engine about proc info changes
                if let Some(ref notify_engine) = notify {
                    let engine = notify_engine.lock().unwrap();
                    engine.on_proc_update(
                        session.session_id(),
                        &info.proc,
                        &info.cmdline,
                        info.base_shell,
                    );
                }
            }
        }
    }
}

/// Background task that reacts to OSC 133 semantic events and forwards them to clients.
async fn watch_semantic(
    session: Arc<dyn SessionHandle>,
    sender: Arc<dyn Sender>,
    cancel: CancellationToken,
    notify: Option<Arc<Mutex<NotificationEngine>>>,
) {
    let mut rx = session.semantic_watch();
    let mut done_rx = session.done();
    let sid = session.session_id().to_string();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = done_rx.changed() => break,
            result = rx.changed() => {
                if result.is_err() { break; }
                let event = match rx.borrow_and_update().clone() {
                    Some(e) => e,
                    None => continue, // Skip the initial synthetic None
                };

                // Map action to wire format
                let (action_str, exit_code) = match &event.action {
                    crate::pty::osc::SemanticAction::PromptStart => ("A", None),
                    crate::pty::osc::SemanticAction::InputStart => ("B", None),
                    crate::pty::osc::SemanticAction::OutputStart => ("C", None),
                    crate::pty::osc::SemanticAction::CommandFinished { exit_code } => ("D", *exit_code),
                };

                let _ = sender.send(&ServerEvent::semantic_boundary(
                    sid.clone(), action_str.to_string(), exit_code,
                ));

                // Notify engine about semantic events
                if let Some(ref notify_engine) = notify {
                    let engine = notify_engine.lock().unwrap();
                    engine.on_semantic_event(&sid, &event.action);
                }
            }
        }
    }
}

/// Background task that reacts to BEL events and forwards them to clients.
async fn watch_bell(
    session: Arc<dyn SessionHandle>,
    sender: Arc<dyn Sender>,
    cancel: CancellationToken,
) {
    let mut rx = session.bell_watch();
    let mut done_rx = session.done();
    let sid = session.session_id().to_string();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = done_rx.changed() => break,
            result = rx.changed() => {
                if result.is_err() { break; }
                let event = *rx.borrow_and_update();
                if event.is_none() { continue; } // Skip initial synthetic None

                let _ = sender.send(&ServerEvent::bell(sid.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ServerEvent;
    use crate::pty::{
        BellEvent, CreateSessionOpts, PtyError, SemanticEvent, SemanticState, SessionStats,
        TitleInfo,
    };
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use tokio::sync::watch;

    // ---- Mock SessionHandle ----

    struct MockSession {
        id: String,
        written: Mutex<Vec<Vec<u8>>>,
        /// Everything the session has "printed"; doubles as its scrollback.
        output: Mutex<MockOutput>,
        /// Printed by `subscribe` itself, after registering the subscriber,
        /// standing in for a reader thread that records a chunk meanwhile.
        print_on_subscribe: Vec<u8>,
        resizes: Mutex<Vec<(u16, u16)>>,
        closed: AtomicBool,
        exit_code: AtomicI32,
        exit_reported: AtomicBool,
        #[expect(
            dead_code,
            reason = "held so the done channel stays open; dropping it would end every output stream at once"
        )]
        done_tx: watch::Sender<bool>,
        done_rx: watch::Receiver<bool>,
    }

    #[derive(Default)]
    struct MockOutput {
        history: Vec<u8>,
        subscribers: Vec<tokio::sync::mpsc::UnboundedSender<Arc<[u8]>>>,
    }

    impl MockSession {
        fn new(id: &str) -> Self {
            let (done_tx, done_rx) = watch::channel(false);
            Self {
                id: id.to_string(),
                written: Mutex::new(Vec::new()),
                output: Mutex::new(MockOutput::default()),
                print_on_subscribe: Vec::new(),
                resizes: Mutex::new(Vec::new()),
                closed: AtomicBool::new(false),
                exit_code: AtomicI32::new(0),
                exit_reported: AtomicBool::new(false),
                done_tx,
                done_rx,
            }
        }

        fn with_scrollback(self, data: Vec<u8>) -> Self {
            self.output.lock().unwrap().history = data;
            self
        }

        fn printing_on_subscribe(mut self, data: Vec<u8>) -> Self {
            self.print_on_subscribe = data;
            self
        }

        /// Record output the way the reader thread does.
        fn print(&self, data: &[u8]) {
            let mut output = self.output.lock().unwrap();
            output.history.extend_from_slice(data);
            let chunk: Arc<[u8]> = Arc::from(data);
            output
                .subscribers
                .retain(|tx| tx.send(Arc::clone(&chunk)).is_ok());
        }
    }

    #[async_trait::async_trait]
    impl SessionHandle for MockSession {
        fn session_id(&self) -> &str {
            &self.id
        }

        fn subscribe(&self) -> Subscription {
            let (tx, receiver) = tokio::sync::mpsc::unbounded_channel();
            let snapshot = {
                let mut output = self.output.lock().unwrap();
                output.subscribers.push(tx);
                output.history.clone()
            };
            if !self.print_on_subscribe.is_empty() {
                self.print(&self.print_on_subscribe);
            }
            Subscription { snapshot, receiver }
        }

        async fn write(&self, data: &[u8]) -> std::io::Result<()> {
            self.written.lock().unwrap().push(data.to_vec());
            Ok(())
        }

        fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
            self.resizes.lock().unwrap().push((cols, rows));
            Ok(())
        }

        fn close(&self) -> std::io::Result<()> {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn done(&self) -> watch::Receiver<bool> {
            self.done_rx.clone()
        }

        fn exit_code(&self) -> i32 {
            self.exit_code.load(Ordering::SeqCst)
        }

        fn title(&self) -> TitleInfo {
            TitleInfo::default()
        }

        fn osc_title_watch(&self) -> watch::Receiver<crate::pty::OscTitleEvent> {
            watch::channel(crate::pty::OscTitleEvent::default()).1
        }

        fn semantic_state(&self) -> SemanticState {
            SemanticState::default()
        }

        fn semantic_watch(&self) -> watch::Receiver<SemanticEvent> {
            watch::channel(None).1
        }

        fn bell_watch(&self) -> watch::Receiver<BellEvent> {
            watch::channel(None).1
        }

        fn scrollback(&self) -> Vec<u8> {
            self.output.lock().unwrap().history.clone()
        }

        fn stats(&self) -> Option<SessionStats> {
            None
        }

        fn claim_exit_report(&self) -> bool {
            !self.exit_reported.swap(true, Ordering::SeqCst)
        }
    }

    // ---- Mock Backend ----

    struct MockBackend {
        sessions: Mutex<HashMap<String, Arc<MockSession>>>,
        next_id: Mutex<String>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                sessions: Mutex::new(HashMap::new()),
                next_id: Mutex::new("test-session-1".to_string()),
            }
        }

        fn with_session(self, session: Arc<MockSession>) -> Self {
            self.sessions
                .lock()
                .unwrap()
                .insert(session.id.clone(), session);
            self
        }

        fn set_next_id(&self, id: &str) {
            *self.next_id.lock().unwrap() = id.to_string();
        }
    }

    #[async_trait::async_trait]
    impl Backend for MockBackend {
        async fn create(
            &self,
            opts: CreateSessionOpts,
        ) -> Result<Arc<dyn SessionHandle>, PtyError> {
            let id = if opts.id.is_empty() {
                self.next_id.lock().unwrap().clone()
            } else {
                opts.id
            };
            let session = Arc::new(MockSession::new(&id));
            self.sessions
                .lock()
                .unwrap()
                .insert(id, Arc::clone(&session));
            Ok(session as Arc<dyn SessionHandle>)
        }

        fn get(&self, id: &str) -> Option<Arc<dyn SessionHandle>> {
            self.sessions
                .lock()
                .unwrap()
                .get(id)
                .map(|s| Arc::clone(s) as Arc<dyn SessionHandle>)
        }

        fn close(&self, id: &str) -> Result<(), PtyError> {
            if let Some(s) = self.sessions.lock().unwrap().remove(id) {
                s.closed.store(true, Ordering::SeqCst);
            }
            Ok(())
        }

        fn close_all(&self) {
            let sessions = self.sessions.lock().unwrap();
            for s in sessions.values() {
                s.closed.store(true, Ordering::SeqCst);
            }
        }

        fn sessions(&self) -> Vec<Arc<dyn SessionHandle>> {
            self.sessions
                .lock()
                .unwrap()
                .values()
                .map(|s| Arc::clone(s) as Arc<dyn SessionHandle>)
                .collect()
        }
    }

    // ---- Mock Sender ----

    struct MockSender {
        events: Mutex<Vec<ServerEvent>>,
        binary_chunks: Mutex<Vec<(String, Vec<u8>)>>,
        /// "binary", or the event's wire type, in the order sent.
        order: Mutex<Vec<String>>,
        /// A payload whose send blocks until the paired sender fires, to hold
        /// a stream in the middle of forwarding a chunk.
        hold: Mutex<Option<(Vec<u8>, std::sync::mpsc::Receiver<()>)>>,
        holding: AtomicBool,
    }

    impl MockSender {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                binary_chunks: Mutex::new(Vec::new()),
                order: Mutex::new(Vec::new()),
                hold: Mutex::new(None),
                holding: AtomicBool::new(false),
            }
        }

        /// Block the send of `payload` until `release` fires.
        fn hold_send_of(&self, payload: &[u8], release: std::sync::mpsc::Receiver<()>) {
            *self.hold.lock().unwrap() = Some((payload.to_vec(), release));
        }

        fn order(&self) -> Vec<String> {
            self.order.lock().unwrap().clone()
        }

        /// All binary output for one session, in order.
        fn output(&self, session_id: &str) -> Vec<u8> {
            self.binary_chunks()
                .into_iter()
                .filter(|(sid, _)| sid == session_id)
                .flat_map(|(_, data)| data)
                .collect()
        }

        fn events(&self) -> Vec<ServerEvent> {
            self.events.lock().unwrap().clone()
        }

        fn binary_chunks(&self) -> Vec<(String, Vec<u8>)> {
            self.binary_chunks.lock().unwrap().clone()
        }
    }

    impl Sender for MockSender {
        fn send(
            &self,
            event: &ServerEvent,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let kind = serde_json::to_value(event)
                .ok()
                .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
                .unwrap_or_default();
            self.order.lock().unwrap().push(kind);
            self.events.lock().unwrap().push(event.clone());
            Ok(())
        }

        fn send_binary(
            &self,
            session_id: &str,
            data: &[u8],
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let held = {
                let mut hold = self.hold.lock().unwrap();
                match hold.take() {
                    Some((payload, release)) if payload == data => Some(release),
                    other => {
                        *hold = other;
                        None
                    }
                }
            };
            if let Some(release) = held {
                self.holding.store(true, Ordering::SeqCst);
                let _ = release.recv();
            }
            self.order.lock().unwrap().push("binary".to_string());
            self.binary_chunks
                .lock()
                .unwrap()
                .push((session_id.to_string(), data.to_vec()));
            Ok(())
        }
    }

    // ---- Tests ----

    #[tokio::test]
    async fn managed_pending_failed_and_interrupted_panes_never_create_a_shell() {
        use crate::mux::jobs::JobState;
        for state in [
            JobState::Starting,
            JobState::Failed,
            JobState::Interrupted,
            JobState::Exited,
        ] {
            let backend = Arc::new(MockBackend::new());
            let dispatcher = Arc::new(Dispatcher::new(backend.clone()));
            dispatcher.set_job_resolver(Arc::new(move |_| Some(("managed-session".into(), state))));
            let sender = Arc::new(MockSender::new());
            let conn = dispatcher.new_connection(sender.clone());
            let request = Request {
                msg_type: MSG_CREATE.into(),
                pane_id: "job-pane".into(),
                ..Default::default()
            };
            conn.handle(&request).await;
            assert!(backend.sessions().is_empty());
            assert!(matches!(
                sender.events().as_slice(),
                [ServerEvent::Error { .. }]
            ));
        }
    }

    #[tokio::test]
    async fn managed_fallback_and_attach_reuse_the_started_program() {
        if !pty_available() {
            return;
        }
        let backend = Arc::new(crate::pty::manager::Manager::new());
        let mut opts = sh_opts("managed-session", "");
        opts.dir = std::env::temp_dir().to_string_lossy().into_owned();
        let session = backend
            .create_program(
                opts,
                vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "printf started-once; read line; printf '%s' \"$line\"".into(),
                ],
            )
            .await
            .expect("job");
        let dispatcher = Arc::new(Dispatcher::new(backend.clone()));
        dispatcher.set_job_resolver(Arc::new(|_| {
            Some((
                "managed-session".into(),
                crate::mux::jobs::JobState::Running,
            ))
        }));
        let a = Arc::new(MockSender::new());
        let b = Arc::new(MockSender::new());
        let create_conn = dispatcher.new_connection(a.clone());
        let attach_conn = dispatcher.new_connection(b.clone());
        let create = Request {
            msg_type: MSG_CREATE.into(),
            pane_id: "job-pane".into(),
            command: "must not run".into(),
            ..Default::default()
        };
        let attach_request = attach("managed-session");
        tokio::join!(
            create_conn.handle(&create),
            attach_conn.handle(&attach_request)
        );
        assert_eq!(backend.sessions().len(), 1);
        session.write(b"finished\n").await.expect("input");
        assert!(
            wait_until(Duration::from_secs(5), || !closed_events(&a).is_empty()
                && !closed_events(&b).is_empty())
            .await
        );
        assert!(text(&a.output("managed-session")).contains("started-once"));
        assert!(text(&b.output("managed-session")).contains("started-once"));
        backend.close_all();
    }

    #[tokio::test]
    async fn test_handle_create() {
        let backend = Arc::new(MockBackend::new());
        backend.set_next_id("sess-1");
        let dispatcher = Arc::new(Dispatcher::new(backend.clone()));
        let sender = Arc::new(MockSender::new());
        let conn = dispatcher.new_connection(sender.clone());

        conn.handle(&Request {
            msg_type: MSG_CREATE.to_string(),
            shell: "/bin/bash".to_string(),
            cols: 80,
            rows: 24,
            ..Default::default()
        })
        .await;

        // Give spawned tasks a moment to start
        tokio::task::yield_now().await;

        let events = sender.events();
        assert!(!events.is_empty(), "should have sent at least one event");
        match &events[0] {
            ServerEvent::SessionCreated {
                session_id,
                home_dir,
                ..
            } => {
                assert_eq!(session_id, "sess-1");
                assert!(home_dir.is_some(), "home_dir should be set");
            }
            other => panic!("expected SessionCreated, got {:?}", other),
        }

        // Verify session was created in backend
        assert!(backend.get("sess-1").is_some());

        conn.close();
    }

    #[tokio::test]
    async fn test_handle_input() {
        let session = Arc::new(MockSession::new("sess-input"));
        let backend = Arc::new(MockBackend::new().with_session(session.clone()));
        let dispatcher = Arc::new(Dispatcher::new(backend));
        let sender = Arc::new(MockSender::new());
        let conn = dispatcher.new_connection(sender.clone());

        conn.handle(&Request {
            msg_type: MSG_INPUT.to_string(),
            id: "sess-input".to_string(),
            data: b"hello\n".to_vec(),
            ..Default::default()
        })
        .await;

        let written = session.written.lock().unwrap();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0], b"hello\n");
    }

    #[tokio::test]
    async fn test_handle_close() {
        let session = Arc::new(MockSession::new("sess-close"));
        let backend = Arc::new(MockBackend::new().with_session(session.clone()));
        let dispatcher = Arc::new(Dispatcher::new(backend.clone()));
        let sender = Arc::new(MockSender::new());
        let conn = dispatcher.new_connection(sender.clone());

        conn.handle(&Request {
            msg_type: MSG_CLOSE.to_string(),
            id: "sess-close".to_string(),
            ..Default::default()
        })
        .await;

        let events = sender.events();
        assert_eq!(events.len(), 1);
        match &events[0] {
            ServerEvent::SessionClosed { session_id, .. } => {
                assert_eq!(session_id, "sess-close");
            }
            other => panic!("expected SessionClosed, got {:?}", other),
        }

        // Session should be removed from backend
        assert!(backend.get("sess-close").is_none());
    }

    #[tokio::test]
    async fn test_handle_detach() {
        let backend = Arc::new(MockBackend::new());
        backend.set_next_id("sess-detach");
        let dispatcher = Arc::new(Dispatcher::new(backend));
        let sender = Arc::new(MockSender::new());
        let conn = dispatcher.new_connection(sender.clone());

        // Create a session first to get a stream entry
        conn.handle(&Request {
            msg_type: MSG_CREATE.to_string(),
            cols: 80,
            rows: 24,
            ..Default::default()
        })
        .await;

        tokio::task::yield_now().await;

        // Verify stream exists
        assert!(conn.streams.lock().unwrap().contains_key("sess-detach"));

        // Detach
        conn.handle(&Request {
            msg_type: MSG_DETACH.to_string(),
            id: "sess-detach".to_string(),
            ..Default::default()
        })
        .await;

        // Stream should be removed
        assert!(!conn.streams.lock().unwrap().contains_key("sess-detach"));

        conn.close();
    }

    #[tokio::test]
    async fn test_scrollback_replay_chunking() {
        // Create scrollback data larger than one chunk (32768 bytes)
        let scrollback_size = 32768 * 2 + 1000; // 2 full chunks + partial
        let scrollback_data: Vec<u8> = (0..scrollback_size).map(|i| (i % 256) as u8).collect();

        let session =
            Arc::new(MockSession::new("sess-scroll").with_scrollback(scrollback_data.clone()));
        let backend = Arc::new(MockBackend::new().with_session(session));
        let dispatcher = Arc::new(Dispatcher::new(backend));
        let sender = Arc::new(MockSender::new());
        let conn = dispatcher.new_connection(sender.clone());

        conn.handle(&Request {
            msg_type: MSG_ATTACH.to_string(),
            id: "sess-scroll".to_string(),
            cols: 80,
            rows: 24,
            ..Default::default()
        })
        .await;

        tokio::task::yield_now().await;

        let chunks = sender.binary_chunks();
        assert_eq!(
            chunks.len(),
            3,
            "should send 3 chunks for {} bytes",
            scrollback_size
        );

        // First two chunks should be exactly 32768 bytes
        assert_eq!(chunks[0].1.len(), 32768);
        assert_eq!(chunks[1].1.len(), 32768);
        // Last chunk should be the remainder
        assert_eq!(chunks[2].1.len(), 1000);

        // All chunks should have the correct session ID
        for (sid, _) in &chunks {
            assert_eq!(sid, "sess-scroll");
        }

        // Reassemble and verify data integrity
        let mut reassembled = Vec::new();
        for (_, data) in &chunks {
            reassembled.extend_from_slice(data);
        }
        assert_eq!(reassembled, scrollback_data);

        // The replay comes first, then the "attached" event.
        assert_eq!(sender.order(), ["binary", "binary", "binary", "attached"]);

        conn.close();
    }

    /// terminal-sessions, No gap between replay and live output: output the
    /// session records while the client subscribes arrives exactly once,
    /// after the replay and after `attached`.
    #[tokio::test]
    async fn no_gap_between_replay_and_live_output() {
        let session = Arc::new(
            MockSession::new("sess-gap")
                .with_scrollback(b"replayed;".to_vec())
                .printing_on_subscribe(b"during-subscribe;".to_vec()),
        );
        let backend = Arc::new(MockBackend::new().with_session(session.clone()));
        let dispatcher = Arc::new(Dispatcher::new(backend));
        let sender = Arc::new(MockSender::new());
        let conn = dispatcher.new_connection(sender.clone());

        conn.handle(&attach("sess-gap")).await;
        session.print(b"later;");

        let expected = b"replayed;during-subscribe;later;";
        wait_until(Duration::from_secs(5), || {
            sender.output("sess-gap").len() >= expected.len()
        })
        .await;
        assert_eq!(
            String::from_utf8_lossy(&sender.output("sess-gap")),
            String::from_utf8_lossy(expected)
        );
        assert_eq!(sender.order(), ["binary", "attached", "binary", "binary"]);

        conn.close();
    }

    /// A replacing attach waits until the connection's previous stream of the
    /// session has stopped: a chunk that stream is still forwarding arrives
    /// before the new replay, never after it, where it would repeat a byte the
    /// replay already holds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replacing_attach_waits_for_the_previous_stream() {
        let session = Arc::new(MockSession::new("sess-replace").with_scrollback(b"old;".to_vec()));
        let backend = Arc::new(MockBackend::new().with_session(session.clone()));
        let dispatcher = Arc::new(Dispatcher::new(backend));
        let sender = Arc::new(MockSender::new());
        let (release, gate) = std::sync::mpsc::channel();
        sender.hold_send_of(b"in-flight;", gate);
        let conn = dispatcher.new_connection(sender.clone());

        conn.handle(&attach("sess-replace")).await;
        session.print(b"in-flight;");
        let held = wait_until(Duration::from_secs(5), || {
            sender.holding.load(Ordering::SeqCst)
        })
        .await;
        assert!(held, "the first stream never started forwarding the chunk");

        // Let the held chunk go only once the second attach has had time to
        // take its snapshot, had it not waited.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let _ = release.send(());
        });
        conn.handle(&attach("sess-replace")).await;

        let chunks: Vec<String> = sender
            .binary_chunks()
            .into_iter()
            .map(|(_, data)| String::from_utf8_lossy(&data).into_owned())
            .collect();
        assert_eq!(chunks, ["old;", "in-flight;", "old;in-flight;"]);
        assert_eq!(
            sender.order(),
            ["binary", "attached", "binary", "binary", "attached"]
        );

        conn.close();
    }

    #[tokio::test]
    async fn test_close_non_mux_closes_sessions() {
        let backend = Arc::new(MockBackend::new());
        backend.set_next_id("sess-nm");
        let dispatcher = Arc::new(Dispatcher::new(backend.clone()));
        let sender = Arc::new(MockSender::new());
        let conn = dispatcher.new_connection(sender);

        // Create a session
        conn.handle(&Request {
            msg_type: MSG_CREATE.to_string(),
            cols: 80,
            rows: 24,
            ..Default::default()
        })
        .await;

        tokio::task::yield_now().await;

        // Session should exist
        assert!(backend.get("sess-nm").is_some());

        // Close connection (not mux managed)
        assert!(!conn.mux_managed);
        conn.close();

        // Session should be closed in backend
        assert!(backend.get("sess-nm").is_none());
    }

    #[tokio::test]
    async fn test_close_mux_managed_keeps_sessions() {
        let backend = Arc::new(MockBackend::new());
        backend.set_next_id("sess-mux");
        let dispatcher = Arc::new(Dispatcher::new(backend.clone()));
        let sender = Arc::new(MockSender::new());
        let mut conn = dispatcher.new_connection(sender);
        conn.mux_managed = true;

        // Create a session
        conn.handle(&Request {
            msg_type: MSG_CREATE.to_string(),
            cols: 80,
            rows: 24,
            ..Default::default()
        })
        .await;

        tokio::task::yield_now().await;

        // Session should exist
        assert!(backend.get("sess-mux").is_some());

        // Close connection (mux managed)
        conn.close();

        // Session should still exist in backend
        assert!(backend.get("sess-mux").is_some());
    }

    // ---- Pasted images ----

    const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

    fn save_image(request_id: &str, data: Vec<u8>) -> Request {
        Request {
            msg_type: MSG_SAVE_IMAGE.to_string(),
            request_id: request_id.to_string(),
            data,
            ..Default::default()
        }
    }

    fn encode_base64(bytes: &[u8]) -> Vec<u8> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .encode(bytes)
            .into_bytes()
    }

    /// A connection whose pasted images go to `dir`.
    fn paste_connection(dir: &std::path::Path, sender: Arc<MockSender>) -> Connection {
        let mut dispatcher = Dispatcher::new(Arc::new(MockBackend::new()));
        dispatcher.paste_dir = Some(dir.to_path_buf());
        Arc::new(dispatcher).new_connection(sender)
    }

    /// terminal-sessions, Image Upload, A screenshot: the PNG is stored with
    /// mode 0600 and the answer carries the request id and the absolute path.
    #[tokio::test]
    async fn save_image_stores_a_png_and_answers_its_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("splitty").join("paste");
        let sender = Arc::new(MockSender::new());
        let conn = paste_connection(&dir, sender.clone());
        let png = [PNG_SIGNATURE, &[1u8; 200 * 1024]].concat();

        conn.handle(&save_image("r1", encode_base64(&png))).await;

        let events = sender.events();
        let [event @ ServerEvent::ImageSaved {
            request_id, path, ..
        }] = &events[..]
        else {
            panic!("expected one image_saved, got {events:?}");
        };
        assert_eq!(request_id, "r1");
        let path = std::path::Path::new(path);
        assert!(path.is_absolute(), "{path:?}");
        assert_eq!(path.parent(), Some(dir.as_path()));
        assert_eq!(path.extension().unwrap(), "png");
        assert_eq!(std::fs::read(path).unwrap(), png);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["type"], "image_saved");
        assert_eq!(json["requestId"], "r1");
    }

    /// terminal-sessions, Image Upload, Not an image and Too large, and data
    /// that is not base64: `image_failed` with the request id, nothing stored.
    #[tokio::test]
    async fn save_image_refuses_and_stores_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("paste");
        let mut too_large = PNG_SIGNATURE.to_vec();
        too_large.resize(11 * 1024 * 1024, 0);
        let cases = [
            ("not-an-image", encode_base64(b"#!/bin/sh\necho pasted\n")),
            ("invalid-base64", b"%%% not base64 %%%".to_vec()),
            ("too-large", encode_base64(&too_large)),
        ];

        for (request_id, data) in cases {
            let sender = Arc::new(MockSender::new());
            let conn = paste_connection(&dir, sender.clone());

            conn.handle(&save_image(request_id, data)).await;

            let events = sender.events();
            let [event @ ServerEvent::ImageFailed {
                request_id: echoed,
                error,
                ..
            }] = &events[..]
            else {
                panic!("{request_id}: expected one image_failed, got {events:?}");
            };
            assert_eq!(echoed, request_id);
            assert!(!error.is_empty());
            let json = serde_json::to_value(event).unwrap();
            assert_eq!(json["type"], "image_failed");
            assert_eq!(json["requestId"], request_id);
            assert!(!dir.exists(), "{request_id}: something was stored");
        }
    }

    // ---- Uploaded files ----

    fn upload_chunk(
        request_id: &str,
        offset: u64,
        bytes: &[u8],
        first: Option<(&str, u64)>,
    ) -> Request {
        Request {
            msg_type: MSG_UPLOAD.to_string(),
            request_id: request_id.to_string(),
            data: encode_base64(bytes),
            offset,
            name: first.map(|(name, _)| name.to_string()).unwrap_or_default(),
            size: first.map(|(_, size)| size),
            ..Default::default()
        }
    }

    /// The `upload-` directories of `dir`.
    fn upload_dirs(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("upload-")
            })
            .collect()
    }

    /// terminal-sessions, Chunked File Uploads, Upload in two chunks: the
    /// first answer counts the bytes received, the second names the file.
    #[tokio::test]
    async fn upload_in_two_chunks_answers_progress_then_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("splitty").join("paste");
        let sender = Arc::new(MockSender::new());
        let conn = paste_connection(&dir, sender.clone());
        let first = vec![3u8; upload::MAX_CHUNK_BYTES];
        let second = vec![4u8; upload::MAX_CHUNK_BYTES / 2];
        let size = (first.len() + second.len()) as u64;

        conn.handle(&upload_chunk("u1", 0, &first, Some(("notes.txt", size))))
            .await;
        conn.handle(&upload_chunk("u1", first.len() as u64, &second, None))
            .await;

        let events = sender.events();
        let [progress, saved] = &events[..] else {
            panic!("expected two answers, got {events:?}");
        };
        let progress = serde_json::to_value(progress).unwrap();
        assert_eq!(progress["type"], "upload_progress");
        assert_eq!(progress["requestId"], "u1");
        assert_eq!(progress["received"], upload::MAX_CHUNK_BYTES as u64);
        let saved = serde_json::to_value(saved).unwrap();
        assert_eq!(saved["type"], "upload_saved");
        assert_eq!(saved["requestId"], "u1");
        let path = std::path::Path::new(saved["path"].as_str().unwrap());
        assert!(path.is_absolute());
        assert!(path.ends_with("notes.txt"), "{path:?}");
        assert_eq!(path.parent().unwrap().parent(), Some(dir.as_path()));
        assert_eq!(std::fs::read(path).unwrap(), [first, second].concat());
    }

    /// terminal-sessions, Chunked File Uploads, Chunk out of order: the upload
    /// fails and nothing of it remains; a chunk that starts no upload and a
    /// fifth upload at once fail too.
    #[tokio::test]
    async fn an_upload_out_of_order_or_beyond_its_limits_fails_and_leaves_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("paste");
        let sender = Arc::new(MockSender::new());
        let conn = paste_connection(&dir, sender.clone());
        let chunk = vec![1u8; 1024];

        conn.handle(&upload_chunk("u1", 0, &chunk, Some(("a.bin", 4096))))
            .await;
        conn.handle(&upload_chunk("u1", 0, &chunk, None)).await;
        conn.handle(&upload_chunk("nope", 1024, &chunk, None)).await;
        conn.handle(&upload_chunk(
            "big",
            0,
            &chunk,
            Some(("b", upload::MAX_FILE_BYTES + 1)),
        ))
        .await;
        conn.handle(&upload_chunk("nosize", 0, &chunk, None)).await;

        let types: Vec<_> = sender
            .events()
            .iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .map(|v| {
                (
                    v["type"].as_str().unwrap().to_owned(),
                    v["requestId"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            types,
            [
                ("upload_progress".to_owned(), "u1".to_owned()),
                ("upload_failed".to_owned(), "u1".to_owned()),
                ("upload_failed".to_owned(), "nope".to_owned()),
                ("upload_failed".to_owned(), "big".to_owned()),
                ("upload_failed".to_owned(), "nosize".to_owned()),
            ]
        );
        assert!(upload_dirs(&dir).is_empty(), "{:?}", upload_dirs(&dir));

        for i in 0..upload::MAX_IN_FLIGHT {
            conn.handle(&upload_chunk(
                &format!("p{i}"),
                0,
                &chunk,
                Some(("p", 4096)),
            ))
            .await;
        }
        conn.handle(&upload_chunk("fifth", 0, &chunk, Some(("p", 4096))))
            .await;
        let last = serde_json::to_value(sender.events().last().unwrap()).unwrap();
        assert_eq!(last["type"], "upload_failed");
        assert_eq!(last["requestId"], "fifth");
        assert_eq!(upload_dirs(&dir).len(), upload::MAX_IN_FLIGHT);
    }

    /// terminal-sessions, Chunked File Uploads, Connection lost during an
    /// upload: its directory is removed.
    #[tokio::test]
    async fn a_closed_connection_removes_its_unfinished_uploads() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("paste");
        let sender = Arc::new(MockSender::new());
        let conn = paste_connection(&dir, sender.clone());

        conn.handle(&upload_chunk(
            "u1",
            0,
            &[7u8; 2048],
            Some(("half.bin", 4096)),
        ))
        .await;
        assert_eq!(upload_dirs(&dir).len(), 1);
        conn.close();

        assert!(upload_dirs(&dir).is_empty());
    }

    /// An empty file is one first chunk with no data.
    #[tokio::test]
    async fn an_empty_file_uploads_in_one_chunk() {
        let tmp = tempfile::tempdir().unwrap();
        let sender = Arc::new(MockSender::new());
        let conn = paste_connection(tmp.path(), sender.clone());

        conn.handle(&upload_chunk("e", 0, b"", Some(("empty.txt", 0))))
            .await;

        let saved = serde_json::to_value(&sender.events()[0]).unwrap();
        assert_eq!(saved["type"], "upload_saved");
        assert_eq!(std::fs::read(saved["path"].as_str().unwrap()).unwrap(), b"");
    }

    // ---- PTY size ----

    fn resize(id: &str, cols: u16, rows: u16) -> Request {
        Request {
            msg_type: MSG_RESIZE.to_string(),
            id: id.to_string(),
            cols,
            rows,
            ..Default::default()
        }
    }

    fn activity() -> Request {
        Request {
            msg_type: MSG_ACTIVITY.to_string(),
            ..Default::default()
        }
    }

    /// The `size` events a connection received for `session`, as (cols, rows).
    fn sizes(sender: &MockSender, session: &str) -> Vec<(u16, u16)> {
        sender
            .events()
            .into_iter()
            .filter_map(|event| match event {
                ServerEvent::Size {
                    session_id,
                    cols,
                    rows,
                    ..
                } if session_id == session => Some((cols, rows)),
                _ => None,
            })
            .collect()
    }

    /// A dispatcher over one mock session and `count` connections that
    /// attached it and had an activity.
    async fn shown_by(
        count: usize,
        scrollback: &[u8],
    ) -> (Arc<MockSession>, Vec<Arc<MockSender>>, Vec<Connection>) {
        let session = Arc::new(MockSession::new("sess-1").with_scrollback(scrollback.to_vec()));
        let backend = Arc::new(MockBackend::new().with_session(session.clone()));
        let dispatcher = Arc::new(Dispatcher::new(backend));
        let mut senders = Vec::new();
        let mut conns = Vec::new();
        for _ in 0..count {
            let sender = Arc::new(MockSender::new());
            let conn = dispatcher.new_connection(sender.clone());
            conn.handle(&activity()).await;
            conn.handle(&attach("sess-1")).await;
            senders.push(sender);
            conns.push(conn);
        }
        (session, senders, conns)
    }

    #[tokio::test]
    async fn resize_applies_to_the_pty_and_is_answered() {
        let (session, senders, conns) = shown_by(1, b"").await;
        conns[0].handle(&resize("sess-1", 100, 30)).await;
        assert_eq!(*session.resizes.lock().unwrap(), [(100, 30)]);
        assert!(
            wait_until(Duration::from_secs(5), || sizes(&senders[0], "sess-1")
                == [(100, 30)])
            .await,
            "events: {:?}",
            senders[0].events()
        );
    }

    #[tokio::test]
    async fn resize_of_an_unknown_session_is_an_error() {
        let (session, senders, conns) = shown_by(1, b"").await;
        conns[0].handle(&resize("nope", 100, 30)).await;
        assert!(session.resizes.lock().unwrap().is_empty());
        assert!(senders[0].events().iter().any(|event| matches!(
            event,
            ServerEvent::Error { session_id: Some(id), error, .. }
                if id == "nope" && error == "session not found"
        )));
    }

    #[tokio::test]
    async fn resize_to_zero_is_refused() {
        let (session, senders, conns) = shown_by(1, b"").await;
        conns[0].handle(&resize("sess-1", 0, 30)).await;
        assert!(session.resizes.lock().unwrap().is_empty());
        assert!(senders[0].events().iter().any(|event| matches!(
            event,
            ServerEvent::Error { error, .. } if error == "invalid size"
        )));
    }

    #[tokio::test]
    async fn an_unchanged_size_is_answered_and_others_learn_a_new_one() {
        let (session, senders, conns) = shown_by(2, b"").await;
        conns[0].handle(&resize("sess-1", 120, 40)).await;
        conns[1].handle(&resize("sess-1", 100, 30)).await;
        assert_eq!(session.resizes.lock().unwrap().last(), Some(&(100, 30)));
        assert!(
            wait_until(Duration::from_secs(5), || sizes(&senders[0], "sess-1")
                .last()
                == Some(&(100, 30)))
            .await,
            "A learns B's smaller size; events: {:?}",
            senders[0].events()
        );

        let before = sizes(&senders[0], "sess-1").len();
        conns[0].handle(&resize("sess-1", 120, 40)).await;
        let answers = sizes(&senders[0], "sess-1");
        assert_eq!(answers.len(), before + 1, "answered right away");
        assert_eq!(answers.last(), Some(&(100, 30)));
        assert_eq!(session.resizes.lock().unwrap().last(), Some(&(100, 30)));
    }

    #[tokio::test]
    async fn redraw_without_a_size_change_nudges_the_pty() {
        let (session, senders, conns) = shown_by(2, b"").await;
        conns[0].handle(&resize("sess-1", 120, 40)).await;
        conns[1].handle(&resize("sess-1", 100, 30)).await;
        session.resizes.lock().unwrap().clear();

        conns[0]
            .handle(&Request {
                redraw: true,
                ..resize("sess-1", 120, 40)
            })
            .await;
        assert_eq!(*session.resizes.lock().unwrap(), [(100, 31), (100, 30)]);
        tokio::time::sleep(Duration::from_millis(50)).await;
        for sender in &senders {
            assert!(!sizes(sender, "sess-1").contains(&(100, 31)));
        }
    }

    #[tokio::test]
    async fn an_attach_gets_the_size_before_the_replay() {
        let (_session, _senders, conns) = shown_by(1, b"replayed").await;
        conns[0].handle(&resize("sess-1", 100, 30)).await;

        let sender = Arc::new(MockSender::new());
        let late = conns[0].dispatcher.new_connection(sender.clone());
        late.handle(&attach("sess-1")).await;
        let order = sender.order();
        let size = order.iter().position(|kind| kind == "size");
        let replay = order.iter().position(|kind| kind == "binary");
        assert!(size.is_some() && size < replay, "order: {order:?}");
        assert_eq!(sizes(&sender, "sess-1"), [(100, 30)]);
    }

    #[tokio::test]
    async fn create_records_its_size_and_sends_it_after_created() {
        let backend = Arc::new(MockBackend::new());
        backend.set_next_id("sess-1");
        let dispatcher = Arc::new(Dispatcher::new(backend.clone()));
        let sender = Arc::new(MockSender::new());
        let conn = dispatcher.new_connection(sender.clone());
        conn.handle(&Request {
            msg_type: MSG_CREATE.to_string(),
            cols: 90,
            rows: 20,
            ..Default::default()
        })
        .await;
        let order = sender.order();
        assert_eq!(order.first().map(String::as_str), Some("created"));
        assert_eq!(sizes(&sender, "sess-1"), [(90, 20)]);
    }

    #[tokio::test]
    async fn a_client_that_disconnects_gives_the_size_back() {
        let (session, _senders, conns) = shown_by(2, b"").await;
        conns[0].handle(&resize("sess-1", 120, 40)).await;
        conns[1].handle(&resize("sess-1", 100, 30)).await;
        conns[1].close();
        assert_eq!(session.resizes.lock().unwrap().last(), Some(&(120, 40)));
    }

    #[tokio::test]
    async fn a_hidden_or_detached_session_gives_the_size_back() {
        let (session, _senders, conns) = shown_by(2, b"").await;
        conns[0].handle(&resize("sess-1", 120, 40)).await;
        conns[1].handle(&resize("sess-1", 100, 30)).await;
        conns[1]
            .handle(&Request {
                msg_type: MSG_HIDE.to_string(),
                id: "sess-1".to_string(),
                ..Default::default()
            })
            .await;
        assert_eq!(session.resizes.lock().unwrap().last(), Some(&(120, 40)));

        conns[1].handle(&resize("sess-1", 100, 30)).await;
        assert_eq!(session.resizes.lock().unwrap().last(), Some(&(100, 30)));
        conns[1].handle(&detach("sess-1")).await;
        assert_eq!(session.resizes.lock().unwrap().last(), Some(&(120, 40)));
    }

    #[tokio::test]
    async fn takeover_and_idle_messages_reach_the_arbiter() {
        let (session, _senders, conns) = shown_by(2, b"").await;
        conns[0].handle(&resize("sess-1", 120, 40)).await;
        conns[1].handle(&resize("sess-1", 100, 30)).await;
        conns[0]
            .handle(&Request {
                takeover: true,
                ..activity()
            })
            .await;
        assert_eq!(session.resizes.lock().unwrap().last(), Some(&(120, 40)));
        conns[1].handle(&activity()).await;
        assert_eq!(session.resizes.lock().unwrap().last(), Some(&(100, 30)));
        conns[1]
            .handle(&Request {
                msg_type: MSG_IDLE.to_string(),
                ..Default::default()
            })
            .await;
        assert_eq!(session.resizes.lock().unwrap().last(), Some(&(120, 40)));
    }

    // ---- Real PTYs ----

    use crate::pty::session::Session;
    use crate::pty::testing::{pty_available, sh_opts, wait_until};

    /// Real PTY sessions running exactly the requested argv. `Manager` would
    /// resolve a shell profile instead.
    struct PtyBackend {
        sessions: Mutex<HashMap<String, Arc<dyn SessionHandle>>>,
        /// When set, `create` returns only once the new session has printed
        /// this, so the output predates any subscription.
        settle_on: Option<&'static str>,
    }

    impl PtyBackend {
        fn new() -> Self {
            Self {
                sessions: Mutex::new(HashMap::new()),
                settle_on: None,
            }
        }
    }

    #[async_trait::async_trait]
    impl Backend for PtyBackend {
        async fn create(
            &self,
            opts: CreateSessionOpts,
        ) -> Result<Arc<dyn SessionHandle>, PtyError> {
            let session: Arc<dyn SessionHandle> = Session::new(opts)?;
            if let Some(marker) = self.settle_on {
                let printed = wait_until(Duration::from_secs(5), || {
                    String::from_utf8_lossy(&session.scrollback()).contains(marker)
                })
                .await;
                assert!(printed, "session never printed {marker:?}");
            }
            self.sessions
                .lock()
                .unwrap()
                .insert(session.session_id().to_string(), Arc::clone(&session));
            Ok(session)
        }

        fn get(&self, id: &str) -> Option<Arc<dyn SessionHandle>> {
            self.sessions.lock().unwrap().get(id).cloned()
        }

        fn close(&self, id: &str) -> Result<(), PtyError> {
            let session = self
                .sessions
                .lock()
                .unwrap()
                .remove(id)
                .ok_or_else(|| PtyError::SessionNotFound(id.to_string()))?;
            session.close().map_err(PtyError::WriteFailed)
        }

        fn close_all(&self) {
            for (_, session) in self.sessions.lock().unwrap().drain() {
                let _ = session.close();
            }
        }

        fn sessions(&self) -> Vec<Arc<dyn SessionHandle>> {
            self.sessions.lock().unwrap().values().cloned().collect()
        }
    }

    fn attach(id: &str) -> Request {
        Request {
            msg_type: MSG_ATTACH.to_string(),
            id: id.to_string(),
            ..Default::default()
        }
    }

    fn detach(id: &str) -> Request {
        Request {
            msg_type: MSG_DETACH.to_string(),
            id: id.to_string(),
            ..Default::default()
        }
    }

    fn closed_events(sender: &MockSender) -> Vec<ServerEvent> {
        sender
            .events()
            .into_iter()
            .filter(|e| matches!(e, ServerEvent::SessionClosed { .. }))
            .collect()
    }

    const TICKER: &str = "while :; do echo tick; sleep 0.1; done";

    /// Detaching and attaching again must not end a running session's output.
    /// The PTY reader used to be dropped with the first stream, so the second
    /// attach got `closed` while the process was still running.
    #[tokio::test]
    async fn reattach_keeps_a_real_pty_streaming() {
        if !pty_available() {
            return;
        }
        let backend = Arc::new(PtyBackend::new());
        backend
            .create(sh_opts("reattach", TICKER))
            .await
            .expect("spawn sh");
        let dispatcher = Arc::new(Dispatcher::new(backend.clone()));
        let sender = Arc::new(MockSender::new());
        let conn = dispatcher.new_connection(sender.clone());

        conn.handle(&attach("reattach")).await;
        let first = wait_until(Duration::from_secs(5), || {
            !sender.binary_chunks().is_empty()
        })
        .await;
        assert!(
            first,
            "no output on the first attach; events: {:?}",
            sender.events()
        );

        conn.handle(&detach("reattach")).await;
        conn.handle(&attach("reattach")).await;
        let after_replay = sender.binary_chunks().len();

        wait_until(Duration::from_secs(5), || {
            sender.binary_chunks().len() > after_replay || !closed_events(&sender).is_empty()
        })
        .await;
        let closed = closed_events(&sender);
        assert!(
            closed.is_empty(),
            "`closed` while the process is running: {closed:?}"
        );
        assert!(
            sender.binary_chunks().len() > after_replay,
            "no live output after re-attach"
        );

        conn.close();
        backend.close_all();
    }

    fn input(id: &str, data: &[u8]) -> Request {
        Request {
            msg_type: MSG_INPUT.to_string(),
            id: id.to_string(),
            data: data.to_vec(),
            ..Default::default()
        }
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// Output a session printed before the creating connection subscribed is
    /// delivered to that connection, after `created`.
    #[tokio::test]
    async fn create_delivers_output_printed_before_subscribing() {
        if !pty_available() {
            return;
        }
        let backend = Arc::new(PtyBackend {
            settle_on: Some("early-bird"),
            ..PtyBackend::new()
        });
        let dispatcher = Arc::new(Dispatcher::new(backend.clone()));
        let sender = Arc::new(MockSender::new());
        let conn = dispatcher.new_connection(sender.clone());

        conn.handle(&Request {
            msg_type: MSG_CREATE.to_string(),
            shell: "/bin/sh".to_string(),
            command: "echo early-bird; sleep 5".to_string(),
            restore_mode: "close".to_string(),
            ..Default::default()
        })
        .await;

        let sid = match sender.events().first() {
            Some(ServerEvent::SessionCreated { session_id, .. }) => session_id.clone(),
            other => panic!("expected created first, got {other:?}"),
        };
        assert!(
            text(&sender.output(&sid)).contains("early-bird"),
            "output: {:?}",
            text(&sender.output(&sid))
        );
        assert_eq!(sender.order().first().map(String::as_str), Some("created"));

        conn.close();
        backend.close_all();
    }

    /// terminal-sessions, Reconnect keeps the session streaming: a connection
    /// goes away, a new one attaches, and the session keeps streaming.
    #[tokio::test]
    async fn reconnect_keeps_the_session_streaming() {
        if !pty_available() {
            return;
        }
        let backend = Arc::new(PtyBackend::new());
        backend
            .create(sh_opts("reconnect", TICKER))
            .await
            .expect("spawn sh");
        let dispatcher = Arc::new(Dispatcher::new(backend.clone()));

        let first = Arc::new(MockSender::new());
        let mut conn = dispatcher.new_connection(first.clone());
        conn.mux_managed = true; // as the WebSocket layer runs it: sessions outlive connections
        conn.handle(&attach("reconnect")).await;
        assert!(
            wait_until(Duration::from_secs(5), || !first.binary_chunks().is_empty()).await,
            "no output"
        );
        conn.close();

        let second = Arc::new(MockSender::new());
        let mut conn = dispatcher.new_connection(second.clone());
        conn.mux_managed = true;
        conn.handle(&attach("reconnect")).await;
        let after_replay = second.binary_chunks().len();

        let live = wait_until(Duration::from_secs(5), || {
            second.binary_chunks().len() > after_replay
        })
        .await;
        assert!(
            live,
            "no live output after reconnecting; events: {:?}",
            second.events()
        );
        for sender in [&first, &second] {
            let closed = closed_events(sender);
            assert!(
                closed.is_empty(),
                "`closed` while the process is running: {closed:?}"
            );
        }

        conn.close();
        backend.close_all();
    }

    /// terminal-sessions, Two connections stream one session: both receive
    /// the line the process prints.
    #[tokio::test]
    async fn two_connections_stream_one_session() {
        if !pty_available() {
            return;
        }
        let backend = Arc::new(PtyBackend::new());
        backend
            .create(sh_opts("shared", "read x; echo \"line:$x\"; sleep 5"))
            .await
            .expect("spawn sh");
        let dispatcher = Arc::new(Dispatcher::new(backend.clone()));
        let senders = [Arc::new(MockSender::new()), Arc::new(MockSender::new())];
        let conns: Vec<_> = senders
            .iter()
            .map(|s| dispatcher.new_connection(s.clone()))
            .collect();
        for conn in &conns {
            conn.handle(&attach("shared")).await;
        }

        conns[0].handle(&input("shared", b"hi\n")).await;

        for (i, sender) in senders.iter().enumerate() {
            let got = wait_until(Duration::from_secs(5), || {
                text(&sender.output("shared")).contains("line:hi")
            })
            .await;
            assert!(
                got,
                "connection {i} missed the line; output: {:?}",
                text(&sender.output("shared"))
            );
        }

        for conn in &conns {
            conn.close();
        }
        backend.close_all();
    }

    /// A failing process yields `closed` with a non-zero exit code, although
    /// its output ends before its exit status is known. Repeated, because the
    /// two used to race.
    #[tokio::test]
    async fn closed_carries_the_exit_status_of_a_failed_process() {
        if !pty_available() {
            return;
        }
        let backend = Arc::new(PtyBackend::new());
        let dispatcher = Arc::new(Dispatcher::new(backend.clone()));

        for run in 0..20 {
            let id = format!("exit-3-{run}");
            backend
                .create(sh_opts(&id, "exit 3"))
                .await
                .expect("spawn sh");
            let sender = Arc::new(MockSender::new());
            let conn = dispatcher.new_connection(sender.clone());
            conn.handle(&attach(&id)).await;

            assert!(
                wait_until(Duration::from_secs(5), || !closed_events(&sender)
                    .is_empty())
                .await,
                "run {run}: no `closed`"
            );
            match &closed_events(&sender)[..] {
                [ServerEvent::SessionClosed {
                    exit_code: Some(code),
                    ..
                }] => {
                    assert_eq!(*code, 3, "run {run}: `closed` lost the actual exit code")
                }
                other => {
                    panic!("run {run}: expected one `closed` with an exit code, got {other:?}")
                }
            }
        }
        backend.close_all();
    }

    #[tokio::test]
    async fn direct_process_stream_drains_before_reporting_the_exact_result() {
        if !pty_available() {
            return;
        }
        for code in [0, 7, 23] {
            let mut opts = sh_opts("direct-exit", "");
            opts.dir = std::env::temp_dir().to_string_lossy().into_owned();
            let session = crate::pty::session::Session::new_program(
                opts,
                vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    format!("printf final-output; exit {code}"),
                ],
            )
            .expect("direct process");
            let sender = Arc::new(MockSender::new());
            let mut sub = session.subscribe();
            sender
                .send_binary(session.session_id(), &std::mem::take(&mut sub.snapshot))
                .expect("replay");
            stream_output(session, sub, sender.clone(), CancellationToken::new(), None).await;
            assert!(text(&sender.output("direct-exit")).contains("final-output"));
            assert!(matches!(closed_events(&sender).as_slice(),
                [ServerEvent::SessionClosed {exit_code: Some(actual), signal: None, ..}] if *actual == code));
        }
    }

    /// One exit, one `session.exited` notification: however many streams see
    /// the exit, including one attached after it.
    #[tokio::test]
    async fn session_exit_is_reported_once() {
        if !pty_available() {
            return;
        }
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let collected = notifications.clone();
        let engine =
            NotificationEngine::new(Box::new(move |event| collected.lock().unwrap().push(event)));

        let backend = Arc::new(PtyBackend::new());
        backend
            .create(sh_opts("exits", "sleep 0.5; exit 1"))
            .await
            .expect("spawn sh");
        let mut dispatcher = Dispatcher::new(backend.clone());
        dispatcher.notify = Some(Arc::new(Mutex::new(engine)));
        let dispatcher = Arc::new(dispatcher);

        let senders = [
            Arc::new(MockSender::new()),
            Arc::new(MockSender::new()),
            Arc::new(MockSender::new()),
        ];
        let conns: Vec<_> = senders
            .iter()
            .map(|s| dispatcher.new_connection(s.clone()))
            .collect();
        conns[0].handle(&attach("exits")).await;
        conns[1].handle(&attach("exits")).await;
        for sender in &senders[..2] {
            assert!(
                wait_until(Duration::from_secs(5), || !closed_events(sender).is_empty()).await,
                "no `closed`"
            );
        }
        // A stream started after the exit sees it as well.
        conns[2].handle(&attach("exits")).await;
        assert!(
            wait_until(Duration::from_secs(5), || !closed_events(&senders[2])
                .is_empty())
            .await,
            "no `closed`"
        );

        let exits = notifications
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, ServerEvent::SessionExited { .. }))
            .count();
        assert_eq!(
            exits,
            1,
            "notifications: {:?}",
            notifications.lock().unwrap()
        );
        backend.close_all();
    }
}
