//! Production PTY backend: a session registry implementing the `Backend` trait.
//!
//! `Manager` stores active sessions in a `HashMap` behind a `Mutex` and starts
//! each session on a terminal of its factory: in this process by default, in
//! session keepers for a daemon.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock, Weak};

use async_trait::async_trait;

use super::session::Session;
use super::terminal::{Feed, LocalTerminals, Terminal, TerminalFactory};
use super::{Backend, CreateSessionOpts, PtyError, SessionHandle};
use crate::shell_profiles::{state_from_settings, ShellProfilesState, ShellSettings};

/// Session registry that manages the lifecycle of terminal sessions.
pub struct Manager {
    sessions: Mutex<HashMap<String, Arc<dyn SessionHandle>>>,
    // Closed panes can still own live processes during signal escalation.
    // Always lock sessions before retiring when transferring ownership.
    retiring: Mutex<Vec<Arc<dyn SessionHandle>>>,
    shell_profiles: Arc<RwLock<ShellProfilesState>>,
    creating: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    terminals: Arc<dyn TerminalFactory>,
    ending: std::sync::atomic::AtomicBool,
    lifecycle: tokio::sync::RwLock<()>,
}

impl Manager {
    pub fn new() -> Self {
        let state = state_from_settings(&ShellSettings::default());
        Self::with_shell_profiles(Arc::new(RwLock::new(state)))
    }

    pub fn with_shell_profiles(shell_profiles: Arc<RwLock<ShellProfilesState>>) -> Self {
        Self::with_terminals(shell_profiles, Arc::new(LocalTerminals))
    }

