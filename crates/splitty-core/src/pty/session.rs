//! A single terminal session: the session layer over a [`Terminal`].
//!
//! Handles shell resolution and the session's spec (program, environment,
//! directory, size), scrollback buffering, OSC title scanning, terminal mode
//! tracking, program status records and the job log text. The terminal under
//! it, in this process or in a session keeper, feeds all output from spawn to
//! exit into the session; connections subscribe to it.
//!
//! The status probe (`OSC 7501 ; ?`) is removed from the output first, before
//! anything records or forwards it, and its answer is queued on the session's
//! input. Every write to the terminal goes through one input gate, held for
//! the whole write: a writer first writes the queued answers, then its own
//! bytes, and a task writes answers while nobody else writes. So an answer
//! lands between two writes, never inside one, and ahead of any input a
//! client sends after it received the output that carried the probe.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use portable_pty::CommandBuilder;
use splitty_transport::queue;
use tokio::sync::watch;

use super::modes::ModeTracker;
use super::osc::{OscFinding, OscScanner, SemanticAction};
use super::probe::ProbeMatcher;
use super::procinfo;
use super::ringbuffer::RingBuffer;
use super::startup::build_command_args;
use super::status::{RootStatus, StatusStore};
use super::terminal::{Feed, LocalTerminals, SessionSpec, Terminal, TerminalFactory};
use super::{
    BellEvent, CreateSessionOpts, OscTitleEvent, PtyError, SemanticEvent, SemanticEventInner,
    SemanticState, SessionHandle, SessionStats, Subscription, TitleInfo,
};

const DEFAULT_SCROLLBACK_SIZE: usize = 1 << 20; // 1 MB

/// The status probe answers that wait for the input gate at most; a probe
/// beyond them gets no answer.
pub const MAX_WAITING_ANSWERS: usize = 16;

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
    shell_profile_fallback_reason: std::sync::OnceLock<String>,
    terminal: Box<dyn Terminal>,
    managed: bool,
    exit_reported: AtomicBool,
    /// Shared with the terminal, which feeds it.
    output: Arc<Output>,
    /// Shared with the output, which queues the probe answers.
    input: Arc<Input>,
    osc_rx: watch::Receiver<OscTitleEvent>,
    semantic_rx: watch::Receiver<SemanticEvent>,
    bell_rx: watch::Receiver<BellEvent>,
}

/// The session's input: one gate every write to the terminal holds, and the
/// status probe answers waiting for it.
struct Input {
    gate: tokio::sync::Mutex<()>,
    answers: Mutex<VecDeque<&'static [u8]>>,
    /// Wakes the task that writes answers while nobody else writes.
    wake: tokio::sync::Notify,
    closed: AtomicBool,
}

impl Input {
    fn new() -> Self {
        Self {
            gate: tokio::sync::Mutex::new(()),
            answers: Mutex::new(VecDeque::new()),
            wake: tokio::sync::Notify::new(),
            closed: AtomicBool::new(false),
        }
    }

    /// Queue an answer; false when [`MAX_WAITING_ANSWERS`] wait already.
    fn queue(&self, answer: &'static [u8]) -> bool {
        let mut answers = self.answers.lock().expect("probe answers");
        if answers.len() >= MAX_WAITING_ANSWERS {
            return false;
        }
        answers.push_back(answer);
        drop(answers);
        self.wake.notify_one();
        true
    }

    fn next(&self) -> Option<&'static [u8]> {
        self.answers.lock().expect("probe answers").pop_front()
    }

    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.answers.lock().expect("probe answers").len()
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }
}

/// What the session records for every chunk of its terminal's output.
struct Output {
    /// Held while a chunk is recorded and fanned out, and while a subscriber
    /// takes its snapshot, so every byte lands in a subscriber's snapshot or
    /// in its receiver, never both and never neither.
    subscribers: Mutex<Subscribers>,
    input: Arc<Input>,
    scrollback: RingBuffer,
    /// The modes the output leaves set, for the start of a subscriber's
    /// snapshot.
    modes: Mutex<ModeTracker>,
    text: Option<(Mutex<super::log_text::LogText>, RingBuffer)>,
    generation: String,
    changed: watch::Sender<u64>,
    osc: Mutex<OscScanner>,
    osc_tx: watch::Sender<OscTitleEvent>,
    semantic_state: Mutex<SemanticState>,
    semantic_tx: watch::Sender<SemanticEvent>,
    bell_tx: watch::Sender<BellEvent>,
    status: Mutex<StatusStore>,
    /// The root record's revision, for a check without the lock.
    status_revision: AtomicU64,
}

struct Subscribers {
    senders: Vec<queue::Sender<Arc<[u8]>>>,
    /// Set at EOF: later subscribers get the snapshot and an ended receiver.
    ended: bool,
    /// Removes the status probes before anything else sees the output.
    probe: ProbeMatcher,
}

/// Unix milliseconds, server clock.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

impl Output {
    /// Record one chunk of live output.
    fn record(&self, data: &[u8]) {
        self.record_from(data, false);
    }

