//! The controller's side: starting a keeper for a new session, the kept
//! terminal a daemon's session runs on, and finding the keepers a previous
//! daemon left running.

use std::collections::HashMap;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{oneshot, watch};

use super::dir;
use super::wire::{self, Exit, Frame, Hello, SpawnSpec, PROTOCOL};
use crate::pty::terminal::{Feed, LocalTerminals, SessionSpec, Terminal, TerminalFactory};
use crate::pty::{ProcessResult, PtyError};

/// How long a new keeper may take to start its program and answer.
const HELLO_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a running keeper may take to say hello or confirm a claim.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);
/// Input per frame; larger writes go in several.
const MAX_INPUT_FRAME: usize = 512 * 1024;
const RECONNECT_ATTEMPTS: u32 = 5;
/// What a managed job's keeper answers when the job does not read its input.
const NOT_READING: &str = "the program does not read its input";

/// How a keeper is started.
pub trait Launch: Send + Sync {
    /// Start a keeper whose connection to this process is `keeper_end`.
    fn launch(&self, keeper_end: UnixStream) -> io::Result<()>;
}

/// Keepers as processes: an executable run as `<executable> session-keeper`
/// in a session of its own, with `keeper_end` as standard input, standard
/// output discarded and standard error shared with this process (the daemon's
/// log). While this process runs, it reaps the keepers it started.
pub struct ProcessLauncher {
    executable: PathBuf,
    children: Arc<Mutex<Vec<std::process::Child>>>,
}

