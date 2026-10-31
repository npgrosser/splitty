//! Executes side effects (PTY spawn/kill) produced by the mux reducer.
//!
//! The `EffectRunner` bridges the pure reducer world with the async PTY backend.
//! It processes all effects in order, collecting errors without aborting early.

use std::sync::Arc;

use crate::pty::{Backend, CreateSessionOpts, PtyError};

use super::Effect;

/// What a spawned shell learns about its Splitty server.
///
/// Formerly `server_addr` + `token`, i.e. a network address and the master
/// credential for it (security review SR-05). Now a path to the
/// local control socket, which is not usable from anywhere but this machine and
/// reaches only multiplexer operations.
#[derive(Clone, Debug, Default)]
pub struct EnvContext {
    pub control_socket: String,
}

/// Executes side effects produced by the reducer against a PTY backend.
pub struct EffectRunner {
    pty_backend: Arc<dyn Backend>,
}

/// Errors that can occur during effect execution.
#[derive(Debug, thiserror::Error)]
pub enum EffectError {
    #[error("PTY error: {0}")]
    Pty(#[from] PtyError),
}

/// An outcome retains its target even on failure; callers must not announce
/// sessions merely because a spawn was requested by the reducer.
#[derive(Debug)]
pub struct EffectOutcome {
    pub effect: Effect,
    pub result: Result<(), EffectError>,
}

impl EffectOutcome {
    pub fn spawned(
        &self,
    ) -> Option<(
        crate::ids::ScreenId,
        crate::ids::PaneId,
        crate::ids::SessionId,
    )> {
        if self.result.is_err() {
            return None;
        }
        match &self.effect {
            Effect::SpawnSession {
                screen_id,
                pane_id,
                session_id,
                ..
            } => Some((screen_id.clone(), pane_id.clone(), session_id.clone())),
            Effect::SpawnJob { job } => Some((
                job.screen_id.clone(),
                job.pane_id.clone(),
                job.session_id.clone(),
            )),
            Effect::KillSession { .. } => None,
        }
    }
}

impl EffectRunner {
    /// Create a new effect runner backed by the given PTY backend.
    pub fn new(pty_backend: Arc<dyn Backend>) -> Self {
        Self { pty_backend }
    }

    /// Execute a batch of effects, returning every target and its actual outcome.
    ///
    /// All effects are attempted even if some fail; errors are collected and returned.
    pub async fn execute(&self, effects: Vec<Effect>, env: &EnvContext) -> Vec<EffectOutcome> {
        let mut outcomes = Vec::with_capacity(effects.len());

        for effect in effects {
            let result = self.execute_one(effect.clone(), env).await;
            outcomes.push(EffectOutcome { effect, result });
        }

        outcomes
    }

