//! A single terminal session: the session layer over a [`Terminal`].
//!
//! Handles shell resolution and the session's spec (program, environment,
//! directory, size), scrollback buffering, OSC title scanning and the job log
//! text. The terminal under it, in this process or in a session keeper, feeds
//! all output from spawn to exit into the session; connections subscribe to it.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use portable_pty::CommandBuilder;
use tokio::sync::{mpsc, watch};

use super::osc::OscScanner;
use super::procinfo;
use super::ringbuffer::RingBuffer;
use super::startup::build_command_args;
use super::terminal::{Feed, LocalTerminals, SessionSpec, Terminal, TerminalFactory};
use super::{
    BellEvent, CreateSessionOpts, OscTitleEvent, PtyError, SemanticEvent, SemanticEventInner,
    SemanticState, SessionHandle, SessionStats, Subscription, TitleInfo,
};

const DEFAULT_SCROLLBACK_SIZE: usize = 1 << 20; // 1 MB

/// The size of a session's scrollback: `SPLITTY_SCROLLBACK_SIZE` bytes when
/// that is a positive number, else 1 MiB.
pub fn scrollback_size() -> usize {
    std::env::var("SPLITTY_SCROLLBACK_SIZE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_SCROLLBACK_SIZE)
}

/// A single terminal session over its terminal.
pub struct Session {
    id: String,
    shell_profile_id: Option<String>,
    terminal: Box<dyn Terminal>,
    managed: bool,
    exit_reported: AtomicBool,
    /// Shared with the terminal, which feeds it.
    output: Arc<Output>,
    osc_rx: watch::Receiver<OscTitleEvent>,
    semantic_rx: watch::Receiver<SemanticEvent>,
    bell_rx: watch::Receiver<BellEvent>,
}

/// What the session records for every chunk of its terminal's output.
struct Output {
    /// Held while a chunk is recorded and fanned out, and while a subscriber
    /// takes its snapshot, so every byte lands in a subscriber's snapshot or
    /// in its receiver, never both and never neither.
    subscribers: Mutex<Subscribers>,
    scrollback: RingBuffer,
    text: Option<(Mutex<super::log_text::LogText>, RingBuffer)>,
    generation: String,
    changed: watch::Sender<u64>,
    osc: Mutex<OscScanner>,
    osc_tx: watch::Sender<OscTitleEvent>,
    semantic_state: Mutex<SemanticState>,
    semantic_tx: watch::Sender<SemanticEvent>,
    bell_tx: watch::Sender<BellEvent>,
}

struct Subscribers {
    senders: Vec<mpsc::UnboundedSender<Arc<[u8]>>>,
    /// Set at EOF: later subscribers get the snapshot and an ended receiver.
    ended: bool,
}

impl Output {
    /// Record one chunk: scrollback, title and prompt tracking, subscribers.
    fn record(&self, data: &[u8]) {
        let mut subscribers = self.subscribers.lock().unwrap();
        self.scrollback.write(data);
        if let Some((parser, ring)) = &self.text {
            ring.write(&parser.lock().expect("log parser lock").push(data));
        }
        self.scan(data);
        let chunk: Arc<[u8]> = Arc::from(data);
        // A send fails only when that stream has ended; forget it.
        subscribers
            .senders
            .retain(|tx| tx.send(Arc::clone(&chunk)).is_ok());
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    /// Feed the OSC scanner and publish title, OSC 133 and bell updates.
    fn scan(&self, data: &[u8]) {
        let Ok(mut osc) = self.osc.lock() else { return };
        if osc.feed(data) {
            let _ = self.osc_tx.send(OscTitleEvent {
                title: osc.title().to_string(),
                changed_at: Instant::now(),
            });
        }
        let actions = osc.drain_semantic();
        if !actions.is_empty() {
            if let Ok(mut state) = self.semantic_state.lock() {
                for action in actions {
                    state.apply(&action);
                    let _ = self.semantic_tx.send(Some(SemanticEventInner {
                        action,
                        timestamp: Instant::now(),
                    }));
                }
            }
        }
        if osc.drain_bell() > 0 {
            let _ = self.bell_tx.send(Some(Instant::now()));
        }
    }

    /// The terminal closed: end every subscription once it has drained.
    fn end(&self) {
        let mut subscribers = self.subscribers.lock().unwrap();
        if let Some((parser, ring)) = &self.text {
            ring.write(&parser.lock().expect("log parser lock").finish());
        }
        subscribers.ended = true;
        subscribers.senders.clear();
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    fn read(
        &self,
        job: &str,
        query: &super::output::OutputQuery,
    ) -> Result<super::output::OutputPage, super::output::OutputError> {
        use super::output::{LogFormat, OutputError, OutputPage};
        let selection = query.selection(job, &self.generation)?;
        // Capture EOF and the buffer under the same lock as record/end: EOF must
        // not claim completion of a snapshot taken before its final bytes arrived.
        let output = self.subscribers.lock().expect("output lock");
        let ring = match query.format {
            LogFormat::Raw => &self.scrollback,
            LogFormat::Text => &self.text.as_ref().ok_or(OutputError::Unavailable)?.1,
        };
        let slice = ring
            .read(selection, query.limit, query.format == LogFormat::Text)
            .map_err(|_| OutputError::InvalidCursor)?;
        OutputPage::from_slice(job, &self.generation, query.format, slice, output.ended)
    }

    fn subscribe(&self) -> Subscription {
        let mut subscribers = self.subscribers.lock().unwrap();
        let snapshot = self.scrollback.read_all();
        let (tx, receiver) = mpsc::unbounded_channel();
        if !subscribers.ended {
            subscribers.senders.push(tx);
        }
        Subscription { snapshot, receiver }
    }
}

impl Feed for Output {
    fn record(&self, data: &[u8]) {
        Output::record(self, data);
    }

    fn end(&self) {
        Output::end(self);
    }
}

impl Session {
    /// Create a new PTY session in this process with the given options.
    pub fn new(opts: CreateSessionOpts) -> Result<Arc<Session>, PtyError> {
        Self::start(opts, None, &LocalTerminals)
    }

    /// Execute a program directly in this process, with no
    /// login/profile/command-replay semantics.
    pub fn new_program(
        opts: CreateSessionOpts,
        argv: Vec<String>,
    ) -> Result<Arc<Session>, PtyError> {
        Self::start(opts, Some(argv), &LocalTerminals)
    }

    /// Start a session on a terminal of `terminals`: a shell, or with `argv`
    /// a program executed directly.
    pub fn start(
        opts: CreateSessionOpts,
        argv: Option<Vec<String>>,
        terminals: &dyn TerminalFactory,
    ) -> Result<Arc<Session>, PtyError> {
        if let Some(argv) = &argv {
            if argv.is_empty() || argv[0].is_empty() || argv.iter().any(|arg| arg.contains('\0')) {
                return Err(PtyError::SpawnFailed(
                    "program argv is empty or contains NUL".into(),
                ));
            }
            if !std::path::Path::new(&opts.dir).is_absolute()
                || !std::path::Path::new(&opts.dir).is_dir()
            {
                return Err(PtyError::SpawnFailed(
                    "working directory must be an existing absolute directory".into(),
                ));
            }
        }
        let cli_dir = super::cli_path::current_cli_dir();
        Self::start_with_cli(opts, argv, cli_dir.as_deref(), terminals)
    }

    #[cfg(test)]
    fn spawn_with_cli(
        opts: CreateSessionOpts,
        argv: Option<Vec<String>>,
        cli_dir: Option<&std::path::Path>,
    ) -> Result<Arc<Session>, PtyError> {
        Self::start_with_cli(opts, argv, cli_dir, &LocalTerminals)
    }

    fn start_with_cli(
        opts: CreateSessionOpts,
        argv: Option<Vec<String>>,
        cli_dir: Option<&std::path::Path>,
        terminals: &dyn TerminalFactory,
    ) -> Result<Arc<Session>, PtyError> {
        let (spec, inject) = build_spec(&opts, argv.as_deref(), cli_dir)?;
        let shell_profile_id = (argv.is_none() && !opts.shell_profile_id.is_empty())
            .then_some(opts.shell_profile_id.clone());
        let session = Self::with_terminal(
            spec.session_id.clone(),
            shell_profile_id,
            spec.managed,
            spec.buffer_size,
            |feed| terminals.start(&spec, feed),
        )?;

        // Type the start command into the shell once it is ready.
        if let Some(command) = inject {
            let session_weak = Arc::downgrade(&session);
            tokio::spawn(async move {
                // Wait for shell to produce output (prompt).
                // Use a brief delay as a portable approach to detect readiness.
                tokio::task::spawn_blocking(|| {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                })
                .await
                .ok();

                // Give prompt rendering a brief moment to settle.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;

                if let Some(session) = session_weak.upgrade() {
                    let cmd_bytes = format!("{}\n", command);
                    let _ = session.terminal.write(cmd_bytes.as_bytes()).await;
                }
            });
        }

        Ok(session)
    }

    /// A session around the terminal `start` returns, which feeds it from its
    /// first byte on: a newly started one, or one adopted from a keeper.
    pub fn with_terminal(
        id: String,
        shell_profile_id: Option<String>,
        managed: bool,
        buffer_size: usize,
        start: impl FnOnce(Arc<dyn Feed>) -> Result<Box<dyn Terminal>, PtyError>,
    ) -> Result<Arc<Session>, PtyError> {
        let (osc_tx, osc_rx) = watch::channel(OscTitleEvent::default());
        let (semantic_tx, semantic_rx) = watch::channel(None);
        let (bell_tx, bell_rx) = watch::channel(None);
        let output = Arc::new(Output {
            subscribers: Mutex::new(Subscribers {
                senders: Vec::new(),
                ended: false,
            }),
            scrollback: RingBuffer::new(buffer_size),
            text: managed.then(|| {
                (
                    Mutex::new(super::log_text::LogText::default()),
                    RingBuffer::new(buffer_size),
                )
            }),
            generation: uuid::Uuid::new_v4().to_string(),
            changed: watch::channel(0).0,
            osc: Mutex::new(OscScanner::new()),
            osc_tx,
            semantic_state: Mutex::new(SemanticState::default()),
            semantic_tx,
            bell_tx,
        });
        let terminal = start(Arc::clone(&output) as Arc<dyn Feed>)?;
        Ok(Arc::new(Session {
            id,
            shell_profile_id,
            terminal,
            managed,
            exit_reported: AtomicBool::new(false),
            output,
            osc_rx,
            semantic_rx,
            bell_rx,
        }))
    }
}

/// The program, environment, directory and size a session's terminal starts
/// with, and the command to type into a shell once it is ready.
fn build_spec(
    opts: &CreateSessionOpts,
    argv: Option<&[String]>,
    cli_dir: Option<&std::path::Path>,
) -> Result<(SessionSpec, Option<String>), PtyError> {
    // A program executed directly is a managed job.
    let managed = argv.is_some();

    // Shell resolution
    let shell = if !opts.shell.is_empty() {
        opts.shell.clone()
    } else {
        default_shell()
    };

    // ID generation
    let id = if !opts.id.is_empty() {
        opts.id.clone()
    } else {
        generate_id()
    };

    // Build restore-mode args. For a normal/replay start, preserve the
    // profile argv exactly as separate arguments.
    let (command_args, inject_command) = if argv.is_some() {
        (Vec::new(), false)
    } else {
        build_command_args(&shell, &opts.shell_args, &opts.command, &opts.restore_mode)
    };

    // Determine working directory
    let home = dirs_or_home();
    let dir = if argv.is_some() {
        opts.dir.clone()
    } else {
        resolve_dir(&opts.dir, &home)
    };

    // Terminal size
    let cols = if opts.cols == 0 { 80 } else { opts.cols };
    let rows = if opts.rows == 0 { 24 } else { opts.rows };

    // Build command. Shell-integration arguments must precede profile
    // arguments: Bash requires GNU long options such as `--posix` before
    // short options such as the detected profile's `-l`.
    let integration_dir = find_shell_integration_dir();
    let integration_dir_str = integration_dir
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned());
    let mut cmd = if let Some(argv) = argv {
        let mut cmd = CommandBuilder::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd
    } else {
        build_shell_command(
            &shell,
            &command_args,
            integration_dir_str.as_deref(),
            opts.command.is_empty() || matches!(opts.restore_mode.as_str(), "" | "replay"),
        )
    };

    // These belong to the new session, never to the process that launched its server.
    for key in [
        "SPLITTY_SOCK",
        "SPLITTY_AGENT_SOCK",
        "SPLITTY_TOKEN",
        "SPLITTY_SERVER",
        "SPLITTY_ADDR",
        "SPLITTY_BOOTSTRAP",
        "SPLITTY_TICKET",
        "SPLITTY_URL",
        "SPLITTY_PANE_ID",
        "SPLITTY_SCREEN_ID",
        "SPLITTY_SESSION_ID",
    ] {
        cmd.env_remove(key);
    }

    // Set environment
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("SPLITTY_SESSION_ID", &id);

    // Add extra environment variables
    for env_str in &opts.extra_env {
        if let Some((key, val)) = env_str.split_once('=') {
            cmd.env(key, val);
        }
    }

    super::cli_path::configure_command(&mut cmd, cli_dir);

    let argv = cmd
        .get_argv()
        .iter()
        .map(|arg| {
            arg.clone()
                .into_string()
                .map_err(|_| PtyError::SpawnFailed("an argument is not valid UTF-8".into()))
        })
        .collect::<Result<Vec<String>, PtyError>>()?;
    // The inherited environment with every change above. A variable that is
    // not UTF-8 cannot be carried and is left out.
    let env: Vec<(String, String)> = cmd
        .iter_full_env_as_str()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    let variable = |name: &str| {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    let spec = SessionSpec {
        pane_id: variable("SPLITTY_PANE_ID"),
        screen_id: variable("SPLITTY_SCREEN_ID"),
        session_id: id,
        argv,
        env,
        cwd: (!dir.is_empty()).then_some(dir),
        cols,
        rows,
        managed,
        buffer_size: scrollback_size(),
    };
    let inject = (inject_command && !opts.command.is_empty()).then(|| opts.command.clone());
    Ok((spec, inject))
}

#[async_trait]
impl SessionHandle for Session {
    fn session_id(&self) -> &str {
        &self.id
    }

    fn shell_profile_id(&self) -> Option<&str> {
        self.shell_profile_id.as_deref()
    }

    fn shell_pid(&self) -> Option<u32> {
        // Once the shell has exited, its pid may belong to another process.
        if *self.terminal.done().borrow() {
            return None;
        }
        self.terminal.pid()
    }

    fn subscribe(&self) -> Subscription {
        self.output.subscribe()
    }

    async fn write(&self, data: &[u8]) -> io::Result<()> {
        self.terminal.write(data).await
    }

    fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.terminal.resize(cols, rows)
    }

    fn close(&self) -> io::Result<()> {
        self.terminal.close()
    }

    fn release(&self) {
        self.terminal.release();
    }

    fn done(&self) -> watch::Receiver<bool> {
        self.terminal.done()
    }

    fn exit_code(&self) -> i32 {
        self.terminal
            .result()
            .and_then(|result| result.exit_code)
            .and_then(|code| i32::try_from(code).ok())
            .unwrap_or(-1)
    }

    fn process_result(&self) -> Option<super::ProcessResult> {
        self.terminal.result()
    }
    fn is_managed(&self) -> bool {
        self.managed
    }
    async fn stop(&self) -> io::Result<super::ProcessResult> {
        self.terminal.stop().await
    }

    fn title(&self) -> TitleInfo {
        let shell_pid = self.terminal.pid().unwrap_or(0);

        let pgrp = match self.terminal.foreground_pgid() {
            Some(p) => p,
            None => {
                return TitleInfo {
                    base_shell: true,
                    ..Default::default()
                }
            }
        };

        match procinfo::process_info(pgrp) {
            Some(info) => TitleInfo {
                proc: info.name,
                cwd: info.cwd,
                cmdline: info.cmdline,
                base_shell: pgrp == shell_pid,
            },
            None => TitleInfo {
                base_shell: true,
                ..Default::default()
            },
        }
    }

    fn osc_title_watch(&self) -> watch::Receiver<OscTitleEvent> {
        self.osc_rx.clone()
    }

    fn semantic_state(&self) -> SemanticState {
        self.output.semantic_state.lock().unwrap().clone()
    }

    fn semantic_watch(&self) -> watch::Receiver<SemanticEvent> {
        self.semantic_rx.clone()
    }

    fn bell_watch(&self) -> watch::Receiver<BellEvent> {
        self.bell_rx.clone()
    }

    fn scrollback(&self) -> Vec<u8> {
        self.output.scrollback.read_all()
    }

    fn read_output(
        &self,
        job: &str,
        query: &super::output::OutputQuery,
    ) -> Result<super::output::OutputPage, super::output::OutputError> {
        self.output.read(job, query)
    }

    fn output_watch(&self) -> watch::Receiver<u64> {
        self.output.changed.subscribe()
    }

    fn output_complete(&self) -> bool {
        self.output.subscribers.lock().expect("output lock").ended
    }

    fn stats(&self) -> Option<SessionStats> {
        let title = self.title();
        let (used, max) = self.output.scrollback.stats();
        let shell_pid = self.terminal.pid().unwrap_or(0);
        let rss = procinfo::read_rss(shell_pid);

        Some(SessionStats {
            id: self.id.clone(),
            pid: shell_pid,
            proc_name: title.proc,
            cwd: title.cwd,
            cmdline: title.cmdline,
            base_shell: title.base_shell,
            scrollback_used: used,
            scrollback_max: max,
            child_rss_bytes: rss,
        })
    }

    fn claim_exit_report(&self) -> bool {
        !self.exit_reported.swap(true, Ordering::SeqCst)
    }
}

impl Drop for Session {
    /// A session dropped without `close` hangs up like a closed one, as it did
    /// when dropping it closed the PTY; a released one leaves its program
    /// running.
    fn drop(&mut self) {
        let _ = self.terminal.close();
    }
}

/// Find the shell-integration directory relative to the current executable.
/// Checks next to the executable first, then falls back to common locations.
fn find_shell_integration_dir() -> Option<std::path::PathBuf> {
    // Try relative to the executable, where it was started from
    if let Some(exe) = super::cli_path::started_from() {
        if let Some(parent) = exe.parent() {
            // Development: exe is in target/debug or target/release,
            // shell-integration is at the repo root.
            for ancestor in [parent, parent.parent().unwrap_or(parent)] {
                let candidate = ancestor.join("shell-integration");
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
            // Packaged: shell-integration alongside the binary
            let candidate = parent.join("shell-integration");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }

    // Try from CARGO_MANIFEST_DIR (development builds)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let candidate = std::path::PathBuf::from(manifest_dir)
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("shell-integration"));
        if let Some(c) = candidate {
            if c.is_dir() {
                return Some(c);
            }
        }
    }

    None
}

/// Detect the shell type from the shell path/command.
fn detect_shell(shell: &str) -> Option<&'static str> {
    let basename = std::path::Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(shell);
    // Strip leading '-' (login shell convention)
    let name = basename.strip_prefix('-').unwrap_or(basename);
    match name {
        "zsh" => Some("zsh"),
        "bash" => Some("bash"),
        _ => None,
    }
}

/// Set up automatic shell integration by manipulating environment variables
/// and/or command arguments so the shell sources our integration scripts
/// without the user modifying their rc files.
fn setup_shell_integration(
    shell: &str,
    cmd: &mut CommandBuilder,
    integration_dir: &str,
    plain_shell: bool,
) {
    match detect_shell(shell) {
        Some("zsh") => {
            // ZDOTDIR trick: point zsh's config directory at our integration
            // dir, which contains a .zshenv that sources our hooks and then
            // restores + loads the user's real ZDOTDIR.
            if let Ok(old) = std::env::var("ZDOTDIR") {
                cmd.env("SPLITTY_ZSH_ZDOTDIR", &old);
            }
            let zsh_dir = format!("{}/zsh", integration_dir);
            cmd.env("ZDOTDIR", &zsh_dir);
        }
        Some("bash") if plain_shell => {
            // POSIX/ENV trick: start bash in POSIX mode so it reads the ENV
            // variable on startup. Our script disables POSIX mode and
            // re-creates the normal startup sequence.
            let script = format!("{}/bash/splitty.bash", integration_dir);
            if !std::path::Path::new(&script).exists() {
                return;
            }

            // Preserve existing ENV
            if let Ok(old) = std::env::var("ENV") {
                cmd.env("SPLITTY_BASH_ENV", &old);
            }
            cmd.env("ENV", &script);
            cmd.env("SPLITTY_BASH_INJECT", "1");

            // In POSIX mode HISTFILE defaults to ~/.sh_history; fix it.
            if std::env::var("HISTFILE").is_err() {
                if let Some(home) = dirs::home_dir() {
                    cmd.env(
                        "HISTFILE",
                        home.join(".bash_history").to_string_lossy().as_ref(),
                    );
                    cmd.env("SPLITTY_BASH_UNEXPORT_HISTFILE", "1");
                }
            }

            cmd.arg("--posix");
        }
        _ => {}
    }
}

fn build_shell_command(
    shell: &str,
    command_args: &[String],
    integration_dir: Option<&str>,
    plain_shell: bool,
) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(shell);
    if let Some(dir) = integration_dir {
        cmd.env("SPLITTY_SHELL_INTEGRATION", dir);
        setup_shell_integration(shell, &mut cmd, dir, plain_shell);
    }
    for arg in command_args {
        cmd.arg(arg);
    }
    cmd
}