impl ProcessLauncher {
    pub fn new(executable: PathBuf) -> Self {
        let children: Arc<Mutex<Vec<std::process::Child>>> = Arc::default();
        let reaped = Arc::downgrade(&children);
        let _ = std::thread::Builder::new()
            .name("keeper-reaper".into())
            .spawn(move || {
                while let Some(children) = reaped.upgrade() {
                    children
                        .lock()
                        .expect("keeper children")
                        .retain_mut(|child| matches!(child.try_wait(), Ok(None)));
                    drop(children);
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
        Self {
            executable,
            children,
        }
    }
}

impl Launch for ProcessLauncher {
    fn launch(&self, keeper_end: UnixStream) -> io::Result<()> {
        use std::os::unix::process::CommandExt;

        let mut command = std::process::Command::new(&self.executable);
        command
            .arg("session-keeper")
            .stdin(std::process::Stdio::from(OwnedFd::from(keeper_end)))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit());
        // The program's environment comes with the spec; the keeper's own
        // carries nothing that belongs to a session or a client.
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
            command.env_remove(key);
        }
        // SAFETY: `setsid` is async-signal-safe and touches no memory of the
        // parent; it runs in the child between fork and exec. A session of its
        // own keeps the keeper apart from anything that signals the daemon's
        // process group or session.
        unsafe {
            command.pre_exec(|| nix::unistd::setsid().map(|_| ()).map_err(io::Error::from));
        }
        let child = command.spawn()?;
        self.children.lock().expect("keeper children").push(child);
        Ok(())
    }
}

/// A daemon's terminal factory: every session in a keeper of its own. A
/// session whose keeper cannot be set up runs in this process instead, and
/// the log says why.
pub struct Keepers {
    dir: PathBuf,
    launcher: Arc<dyn Launch>,
}

impl Keepers {
    pub fn new(dir: PathBuf, launcher: Arc<dyn Launch>) -> Self {
        Self { dir, launcher }
    }
}

impl TerminalFactory for Keepers {
    fn start(
        &self,
        spec: &SessionSpec,
        feed: Arc<dyn Feed>,
    ) -> Result<Box<dyn Terminal>, PtyError> {
        match KeptTerminal::start(&self.dir, self.launcher.as_ref(), spec, Arc::clone(&feed)) {
            Ok(terminal) => Ok(Box::new(terminal)),
            Err(Failure::Program(error)) => Err(PtyError::SpawnFailed(error)),
            Err(Failure::Keeper(error)) => {
                tracing::warn!(
                    session = %spec.session_id,
                    "cannot keep the session; it runs in the daemon and ends with it: {error}"
                );
                LocalTerminals.start(spec, feed)
            }
        }
    }
}

/// Why a keeper did not start a session.
#[derive(Debug)]
pub enum Failure {
    /// The program could not start: the session's own error.
    Program(String),
    /// The keeper could not be set up.
    Keeper(String),
}

/// A session's terminal in a keeper.
pub struct KeptTerminal {
    shared: Arc<Shared>,
}

struct Shared {
    session_id: String,
    socket: PathBuf,
    pid: Option<u32>,
    /// The connection frames go out on; replaced after a reconnect.
    stream: Mutex<Option<UnixStream>>,
    next_seq: AtomicU32,
    pending: Mutex<HashMap<u32, oneshot::Sender<Result<(), String>>>>,
    foreground: AtomicU32,
    done_tx: watch::Sender<bool>,
    result: Mutex<Option<ProcessResult>>,
    state: Mutex<ClientState>,
    feed: Arc<dyn Feed>,
}

#[derive(Default)]
struct ClientState {
    /// The offset of the next byte expected from the keeper.
    offset: u64,
    exited: bool,
    output_ended: bool,
    acknowledged: bool,
    released: bool,
}

impl KeptTerminal {
    /// Start a keeper for a new session and wait for its program to start.
    pub fn start(
        dir: &Path,
        launcher: &dyn Launch,
        spec: &SessionSpec,
        feed: Arc<dyn Feed>,
    ) -> Result<Self, Failure> {
        let keeper = |error: io::Error| Failure::Keeper(error.to_string());
        dir::prepare(dir).map_err(keeper)?;
        let socket = dir::socket_path(dir, &spec.session_id).map_err(keeper)?;
        let (mut mine, theirs) = UnixStream::pair().map_err(keeper)?;
        launcher.launch(theirs).map_err(keeper)?;
        mine.set_read_timeout(Some(HELLO_TIMEOUT)).map_err(keeper)?;
        let spawn = SpawnSpec {
            session_id: spec.session_id.clone(),
            pane_id: spec.pane_id.clone(),
            screen_id: spec.screen_id.clone(),
            argv: spec.argv.clone(),
            env: spec.env.clone(),
            cwd: spec.cwd.clone(),
            cols: spec.cols,
            rows: spec.rows,
            managed: spec.managed,
            buffer_size: spec.buffer_size,
            socket: socket.to_string_lossy().into_owned(),
        };
        wire::write_frame(&mut mine, &Frame::Spawn(Box::new(spawn))).map_err(keeper)?;
        let hello = match wire::read_frame(&mut mine) {
            Ok(Some(Frame::Hello(hello))) => hello,
            Ok(Some(Frame::SpawnFailed { error, program })) => {
                return Err(if program {
                    Failure::Program(error)
                } else {
                    Failure::Keeper(error)
                })
            }
            Ok(_) => {
                return Err(Failure::Keeper(
                    "the keeper ended before it answered".into(),
                ))
            }
            Err(error) => return Err(keeper(error)),
        };
        mine.set_read_timeout(None).map_err(keeper)?;
        Self::begin(socket, *hello, mine, 0, feed).map_err(keeper)
    }