    /// A manager whose sessions start on terminals of `terminals`.
    pub fn with_terminals(
        shell_profiles: Arc<RwLock<ShellProfilesState>>,
        terminals: Arc<dyn TerminalFactory>,
    ) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            retiring: Mutex::new(Vec::new()),
            shell_profiles,
            creating: Mutex::new(HashMap::new()),
            terminals,
            ending: std::sync::atomic::AtomicBool::new(false),
            lifecycle: tokio::sync::RwLock::new(()),
        }
    }

    /// Register a session around a terminal that already runs (a keeper a
    /// starting daemon adopts), fed by it from `start` on. A session of that
    /// id that is already registered is kept, and the terminal is not taken.
    pub fn adopt(
        &self,
        id: &str,
        managed: bool,
        start: impl FnOnce(Arc<dyn Feed>) -> Result<Box<dyn Terminal>, PtyError>,
    ) -> Result<Arc<dyn SessionHandle>, PtyError> {
        let _active = self
            .lifecycle
            .try_read()
            .map_err(|_| PtyError::Other("terminal service is ending".into()))?;
        self.check_active()?;
        if let Some(existing) = self.get(id) {
            return Ok(existing);
        }
        let session: Arc<dyn SessionHandle> = Session::with_terminal(
            id.to_owned(),
            None,
            managed,
            super::session::scrollback_size(),
            start,
        )?;
        self.sessions
            .lock()
            .unwrap()
            .insert(id.to_owned(), Arc::clone(&session));
        Ok(session)
    }

    fn creation_lane(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut lanes = self.creating.lock().expect("session creation lanes");
        lanes.retain(|_, lane| lane.strong_count() > 0);
        if let Some(lane) = lanes.get(id).and_then(Weak::upgrade) {
            return lane;
        }
        let lane = Arc::new(tokio::sync::Mutex::new(()));
        lanes.insert(id.to_owned(), Arc::downgrade(&lane));
        lane
    }

    fn check_active(&self) -> Result<(), PtyError> {
        if self.ending.load(std::sync::atomic::Ordering::Acquire) {
            Err(PtyError::Other("terminal service is ending".into()))
        } else {
            Ok(())
        }
    }

    /// Seal creation before waiting for already admitted spawns. In particular,
    /// a spawn cannot register a new child just after close_all drained the map.
    pub async fn shutdown(&self, preserve: bool) -> std::io::Result<()> {
        self.ending
            .store(true, std::sync::atomic::Ordering::Release);
        let _exclusive = self.lifecycle.write().await;
        if preserve {
            self.release_all();
            return Ok(());
        }
        let sessions = self.take_all();
        let mut error = None;
        // Signal every lifecycle before awaiting any one of them. A failed
        // termination must not prevent the other owned children being reaped.
        for session in &sessions {
            if let Err(reason) = session.begin_shutdown() {
                error.get_or_insert(reason);
            }
        }
        for session in sessions {
            if let Err(reason) = session.shutdown().await {
                error.get_or_insert(reason);
            }
        }
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn take_all(&self) -> Vec<Arc<dyn SessionHandle>> {
        let mut sessions = self.sessions.lock().expect("sessions");
        let mut retiring = self.retiring.lock().expect("retiring sessions");
        let mut all: Vec<_> = sessions.drain().map(|(_, session)| session).collect();
        all.extend(retiring.drain(..));
        all
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Backend for Manager {
    async fn create_program(
        &self,
        mut opts: CreateSessionOpts,
        argv: Vec<String>,
    ) -> Result<Arc<dyn SessionHandle>, PtyError> {
        let _active = self.lifecycle.read().await;
        self.check_active()?;
        if opts.id.is_empty() {
            opts.id = super::session::generate_id();
        }
        let _creation = self.creation_lane(&opts.id).lock_owned().await;
        if !opts.id.is_empty() {
            if let Some(existing) = self.sessions.lock().unwrap().get(&opts.id) {
                return Ok(Arc::clone(existing));
            }
        }
        let session: Arc<dyn SessionHandle> =
            Session::start(opts, Some(argv), self.terminals.as_ref())?;
        self.sessions
            .lock()
            .unwrap()
            .insert(session.session_id().to_string(), Arc::clone(&session));
        Ok(session)
    }

    async fn create(
        &self,
        mut opts: CreateSessionOpts,
    ) -> Result<Arc<dyn SessionHandle>, PtyError> {
        let _active = self.lifecycle.read().await;
        self.check_active()?;
        if opts.id.is_empty() {
            opts.id = super::session::generate_id();
        }
        let _creation = self.creation_lane(&opts.id).lock_owned().await;
        // Get-or-create: if a session with this ID already exists, return it.
        // This happens when the mux effect runner spawns the PTY and the frontend
        // also sends a `create` message for the same pane — both resolve to the
        // same session_id.  Returning the existing session makes the duplicate harmless.
        if !opts.id.is_empty() {
            let sessions = self.sessions.lock().unwrap();
            if let Some(existing) = sessions.get(&opts.id) {
                return Ok(Arc::clone(existing));
            }
        }

        let mut opts = opts;
        let requested_profile_id =
            (!opts.shell_profile_id.is_empty()).then_some(opts.shell_profile_id.clone());
        let resolved = self
            .shell_profiles
            .read()
            .unwrap()
            .resolve(
                (!opts.shell_profile_id.is_empty()).then_some(opts.shell_profile_id.as_str()),
                (!opts.shell.is_empty()).then_some(opts.shell.as_str()),
            )
            .map_err(|e| PtyError::SpawnFailed(e.to_string()))?;
        if requested_profile_id.as_deref() != resolved.id.as_deref()
            && requested_profile_id.is_some()
        {
            tracing::warn!(
                requested = requested_profile_id.as_deref().unwrap_or_default(),
                fallback = resolved.id.as_deref().unwrap_or_default(),
                reason = resolved.fallback_reason.as_deref().unwrap_or_default(),
                "shell profile unavailable; using configured default"
            );
        }
        opts.shell_profile_id = resolved.id.unwrap_or_default();
        opts.shell = resolved.command;
        opts.shell_args = resolved.args;

        let session = Session::start(opts, None, self.terminals.as_ref())?;
        if let Some(reason) = resolved.fallback_reason {
            session.set_shell_profile_fallback_reason(reason);
        }
        let id = session.session_id().to_string();
        let handle: Arc<dyn SessionHandle> = session;
        self.sessions
            .lock()
            .unwrap()
            .insert(id, Arc::clone(&handle));
        Ok(handle)
    }

    fn get(&self, id: &str) -> Option<Arc<dyn SessionHandle>> {
        self.sessions.lock().unwrap().get(id).cloned()
    }

    fn close(&self, id: &str) -> Result<(), PtyError> {
        let session = {
            let mut sessions = self.sessions.lock().expect("sessions");
            let mut retiring = self.retiring.lock().expect("retiring sessions");
            retiring.retain(|session| !*session.done().borrow());
            let session = sessions
                .remove(id)
                .ok_or_else(|| PtyError::SessionNotFound(id.to_string()))?;
            retiring.push(session.clone());
            session
        };
        session.close().map_err(PtyError::WriteFailed)?;
        Ok(())
    }

    fn close_all(&self) {
        let sessions: Vec<_> = {
            let mut sessions = self.sessions.lock().expect("sessions");
            let mut retiring = self.retiring.lock().expect("retiring sessions");
            retiring.retain(|session| !*session.done().borrow());
            let closing: Vec<_> = sessions.drain().map(|(_, session)| session).collect();
            retiring.extend(closing.iter().cloned());
            closing
        };
        for session in sessions {
            let _ = session.close();
        }
    }

    fn release_all(&self) {
        for session in self.take_all() {
            session.release();
            // Closes what could not be released; a no-op for the rest.
            let _ = session.close();
        }
    }

    fn sessions(&self) -> Vec<Arc<dyn SessionHandle>> {
        self.sessions.lock().unwrap().values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unavailable_profile_spawns_default_and_reports_reason() {
        use crate::pty::testing::{pty_available, sh_opts};
        use crate::shell_profiles::{ShellProfile, ShellProfileSource, ShellTarget};
        if !pty_available() {
            return;
        }
        let default = ShellProfile {
            id: "default".into(),
            name: "Default".into(),
            command: "/bin/sh".into(),
            args: vec!["-c".into(), "printf ready".into()],
            enabled: true,
            target: ShellTarget::Local,
            source: ShellProfileSource::Custom,
            available: true,
            reason: None,
            experimental: false,
            detected_defaults: None,
        };
        let missing = ShellProfile {
            id: "unavailable".into(),
            available: false,
            ..default.clone()
        };
        let profiles = ShellProfilesState {
            default_profile_id: default.id.clone(),
            profiles: vec![default, missing],
            ..ShellProfilesState::default()
        };
        let manager = Manager::with_shell_profiles(Arc::new(RwLock::new(profiles)));
        let mut opts = sh_opts("fallback-session", "");
        opts.shell_profile_id = "unavailable".into();
        let session = manager.create(opts).await.unwrap();
        assert_eq!(session.shell_profile_id(), Some("default"));
        assert_eq!(session.shell_profile_fallback_reason(), Some("unavailable"));
        manager.close_all();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn service_shutdown_waits_for_an_admitted_spawn_and_refuses_late_creates() {
        use crate::pty::{
            terminal::SessionSpec,
            testing::{pty_available, sh_opts},
        };
        use std::time::Duration;
        if !pty_available() {
            return;
        }
        struct Paused {
            entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            release: Mutex<std::sync::mpsc::Receiver<()>>,
        }
        impl TerminalFactory for Paused {
            fn start(
                &self,
                spec: &SessionSpec,
                feed: Arc<dyn Feed>,
            ) -> Result<Box<dyn Terminal>, PtyError> {
                self.entered
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(())
                    .unwrap();
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                LocalTerminals.start(spec, feed)
            }
        }
        let (entered, wait) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let manager = Arc::new(Manager::with_terminals(
            Arc::new(RwLock::new(state_from_settings(&ShellSettings::default()))),
            Arc::new(Paused {
                entered: Mutex::new(Some(entered)),
                release: Mutex::new(released),
            }),
        ));
        let creating = {
            let manager = manager.clone();
            tokio::spawn(
                async move { manager.create(sh_opts("racing", "sleep 30")).await.unwrap() },
            )
        };
        wait.await.unwrap();
        let shutdown = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.shutdown(false).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !shutdown.is_finished(),
            "an in-flight spawn is still owned until it can be closed"
        );
        release.send(()).unwrap();
        let session = creating.await.unwrap();
        shutdown.await.unwrap().unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            session.done().wait_for(|done| *done),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(manager.sessions().is_empty());
        assert!(manager.create(sh_opts("late", "sleep 30")).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn simultaneous_creates_of_one_id_start_exactly_one_program() {
        use crate::pty::testing::{pty_available, sh_opts, wait_until};
        use std::time::Duration;
        if !pty_available() {
            return;
        }
        let manager = Arc::new(Manager::new());
        let directory = tempfile::tempdir().expect("cwd");
        let barrier = Arc::new(tokio::sync::Barrier::new(16));
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let manager = manager.clone();
            let barrier = barrier.clone();
            let mut opts = sh_opts("same-job", "");
            opts.dir = directory.path().to_string_lossy().into_owned();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                manager
                    .create_program(
                        opts,
                        vec![
                            "/bin/sh".into(),
                            "-c".into(),
                            "echo one >> starts; printf ready; read line".into(),
                        ],
                    )
                    .await
                    .expect("create")
            }));
        }
        let mut handles = Vec::new();
        for task in tasks {
            handles.push(task.await.expect("task"));
        }
        assert!(handles
            .iter()
            .all(|handle| Arc::ptr_eq(handle, &handles[0])));
        assert_eq!(manager.sessions().len(), 1);
        assert!(
            wait_until(Duration::from_secs(5), || !handles[0]
                .scrollback()
                .is_empty())
            .await
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("starts")).expect("starts"),
            "one\n"
        );
        handles[0].write(b"done\n").await.expect("finish");
        assert!(wait_until(Duration::from_secs(5), || *handles[0].done().borrow()).await);
        manager.close_all();
    }

    #[tokio::test]
    async fn test_manager_crud() {
        if !crate::pty::testing::pty_available() {
            return;
        }
        let mgr = Manager::new();

        let opts = CreateSessionOpts {
            id: "mgr-test-1".to_string(),
            shell_profile_id: String::new(),
            shell: String::new(),
            shell_args: vec![],
            cols: 80,
            rows: 24,
            dir: String::new(),
            command: String::new(),
            restore_mode: String::new(),
            extra_env: vec![],
        };

        let session = mgr.create(opts).await.expect("create failed");
        assert_eq!(session.session_id(), "mgr-test-1");

        // Get
        assert!(mgr.get("mgr-test-1").is_some());
        assert!(mgr.get("nonexistent").is_none());

        // Close
        mgr.close("mgr-test-1").expect("close failed");
        assert!(mgr.get("mgr-test-1").is_none());

        // Close nonexistent
        assert!(mgr.close("nonexistent").is_err());
    }
}