pub(super) fn generate_id() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill(&mut bytes);
    hex::encode(bytes)
}

fn default_shell() -> String {
    #[cfg(unix)]
    {
        std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".to_string())
    }
    #[cfg(windows)]
    {
        std::env::var("COMSPEC")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "cmd.exe".to_string())
    }
    #[cfg(not(any(unix, windows)))]
    {
        "/bin/sh".to_string()
    }
}

fn dirs_or_home() -> String {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn resolve_dir(dir: &str, home: &str) -> String {
    if dir.is_empty() {
        return home.to_string();
    }

    let mut resolved = dir.to_string();

    // Expand ~ prefix
    if resolved.starts_with('~') && !home.is_empty() {
        if resolved == "~" {
            resolved = home.to_string();
        } else {
            resolved = format!("{}{}", home, &resolved[1..]);
        }
    }

    // Fall back to home if dir doesn't exist
    match std::fs::metadata(&resolved) {
        Ok(m) if m.is_dir() => resolved,
        _ => home.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use crate::pty::testing::{pty_available, sh_opts, wait_until};

    const TIMEOUT: Duration = Duration::from_secs(10);

    #[cfg(unix)]
    fn available_cli_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().expect("fixture");
        let directory = home.path().join("bundled CLI ö");
        std::fs::create_dir(&directory).expect("directory");
        let cli = directory.join("splitty");
        std::fs::write(
            &cli,
            "#!/bin/sh\nprintf 'CLI_FIXTURE\\n'\nprintf 'ARG:<%s>\\n' \"$@\"\n",
        )
        .expect("CLI fixture");
        std::fs::set_permissions(cli, std::fs::Permissions::from_mode(0o700)).expect("executable");
        (home, directory)
    }

    /// terminal-sessions, Host CLI Follows The Daemon (An update gives a kept
    /// shell the new CLI): a running shell with the stable directory runs the
    /// link's new target without being started again.
    #[tokio::test]
    #[cfg(unix)]
    async fn a_running_shell_follows_the_stable_cli_link() {
        use std::os::unix::fs::PermissionsExt;
        if !pty_available() {
            return;
        }
        let fixture = tempfile::tempdir().expect("fixture");
        let installation = |version: &str| {
            let directory = fixture.path().join(version);
            std::fs::create_dir(&directory).expect("installation");
            let cli = directory.join("splitty");
            std::fs::write(&cli, format!("#!/bin/sh\nprintf 'CLI:%s\\n' {version}\n"))
                .expect("CLI fixture");
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700))
                .expect("executable");
            cli
        };
        let (old, new) = (installation("old"), installation("new"));
        let stable = fixture.path().join("run").join("bin");
        super::super::cli_path::publish_in(&stable, &old).expect("publish");
        let mut opts = sh_opts(
            "follows",
            "while IFS= read -r line; do eval \"$line\"; done",
        );
        opts.extra_env = vec!["PATH=/usr/bin:/bin".into()];
        let session = Session::spawn_with_cli(opts, None, Some(&stable)).expect("shell");
        session.write(b"splitty\n").await.expect("first");
        assert!(
            wait_until(TIMEOUT, || text(&session.scrollback()).contains("CLI:old")).await,
            "{}",
            text(&session.scrollback())
        );

        super::super::cli_path::publish_in(&stable, &new).expect("update");
        session
            .write(b"splitty; printf 'DIR:%s\\n' \"$SPLITTY_CLI_DIR\"; exit\n")
            .await
            .expect("second");
        assert!(wait_until(TIMEOUT, || session.output_complete()).await);
        let output = text(&session.scrollback());
        assert!(output.contains("CLI:new"), "{output}");
        assert!(
            output.contains(&format!("DIR:{}", stable.display())),
            "{output}"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn cli_path_is_child_only_and_keeps_owner_and_scoped_environments_separate() {
        if !pty_available() {
            return;
        }
        let (home, directory) = available_cli_fixture();
        let global_path = std::env::var_os("PATH");
        let mut old_opts = sh_opts(
            "old-path",
            "printf 'OLD:%s\\n' \"$PATH\"; read line; printf 'AFTER:%s\\n' \"$PATH\"",
        );
        old_opts.extra_env = vec!["PATH=/usr/bin:/bin".into(), "SPLITTY_CLI_DIR=/stale".into()];
        let old = Session::spawn_with_cli(old_opts, None, None).expect("existing session");
        assert!(
            wait_until(TIMEOUT, || text(&old.scrollback())
                .contains("OLD:/usr/bin:/bin"))
            .await
        );
        for kind in ["shell", "direct", "grant"] {
            let script = "printf 'RESOLVE:'; command -v splitty; splitty; env";
            let mut opts = sh_opts(kind, script);
            opts.dir = home.path().to_string_lossy().into_owned();
            opts.extra_env = vec![
                "PATH=/usr/bin:/bin".into(),
                "SPLITTY_CLI_DIR=/wrong-hint".into(),
                if kind == "grant" {
                    "SPLITTY_AGENT_SOCK=own-grant".into()
                } else {
                    "SPLITTY_SOCK=owner".into()
                },
            ];
            let argv =
                (kind != "shell").then(|| vec!["/bin/sh".into(), "-c".into(), script.into()]);
            let session = Session::spawn_with_cli(opts, argv, Some(&directory)).expect("session");
            assert!(wait_until(TIMEOUT, || session.output_complete()).await);
            let output = text(&session.scrollback());
            assert!(
                output.contains(&format!("RESOLVE:{}/splitty", directory.display())),
                "{output}"
            );
            assert!(output.contains("CLI_FIXTURE"));
            assert!(!output.contains("/wrong-hint"));
            assert!(output.contains(&format!("SPLITTY_CLI_DIR={}", directory.display())));
            for forbidden in [
                "SPLITTY_TOKEN=",
                "SPLITTY_SERVER=",
                "SPLITTY_BOOTSTRAP=",
                "SPLITTY_TICKET=",
            ] {
                assert!(!output.contains(forbidden));
            }
            if kind == "grant" {
                assert!(output.contains("SPLITTY_AGENT_SOCK=own-grant"));
                assert!(!output.contains("SPLITTY_SOCK="));
            } else {
                assert!(output.contains("SPLITTY_SOCK=owner"));
                assert!(!output.contains("SPLITTY_AGENT_SOCK="));
            }
        }
        old.write(b"continue\n").await.expect("old input");
        assert!(wait_until(TIMEOUT, || old.output_complete()).await);
        assert!(text(&old.scrollback()).contains("AFTER:/usr/bin:/bin"));
        assert_eq!(std::env::var_os("PATH"), global_path);
        assert_eq!(
            std::fs::read_dir(home.path()).expect("files").count(),
            1,
            "no installation files"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn cli_path_survives_bash_and_zsh_startup_without_editing_user_files() {
        if !pty_available() {
            return;
        }
        for shell in ["/bin/bash", "/bin/zsh"] {
            if !std::path::Path::new(shell).is_file() {
                eprintln!("skipping unavailable shell {shell}");
                continue;
            }
            let (home, directory) = available_cli_fixture();
            let profile = home.path().join(if shell.ends_with("bash") {
                ".bash_profile"
            } else {
                ".zshrc"
            });
            let content =
                "export PATH=/usr/bin:/bin\nPS1='PATH_TEST_PROMPT> '\nprintf 'USER_RC_LOADED\\n'\n";
            std::fs::write(&profile, content).expect("profile");
            let mut opts = sh_opts("startup-cli", "");
            opts.shell = shell.into();
            opts.shell_args = vec!["-l".into()];
            opts.dir = home.path().to_string_lossy().into_owned();
            opts.extra_env = vec![
                format!("HOME={}", home.path().display()),
                "PATH=/usr/bin:/bin".into(),
                "HISTFILE=/dev/null".into(),
                format!("SPLITTY_ZSH_ZDOTDIR={}", home.path().display()),
            ];
            let session =
                Session::spawn_with_cli(opts, None, Some(&directory)).expect("integrated shell");
            assert!(
                wait_until(TIMEOUT, || text(&session.scrollback())
                    .contains("PATH_TEST_PROMPT> "))
                .await,
                "{shell}: {}",
                text(&session.scrollback())
            );
            session.write(b"printf '\\nRESOLVE:'; command -v splitty; splitty; splitty open 'file with spaces.rs' --with zed; printf '\\nWRAPPER:%s\\n' \"$(command -v splitty-open || echo none)\"; __splitty_restore_cli_path; __splitty_restore_cli_path; printf '\\nFINAL_PATH:%s\\n' \"$PATH\"; exit\n").await.expect("command");
            assert!(wait_until(TIMEOUT, || session.output_complete()).await);
            let output = text(&session.scrollback());
            assert!(output.contains("USER_RC_LOADED"));
            assert!(
                output.contains(&format!("RESOLVE:{}/splitty", directory.display())),
                "{shell}: {output}"
            );
            assert!(output.contains("CLI_FIXTURE"));
            assert!(output.contains("WRAPPER:none"), "{shell}: {output}");
            for argument in ["open", "file with spaces.rs", "--with", "zed"] {
                assert!(
                    output.contains(&format!("ARG:<{argument}>")),
                    "{shell}: {output}"
                );
            }
            let path = output
                .lines()
                .find_map(|line| line.strip_prefix("FINAL_PATH:"))
                .expect("final PATH")
                .trim_end_matches('\r');
            assert_eq!(
                path,
                format!("{}:/usr/bin:/bin", directory.display()),
                "{shell}: {output}"
            );
            assert_eq!(
                std::fs::read_to_string(&profile).expect("unchanged profile"),
                content
            );
            assert!(!home.path().join(".bashrc").exists());
        }
    }

    #[test]
    fn managed_and_ordinary_environments_strip_inherited_authority() {
        const CHILD: &str = "SPLITTY_ENV_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let result = std::process::Command::new(
                std::env::current_exe().expect("test executable"),
            )
            .args([
                "--exact",
                "pty::session::tests::managed_and_ordinary_environments_strip_inherited_authority",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("SPLITTY_SOCK", "inherited-owner")
            .env("SPLITTY_AGENT_SOCK", "inherited-grant")
            .env("SPLITTY_TOKEN", "credential-must-not-leak")
            .env("SPLITTY_SERVER", "http://inherited")
            .env("SPLITTY_BOOTSTRAP", "code-must-not-leak")
            .env("SPLITTY_TICKET", "ticket-must-not-leak")
            .env("SPLITTY_URL", "http://inherited/?bootstrap=secret")
            .output()
            .expect("isolated environment test");
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        if !pty_available() {
            return;
        }
        tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(async {
                for managed in [false, true] {
                    let mut opts = sh_opts("environment", "env");
                    opts.dir = std::env::temp_dir().to_string_lossy().into_owned();
                    opts.extra_env = vec![
                        "SPLITTY_PANE_ID=pane".into(),
                        "SPLITTY_SCREEN_ID=screen".into(),
                        if managed {
                            "SPLITTY_AGENT_SOCK=own-grant".into()
                        } else {
                            "SPLITTY_SOCK=own-socket".into()
                        },
                    ];
                    let session = if managed {
                        Session::new_program(opts, vec!["/usr/bin/env".into()])
                    } else {
                        Session::new(opts)
                    }
                    .expect("session");
                    assert!(wait_until(TIMEOUT, || session.output_complete()).await);
                    let output = text(&session.scrollback());
                    for marker in [
                        "inherited-owner",
                        "inherited-grant",
                        "credential-must-not-leak",
                        "http://inherited",
                        "code-must-not-leak",
                        "ticket-must-not-leak",
                    ] {
                        assert!(!output.contains(marker), "leaked {marker}");
                    }
                    assert!(output.contains("SPLITTY_SESSION_ID=environment"));
                    assert!(output.contains("SPLITTY_PANE_ID=pane"));
                    assert!(output.contains("SPLITTY_SCREEN_ID=screen"));
                    assert!(output.contains("TERM=xterm-256color"));
                    assert!(output.contains(if managed {
                        "SPLITTY_AGENT_SOCK=own-grant"
                    } else {
                        "SPLITTY_SOCK=own-socket"
                    }));
                    assert!(!output.contains(if managed {
                        "SPLITTY_SOCK="
                    } else {
                        "SPLITTY_AGENT_SOCK="
                    }));
                }
            });
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn managed_input_backpressure_does_not_block_revocation_or_stop() {
        if !pty_available() {
            return;
        }
        let mut opts = sh_opts("backpressure", "");
        opts.dir = std::env::temp_dir().to_string_lossy().into_owned();
        let session = Session::new_program(
            opts,
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "stty raw -echo; printf READY; sleep 30".into(),
            ],
        )
        .expect("job");
        assert!(wait_until(TIMEOUT, || text(&session.scrollback()).contains("READY")).await);
        // A watchdog makes a regression fail instead of stranding the test runner.
        let cleanup = session.clone();
        let watchdog = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let _ = cleanup.stop().await;
        });
        let started = Instant::now();
        let mut backpressure = false;
        for _ in 0..64 {
            if let Err(error) = session.write(&[b'x'; 16384]).await {
                backpressure = error.kind() == io::ErrorKind::WouldBlock;
                break;
            }
        }
        let elapsed = started.elapsed();
        session.stop().await.expect("stop remains possible");
        watchdog.abort();
        assert!(backpressure, "full input must report backpressure");
        assert!(
            elapsed < Duration::from_secs(1),
            "PTY write blocked for {elapsed:?}"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn logs_keep_final_bytes_when_the_process_exits_before_capture_ends() {
        use super::super::output::{read_waiting, OutputQuery};
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if !pty_available() {
            return;
        }
        let directory = tempfile::tempdir().expect("fixture");
        let gate = directory.path().join("gate");
        nix::unistd::mkfifo(
            &gate,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .expect("gate");
        let mut opts = sh_opts("trailing-output", "");
        opts.dir = directory.path().to_string_lossy().into_owned();
        let session = Session::new_program(
            opts,
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "trap '' HUP; (read line < gate; printf FINAL_BYTES) & exit 7".into(),
            ],
        )
        .expect("job");
        assert!(wait_until(TIMEOUT, || *session.done().borrow()).await);
        let result = session.process_result();
        let before = session
            .read_output("job", &OutputQuery::default())
            .expect("before tail");
        // Always release the child before asserting, so a failed assertion leaves no writer behind.
        assert!(
            wait_until(TIMEOUT, || std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(nix::libc::O_NONBLOCK)
                .open(&gate)
                .and_then(|mut file| file.write_all(b"continue\n"))
                .is_ok())
            .await
        );
        assert_eq!(result.expect("exit").exit_code, Some(7));
        assert!(!before.output_complete);
        let tail = read_waiting(
            session.as_ref(),
            "job",
            &OutputQuery {
                cursor: before.next_cursor,
                ..Default::default()
            },
            Duration::from_secs(5),
        )
        .await
        .expect("tail");
        assert!(tail.data.contains("FINAL_BYTES"));
        assert!(wait_until(TIMEOUT, || session.output_complete()).await);
    }

    #[tokio::test]
    async fn managed_output_queries_are_independent_of_ui_and_terminal_size() {
        use crate::pty::output::{LogFormat, OutputQuery};
        use base64::Engine;
        if !pty_available() {
            return;
        }
        let mut opts = sh_opts("logs-shared", "");
        opts.dir = std::env::temp_dir().to_string_lossy().into_owned();
        let session = Session::new_program(opts, vec!["/bin/sh".into(), "-c".into(),
            r"stty size; printf '\033[31mFIRST\033[0m\n'; read line; stty size; printf SECOND; exit 7".into()]).expect("job");
        assert!(wait_until(TIMEOUT, || text(&session.scrollback()).contains("FIRST")).await);
        let first = session
            .read_output("job", &OutputQuery::default())
            .expect("first text");
        assert!(first.data.contains("24 80\nFIRST\n"));
        let second = session
            .read_output("job", &OutputQuery::default())
            .expect("independent reader");
        assert_eq!(first.data, second.data);
        assert_eq!(first.next_cursor, second.next_cursor);
        let mut ui = session.subscribe();
        session.write(b"go\n").await.expect("continue");
        let (live, ended) = receive(&mut ui, |_| false).await;
        assert!(ended);
        let incremental = session
            .read_output(
                "job",
                &OutputQuery {
                    cursor: first.next_cursor,
                    ..Default::default()
                },
            )
            .expect("incremental");
        assert!(incremental.data.contains("24 80\nSECOND"));
        assert!(!incremental.data.contains("FIRST"));
        assert!(incremental.output_complete);
        let mut ui_bytes = ui.snapshot;
        ui_bytes.extend(live);
        let raw = session
            .read_output(
                "job",
                &OutputQuery {
                    format: LogFormat::Raw,
                    ..Default::default()
                },
            )
            .expect("raw");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(raw.data)
                .expect("bytes"),
            ui_bytes
        );
        assert!(session
            .read_output(
                "other-job",
                &OutputQuery {
                    cursor: incremental.next_cursor,
                    ..Default::default()
                }
            )
            .is_err());
    }

    #[tokio::test]
    async fn managed_log_waits_have_deadlines_and_release_receivers_on_cancellation() {
        use crate::pty::output::{read_waiting, OutputQuery};
        if !pty_available() {
            return;
        }
        let mut opts = sh_opts("logs-wait", "");
        opts.dir = std::env::temp_dir().to_string_lossy().into_owned();
        let session = Session::new_program(
            opts,
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "read line; printf FINAL".into(),
            ],
        )
        .expect("job");
        let empty = read_waiting(
            session.as_ref(),
            "job",
            &OutputQuery::default(),
            Duration::from_millis(20),
        )
        .await
        .expect("deadline");
        assert!(empty.data.is_empty() && !empty.output_complete);
        let reader = session.clone();
        let task = tokio::spawn(async move {
            read_waiting(
                reader.as_ref(),
                "job",
                &OutputQuery::default(),
                Duration::from_secs(30),
            )
            .await
        });
        assert!(wait_until(TIMEOUT, || session.output.changed.receiver_count() == 1).await);
        task.abort();
        let _ = task.await;
        assert_eq!(session.output.changed.receiver_count(), 0);
        assert!(session
            .output
            .subscribers
            .lock()
            .expect("subscribers")
            .senders
            .is_empty());
        // Subscribe before input; the watch-based reader cannot lose the first wakeup.
        let reader = session.clone();
        let task = tokio::spawn(async move {
            read_waiting(
                reader.as_ref(),
                "job",
                &OutputQuery::default(),
                Duration::from_secs(5),
            )
            .await
        });
        assert!(wait_until(TIMEOUT, || session.output.changed.receiver_count() == 1).await);
        session.write(b"finish\n").await.expect("finish");
        assert!(!task.await.expect("reader").expect("output").data.is_empty());
        assert!(wait_until(TIMEOUT, || session.output_complete()).await);
        let final_page = session
            .read_output("job", &OutputQuery::default())
            .expect("final");
        assert!(final_page.output_complete && final_page.data.contains("FINAL"));
        let eof = read_waiting(
            session.as_ref(),
            "job",
            &OutputQuery {
                cursor: final_page.next_cursor,
                ..Default::default()
            },
            Duration::from_secs(30),
        )
        .await
        .expect("EOF");
        assert!(eof.data.is_empty() && eof.output_complete);
    }

    #[tokio::test]
    async fn managed_busy_output_is_bounded_without_a_ui() {
        use crate::pty::output::{LogFormat, OutputQuery};
        if !pty_available() {
            return;
        }
        let mut opts = sh_opts("logs-busy", "");
        opts.dir = std::env::temp_dir().to_string_lossy().into_owned();
        let session = Session::new_program(
            opts,
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "read line; seq 1 300000".into(),
            ],
        )
        .expect("job");
        let start = session
            .read_output(
                "job",
                &OutputQuery {
                    tail: Some(0),
                    ..Default::default()
                },
            )
            .expect("end cursor");
        session.write(b"go\n").await.expect("produce");
        assert!(wait_until(TIMEOUT, || session.output_complete()).await);
        let page = session
            .read_output(
                "job",
                &OutputQuery {
                    cursor: start.next_cursor,
                    limit: 64,
                    ..Default::default()
                },
            )
            .expect("bounded text");
        assert!(page.lost_bytes > 0 && page.has_more && page.data.len() <= 64);
        let raw = session
            .read_output(
                "job",
                &OutputQuery {
                    format: LogFormat::Raw,
                    limit: 64,
                    ..Default::default()
                },
            )
            .expect("bounded raw");
        assert!(raw.has_more);
        assert!(session
            .output
            .subscribers
            .lock()
            .expect("subscribers")
            .senders
            .is_empty());
        let late = session.subscribe();
        assert!(text(&late.snapshot).ends_with("300000\r\n"));
        assert!(late.snapshot.len() <= DEFAULT_SCROLLBACK_SIZE);
    }

    #[tokio::test]
    async fn direct_program_preserves_argv_cwd_and_exact_exit() {
        if !pty_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("working dir");
        let mut opts = sh_opts("direct", "must not execute");
        opts.dir = dir.path().to_string_lossy().into_owned();
        opts.shell_profile_id = "unused-profile".into();
        opts.command = "touch unexpected-replay".into();
        let session = Session::new_program(
            opts,
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf '%s\\n' \"$PWD\" \"$1\" \"$2\"; exit 23".into(),
                "argv-test".into(),
                "$(touch marker)".into(),
                "two words".into(),
            ],
        )
        .expect("direct launch");
        let mut output = session.subscribe();
        let (live, ended) = receive(&mut output, |_| false).await;
        assert!(ended);
        let mut bytes = output.snapshot;
        bytes.extend(live);
        let output = text(&bytes);
        assert!(output.contains(&dir.path().to_string_lossy().to_string()));
        assert!(output.contains("$(touch marker)\r\ntwo words"));
        assert!(!dir.path().join("marker").exists());
        assert!(!dir.path().join("unexpected-replay").exists());
        assert!(session.shell_profile_id().is_none());
        assert!(wait_until(TIMEOUT, || *session.done().borrow()).await);
        assert_eq!(session.process_result().expect("exit").exit_code, Some(23));
    }

    #[tokio::test]
    async fn direct_program_rejects_missing_directory_and_executable() {
        if !pty_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("working dir");
        let mut opts = sh_opts("invalid", "");
        opts.dir = dir.path().join("missing").to_string_lossy().into_owned();
        assert!(Session::new_program(opts.clone(), vec!["/bin/sh".into()]).is_err());
        opts.dir = dir.path().to_string_lossy().into_owned();
        assert!(Session::new_program(
            opts,
            vec![dir
                .path()
                .join("missing-program")
                .to_string_lossy()
                .into_owned()]
        )
        .is_err());
    }

    #[tokio::test]
    async fn managed_stop_escalates_and_retains_final_output() {
        if !pty_available() {
            return;
        }
        let mut opts = sh_opts("stop-managed", "");
        opts.dir = std::env::temp_dir().to_string_lossy().into_owned();
        let session = Session::new_program(
            opts,
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "trap '' TERM; printf ready; while :; do sleep 1; done".into(),
            ],
        )
        .expect("spawn");
        assert!(wait_until(TIMEOUT, || text(&session.scrollback()).contains("ready")).await);
        let (a, b) = tokio::join!(session.stop(), session.stop());
        assert_eq!(a.expect("first stop").signal.as_deref(), Some("SIGKILL"));
        assert_eq!(
            b.expect("second stop"),
            session.process_result().expect("exit")
        );
        assert!(text(&session.scrollback()).contains("ready"));
        assert_eq!(
            session.stop().await.expect("already stopped"),
            session.process_result().expect("known result")
        );
    }

    #[tokio::test]
    async fn signal_exit_is_not_a_fabricated_exit_code() {
        if !pty_available() {
            return;
        }
        let session = spawn("signal", "kill -TERM $$");
        assert!(wait_until(TIMEOUT, || *session.done().borrow()).await);
        let result = session.process_result().expect("result");
        assert_eq!(result.exit_code, None);
        assert_eq!(result.signal.as_deref(), Some("SIGTERM"));
    }

    fn spawn(id: &str, script: &str) -> Arc<Session> {
        Session::new(sh_opts(id, script)).expect("spawn sh")
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// Receive until `until` holds for everything received so far, the stream
    /// ends, or [`TIMEOUT`] passes. Returns what was received and whether the
    /// stream ended.
    async fn receive(
        sub: &mut Subscription,
        mut until: impl FnMut(&[u8]) -> bool,
    ) -> (Vec<u8>, bool) {
        let mut got = Vec::new();
        let mut ended = false;
        let _ = tokio::time::timeout(TIMEOUT, async {
            while !until(&got) {
                match sub.receiver.recv().await {
                    Some(chunk) => got.extend_from_slice(&chunk),
                    None => {
                        ended = true;
                        break;
                    }
                }
            }
        })
        .await;
        (got, ended)
    }

    /// Input written to a real PTY reaches the process, and its output comes back.
    #[tokio::test]
    async fn real_pty_round_trips_input_and_output() {
        if !pty_available() {
            return;
        }
        let session = spawn("round-trip", "read line; echo \"got $line\"; sleep 5");
        assert_eq!(session.session_id(), "round-trip");
        let mut sub = session.subscribe();

        session.write(b"ping\n").await.unwrap();

        let mut output = sub.snapshot.clone();
        let (got, _) = receive(&mut sub, |got| text(got).contains("got ping")).await;
        output.extend_from_slice(&got);
        assert!(
            text(&output).contains("got ping"),
            "output: {:?}",
            text(&output)
        );

        session.close().unwrap();
    }

    /// terminal-sessions, Output while nobody streams: output and title
    /// tracking go on without any subscriber.
    #[tokio::test]
    async fn output_while_nobody_subscribes_reaches_the_scrollback() {
        if !pty_available() {
            return;
        }
        let session = spawn(
            "unwatched",
            "printf '\\033]2;unwatched-title\\007'; head -c 10240 /dev/zero | tr '\\000' x; sleep 5",
        );
        let x_count = |bytes: &[u8]| bytes.iter().filter(|&&b| b == b'x').count();

        let captured = wait_until(TIMEOUT, || x_count(&session.scrollback()) == 10240).await;
        assert!(
            captured,
            "scrollback: {} x of 10240",
            x_count(&session.scrollback())
        );
        assert_eq!(session.osc_title_watch().borrow().title, "unwatched-title");

        // An attach afterwards replays it.
        assert_eq!(x_count(&session.subscribe().snapshot), 10240);
        session.close().unwrap();
    }

    /// A snapshot and its receiver line up exactly, even while output flows:
    /// once the output has ended, the two together equal the scrollback.
    #[tokio::test]
    async fn subscription_snapshot_and_receiver_line_up() {
        if !pty_available() {
            return;
        }
        // About 30 KB in 50 bursts over half a second, far below the 1 MiB
        // scrollback, so the scrollback ends up holding all of it.
        let session = spawn(
            "line-up",
            "for i in $(seq 1 50); do seq $((i*100)) $((i*100+99)); sleep 0.01; done",
        );
        assert!(
            wait_until(TIMEOUT, || !session.scrollback().is_empty()).await,
            "no output"
        );

        let mut sub = session.subscribe();
        let (got, ended) = receive(&mut sub, |_| false).await;
        assert!(ended, "output did not end within {TIMEOUT:?}");

        let mut combined = sub.snapshot.clone();
        combined.extend_from_slice(&got);
        let scrollback = session.scrollback();
        assert!(
            text(&scrollback).contains("5099"),
            "output incomplete: {} bytes",
            scrollback.len()
        );
        assert!(
            combined == scrollback,
            "snapshot {} + received {} bytes != scrollback {} bytes",
            sub.snapshot.len(),
            got.len(),
            scrollback.len()
        );
    }

    /// terminal-sessions, Busy process without a client: a process printing far
    /// more than the PTY buffer holds runs to completion with nobody reading.
    #[tokio::test]
    async fn busy_process_without_subscriber_runs_to_completion() {
        if !pty_available() {
            return;
        }
        // About 1.4 MB: kilobytes of PTY buffer, more than the 1 MiB scrollback.
        let session = spawn("busy", "seq 1 200000");
        let mut done = session.done();

        let finished = tokio::time::timeout(TIMEOUT, done.wait_for(|d| *d))
            .await
            .is_ok();
        assert!(
            finished,
            "the process blocked; scrollback has {} bytes",
            session.scrollback().len()
        );

        let newest = wait_until(TIMEOUT, || {
            text(&session.scrollback()).trim_end().ends_with("\n200000")
        })
        .await;
        let scrollback = session.scrollback();
        assert!(
            newest,
            "newest output missing; tail: {:?}",
            text(&scrollback[scrollback.len().saturating_sub(40)..])
        );
        assert!(
            !scrollback.starts_with(b"1\r\n2\r\n"),
            "oldest output should have been dropped"
        );
    }

    /// Subscribers see the end of the stream when the process exits, and a
    /// subscriber arriving later gets the snapshot and an ended stream.
    #[tokio::test]
    async fn subscribers_see_the_end_when_the_process_exits() {
        if !pty_available() {
            return;
        }
        let session = spawn("ends", "echo bye; sleep 0.2");
        let mut sub = session.subscribe();

        let (got, ended) = receive(&mut sub, |_| false).await;
        assert!(ended, "stream did not end after the process exited");
        let mut output = sub.snapshot.clone();
        output.extend_from_slice(&got);
        assert!(text(&output).contains("bye"), "output: {:?}", text(&output));

        let mut late = session.subscribe();
        assert!(text(&late.snapshot).contains("bye"));
        let next = tokio::time::timeout(TIMEOUT, late.receiver.recv()).await;
        assert!(
            matches!(next, Ok(None)),
            "late subscriber's stream should be ended"
        );
    }

    /// Closing hangs up the terminal's processes and ends the output, although
    /// the reader thread holds its own descriptor of the PTY master.
    #[tokio::test]
    async fn close_hangs_up_the_process() {
        if !pty_available() {
            return;
        }
        let session = spawn("hang-up", "sleep 30");
        let mut sub = session.subscribe();
        let mut done = session.done();

        session.close().unwrap();

        let exited = tokio::time::timeout(TIMEOUT, done.wait_for(|d| *d))
            .await
            .is_ok();
        assert!(exited, "process still running after close");
        let (_, ended) = receive(&mut sub, |_| false).await;
        assert!(ended, "output did not end after close");
    }

    #[test]
    fn test_bash_integration_long_option_precedes_login_argument() {
        let dir = tempfile::TempDir::new().unwrap();
        let bash_dir = dir.path().join("bash");
        std::fs::create_dir_all(&bash_dir).unwrap();
        std::fs::write(bash_dir.join("splitty.bash"), "").unwrap();

        let integration_dir = dir.path().to_string_lossy();
        let cmd = build_shell_command(
            "/bin/bash",
            &["-l".to_string()],
            Some(integration_dir.as_ref()),
            true,
        );
        let argv = cmd
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(argv, vec!["/bin/bash", "--posix", "-l"]);
    }
}