    fn begin(
        socket: PathBuf,
        hello: Hello,
        stream: UnixStream,
        offset: u64,
        feed: Arc<dyn Feed>,
    ) -> io::Result<Self> {
        let reader = stream.try_clone()?;
        let shared = Arc::new(Shared {
            session_id: hello.session_id.clone(),
            socket,
            pid: hello.pid,
            stream: Mutex::new(Some(stream)),
            next_seq: AtomicU32::new(1),
            pending: Mutex::new(HashMap::new()),
            foreground: AtomicU32::new(0),
            done_tx: watch::channel(false).0,
            result: Mutex::new(None),
            state: Mutex::new(ClientState {
                offset,
                ..ClientState::default()
            }),
            feed,
        });
        let reading = Arc::clone(&shared);
        std::thread::Builder::new()
            .name(format!("keeper-client-{}", hello.session_id))
            .spawn(move || reading.read_frames(reader))?;
        Ok(Self { shared })
    }
}

impl Shared {
    fn send(&self, frame: &Frame) -> io::Result<()> {
        match self.stream.lock().expect("keeper stream").as_mut() {
            Some(stream) => wire::write_frame(stream, frame),
            None => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the keeper connection is closed",
            )),
        }
    }

    /// The keeper's frames, across reconnects, until the session has ended
    /// and was acknowledged, the terminal was released, or the keeper is gone.
    fn read_frames(self: Arc<Self>, mut stream: UnixStream) {
        loop {
            let taken_over = loop {
                match wire::read_frame(&mut stream) {
                    Ok(Some(Frame::TakenOver)) => break true,
                    Ok(Some(frame)) => self.handle(frame),
                    Ok(None) | Err(_) => break false,
                }
            };
            {
                let state = self.state.lock().expect("keeper client");
                if state.released || state.acknowledged {
                    return;
                }
            }
            if taken_over {
                tracing::warn!(session = %self.session_id, "a keeper was taken over; claiming it back");
            }
            // Pending writes went out on the old connection; their answers
            // will not come.
            self.pending.lock().expect("keeper inputs").clear();
            match self.reconnect() {
                Some(next) => stream = next,
                None => {
                    self.lost();
                    return;
                }
            }
        }
    }

    fn handle(&self, frame: Frame) {
        match frame {
            Frame::Output { offset, data } => {
                let data = {
                    let mut state = self.state.lock().expect("keeper client");
                    let end = offset + u64::try_from(data.len()).unwrap_or(0);
                    if end <= state.offset {
                        return;
                    }
                    let skip = usize::try_from(state.offset.saturating_sub(offset)).unwrap_or(0);
                    state.offset = end;
                    data[skip.min(data.len())..].to_vec()
                };
                self.feed.record(&data);
            }
            Frame::OutputEnded => {
                let first = {
                    let mut state = self.state.lock().expect("keeper client");
                    let first = !state.output_ended;
                    state.output_ended = true;
                    first
                };
                if first {
                    self.feed.end();
                }
                self.acknowledge_if_ended();
            }
            Frame::Exited(exit) => {
                *self.result.lock().expect("keeper result") = Some(exit.into());
                self.state.lock().expect("keeper client").exited = true;
                self.done_tx.send_replace(true);
                self.acknowledge_if_ended();
            }
            Frame::InputResult { seq, error } => {
                if let Some(reply) = self.pending.lock().expect("keeper inputs").remove(&seq) {
                    let _ = reply.send(match error {
                        None => Ok(()),
                        Some(error) => Err(error),
                    });
                }
            }
            Frame::Foreground { pid } => {
                self.foreground.store(pid.unwrap_or(0), Ordering::Relaxed);
            }
            _ => {}
        }
    }

    /// Once the program has ended and its terminal closed, the session has
    /// recorded both: the keeper may go.
    fn acknowledge_if_ended(&self) {
        let mut state = self.state.lock().expect("keeper client");
        if state.exited && state.output_ended && !state.acknowledged {
            state.acknowledged = true;
            drop(state);
            let _ = self.send(&Frame::ExitAck);
        }
    }

    /// Claim the keeper again where the last connection stopped.
    fn reconnect(&self) -> Option<UnixStream> {
        for attempt in 0..RECONNECT_ATTEMPTS {
            std::thread::sleep(Duration::from_millis(100 * u64::from(attempt)));
            let Ok(mut stream) = dir::connect(&self.socket) else {
                continue;
            };
            let from = self.state.lock().expect("keeper client").offset;
            if stream.set_read_timeout(Some(ANSWER_TIMEOUT)).is_err() {
                continue;
            }
            if !matches!(wire::read_frame(&mut stream), Ok(Some(Frame::Hello(_)))) {
                continue;
            }
            let claim = Frame::Claim {
                takeover: true,
                from: Some(from),
            };
            if wire::write_frame(&mut stream, &claim).is_err() {
                continue;
            }
            match wire::read_frame(&mut stream) {
                Ok(Some(Frame::Claimed {
                    lost,
                    from: resumed,
                })) => {
                    if lost {
                        tracing::warn!(
                            session = %self.session_id,
                            "output the keeper no longer held was lost ({} bytes)",
                            resumed.saturating_sub(from)
                        );
                    }
                }
                _ => continue,
            }
            if stream.set_read_timeout(None).is_err() {
                continue;
            }
            let Ok(writer) = stream.try_clone() else {
                continue;
            };
            *self.stream.lock().expect("keeper stream") = Some(writer);
            return Some(stream);
        }
        None
    }

    /// The keeper is gone without a word: the session ends with what it had.
    fn lost(&self) {
        tracing::warn!(session = %self.session_id, "the session's keeper is gone");
        *self.stream.lock().expect("keeper stream") = None;
        self.pending.lock().expect("keeper inputs").clear();
        let (end, exit) = {
            let mut state = self.state.lock().expect("keeper client");
            let end = !state.output_ended;
            state.output_ended = true;
            let exit = !state.exited;
            state.exited = true;
            (end, exit)
        };
        if exit {
            *self.result.lock().expect("keeper result") = Some(ProcessResult {
                error: Some("the session's keeper ended unexpectedly".into()),
                ..ProcessResult::default()
            });
            self.done_tx.send_replace(true);
        }
        if end {
            self.feed.end();
        }
    }
}