    /// Remove the status probes, queue their answers unless the output is
    /// read again, and record the rest.
    fn record_from(&self, data: &[u8], read_again: bool) {
        let mut subscribers = self.subscribers.lock().unwrap();
        let (data, probes): (Cow<'_, [u8]>, _) = subscribers.probe.filter(data);
        if !read_again {
            for probe in probes {
                if !self.input.queue(probe.answer()) {
                    tracing::debug!("a status probe got no answer: too many wait");
                }
            }
        }
        if !data.is_empty() {
            self.record_locked(&mut subscribers, &data, read_again);
        }
    }

    /// Record a chunk without probes, under the output lock: scrollback,
    /// terminal modes, log text, title, prompt and status tracking,
    /// subscribers.
    fn record_locked(&self, subscribers: &mut Subscribers, data: &[u8], read_again: bool) {
        self.scrollback.write(data);
        self.modes
            .lock()
            .expect("mode tracker lock")
            .feed(data, self.scrollback.oldest());
        if let Some((parser, ring)) = &self.text {
            ring.write(&parser.lock().expect("log parser lock").push(data));
        }
        self.scan(data, read_again);
        let mut chunk = None;
        // Acquire the first recipient's allowance before allocating. A stalled
        // recipient closes independently while capture and other readers run.
        subscribers.senders.retain(|tx| {
            tx.send(data.len(), false, || {
                Arc::clone(chunk.get_or_insert_with(|| Arc::from(data)))
            })
            .is_ok()
        });
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    /// Feed the OSC scanner, apply what it found in the order of the output,
    /// and publish title, OSC 133, status and bell updates.
    fn scan(&self, data: &[u8], read_again: bool) {
        let Ok(mut osc) = self.osc.lock() else { return };
        if osc.feed(data) {
            let _ = self.osc_tx.send(OscTitleEvent {
                title: osc.title().to_string(),
                changed_at: Instant::now(),
            });
        }
        let mut bell = false;
        for finding in osc.drain() {
            match finding {
                OscFinding::Title => {}
                OscFinding::Semantic(action) => {
                    if action == SemanticAction::PromptStart {
                        self.change_status(StatusStore::end_program);
                    }
                    if let Ok(mut state) = self.semantic_state.lock() {
                        state.apply(&action);
                    }
                    let _ = self.semantic_tx.send(Some(SemanticEventInner {
                        action,
                        timestamp: Instant::now(),
                    }));
                }
                OscFinding::Status(payload) => {
                    let now = now_millis();
                    self.change_status(|store| store.apply(&payload, read_again, now));
                }
                OscFinding::FullReset => self.change_status(StatusStore::reset),
                OscFinding::Bell => bell = true,
            }
        }
        if bell {
            let _ = self.bell_tx.send(Some(Instant::now()));
        }
    }

    /// Change the status records and publish the root record's revision.
    fn change_status(&self, change: impl FnOnce(&mut StatusStore) -> bool) {
        let mut store = self.status.lock().expect("status records");
        if change(&mut store) {
            self.status_revision
                .store(store.revision(), Ordering::Release);
        }
    }

    /// The terminal closed: end every subscription once it has drained, and
    /// the records of the program with it.
    fn end(&self) {
        let mut subscribers = self.subscribers.lock().unwrap();
        // A beginning of the probe the output ended in was none.
        let held = subscribers.probe.finish();
        if !held.is_empty() {
            self.record_locked(&mut subscribers, &held, false);
        }
        self.change_status(StatusStore::end_program);
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

    fn subscribe(&self, context: queue::Context) -> Subscription {
        let mut subscribers = self.subscribers.lock().unwrap();
        // The modes the buffered output starts in go ahead of it, so a fresh
        // terminal reads that output as the session's terminal did. A few
        // hundred bytes at most, charged with the snapshot.
        let restore = self
            .modes
            .lock()
            .expect("mode tracker lock")
            .restore(self.scrollback.oldest());
        let length = restore.len() + self.scrollback.len();
        let snapshot = context
            .budget
            .charge(length, false, || {
                let mut snapshot = Vec::with_capacity(length);
                snapshot.extend_from_slice(&restore);
                self.scrollback.read_all_into(&mut snapshot);
                snapshot
            })
            .unwrap_or_else(|_| {
                context.close();
                Default::default()
            });
        let (tx, receiver) = context.channel(queue::QUEUE_ITEMS);
        if !subscribers.ended && !context.is_closed() {
            subscribers.senders.push(tx);
        }
        Subscription { snapshot, receiver }
    }
}

impl Feed for Output {
    fn record(&self, data: &[u8]) {
        Output::record(self, data);
    }

    fn record_again(&self, data: &[u8]) {
        Output::record_from(self, data, true);
    }

    fn end(&self) {
        Output::end(self);
    }
}

impl Session {
    pub(crate) fn set_shell_profile_fallback_reason(&self, reason: String) {
        let _ = self.shell_profile_fallback_reason.set(reason);
    }
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
                    let _ = SessionHandle::write(session.as_ref(), cmd_bytes.as_bytes()).await;
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
        let input = Arc::new(Input::new());
        let output = Arc::new(Output {
            subscribers: Mutex::new(Subscribers {
                senders: Vec::new(),
                ended: false,
                probe: ProbeMatcher::new(),
            }),
            input: Arc::clone(&input),
            scrollback: RingBuffer::new(buffer_size),
            modes: Mutex::new(ModeTracker::new()),
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
            status: Mutex::new(StatusStore::new()),
            status_revision: AtomicU64::new(0),
        });
        let terminal = start(Arc::clone(&output) as Arc<dyn Feed>)?;
        let session = Arc::new(Session {
            id,
            shell_profile_id,
            shell_profile_fallback_reason: std::sync::OnceLock::new(),
            terminal,
            managed,
            exit_reported: AtomicBool::new(false),
            output,
            input: Arc::clone(&input),
            osc_rx,
            semantic_rx,
            bell_rx,
        });
        // Without a runtime (a synchronous test) answers wait for the next
        // write.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(write_answers_while_idle(Arc::downgrade(&session), input));
        }
        Ok(session)
    }

    /// Write the queued probe answers; the caller holds the input gate.
    async fn write_answers(&self) {
        while let Some(answer) = self.input.next() {
            if let Err(error) = self.terminal.write(answer).await {
                tracing::warn!(session = %self.id, "a status probe answer was not written: {error}");
            }
        }
    }
}

/// Write a session's probe answers whenever one is queued, holding its input
/// gate, until the session is gone.
async fn write_answers_while_idle(session: Weak<Session>, input: Arc<Input>) {
    loop {
        input.wake.notified().await;
        if input.closed.load(Ordering::SeqCst) {
            return;
        }
        let Some(session) = session.upgrade() else {
            return;
        };
        let _gate = session.input.gate.lock().await;
        session.write_answers().await;
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
    // arguments: Bash requires GNU long options such as `--noprofile` before
    // short options such as the detected profile's `-l`.
    let integration_dir = find_shell_integration_dir();
    let integration_dir_str = integration_dir
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned());
    let (mut cmd, bash_bootstrap) = if let Some(argv) = argv {
        let mut cmd = CommandBuilder::new(&argv[0]);
        cmd.args(&argv[1..]);
        (cmd, None)
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
    // Both renderers open OSC 8 hyperlinks, but programs such as Claude Code
    // send them only to terminals they recognize. A TUI that wraps a long URL
    // itself breaks it into separate lines, and only a hyperlink still opens
    // the whole URL from every part of it.
    cmd.env("FORCE_HYPERLINK", "1");
    cmd.env("SPLITTY_SESSION_ID", &id);

    // Add extra environment variables
    for env_str in &opts.extra_env {
        if let Some((key, val)) = env_str.split_once('=') {
            cmd.env(key, val);
        }
    }

    // Private bootstrap state comes only from selected startup options, never
    // inherited environment or extra_env. Capture the final user prompt hook.
    for key in [
        "SPLITTY_BASH_INJECT",
        "SPLITTY_BASH_PROMPT_COMMAND",
        "SPLITTY_BASH_RCFILE",
        "SPLITTY_BASH_HISTFILE",
        "SPLITTY_BASH_HISTSIZE",
        "SPLITTY_BASH_HISTFILESIZE",
    ] {
        cmd.env_remove(key);
    }
    if let Some(bootstrap) = bash_bootstrap {
        // Defer Bash's automatic history read/truncation until profiles have
        // chosen the file and limits. Preserve exact inherited presence/value.
        for key in ["HISTFILE", "HISTSIZE", "HISTFILESIZE"] {
            let original = cmd
                .iter_full_env_as_str()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_owned());
            if let Some(value) = original {
                cmd.env(format!("SPLITTY_BASH_{key}"), value);
            }
        }
        cmd.env("HISTFILE", "");
        cmd.env("SPLITTY_BASH_INJECT", bootstrap.flags);
        if let Some(path) = bootstrap.rcfile {
            cmd.env("SPLITTY_BASH_RCFILE", path);
        }
        let original = cmd
            .iter_full_env_as_str()
            .find(|(key, _)| *key == "PROMPT_COMMAND")
            .map(|(_, value)| value.to_owned());
        cmd.env_remove("SPLITTY_BASH_PROMPT_COMMAND");
        if let Some(value) = original {
            cmd.env("SPLITTY_BASH_PROMPT_COMMAND", value);
        }
        // Execute every populated prompt-command array entry at the initial
        // prompt, at top level so a user's `declare` keeps its normal scope.
        // Bash handles subsequent scalar/array prompts itself.
        cmd.env(
            "PROMPT_COMMAND",
            "builtin source \"$SPLITTY_SHELL_INTEGRATION/bash/splitty.bash\" && for __splitty_prompt_command in \"${PROMPT_COMMAND[@]-}\"; do builtin eval -- \"$__splitty_prompt_command\"; done; builtin unset __splitty_prompt_command",
        );
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

    fn shell_profile_fallback_reason(&self) -> Option<&str> {
        self.shell_profile_fallback_reason.get().map(String::as_str)
    }

    fn shell_pid(&self) -> Option<u32> {
        // Once the shell has exited, its pid may belong to another process.
        if *self.terminal.done().borrow() {
            return None;
        }
        self.terminal.pid()
    }

    fn subscribe_with(&self, context: queue::Context) -> Subscription {
        self.output.subscribe(context)
    }

    async fn write(&self, data: &[u8]) -> io::Result<()> {
        let _gate = self.input.gate.lock().await;
        self.write_answers().await;
        self.terminal.write(data).await
    }

    fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.terminal.resize(cols, rows)
    }

    fn close(&self) -> io::Result<()> {
        self.terminal.close()
    }

    fn begin_shutdown(&self) -> io::Result<()> {
        self.terminal.begin_shutdown()
    }

    async fn shutdown(&self) -> io::Result<super::ProcessResult> {
        self.terminal.shutdown().await
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

    fn program_status_revision(&self) -> u64 {
        self.output.status_revision.load(Ordering::Acquire)
    }

    fn program_status(&self) -> RootStatus {
        self.output.status.lock().expect("status records").root()
    }

    fn end_program_status(&self) {
        self.output.change_status(StatusStore::end_program);
    }
}

impl Drop for Session {
    /// A session dropped without `close` hangs up like a closed one, as it did
    /// when dropping it closed the PTY; a released one leaves its program
    /// running.
    fn drop(&mut self) {
        self.input.close();
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
            // Apple Bash 3.2 ignores --posix while choosing interactive
            // startup files. Bootstrap at its first prompt instead, keeping
            // login_shell intact while the integration loads user files.
            let script = format!("{}/bash/splitty.bash", integration_dir);
            if !std::path::Path::new(&script).exists() {
                return;
            }
            cmd.arg("--noprofile");
            cmd.arg("--norc");
        }
        _ => {}
    }
}

struct BashBootstrap {
    flags: String,
    rcfile: Option<String>,
}

fn build_shell_command(
    shell: &str,
    command_args: &[String],
    integration_dir: Option<&str>,
    plain_shell: bool,
) -> (CommandBuilder, Option<BashBootstrap>) {
    let mut cmd = CommandBuilder::new(shell);
    // Parse only startup options, never the command text or its positional
    // arguments. Bash accepts grouped short options and -o/-O values.
    let mut command = false;
    let mut interactive = false;
    let mut payload = None;
    let mut flags = String::from("1");
    let mut rcfile = None;
    let mut index = 0;
    while index < command_args.len() {
        let arg = &command_args[index];
        match arg.as_str() {
            "--noprofile" | "-noprofile" => flags.push_str(" --noprofile"),
            "--norc" | "-norc" => flags.push_str(" --norc"),
            "--login" | "-login" => {}
            "--rcfile" | "-rcfile" | "--init-file" | "-init-file" => {
                index += 1;
                rcfile = command_args.get(index);
            }
            "-o" | "+o" | "-O" | "+O" => {
                index += 1;
            }
            "--" => {
                payload = command_args.get(index + 1).map(|_| index + 1);
                break;
            }
            _ if arg.starts_with("--") => {}
            _ if arg.starts_with('-') && arg.len() > 1 => {
                command |= arg.contains('c');
                interactive |= arg.contains('i');
            }
            _ if arg.starts_with('+') && arg.len() > 1 => {}
            _ => {
                payload = Some(index);
                break;
            }
        }
        index += 1;
    }
    if command {
        flags.push_str(" --splitty-command");
    }
    let bash_inject = detect_shell(shell) == Some("bash")
        && plain_shell
        && (interactive || (!command && payload.is_none()))
        && integration_dir.is_some_and(|dir| {
            std::path::Path::new(dir)
                .join("bash/splitty.bash")
                .is_file()
        });
    if let Some(dir) = integration_dir {
        cmd.env("SPLITTY_SHELL_INTEGRATION", dir);
        setup_shell_integration(shell, &mut cmd, dir, bash_inject);
    }
    for (index, arg) in command_args.iter().enumerate() {
        if bash_inject && command && payload == Some(index) && integration_dir.is_some() {
            cmd.arg(format!(
                "builtin source \"$SPLITTY_SHELL_INTEGRATION/bash/splitty.bash\"\n{arg}"
            ));
        } else {
            cmd.arg(arg);
        }
    }
    (
        cmd,
        bash_inject.then(|| BashBootstrap {
            flags,
            rcfile: rcfile.cloned(),
        }),
    )
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

    #[test]
    #[cfg(unix)]
    fn bash_bootstrap_preserves_inherited_startup_environment() {
        for original in [None, Some(""), Some("printf '\\nINHERITED_PC_EXECUTED\\n'")] {
            let home = tempfile::tempdir().expect("home");
            let profile = home.path().join(".bash_profile");
            let content = "printf 'ENV:%s:<%s>\\n' \"${ENV+x}\" \"${ENV-}\"; printf 'PC:%s:<%s>\\n' \"${PROMPT_COMMAND+x}\" \"${PROMPT_COMMAND-}\"; printf 'PROFILE_ONCE\\n'; printf 'LOGIN:%s\\n' \"$(shopt -q login_shell && echo yes)\"\n";
            std::fs::write(&profile, content).expect("profile");
            let mut opts = sh_opts("posix-bootstrap", "");
            opts.shell = "/bin/bash".into();
            opts.shell_args = vec!["-l".into(), "-i".into()];
            opts.dir = home.path().to_string_lossy().into_owned();
            opts.extra_env = vec![
                format!("HOME={}", home.path().display()),
                "HISTFILE=/dev/null".into(),
            ];
            if let Some(value) = original {
                opts.extra_env.push(format!("ENV={value}"));
                opts.extra_env.push(format!("PROMPT_COMMAND={value}"));
            }
            let (mut spec, _) = build_spec(&opts, None, None).expect("spec");
            spec.env.retain(|(key, _)| key != "POSIXLY_CORRECT");
            if original.is_none() {
                spec.env.retain(|(key, _)| {
                    !matches!(key.as_str(), "ENV" | "SPLITTY_BASH_PROMPT_COMMAND")
                });
            }
            use std::io::Write;
            let mut child = std::process::Command::new(&spec.argv[0])
                .args(&spec.argv[1..])
                .env_clear()
                .envs(spec.env)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("bash");
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(
                    b"type __splitty_restore_cli_path; env | /usr/bin/grep '^ENV='; exit 0\n",
                )
                .expect("input");
            let output = child.wait_with_output().expect("output");
            let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
            assert!(output.status.success(), "{stdout}");
            assert_eq!(
                stdout
                    .lines()
                    .filter(|line| *line == "PROFILE_ONCE")
                    .count(),
                1,
                "{stdout}"
            );
            assert!(
                stdout.contains(&format!(
                    "ENV:{}:<{}>",
                    if original.is_some() { "x" } else { "" },
                    original.unwrap_or("")
                )),
                "{stdout}"
            );
            // System profiles may themselves initialize PROMPT_COMMAND. Match
            // what the same profile observes under native Bash startup.
            let mut native = std::process::Command::new("/bin/bash");
            native
                .args(["-ilc", "exit 0"])
                .env_remove("POSIXLY_CORRECT")
                .env_remove("PROMPT_COMMAND")
                .env_remove("ENV")
                .env("HOME", home.path())
                .env("HISTFILE", "/dev/null")
                .env("TERM", "xterm-256color");
            if let Some(value) = original {
                native.env("PROMPT_COMMAND", value).env("ENV", value);
            }
            let native = native.output().expect("native bash");
            let native = String::from_utf8(native.stdout).expect("native output");
            assert_eq!(
                stdout.lines().find(|line| line.starts_with("PC:")),
                native.lines().find(|line| line.starts_with("PC:")),
                "{stdout}\nNative: {native}"
            );
            assert!(stdout.contains("LOGIN:yes"), "{stdout}");
            assert!(
                stdout.contains("__splitty_restore_cli_path is a function"),
                "{stdout}"
            );
            assert_eq!(
                stdout.lines().find_map(|line| line.strip_prefix("ENV=")),
                original,
                "{stdout}"
            );
            if original.is_some_and(|value| !value.is_empty()) {
                assert_eq!(
                    stdout
                        .lines()
                        .filter(|line| *line == "INHERITED_PC_EXECUTED")
                        .count(),
                    1,
                    "{stdout}"
                );
            }
            assert_eq!(std::fs::read_to_string(profile).expect("profile"), content);
        }
    }

    #[test]
    #[cfg(unix)]
    fn bash_interactive_command_bootstraps_before_its_first_command() {
        let home = tempfile::tempdir().expect("home");
        std::fs::write(
            home.path().join(".bash_profile"),
            "printf 'PROFILE_ONCE\\n'\nalias profile_alias='printf \"FIRST:<%s>:<%s>\\n\"'\n",
        )
        .expect("profile");
        let mut opts = sh_opts("immediate-bash", "");
        opts.shell = "/bin/bash".into();
        opts.shell_args = vec!["-ilc".into(),
            "profile_alias \"$0\" \"$1\"; type __splitty_restore_cli_path; printf '%s' '--noprofile --norc'; exit 7".into(),
            "command-name".into(), "literal argument".into()];
        opts.extra_env = vec![
            format!("HOME={}", home.path().display()),
            "HISTFILE=/dev/null".into(),
        ];
        let (spec, _) = build_spec(&opts, None, None).expect("spec");
        let output = std::process::Command::new(&spec.argv[0])
            .args(&spec.argv[1..])
            .env_clear()
            .envs(spec.env)
            .output()
            .expect("bash");
        let stdout = String::from_utf8(output.stdout).expect("output");
        assert_eq!(output.status.code(), Some(7), "{stdout}");
        assert_eq!(stdout.matches("PROFILE_ONCE").count(), 1, "{stdout}");
        assert!(
            stdout.contains("FIRST:<command-name>:<literal argument>"),
            "{stdout}"
        );
        assert!(
            stdout.contains("__splitty_restore_cli_path is a function"),
            "{stdout}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn bash_bootstrap_honors_startup_file_options() {
        let home = tempfile::tempdir().expect("home");
        for (name, marker) in [
            (".bash_profile", "PROFILE"),
            (".bashrc", "BASHRC"),
            ("custom rc", "CUSTOM"),
        ] {
            std::fs::write(home.path().join(name), format!("printf '{marker}\\n'\n"))
                .expect("startup file");
        }
        let custom = home.path().join("custom rc").to_string_lossy().into_owned();
        for (args, expected) in [
            (vec!["--noprofile", "-ilc"], None),
            (vec!["-noprofile", "-ilc"], None),
            (vec!["-login", "-ic"], Some("PROFILE")),
            (vec!["--norc", "-ic"], None),
            (vec!["-norc", "-ic"], None),
            (vec!["--rcfile", custom.as_str(), "-ic"], Some("CUSTOM")),
            (vec!["-rcfile", custom.as_str(), "-ic"], Some("CUSTOM")),
            (vec!["-init-file", custom.as_str(), "-ic"], Some("CUSTOM")),
            (vec!["-ic"], Some("BASHRC")),
        ] {
            let mut opts = sh_opts("startup-options", "");
            opts.shell = "/bin/bash".into();
            opts.shell_args = args.into_iter().map(str::to_owned).collect();
            opts.shell_args
                .push("type __splitty_restore_cli_path >/dev/null; exit".into());
            opts.extra_env = vec![
                format!("HOME={}", home.path().display()),
                "HISTFILE=/dev/null".into(),
            ];
            let (spec, _) = build_spec(&opts, None, None).expect("spec");
            let output = std::process::Command::new(&spec.argv[0])
                .args(&spec.argv[1..])
                .env_clear()
                .envs(spec.env)
                .output()
                .expect("bash");
            let stdout = String::from_utf8(output.stdout).expect("output");
            assert!(output.status.success(), "{stdout}");
            for marker in ["PROFILE", "BASHRC", "CUSTOM"] {
                assert_eq!(
                    stdout.lines().filter(|line| *line == marker).count(),
                    usize::from(expected == Some(marker)),
                    "{stdout}"
                );
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn bash_first_prompt_preserves_nounset_and_prompt_command_entries() {
        use std::io::Write;
        for posix in [false, true] {
            for (commands, expected) in [
                ("unset PROMPT_COMMAND", vec![]),
                ("PROMPT_COMMAND=\"printf 'USER_ONE\\n'\"", vec!["USER_ONE"]),
                (
                    "PROMPT_COMMAND=(\"declare PROMPT_DEFINED=global; printf 'USER_ONE\\n'\" \"printf 'USER_TWO\\n'\")",
                    vec!["USER_ONE", "USER_TWO"],
                ),
                (
                    "unset PROMPT_COMMAND; PROMPT_COMMAND[3]=\"printf 'USER_TWO\\n'\"",
                    vec!["USER_TWO"],
                ),
            ] {
                let home = tempfile::tempdir().expect("home");
                // Profiles may deliberately unset optional variables. The
                // integration must install hooks without changing shell modes.
                let profile = format!(
                    "{commands}\nunset HISTFILE PS1\nset -u\n{}\n",
                    if posix { "set -o posix" } else { ":" }
                );
                std::fs::write(home.path().join(".bashrc"), &profile).expect("profile");
                let mut opts = sh_opts("first-prompt", "");
                opts.shell = "/bin/bash".into();
                opts.shell_args = vec!["-i".into()];
                opts.extra_env = vec![
                    format!("HOME={}", home.path().display()),
                    "HISTFILE=/dev/null".into(),
                ];
                let (mut spec, _) = build_spec(&opts, None, None).expect("spec");
                spec.env
                    .retain(|(name, _)| !matches!(name.as_str(), "ENV" | "POSIXLY_CORRECT"));
                let mut child = std::process::Command::new(&spec.argv[0])
                    .args(&spec.argv[1..])
                    .env_clear()
                    .envs(spec.env)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .expect("bash");
                child
                    .stdin
                    .take()
                    .expect("stdin")
                    .write_all(
                        b"printf 'FIRST_COMMAND:%s\\n' \"$-\"; declare -p PROMPT_COMMAND; printf 'PROMPT_DEFINED:%s\\n' \"${PROMPT_DEFINED-}\"; exit 0\n",
                    )
                    .expect("input");
                let output = child.wait_with_output().expect("output");
                let stdout = String::from_utf8(output.stdout).expect("stdout");
                let stderr = String::from_utf8(output.stderr).expect("stderr");
                assert!(
                    output.status.success(),
                    "posix={posix} {commands}: {stdout} {stderr}"
                );
                assert!(!stderr.contains("unbound variable"), "{stderr}");
                let (prompt, command) = stdout.split_once("FIRST_COMMAND:").expect(&stdout);
                assert!(prompt.contains("\x1b]133;A\x07"), "{stdout}");
                let mut tail = prompt;
                for marker in expected {
                    assert_eq!(prompt.matches(marker).count(), 1, "{stdout}");
                    tail = tail.split_once(marker).expect(&stdout).1;
                }
                assert!(
                    command.lines().next().expect("flags").contains('u'),
                    "{stdout}"
                );
                assert!(command.contains("__splitty_precmd"), "{stdout}");
                if commands.contains("PROMPT_DEFINED") {
                    assert!(command.contains("PROMPT_DEFINED:global"), "{stdout}");
                }
                assert_eq!(
                    std::fs::read_to_string(home.path().join(".bashrc")).expect("profile"),
                    profile
                );
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn bash_bootstrap_loads_history_after_profiles_without_early_truncation() {
        for mode in ["changed-file", "raised-limits", "seeded"] {
            let mut captures = Vec::new();
            for integrated in [false, true] {
                let home = tempfile::tempdir().expect("home");
                std::fs::write(home.path().join(".hushlogin"), "").expect("quiet login");
                let default_history = (0..700)
                    .map(|index| format!("DEFAULT_{index}\n"))
                    .collect::<String>();
                std::fs::write(home.path().join(".bash_history"), &default_history)
                    .expect("default history");
                std::fs::write(
                    home.path().join("custom_history"),
                    "CUSTOM_1\nCUSTOM_2\nCUSTOM_3\nCUSTOM_4\n",
                )
                .expect("custom history");
                let profile = match mode {
                    "changed-file" => "HISTFILE=\"$HOME/custom_history\"; HISTSIZE=3; HISTFILESIZE=1000; HISTCONTROL=ignoreboth\n",
                    "raised-limits" => "HISTSIZE=1000; HISTFILESIZE=1000; HISTCONTROL=ignoreboth\n",
                    _ => "HISTSIZE=1000; HISTFILESIZE=1000; HISTCONTROL=ignoreboth; history -s PROFILE_SEED\n",
                };
                std::fs::write(home.path().join(".bash_profile"), profile).expect("profile");
                let command = "printf 'SETTINGS:%s:%s:%s\\n' \"$HISTSIZE\" \"$HISTFILESIZE\" \"$HISTCONTROL\"; history; exit 0";
                let mut opts = sh_opts("history-bootstrap", "");
                opts.shell = "/bin/bash".into();
                opts.shell_args = vec!["-il".into()];
                opts.dir = home.path().to_string_lossy().into_owned();
                opts.extra_env = vec![format!("HOME={}", home.path().display())];
                let native = vec!["/bin/bash".into(), "-il".into()];
                let (mut spec, _) =
                    build_spec(&opts, (!integrated).then_some(native.as_slice()), None)
                        .expect("spec");
                // The fixture deliberately exercises Bash's native defaults.
                spec.env.retain(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "POSIXLY_CORRECT"
                            | "HISTSIZE"
                            | "HISTFILESIZE"
                            | "SPLITTY_BASH_HISTSIZE"
                            | "SPLITTY_BASH_HISTFILESIZE"
                    )
                });
                if !integrated {
                    spec.env.retain(|(key, _)| key != "HISTFILE");
                }
                if integrated {
                    spec.env.retain(|(key, _)| key != "SPLITTY_BASH_HISTFILE");
                }
                use std::io::Write;
                let mut child = std::process::Command::new(&spec.argv[0])
                    .args(&spec.argv[1..])
                    .env_clear()
                    .envs(spec.env)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .expect("bash");
                child
                    .stdin
                    .take()
                    .expect("stdin")
                    .write_all(format!("{command}\n").as_bytes())
                    .expect("input");
                let output = child.wait_with_output().expect("output");
                assert!(output.status.success());
                let stdout = String::from_utf8(
                    super::super::log_text::LogText::default().push(&output.stdout),
                )
                .expect("output");
                if mode == "raised-limits" {
                    assert!(
                        stdout.contains("DEFAULT_0\n") && stdout.contains("DEFAULT_699\n"),
                        "{stdout}"
                    );
                } else if mode == "changed-file" {
                    assert_eq!(
                        std::fs::read_to_string(home.path().join(".bash_history"))
                            .expect("default history"),
                        default_history
                    );
                }
                captures.push(stdout);
            }
            assert_eq!(captures[0], captures[1], "{mode}");
        }
    }

    #[test]
    fn private_bash_environment_cannot_enable_bootstrap() {
        for (shell, direct) in [
            ("/bin/zsh", false),
            ("/bin/bash", false),
            ("/bin/bash", true),
        ] {
            let mut opts = sh_opts("forged-bootstrap", "exit 0");
            opts.shell = shell.into();
            opts.dir = std::env::temp_dir().to_string_lossy().into_owned();
            opts.extra_env = vec![
                "SPLITTY_BASH_INJECT=forged".into(),
                "SPLITTY_BASH_RCFILE=/forged".into(),
                "SPLITTY_BASH_PROMPT_COMMAND=forged".into(),
                "SPLITTY_BASH_HISTFILE=/forged".into(),
                "SPLITTY_BASH_HISTSIZE=1".into(),
                "SPLITTY_BASH_HISTFILESIZE=1".into(),
                "PROMPT_COMMAND=: original".into(),
            ];
            let argv = vec!["/bin/sh".into(), "-c".into(), "exit 0".into()];
            let (spec, _) =
                build_spec(&opts, direct.then_some(argv.as_slice()), None).expect("spec");
            assert_eq!(
                spec.env
                    .iter()
                    .find(|(key, _)| key == "PROMPT_COMMAND")
                    .map(|(_, value)| value.as_str()),
                Some(": original")
            );
            assert!(!spec.env.iter().any(|(key, _)| matches!(
                key.as_str(),
                "SPLITTY_BASH_INJECT"
                    | "SPLITTY_BASH_RCFILE"
                    | "SPLITTY_BASH_PROMPT_COMMAND"
                    | "SPLITTY_BASH_HISTFILE"
                    | "SPLITTY_BASH_HISTSIZE"
                    | "SPLITTY_BASH_HISTFILESIZE"
            )));
        }
    }

    #[test]
    fn bash_noninteractive_command_keeps_native_startup() {
        let dir = tempfile::tempdir().expect("integration");
        std::fs::create_dir(dir.path().join("bash")).expect("bash");
        std::fs::write(dir.path().join("bash/splitty.bash"), "exit 99").expect("script");
        let args = vec![
            "-lc".into(),
            "printf '%s' \"$0:$1\"; exit 7".into(),
            "command-name".into(),
            "literal argument".into(),
        ];
        let (cmd, _) = build_shell_command("/bin/bash", &args, dir.path().to_str(), true);
        assert_eq!(
            cmd.get_argv()
                .iter()
                .skip(1)
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            args
        );
        assert!(!cmd
            .iter_full_env_as_str()
            .any(|(key, _)| key == "SPLITTY_BASH_INJECT"));
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
                "stty raw -echo; printf READY; exec sleep 30".into(),
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
    async fn logs_keep_final_bytes_when_the_process_exits_before_capture_ends() {
        use super::super::output::{read_waiting, OutputQuery};
        // A process result can precede reader EOF. Model that ordering with a
        // controlled reader: Darwin revokes the slave when its leader exits,
        // so a background writer cannot portably supply bytes after that exit.
        struct Exited;
        #[async_trait]
        impl Terminal for Exited {
            fn pid(&self) -> Option<u32> {
                None
            }
            async fn write(&self, _: &[u8]) -> io::Result<()> {
                Ok(())
            }
            fn resize(&self, _: u16, _: u16) -> io::Result<()> {
                Ok(())
            }
            fn close(&self) -> io::Result<()> {
                Ok(())
            }
            async fn stop(&self) -> io::Result<super::super::ProcessResult> {
                Ok(self.result().expect("result"))
            }
            fn done(&self) -> watch::Receiver<bool> {
                watch::channel(true).1
            }
            fn result(&self) -> Option<super::super::ProcessResult> {
                Some(super::super::ProcessResult {
                    exit_code: Some(7),
                    ..Default::default()
                })
            }
            fn foreground_pgid(&self) -> Option<u32> {
                None
            }
        }
        struct ControlledReader(std::sync::mpsc::Receiver<Vec<u8>>);
        impl io::Read for ControlledReader {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                match self.0.recv_timeout(Duration::from_secs(5)) {
                    Ok(bytes) => {
                        buffer[..bytes.len()].copy_from_slice(&bytes);
                        Ok(bytes.len())
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Ok(0),
                    Err(error) => Err(io::Error::new(io::ErrorKind::TimedOut, error)),
                }
            }
        }
        let (send, receive) = std::sync::mpsc::channel();
        let mut reader = None;
        let session = Session::with_terminal("trailing-output".into(), None, true, 1024, |feed| {
            reader = Some(std::thread::spawn(move || {
                super::super::terminal::read_output(
                    Box::new(ControlledReader(receive)),
                    feed.as_ref(),
                    None,
                )
            }));
            Ok(Box::new(Exited))
        })
        .expect("session");
        assert!(*session.done().borrow());
        let result = session.process_result();
        let before = session
            .read_output("job", &OutputQuery::default())
            .expect("before tail");
        assert_eq!(result.expect("exit").exit_code, Some(7));
        assert!(!before.output_complete);
        send.send(b"FINAL_BYTES".to_vec()).expect("final chunk");
        assert!(
            wait_until(TIMEOUT, || text(&session.scrollback())
                .contains("FINAL_BYTES"))
            .await
        );
        let buffered = session
            .read_output("job", &OutputQuery::default())
            .expect("buffered final bytes");
        assert!(buffered.data.contains("FINAL_BYTES") && !buffered.output_complete);
        drop(send);
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
        reader.expect("reader").join().expect("reader finished");
        let final_page = session
            .read_output("job", &OutputQuery::default())
            .expect("final page");
        assert!(final_page.output_complete && final_page.data.contains("FINAL_BYTES"));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn logs_capture_final_pty_bytes_written_before_process_exit() {
        use super::super::output::OutputQuery;
        if !pty_available() {
            return;
        }
        let mut opts = sh_opts("final-pty-bytes", "");
        opts.dir = std::env::temp_dir().to_string_lossy().into_owned();
        let session = Session::new_program(
            opts,
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf FINAL_BYTES; exit 7".into(),
            ],
        )
        .expect("job");
        assert!(wait_until(TIMEOUT, || *session.done().borrow()).await);
        assert_eq!(session.process_result().expect("result").exit_code, Some(7));
        assert!(wait_until(TIMEOUT, || session.output_complete()).await);
        let page = session
            .read_output("job", &OutputQuery::default())
            .expect("output");
        assert!(page.output_complete && page.data.contains("FINAL_BYTES"));
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
        let mut ui_bytes = ui.snapshot.value;
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
        let mut bytes = output.snapshot.value;
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

    /// program-status, Status Reports (Report while nobody streams): a report
    /// of a program in a session nobody subscribes to sets the root record.
    #[tokio::test]
    async fn a_report_while_nobody_subscribes_sets_the_root_record() {
        if !pty_available() {
            return;
        }
        let session = spawn(
            "status-unwatched",
            r"printf '\033]7501;state=working:app=claude-code\007'; printf done; sleep 5",
        );
        assert!(wait_until(TIMEOUT, || text(&session.scrollback()).contains("done")).await);
        let root = session.program_status();
        let record = root.record.expect("a root record");
        assert_eq!(record.state, crate::pty::status::StatusState::Working);
        assert_eq!(record.app.as_deref(), Some("claude-code"));
        assert!(!root.read_again);
        assert_eq!(session.program_status_revision(), root.revision);
        assert!(
            text(&session.scrollback()).contains("\x1b]7501;state=working"),
            "reports stay in the output"
        );
        session.close().unwrap();
    }

    /// What a program reads from its terminal within a second of writing
    /// `probe` and `CSI c`, as hex, with `subscribers` subscribed and nobody
    /// answering the device attributes request.
    async fn read_after_probe(id: &str, probe: &str, subscribers: usize) -> String {
        let script = format!(
            "stty raw -echo min 0 time 10; printf '{probe}\\033[c'; printf 'GOT:'; \
             cat | od -An -tx1 | tr -d ' \\n'; printf ':END'; sleep 5"
        );
        let session = spawn(id, &script);
        let subscriptions: Vec<_> = (0..subscribers).map(|_| session.subscribe()).collect();
        assert!(
            wait_until(TIMEOUT, || text(&session.scrollback()).contains(":END")).await,
            "{}",
            text(&session.scrollback())
        );
        let output = text(&session.scrollback());
        assert!(
            !output.contains("7501"),
            "the probe was removed: {output:?}"
        );
        assert!(output.contains("\x1b[c"), "the rest is forwarded");
        drop(subscriptions);
        session.close().unwrap();
        let start = output.find("GOT:").expect("marker") + 4;
        let end = output.find(":END").expect("marker");
        output[start..end].to_owned()
    }

    /// program-status, Probe Answer (Probe without a client, Probe ended by
    /// BEL, Two clients stream the session) and terminal-sessions, Continuous
    /// Output Capture (Status while nobody streams, the answer): the program
    /// reads one answer, ended as its probe was.
    #[tokio::test]
    async fn a_probe_is_answered_once_with_its_terminator() {
        if !pty_available() {
            return;
        }
        let st = "1b5d373530313b3f1b5c";
        let bel = "1b5d373530313b3f07";
        assert_eq!(
            read_after_probe("probe-st", r"\033]7501;?\033\\", 0).await,
            st
        );
        assert_eq!(
            read_after_probe("probe-bel", r"\033]7501;?\007", 0).await,
            bel
        );
        assert_eq!(
            read_after_probe("probe-two", r"\033]7501;?\007", 2).await,
            bel
        );
    }

    /// terminal-sessions, Terminal Modes in the Replay (Modes set before the
    /// buffered output, Kitty keyboard flags of both screens, Synchronized
    /// output is not restored), Attaching to an Existing Session (Replay after
    /// the buffer wrapped) and Continuous Output Capture (Modes set while
    /// nobody streams): a program sets its modes once, then prints more than
    /// the scrollback holds while nobody subscribes; a later subscriber's
    /// snapshot starts in those modes.
    #[tokio::test]
    async fn a_replay_starts_in_the_modes_set_before_the_buffered_output() {
        if !pty_available() {
            return;
        }
        let script = "printf '\\033[>1u\\033[?1049h\\033[>5u\\033[?2004h\\033[?1004h\
            \\033[?1000h\\033[?1006h\\033[?1h\\033=\\033[?2026h'; \
            head -c 20000 /dev/zero | tr '\\000' x; printf END; sleep 5";
        let (mut spec, _) = build_spec(&sh_opts("modes", script), None, None).expect("spec");
        spec.buffer_size = 4096;
        let session = Session::with_terminal(
            spec.session_id.clone(),
            None,
            false,
            spec.buffer_size,
            |feed| LocalTerminals.start(&spec, feed),
        )
        .expect("spawn sh");
        let contains = |haystack: &[u8], needle: &[u8]| {
            haystack
                .windows(needle.len())
                .any(|window| window == needle)
        };

        let printed = wait_until(TIMEOUT, || session.scrollback().ends_with(b"END")).await;
        assert!(printed, "scrollback: {:?}", text(&session.scrollback()));
        let snapshot = session.subscribe().snapshot.to_vec();
        let scrollback = session.scrollback();
        assert_eq!(
            scrollback.len(),
            4096,
            "the program's modes left the buffer"
        );
        assert!(
            !contains(&scrollback, b"\x1b["),
            "the buffer holds no sequence"
        );
        assert!(
            snapshot.ends_with(&scrollback),
            "the snapshot ends with the buffered output"
        );
        let restore = &snapshot[..snapshot.len() - scrollback.len()];
        assert!(
            restore.starts_with(b"\x1b[>1u\x1b[?1049h\x1b[>5u"),
            "normal screen's flags, the switch, the alternate screen's flags: {:?}",
            text(restore)
        );
        for sequence in [
            &b"\x1b[?2004h"[..],
            b"\x1b[?1004h",
            b"\x1b[?1000h",
            b"\x1b[?1006h",
            b"\x1b[?1h",
            b"\x1b=",
        ] {
            assert!(
                contains(restore, sequence),
                "{:?} missing from {:?}",
                text(sequence),
                text(restore)
            );
        }
        assert!(
            !contains(restore, b"2026"),
            "synchronized output restored: {:?}",
            text(restore)
        );
        let mut filtered = snapshot.clone();
        crate::pty::replay::filter_in_place(&mut filtered);
        assert_eq!(filtered, snapshot, "the replay filter keeps the restore");
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
        let (cmd, _) = build_shell_command(
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

        assert_eq!(argv, vec!["/bin/bash", "--noprofile", "--norc", "-l"]);
    }
}

#[cfg(test)]
mod spec_tests {
    use super::*;

    use std::time::Duration;

    use crate::pty::testing::wait_until;

    const TIMEOUT: Duration = Duration::from_secs(10);

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
        assert_eq!(env("FORCE_HYPERLINK"), Some("1"));
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

    /// Also terminal-sessions, Terminal Modes in the Replay (Session adopted
    /// after a daemon restart): the modes are followed from the keeper's
    /// oldest buffered byte on.
    #[test]
    fn an_adopted_session_fills_its_buffers_from_the_replay() {
        let session = Session::with_terminal("adopted".into(), None, false, 1024, |feed| {
            feed.record(b"before\x1b[?2004h\x1b]2;vim\x07");
            Ok(Box::new(Replayed {
                done: tokio::sync::watch::channel(false).1,
            }))
        })
        .expect("adopted");
        assert_eq!(session.scrollback(), b"before\x1b[?2004h\x1b]2;vim\x07");
        assert_eq!(session.osc_title_watch().borrow().title, "vim");
        assert_eq!(session.shell_pid(), Some(4242));
        assert_eq!(
            session.subscribe().snapshot.as_slice(),
            session.scrollback(),
            "the buffer still holds the mode"
        );

        session.output.record(&[b'x'; 2048]);
        let mut expected = b"\x1b[?2004h".to_vec();
        expected.extend_from_slice(&[b'x'; 1024]);
        assert_eq!(session.subscribe().snapshot.as_slice(), expected);
    }

    /// The writes a [`Recording`] terminal took, in order.
    type Writes = Arc<Mutex<Vec<Vec<u8>>>>;

    /// A terminal that records every write, in pieces of at most 4 KiB with a
    /// yield between them, as a PTY takes large input.
    struct Recording {
        writes: Writes,
        done: tokio::sync::watch::Receiver<bool>,
    }

    #[async_trait]
    impl Terminal for Recording {
        fn pid(&self) -> Option<u32> {
            None
        }
        async fn write(&self, data: &[u8]) -> io::Result<()> {
            for piece in data.chunks(4096) {
                self.writes.lock().unwrap().push(piece.to_vec());
                tokio::task::yield_now().await;
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
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

    fn recording(id: &str) -> (Arc<Session>, Writes) {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let terminal = Recording {
            writes: Arc::clone(&writes),
            done: tokio::sync::watch::channel(false).1,
        };
        let session =
            Session::with_terminal(id.into(), None, false, 1 << 16, |_| Ok(Box::new(terminal)))
                .expect("session");
        (session, writes)
    }

    fn state(session: &Session) -> Option<crate::pty::status::StatusState> {
        session.program_status().record.map(|record| record.state)
    }

    const PROBE_ST: &[u8] = b"\x1b]7501;?\x1b\\";

    /// program-status, Status Record Lifetime (Done survives the prompt,
    /// Report after a prompt mark in one read, Full reset, Alternate screen)
    /// and Status Records: the records follow the output in its order.
    #[test]
    fn status_records_follow_the_output_in_order() {
        use crate::pty::status::StatusState::{Done, Working};
        let session = Session::with_terminal("records".into(), None, false, 4096, |_| {
            Ok(Box::new(Replayed {
                done: tokio::sync::watch::channel(false).1,
            }))
        })
        .expect("session");
        let output = &session.output;
        output.record(b"\x1b]7501;state=working\x07");
        assert_eq!(state(&session), Some(Working));
        output.record(b"\x1b]133;A\x07");
        assert_eq!(state(&session), None, "a prompt mark ends working");

        output.record(b"\x1b]133;A\x07\x1b]7501;state=working\x07");
        assert_eq!(state(&session), Some(Working), "in the order of the output");
        let revision = session.program_status_revision();
        for kept in [&b"\x1b[!p"[..], b"\x1b[?1049h", b"text\x1b[?1049l"] {
            output.record(kept);
            assert_eq!(state(&session), Some(Working), "{kept:?}");
        }
        session.resize(100, 40).expect("resize");
        assert_eq!(state(&session), Some(Working), "a resize");
        assert_eq!(session.program_status_revision(), revision);

        output.record(b"\x1b]7501;state=done\x1b\\\x1b]133;A\x07");
        assert_eq!(state(&session), Some(Done), "done survives the prompt");
        output.record(b"\x1bc");
        assert_eq!(state(&session), None, "a full reset ends all");

        output.record(b"\x1b]7501;state=done\x07");
        output.end();
        assert_eq!(state(&session), Some(Done), "done survives the end");
        let ended = Session::with_terminal("ended".into(), None, false, 4096, |feed| {
            feed.record(b"\x1b]7501;state=blocked:kind=permission\x07");
            feed.end();
            Ok(Box::new(Replayed {
                done: tokio::sync::watch::channel(true).1,
            }))
        })
        .expect("session");
        assert_eq!(state(&ended), None, "blocked ends with the output");
    }

    /// program-status, Probe Removed from the Output: the scrollback, the
    /// replay and the mode tracking get the output without the probe, at the
    /// same offsets.
    #[test]
    fn the_probe_reaches_neither_the_buffers_nor_the_modes() {
        let session = Session::with_terminal("probe-buffers".into(), None, false, 16, |feed| {
            feed.record(b"a\x1b]7501;?\x07\x1b[?2004hb");
            Ok(Box::new(Replayed {
                done: tokio::sync::watch::channel(false).1,
            }))
        })
        .expect("session");
        assert_eq!(session.scrollback(), b"a\x1b[?2004hb");
        assert_eq!(session.input.waiting(), 1, "answered at the next write");
        // Fifteen more bytes push `a` and the mode out of the 16-byte buffer.
        // The replay restores the mode from the tracker, whose offset for it
        // (1, not 10) matches the buffer's.
        session.output.record(b"0123456789abcde");
        assert_eq!(
            session.subscribe().snapshot.as_slice(),
            b"\x1b[?2004hb0123456789abcde"
        );
    }

    /// program-status, Probe Answer (Probe split between reads): one answer
    /// however the probe is split.
    #[tokio::test]
    async fn a_probe_split_between_reads_is_answered_once() {
        for split in 0..=PROBE_ST.len() {
            let (session, writes) = recording("probe-split");
            session.output.record(&PROBE_ST[..split]);
            session.output.record(&PROBE_ST[split..]);
            session.output.record(b"after");
            assert!(
                wait_until(TIMEOUT, || writes.lock().unwrap().len() == 1).await,
                "split at {split}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert_eq!(*writes.lock().unwrap(), vec![PROBE_ST.to_vec()]);
            assert_eq!(session.scrollback(), b"after", "split at {split}");
        }
    }

    /// program-status, Probe Answer: at most sixteen answers wait; while the
    /// gate is held by a write, a seventeenth probe gets none.
    #[tokio::test]
    async fn a_probe_beyond_sixteen_waiting_answers_gets_none() {
        let (session, writes) = recording("probe-limit");
        let gate = session.input.gate.lock().await;
        for _ in 0..=MAX_WAITING_ANSWERS {
            session.output.record(PROBE_ST);
        }
        assert_eq!(session.input.waiting(), MAX_WAITING_ANSWERS);
        drop(gate);
        assert!(
            wait_until(TIMEOUT, || writes.lock().unwrap().len()
                == MAX_WAITING_ANSWERS)
            .await
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(writes.lock().unwrap().len(), MAX_WAITING_ANSWERS);
        assert_eq!(session.input.waiting(), 0);
    }

    /// program-status, Probe Answer Order (Large input) and terminal-sessions,
    /// Terminal Input (Status probe answered during input): an answer queued
    /// while 64 KiB of input are written goes after them, never between two of
    /// their pieces; one queued before goes ahead of them.
    #[tokio::test]
    async fn large_input_is_written_whole_around_an_answer() {
        let input = vec![b'x'; 64 * 1024];
        let (session, writes) = recording("probe-input");
        let writer = {
            let session = Arc::clone(&session);
            let input = input.clone();
            tokio::spawn(async move { SessionHandle::write(session.as_ref(), &input).await })
        };
        assert!(wait_until(TIMEOUT, || writes.lock().unwrap().len() >= 2).await);
        session.output.record(PROBE_ST);
        writer.await.expect("writer").expect("input");
        assert!(wait_until(TIMEOUT, || writes.lock().unwrap().len() == 17).await);
        let recorded = writes.lock().unwrap().clone();
        assert_eq!(recorded[..16].concat(), input, "the input in one piece");
        assert_eq!(recorded[16], PROBE_ST, "the answer after it");

        // Queued before the input: written ahead of it by the same write.
        let (session, writes) = recording("probe-before");
        let gate = session.input.gate.lock().await;
        session.output.record(PROBE_ST);
        drop(gate);
        SessionHandle::write(session.as_ref(), b"reply")
            .await
            .expect("input");
        assert_eq!(
            writes.lock().unwrap()[..2],
            [PROBE_ST.to_vec(), b"reply".to_vec()]
        );
    }

    /// program-status, Output Read Again After Adoption: a probe in output
    /// read again is removed but not answered, and its records are marked.
    #[tokio::test]
    async fn output_read_again_answers_no_probe_and_marks_its_records() {
        let (session, writes) = recording("read-again");
        let feed: &dyn Feed = session.output.as_ref();
        feed.record_again(b"a\x1b]7501;?\x07\x1b]7501;state=blocked:kind=permission\x07");
        let root = session.program_status();
        assert!(root.read_again);
        assert_eq!(
            root.record.map(|record| record.state),
            Some(crate::pty::status::StatusState::Blocked)
        );
        feed.record(b"b\x1b]7501;?\x07");
        assert!(wait_until(TIMEOUT, || writes.lock().unwrap().len() == 1).await);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(writes.lock().unwrap().len(), 1, "only the live probe");
        assert!(
            String::from_utf8_lossy(&session.scrollback()).starts_with("a\x1b]7501;state=blocked")
        );
        assert!(!String::from_utf8_lossy(&session.scrollback()).contains(";?"));
    }

    #[tokio::test]
    async fn a_stalled_subscriber_closes_without_interrupting_capture_or_another_client() {
        use splitty_transport::budget::{ConnectionBudget, ProcessBudget};
        let session = Session::with_terminal("bounded".into(), None, false, 4096, |_| {
            Ok(Box::new(Replayed {
                done: tokio::sync::watch::channel(false).1,
            }))
        })
        .unwrap();
        let process = ProcessBudget::new(4096, 256);
        let slow = queue::Context::new(ConnectionBudget::new(process.clone(), 80, 16));
        let fast = queue::Context::new(ConnectionBudget::new(process.clone(), 1024, 64));
        let mut stalled = session.subscribe_with(slow.clone());
        let mut active = session.subscribe_with(fast.clone());
        let bytes = vec![b'x'; 32];
        for _ in 0..10 {
            session.output.record(&bytes);
            assert_eq!(&**active.receiver.recv().await.unwrap(), bytes.as_slice());
        }
        assert!(slow.is_closed());
        assert!(!fast.is_closed());
        assert!(stalled.receiver.recv().await.is_none());
        assert_eq!(session.scrollback(), vec![b'x'; 320]);
        assert_eq!(session.output.subscribers.lock().unwrap().senders.len(), 1);
        drop(stalled);
        drop(active);
        assert_eq!(process.used(), 0);
    }
}