#[cfg(test)]
mod spec_tests {
    use super::*;

    fn opts() -> CreateSessionOpts {
        CreateSessionOpts {
            id: "spec-1".into(),
            shell_profile_id: String::new(),
            shell: "/bin/sh".into(),
            shell_args: vec!["-l".into()],
            cols: 0,
            rows: 0,
            dir: String::new(),
            command: String::new(),
            restore_mode: String::new(),
            extra_env: vec![
                "SPLITTY_PANE_ID=p1".into(),
                "SPLITTY_SCREEN_ID=s1".into(),
                "SPLITTY_SOCK=/run/splitty/daemon.sock".into(),
            ],
        }
    }

    #[test]
    fn a_shell_spec_carries_the_environment_the_terminal_starts_with() {
        let (spec, inject) = build_spec(&opts(), None, None).expect("spec");
        assert_eq!(spec.session_id, "spec-1");
        assert_eq!(spec.argv.first().map(String::as_str), Some("/bin/sh"));
        assert!(spec.argv.contains(&"-l".to_owned()));
        assert_eq!((spec.cols, spec.rows), (80, 24));
        assert!(!spec.managed);
        assert_eq!(spec.pane_id.as_deref(), Some("p1"));
        assert_eq!(spec.screen_id.as_deref(), Some("s1"));
        assert_eq!(inject, None);
        let env = |key: &str| {
            spec.env
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(env("TERM"), Some("xterm-256color"));
        assert_eq!(env("SPLITTY_SESSION_ID"), Some("spec-1"));
        assert_eq!(env("SPLITTY_SOCK"), Some("/run/splitty/daemon.sock"));
        assert_eq!(env("SPLITTY_TOKEN"), None, "never a credential");
        assert!(spec.cwd.is_some(), "the home directory by default");
    }

    #[test]
    fn a_program_spec_keeps_its_argv_and_directory() {
        let mut opts = opts();
        opts.dir = "/".into();
        let argv = vec!["/usr/bin/env".to_owned(), "a b".to_owned()];
        let (spec, _) = build_spec(&opts, Some(&argv), None).expect("spec");
        assert_eq!(spec.argv, argv);
        assert_eq!(spec.cwd.as_deref(), Some("/"));
        assert!(spec.managed);
    }

    /// A terminal that replays what a keeper buffered, then waits.
    struct Replayed {
        done: tokio::sync::watch::Receiver<bool>,
    }

    #[async_trait]
    impl Terminal for Replayed {
        fn pid(&self) -> Option<u32> {
            Some(4242)
        }
        async fn write(&self, _data: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn resize(&self, _cols: u16, _rows: u16) -> io::Result<()> {
            Ok(())
        }
        fn close(&self) -> io::Result<()> {
            Ok(())
        }
        async fn stop(&self) -> io::Result<crate::pty::ProcessResult> {
            Err(io::Error::other("not managed"))
        }
        fn done(&self) -> tokio::sync::watch::Receiver<bool> {
            self.done.clone()
        }
        fn result(&self) -> Option<crate::pty::ProcessResult> {
            None
        }
        fn foreground_pgid(&self) -> Option<u32> {
            None
        }
    }

    #[test]
    fn an_adopted_session_fills_its_buffers_from_the_replay() {
        let session = Session::with_terminal("adopted".into(), None, false, 1024, |feed| {
            feed.record(b"before\x1b]2;vim\x07");
            Ok(Box::new(Replayed {
                done: tokio::sync::watch::channel(false).1,
            }))
        })
        .expect("adopted");
        assert_eq!(session.scrollback(), b"before\x1b]2;vim\x07");
        assert_eq!(session.osc_title_watch().borrow().title, "vim");
        assert_eq!(session.shell_pid(), Some(4242));
    }
}