#[async_trait]
impl Terminal for KeptTerminal {
    fn pid(&self) -> Option<u32> {
        self.shared.pid
    }

    async fn write(&self, data: &[u8]) -> io::Result<()> {
        for chunk in data.chunks(MAX_INPUT_FRAME) {
            let seq = self.shared.next_seq.fetch_add(1, Ordering::Relaxed);
            let (reply, answer) = oneshot::channel();
            self.shared
                .pending
                .lock()
                .expect("keeper inputs")
                .insert(seq, reply);
            if let Err(error) = self.shared.send(&Frame::Input {
                seq,
                data: chunk.to_vec(),
            }) {
                self.shared
                    .pending
                    .lock()
                    .expect("keeper inputs")
                    .remove(&seq);
                return Err(error);
            }
            match answer.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) if error == NOT_READING => {
                    return Err(io::Error::new(io::ErrorKind::WouldBlock, error))
                }
                Ok(Err(error)) => return Err(io::Error::other(error)),
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "the keeper connection was interrupted",
                    ))
                }
            }
        }
        Ok(())
    }

    fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.shared.send(&Frame::Resize { cols, rows })
    }

    fn close(&self) -> io::Result<()> {
        if self.shared.state.lock().expect("keeper client").released {
            return Ok(());
        }
        match self.shared.send(&Frame::Close) {
            // A keeper that is gone has nothing left to close.
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            result => result,
        }
    }

    async fn stop(&self) -> io::Result<ProcessResult> {
        if self.shared.result.lock().expect("keeper result").is_none() {
            self.shared.send(&Frame::Stop)?;
        }
        let mut done = self.shared.done_tx.subscribe();
        let _ = done.wait_for(|done| *done).await;
        self.shared
            .result
            .lock()
            .expect("keeper result")
            .clone()
            .ok_or_else(|| io::Error::other("the keeper reported no result"))
    }

    fn done(&self) -> watch::Receiver<bool> {
        self.shared.done_tx.subscribe()
    }

    fn result(&self) -> Option<ProcessResult> {
        self.shared.result.lock().expect("keeper result").clone()
    }

    fn foreground_pgid(&self) -> Option<u32> {
        match self.shared.foreground.load(Ordering::Relaxed) {
            0 => None,
            pid => Some(pid),
        }
    }

    fn release(&self) {
        self.shared.state.lock().expect("keeper client").released = true;
        let _ = self.shared.send(&Frame::Release);
        if let Some(stream) = self.shared.stream.lock().expect("keeper stream").take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// A keeper this process took over, before a session runs on it.
pub struct Adoption {
    pub hello: Hello,
    socket: PathBuf,
    stream: UnixStream,
    from: u64,
}

impl Adoption {
    /// Run a session on the keeper, fed from where the claim resumed.
    pub fn run(self, feed: Arc<dyn Feed>) -> io::Result<KeptTerminal> {
        KeptTerminal::begin(self.socket, self.hello, self.stream, self.from, feed)
    }
}

/// A running keeper, connected and introduced.
pub struct Found {
    pub hello: Hello,
    pub socket: PathBuf,
    stream: UnixStream,
}

/// The keepers in `dir` that speak this build's protocol. Others are logged
/// and left running; sockets nobody answers on are removed.
pub fn find(dir: &Path) -> Vec<Found> {
    let mut found = Vec::new();
    for socket in dir::sockets(dir) {
        let mut stream = match dir::connect(&socket) {
            Ok(stream) => stream,
            Err(_) => {
                dir::remove_if_dead(&socket);
                continue;
            }
        };
        if stream.set_read_timeout(Some(ANSWER_TIMEOUT)).is_err() {
            continue;
        }
        let hello = match wire::read_frame(&mut stream) {
            Ok(Some(Frame::Hello(hello))) => *hello,
            _ => continue,
        };
        if hello.protocol != PROTOCOL {
            tracing::warn!(
                session = %hello.session_id,
                protocol = hello.protocol,
                "leaving a keeper of another protocol version running"
            );
            continue;
        }
        found.push(Found {
            hello,
            socket,
            stream,
        });
    }
    found
}

impl Found {
    /// Take the keeper over: from now on it answers this process only, and its
    /// output from the oldest buffered byte on waits on the connection until a
    /// session runs on it.
    pub fn take_over(self) -> io::Result<Adoption> {
        let Found {
            hello,
            socket,
            mut stream,
        } = self;
        let from = Self::claim(&mut stream, true, None)?;
        Ok(Adoption {
            hello,
            socket,
            stream,
            from,
        })
    }

    /// Take the keeper over and run a session on it.
    pub fn adopt(self, feed: Arc<dyn Feed>) -> io::Result<KeptTerminal> {
        self.take_over()?.run(feed)
    }

    /// Take the keeper over and end its session: hang up an ordinary one,
    /// stop a managed one.
    pub fn end(mut self) -> io::Result<()> {
        Self::claim(&mut self.stream, true, None)?;
        wire::write_frame(&mut self.stream, &Frame::Close)
    }

    /// Claim the keeper; the offset its output resumes from.
    pub fn claim(stream: &mut UnixStream, takeover: bool, from: Option<u64>) -> io::Result<u64> {
        // Before the claim: a keeper that refuses hangs up, and macOS refuses
        // socket options on a connection whose other end has closed.
        stream.set_read_timeout(Some(ANSWER_TIMEOUT))?;
        wire::write_frame(stream, &Frame::Claim { takeover, from })?;
        let claimed = wire::read_frame(stream)?;
        let _ = stream.set_read_timeout(None);
        match claimed {
            Some(Frame::Claimed { from, .. }) => Ok(from),
            Some(Frame::Busy) => Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "another client controls the session",
            )),
            _ => Err(io::Error::other("the keeper did not confirm the claim")),
        }
    }

    /// The connection, for a client that speaks the protocol itself.
    pub fn into_stream(self) -> UnixStream {
        self.stream
    }

    /// The result of a program that has ended, as the session layer has it.
    pub fn exit(&self) -> Option<ProcessResult> {
        self.hello.exited.clone().map(|exit: Exit| exit.into())
    }
}

#[cfg(test)]
mod tests;