    async fn execute_one(&self, effect: Effect, env: &EnvContext) -> Result<(), EffectError> {
        match effect {
            Effect::SpawnJob { job } => {
                let socket_key = if job.owner.is_some() {
                    "SPLITTY_AGENT_SOCK"
                } else {
                    "SPLITTY_SOCK"
                };
                let mut extra_env = vec![
                    format!("SPLITTY_PANE_ID={}", job.pane_id),
                    format!("SPLITTY_SCREEN_ID={}", job.screen_id),
                ];
                if !env.control_socket.is_empty() {
                    extra_env.push(format!("{socket_key}={}", env.control_socket));
                }
                self.pty_backend
                    .create_program(
                        CreateSessionOpts {
                            id: job.session_id.to_string(),
                            shell_profile_id: String::new(),
                            shell: String::new(),
                            shell_args: vec![],
                            cols: 80,
                            rows: 24,
                            dir: job.launch.cwd,
                            command: String::new(),
                            restore_mode: String::new(),
                            extra_env,
                        },
                        job.launch.argv,
                    )
                    .await?;
                Ok(())
            }
            Effect::SpawnSession {
                session_id,
                pane_id,
                screen_id,
                command,
                cwd,
                shell_profile_id,
                shell,
            } => {
                // No SPLITTY_TOKEN and no SPLITTY_SERVER here any more
                // (security review SR-05): a process in a pane
                // used to inherit the master credential for the HTTP API, which
                // it could exfiltrate and which also authorised configuration
                // and shell-profile writes. It now gets a socket path instead.
                let mut extra_env = vec![
                    format!("SPLITTY_PANE_ID={pane_id}"),
                    format!("SPLITTY_SCREEN_ID={screen_id}"),
                ];
                if !env.control_socket.is_empty() {
                    extra_env.push(format!("SPLITTY_SOCK={}", env.control_socket));
                }

                let opts = CreateSessionOpts {
                    id: session_id.to_string(),
                    shell_profile_id: shell_profile_id.unwrap_or_default(),
                    shell: shell.unwrap_or_default(),
                    shell_args: vec![],
                    cols: 80,
                    rows: 24,
                    dir: cwd.unwrap_or_default(),
                    command: command.unwrap_or_default(),
                    restore_mode: String::new(),
                    extra_env,
                };

                self.pty_backend
                    .create(opts)
                    .await
                    .map_err(EffectError::Pty)?;
                Ok(())
            }
            Effect::KillSession { session_id } => match self.pty_backend.close(session_id.as_str())
            {
                Ok(()) | Err(PtyError::SessionNotFound(_)) => Ok(()),
                Err(error) => Err(EffectError::Pty(error)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{PaneId, ScreenId, SessionId};
    use crate::pty::{SessionHandle, TitleInfo};
    use std::sync::Mutex;

    /// Records what the mock backend was asked to do.
    #[derive(Debug, Clone)]
    enum MockCall {
        Create {
            id: String,
            extra_env: Vec<String>,
        },
        Program {
            id: String,
            argv: Vec<String>,
            dir: String,
            extra_env: Vec<String>,
        },
        Close(String),
    }

    struct MockBackend {
        calls: Mutex<Vec<MockCall>>,
        create_error: Mutex<Option<PtyError>>,
        close_error: Mutex<Option<PtyError>>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                create_error: Mutex::new(None),
                close_error: Mutex::new(None),
            }
        }

        fn with_create_error(self, err: PtyError) -> Self {
            *self.create_error.lock().unwrap() = Some(err);
            self
        }

        fn with_close_error(self, err: PtyError) -> Self {
            *self.close_error.lock().unwrap() = Some(err);
            self
        }

        fn calls(&self) -> Vec<MockCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    /// Minimal stub session for the mock backend.
    struct StubSession {
        id: String,
        done_rx: tokio::sync::watch::Receiver<bool>,
    }

    impl StubSession {
        fn new(id: &str) -> (Self, tokio::sync::watch::Sender<bool>) {
            let (tx, rx) = tokio::sync::watch::channel(false);
            (
                Self {
                    id: id.to_string(),
                    done_rx: rx,
                },
                tx,
            )
        }
    }

    #[async_trait::async_trait]
    impl SessionHandle for StubSession {
        fn session_id(&self) -> &str {
            &self.id
        }
        fn subscribe(&self) -> crate::pty::Subscription {
            // The sender is dropped right away: no output, already ended.
            let (_tx, receiver) = tokio::sync::mpsc::unbounded_channel();
            crate::pty::Subscription {
                snapshot: Vec::new(),
                receiver,
            }
        }
        async fn write(&self, _data: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        fn resize(&self, _cols: u16, _rows: u16) -> std::io::Result<()> {
            Ok(())
        }
        fn close(&self) -> std::io::Result<()> {
            Ok(())
        }
        fn done(&self) -> tokio::sync::watch::Receiver<bool> {
            self.done_rx.clone()
        }
        fn exit_code(&self) -> i32 {
            0
        }
        fn title(&self) -> TitleInfo {
            TitleInfo::default()
        }
        fn osc_title_watch(&self) -> tokio::sync::watch::Receiver<crate::pty::OscTitleEvent> {
            tokio::sync::watch::channel(crate::pty::OscTitleEvent::default()).1
        }
        fn semantic_state(&self) -> crate::pty::SemanticState {
            crate::pty::SemanticState::default()
        }
        fn semantic_watch(&self) -> tokio::sync::watch::Receiver<crate::pty::SemanticEvent> {
            tokio::sync::watch::channel(None).1
        }
        fn bell_watch(&self) -> tokio::sync::watch::Receiver<crate::pty::BellEvent> {
            tokio::sync::watch::channel(None).1
        }
        fn scrollback(&self) -> Vec<u8> {
            Vec::new()
        }
        fn stats(&self) -> Option<crate::pty::SessionStats> {
            None
        }
        fn claim_exit_report(&self) -> bool {
            true
        }
    }

    #[async_trait::async_trait]
    impl Backend for MockBackend {
        async fn create_program(
            &self,
            opts: CreateSessionOpts,
            argv: Vec<String>,
        ) -> Result<Arc<dyn SessionHandle>, PtyError> {
            if let Some(error) = self.create_error.lock().unwrap().take() {
                return Err(error);
            }
            self.calls.lock().unwrap().push(MockCall::Program {
                id: opts.id.clone(),
                argv,
                dir: opts.dir,
                extra_env: opts.extra_env,
            });
            let (session, _) = StubSession::new(&opts.id);
            Ok(Arc::new(session))
        }
        async fn create(
            &self,
            opts: CreateSessionOpts,
        ) -> Result<Arc<dyn SessionHandle>, PtyError> {
            if let Some(err) = self.create_error.lock().unwrap().take() {
                return Err(err);
            }
            self.calls.lock().unwrap().push(MockCall::Create {
                id: opts.id.clone(),
                extra_env: opts.extra_env.clone(),
            });
            let (session, _tx) = StubSession::new(&opts.id);
            Ok(Arc::new(session))
        }

        fn get(&self, _id: &str) -> Option<Arc<dyn SessionHandle>> {
            None
        }

        fn close(&self, id: &str) -> Result<(), PtyError> {
            if let Some(err) = self.close_error.lock().unwrap().take() {
                return Err(err);
            }
            self.calls
                .lock()
                .unwrap()
                .push(MockCall::Close(id.to_string()));
            Ok(())
        }

        fn close_all(&self) {}

        fn sessions(&self) -> Vec<Arc<dyn SessionHandle>> {
            Vec::new()
        }
    }

    fn test_env() -> EnvContext {
        EnvContext {
            control_socket: "/run/user/1000/splitty/42.sock".to_string(),
        }
    }

    #[tokio::test]
    async fn spawn_session_calls_create_with_env_vars() {
        let backend = Arc::new(MockBackend::new());
        let runner = EffectRunner::new(backend.clone());
        let env = test_env();

        let effects = vec![Effect::SpawnSession {
            session_id: SessionId::from("sess-1"),
            pane_id: PaneId::from("pane-1"),
            screen_id: ScreenId::from("screen-1"),
            command: Some("/bin/bash".to_string()),
            cwd: Some("/home/user".to_string()),
            shell_profile_id: None,
            shell: None,
        }];

        let errors = runner.execute(effects, &env).await;
        assert!(
            errors.iter().all(|outcome| outcome.result.is_ok()),
            "expected no errors, got: {errors:?}"
        );
        assert_eq!(
            errors[0].spawned(),
            Some((
                ScreenId::from("screen-1"),
                PaneId::from("pane-1"),
                SessionId::from("sess-1")
            ))
        );

        let calls = backend.calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            MockCall::Create { id, extra_env } => {
                assert_eq!(id, "sess-1");
                assert!(extra_env.contains(&"SPLITTY_PANE_ID=pane-1".to_string()));
                assert!(extra_env.contains(&"SPLITTY_SCREEN_ID=screen-1".to_string()));
                assert!(
                    extra_env.contains(&"SPLITTY_SOCK=/run/user/1000/splitty/42.sock".to_string())
                );

                // SR-05: a spawned shell must not receive a credential for the
                // HTTP API. A process in a pane could exfiltrate one, and it
                // would authorise configuration and shell-profile writes on top
                // of the multiplexer operations this socket allows.
                for var in extra_env {
                    assert!(
                        !var.starts_with("SPLITTY_TOKEN="),
                        "no token may be inherited, found: {var}",
                    );
                    assert!(
                        !var.starts_with("SPLITTY_SERVER="),
                        "no network address may be inherited, found: {var}",
                    );
                }
            }
            other => panic!("expected Create call, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn kill_session_calls_close() {
        let backend = Arc::new(MockBackend::new());
        let runner = EffectRunner::new(backend.clone());
        let env = test_env();

        let effects = vec![Effect::KillSession {
            session_id: SessionId::from("sess-2"),
        }];

        let errors = runner.execute(effects, &env).await;
        assert!(errors.iter().all(|outcome| outcome.result.is_ok()));

        let calls = backend.calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            MockCall::Close(id) => assert_eq!(id, "sess-2"),
            other => panic!("expected Close call, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn multiple_effects_executed_in_order() {
        let backend = Arc::new(MockBackend::new());
        let runner = EffectRunner::new(backend.clone());
        let env = test_env();

        let effects = vec![
            Effect::SpawnSession {
                session_id: SessionId::from("s1"),
                pane_id: PaneId::from("p1"),
                screen_id: ScreenId::from("sc1"),
                command: None,
                cwd: None,
                shell_profile_id: None,
                shell: Some("/bin/zsh".to_string()),
            },
            Effect::KillSession {
                session_id: SessionId::from("s1"),
            },
        ];

        let errors = runner.execute(effects, &env).await;
        assert!(errors.iter().all(|outcome| outcome.result.is_ok()));

        let calls = backend.calls();
        assert_eq!(calls.len(), 2);
        assert!(matches!(&calls[0], MockCall::Create { id, .. } if id == "s1"));
        assert!(matches!(&calls[1], MockCall::Close(id) if id == "s1"));
    }

    #[tokio::test]
    async fn errors_collected_without_stopping_execution() {
        let backend = Arc::new(
            MockBackend::new().with_create_error(PtyError::SpawnFailed("oops".to_string())),
        );
        let runner = EffectRunner::new(backend.clone());
        let env = test_env();

        let effects = vec![
            Effect::SpawnSession {
                session_id: SessionId::from("s-fail"),
                pane_id: PaneId::from("p-fail"),
                screen_id: ScreenId::from("sc-fail"),
                command: None,
                cwd: None,
                shell_profile_id: None,
                shell: None,
            },
            Effect::KillSession {
                session_id: SessionId::from("s-ok"),
            },
        ];

        let errors = runner.execute(effects, &env).await;
        assert_eq!(errors.len(), 2);
        assert!(errors[0]
            .result
            .as_ref()
            .expect_err("spawn failed")
            .to_string()
            .contains("oops"));
        assert!(errors[0].spawned().is_none());
        assert!(errors[1].result.is_ok());

        let calls = backend.calls();
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], MockCall::Close(id) if id == "s-ok"));
    }

    #[tokio::test]
    async fn kill_session_error_is_reported() {
        let backend = Arc::new(
            MockBackend::new()
                .with_close_error(PtyError::Other("sess-gone: close denied".to_string())),
        );
        let runner = EffectRunner::new(backend.clone());
        let env = test_env();

        let effects = vec![Effect::KillSession {
            session_id: SessionId::from("sess-gone"),
        }];

        let errors = runner.execute(effects, &env).await;
        assert_eq!(errors.len(), 1);
        assert!(errors[0]
            .result
            .as_ref()
            .expect_err("close failed")
            .to_string()
            .contains("sess-gone"));
        assert!(backend.calls().is_empty());
    }

    #[tokio::test]
    async fn empty_effects_returns_no_errors() {
        let backend = Arc::new(MockBackend::new());
        let runner = EffectRunner::new(backend.clone());
        let env = test_env();

        let errors = runner.execute(vec![], &env).await;
        assert!(errors.is_empty());
        assert!(backend.calls().is_empty());
    }

    #[tokio::test]
    async fn job_effect_uses_direct_argv_scoped_environment_and_truthful_mapping() {
        use crate::mux::jobs::{JobAdmission, JobLaunch, JobPlacement};
        use crate::mux::{Op, Store};
        let store = Store::new();
        let effects = store
            .dispatch(Op::CreateJob(Box::new(JobAdmission {
                job_id: "job".into(),
                owner: Some("agent".into()),
                created_at: 1,
                placement: JobPlacement::Screen {},
                launch: JobLaunch {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    argv: vec!["program".into(), "two words".into()],
                    cwd: "/project".into(),
                    name: None,
                    placement: JobPlacement::Screen {},
                    new_screen_id: Some("screen".into()),
                    new_pane_id: Some("pane".into()),
                    background: true,
                },
            })))
            .expect("job intent");
        let backend = Arc::new(MockBackend::new());
        let results = EffectRunner::new(backend.clone())
            .execute(effects.clone(), &test_env())
            .await;
        assert!(results[0].result.is_ok());
        let (screen, pane, session) = results[0].spawned().expect("actual mapping");
        assert_eq!(screen, ScreenId::from("screen"));
        assert_eq!(pane, PaneId::from("pane"));
        match backend.calls().as_slice() {
            [MockCall::Program {
                id,
                argv,
                dir,
                extra_env,
            }] => {
                assert_eq!(id, session.as_str());
                assert_eq!(argv, &["program", "two words"]);
                assert_eq!(dir, "/project");
                assert!(extra_env
                    .iter()
                    .any(|e| e.starts_with("SPLITTY_AGENT_SOCK=")));
                assert!(!extra_env
                    .iter()
                    .any(|e| e.starts_with("SPLITTY_SOCK=") || e.starts_with("SPLITTY_TOKEN=")));
            }
            other => panic!("expected one direct launch, got {other:?}"),
        }
        let failing =
            Arc::new(MockBackend::new().with_create_error(PtyError::SpawnFailed("missing".into())));
        let results = EffectRunner::new(failing)
            .execute(effects, &test_env())
            .await;
        assert!(results[0].result.is_err());
        assert!(results[0].spawned().is_none());
    }
}
