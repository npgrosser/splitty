//! The keeper process: one session's PTY and program, kept alive across
//! daemon restarts. It reads the spawn spec from the connection it was started
//! with, binds its socket, starts the program, reads the terminal into an
//! offset ring and serves one controller at a time until the program has
//! ended, its terminal has closed and a controller acknowledged both (or the
//! session was closed, or nobody came for seven days).
//!
//! Plain blocking threads, no runtime: the terminal's reader, the input
//! writer, the foreground check, the acceptor, and a reader and a writer per
//! controller connection.

use std::collections::VecDeque;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::dir;
use super::wire::{self, Exit, Frame, Hello, SpawnSpec, PROTOCOL};
use crate::pty::lifecycle::Lifecycle;
use crate::pty::ringbuffer::{RingBuffer, Selection};
use crate::pty::terminal::{self, Feed, SessionSpec};

/// Unsent output a controller may fall behind by before it is disconnected;
/// it resumes at its last offset.
const MAX_BEHIND: usize = 8 << 20;
/// How long an ended program's result waits for a controller.
pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How often the terminal's foreground process group is checked.
const FOREGROUND_INTERVAL: Duration = Duration::from_millis(500);
/// Replayed output per frame.
const REPLAY_CHUNK: usize = 64 * 1024;
/// How long a new connection may take to claim the keeper or leave.
const CLAIM_TIMEOUT: Duration = Duration::from_secs(10);

/// Run a keeper over `control`, the daemon's end of the socket pair it was
/// started with, until it is done. `retention` is how long an ended program's
/// result waits for a controller ([`RETENTION`] outside tests).
pub fn run(mut control: UnixStream, retention: Duration) -> io::Result<()> {
    let spec = match wire::read_frame(&mut control)? {
        Some(Frame::Spawn(spec)) => *spec,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a keeper expects a spawn spec first",
            ))
        }
    };
    let socket = PathBuf::from(&spec.socket);
    let listener = match bind(&socket) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = wire::write_frame(
                &mut control,
                &Frame::SpawnFailed {
                    error: format!("cannot serve {}: {error}", socket.display()),
                    program: false,
                },
            );
            return Err(error);
        }
    };
    let failed = |control: &mut UnixStream, error: String, program: bool| {
        let _ = std::fs::remove_file(&socket);
        let _ = wire::write_frame(
            control,
            &Frame::SpawnFailed {
                error: error.clone(),
                program,
            },
        );
        io::Error::other(error)
    };

    let spawned = match terminal::spawn(&session_spec(&spec)) {
        Ok(spawned) => spawned,
        Err(error) => return Err(failed(&mut control, error.to_string(), true)),
    };
    let terminal::Spawned {
        master,
        reader,
        writer,
        child,
        pid,
        master_fd,
        readiness,
    } = spawned;

    let (exited_tx, exited_rx) = mpsc::channel();
    let lifecycle = match Lifecycle::start_notifying(child, master_fd, move |result| {
        let _ = exited_tx.send(result.clone());
    }) {
        Ok(lifecycle) => lifecycle,
        Err(error) => {
            return Err(failed(
                &mut control,
                format!("start lifecycle: {error}"),
                false,
            ))
        }
    };
    let (inputs_tx, inputs_rx) = mpsc::channel::<InputJob>();
    let keeper = Arc::new(Keeper {
        session_id: spec.session_id.clone(),
        pane_id: spec.pane_id.clone(),
        screen_id: spec.screen_id.clone(),
        pid,
        started_at: now_millis(),
        managed: spec.managed,
        program: program_name(&spec.argv),
        socket: socket.clone(),
        lifecycle,
        master: Mutex::new(master),
        master_fd: master_fd.unwrap_or(-1),
        inputs: Mutex::new(inputs_tx),
        stopping: AtomicBool::new(false),
        state: Mutex::new(State {
            ring: RingBuffer::new(spec.buffer_size.max(1)),
            end: 0,
            controller: None,
            next_id: 1,
            size: (spec.cols, spec.rows),
            foreground: None,
            output_ended: false,
            exited: None,
            acknowledged: false,
            closing: false,
        }),
        changed: Condvar::new(),
    });

    // The program's first bytes reach the ring before anybody can claim it.
    {
        let feed = KeeperFeed(Arc::clone(&keeper));
        std::thread::Builder::new()
            .name("keeper-reader".into())
            .spawn(move || terminal::read_output(reader, &feed, readiness))?;
    }
    {
        let keeper = Arc::clone(&keeper);
        std::thread::Builder::new()
            .name("keeper-exit".into())
            .spawn(move || {
                if let Ok(result) = exited_rx.recv() {
                    keeper.exited(result.into());
                }
            })?;
    }
    {
        let keeper = Arc::clone(&keeper);
        std::thread::Builder::new()
            .name("keeper-input".into())
            .spawn(move || keeper.write_inputs(writer, inputs_rx))?;
    }
    {
        let keeper = Arc::clone(&keeper);
        std::thread::Builder::new()
            .name("keeper-foreground".into())
            .spawn(move || keeper.watch_foreground())?;
    }
    {
        let keeper = Arc::clone(&keeper);
        std::thread::Builder::new()
            .name("keeper-accept".into())
            .spawn(move || keeper.accept(listener))?;
    }

    // The daemon that started the keeper is its first controller.
    {
        let mut state = keeper.state.lock().expect("keeper state");
        let hello = keeper.hello(&state);
        wire::write_frame(&mut control, &Frame::Hello(Box::new(hello)))?;
        keeper.install(&mut state, control, 0, false);
    }

    keeper.wait_until_done(retention);
    keeper.shut_down();
    Ok(())
}

/// The terminal's spec from the daemon's spawn spec.
fn session_spec(spec: &SpawnSpec) -> SessionSpec {
    SessionSpec {
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
    }
}

/// Bind the keeper's socket, private to the user.
fn bind(socket: &Path) -> io::Result<UnixListener> {
    if let Some(dir) = socket.parent() {
        dir::prepare(dir)?;
    }
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

fn program_name(argv: &[String]) -> String {
    argv.first()
        .map(|program| {
            Path::new(program)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| program.clone())
        })
        .unwrap_or_default()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

struct Keeper {
    session_id: String,
    pane_id: Option<String>,
    screen_id: Option<String>,
    pid: Option<u32>,
    started_at: u64,
    managed: bool,
    program: String,
    socket: PathBuf,
    lifecycle: Lifecycle,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    master_fd: i32,
    inputs: Mutex<mpsc::Sender<InputJob>>,
    stopping: AtomicBool,
    state: Mutex<State>,
    /// Woken by the program's end, the terminal's end, an acknowledgement and
    /// `close`.
    changed: Condvar,
}

struct State {
    ring: RingBuffer,
    /// The offset of the next byte.
    end: u64,
    controller: Option<Controller>,
    next_id: u64,
    size: (u16, u16),
    foreground: Option<u32>,
    output_ended: bool,
    exited: Option<Exit>,
    acknowledged: bool,
    closing: bool,
}

struct Controller {
    id: u64,
    outbox: Arc<Outbox>,
}

struct InputJob {
    controller: u64,
    seq: u32,
    data: Vec<u8>,
}

/// Feeds the terminal's output into the keeper.
struct KeeperFeed(Arc<Keeper>);

impl Feed for KeeperFeed {
    fn record(&self, data: &[u8]) {
        self.0.record(data);
    }

    fn end(&self) {
        self.0.output_ended();
    }
}

impl Keeper {
    fn hello(&self, state: &State) -> Hello {
        Hello {
            protocol: PROTOCOL,
            capabilities: Vec::new(),
            session_id: self.session_id.clone(),
            pane_id: self.pane_id.clone(),
            screen_id: self.screen_id.clone(),
            pid: self.pid,
            started_at: self.started_at,
            managed: self.managed,
            cols: state.size.0,
            rows: state.size.1,
            buffer_start: state.end - u64::try_from(state.ring.len()).unwrap_or(0),
            buffer_end: state.end,
            program: self.program.clone(),
            executable: std::env::current_exe()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
            controlled: state.controller.is_some(),
            exited: state.exited.clone(),
        }
    }

    /// Send `frame` to the controller, or disconnect one that fell behind.
    fn tell(state: &mut State, frame: Frame) {
        if let Some(controller) = &state.controller {
            if !controller.outbox.push(frame) {
                controller.outbox.close();
                state.controller = None;
            }
        }
    }

    fn record(&self, data: &[u8]) {
        let mut state = self.state.lock().expect("keeper state");
        let offset = state.end;
        state.ring.write(data);
        state.end += u64::try_from(data.len()).unwrap_or(0);
        Self::tell(
            &mut state,
            Frame::Output {
                offset,
                data: data.to_vec(),
            },
        );
    }

    fn output_ended(&self) {
        let mut state = self.state.lock().expect("keeper state");
        state.output_ended = true;
        Self::tell(&mut state, Frame::OutputEnded);
        self.changed.notify_all();
    }

    fn exited(&self, exit: Exit) {
        let mut state = self.state.lock().expect("keeper state");
        state.exited = Some(exit.clone());
        Self::tell(&mut state, Frame::Exited(exit));
        self.changed.notify_all();
    }

    /// Make `stream` the controller, resuming at `from`, and start its
    /// threads. Everything the ring holds from `from` on is queued under the
    /// same lock the reader records under, so no byte is missed or doubled.
    fn install(self: &Arc<Self>, state: &mut State, stream: UnixStream, from: u64, announce: bool) {
        if let Some(previous) = state.controller.take() {
            previous.outbox.push(Frame::TakenOver);
            previous.outbox.close();
        }
        let id = state.next_id;
        state.next_id += 1;
        let outbox = Arc::new(Outbox::default());

        let oldest = state.end - u64::try_from(state.ring.len()).unwrap_or(0);
        let start = from.clamp(oldest, state.end);
        if announce {
            outbox.push(Frame::Claimed {
                from: start,
                lost: from < oldest,
            });
        }
        let mut position = start;
        while position < state.end {
            let Ok(slice) = state
                .ring
                .read(Selection::From(position), REPLAY_CHUNK, false)
            else {
                break;
            };
            if slice.data.is_empty() {
                break;
            }
            outbox.push(Frame::Output {
                offset: position,
                data: slice.data,
            });
            position = slice.next;
        }
        if let Some(pid) = state.foreground {
            outbox.push(Frame::Foreground { pid: Some(pid) });
        }
        if let Some(exit) = &state.exited {
            outbox.push(Frame::Exited(exit.clone()));
        }
        if state.output_ended {
            outbox.push(Frame::OutputEnded);
        }
        state.controller = Some(Controller {
            id,
            outbox: Arc::clone(&outbox),
        });

        let writer = match stream.try_clone() {
            Ok(writer) => writer,
            Err(_) => {
                state.controller = None;
                return;
            }
        };
        let spawned = std::thread::Builder::new()
            .name("keeper-out".into())
            .spawn(move || outbox.drain_into(writer))
            .and_then(|_| {
                let keeper = Arc::clone(self);
                std::thread::Builder::new()
                    .name("keeper-in".into())
                    .spawn(move || keeper.serve_controller(id, stream))
            });
        if spawned.is_err() {
            if let Some(controller) = state.controller.take() {
                controller.outbox.close();
            }
        }
    }

    fn detach(&self, id: u64) {
        let mut state = self.state.lock().expect("keeper state");
        if state.controller.as_ref().is_some_and(|c| c.id == id) {
            if let Some(controller) = state.controller.take() {
                controller.outbox.close();
            }
        }
    }

    fn is_current(state: &State, id: u64) -> bool {
        state.controller.as_ref().is_some_and(|c| c.id == id)
    }

    /// The frames of controller `id` until it lets go or its connection ends.
    fn serve_controller(self: Arc<Self>, id: u64, mut stream: UnixStream) {
        while let Ok(Some(frame)) = wire::read_frame(&mut stream) {
            if !Self::is_current(&self.state.lock().expect("keeper state"), id) {
                break;
            }
            match frame {
                Frame::Input { seq, data } => {
                    let job = InputJob {
                        controller: id,
                        seq,
                        data,
                    };
                    let _ = self.inputs.lock().expect("keeper inputs").send(job);
                }
                Frame::Resize { cols, rows } => {
                    if cols > 0 && rows > 0 {
                        let resized = self.master.lock().expect("keeper master").resize(
                            portable_pty::PtySize {
                                rows,
                                cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            },
                        );
                        if resized.is_ok() {
                            self.state.lock().expect("keeper state").size = (cols, rows);
                        }
                    }
                }
                Frame::Close => {
                    let _ = self.lifecycle.close(self.managed);
                    let mut state = self.state.lock().expect("keeper state");
                    state.closing = true;
                    self.changed.notify_all();
                }
                Frame::Stop => {
                    let _ = self.lifecycle.close(true);
                }
                Frame::ExitAck => {
                    let mut state = self.state.lock().expect("keeper state");
                    if state.exited.is_some() && state.output_ended {
                        state.acknowledged = true;
                        self.changed.notify_all();
                    }
                }
                Frame::Release => break,
                _ => {}
            }
        }
        self.detach(id);
    }

    /// Write input in arrival order and answer each write. An ordinary
    /// session's terminal blocks until the program reads; a managed job's
    /// fails at once when full.
    fn write_inputs(&self, mut writer: Box<dyn io::Write + Send>, jobs: mpsc::Receiver<InputJob>) {
        for job in jobs {
            let error = writer
                .write_all(&job.data)
                .and_then(|()| writer.flush())
                .err()
                .map(|error| {
                    if error.kind() == io::ErrorKind::WouldBlock {
                        "the program does not read its input".to_owned()
                    } else {
                        error.to_string()
                    }
                });
            let mut state = self.state.lock().expect("keeper state");
            if Self::is_current(&state, job.controller) {
                Self::tell(
                    &mut state,
                    Frame::InputResult {
                        seq: job.seq,
                        error,
                    },
                );
            }
        }
    }

    /// Report the terminal's foreground process group when it changes.
    fn watch_foreground(&self) {
        loop {
            std::thread::sleep(FOREGROUND_INTERVAL);
            if self.stopping.load(Ordering::Relaxed) {
                return;
            }
            let foreground = {
                let _master = self.master.lock().expect("keeper master");
                terminal::foreground_of(self.master_fd)
            };
            let mut state = self.state.lock().expect("keeper state");
            if state.output_ended {
                return;
            }
            if foreground != state.foreground {
                state.foreground = foreground;
                Self::tell(&mut state, Frame::Foreground { pid: foreground });
            }
        }
    }

    /// New connections: each gets `hello` and may claim the keeper or leave.
    fn accept(self: Arc<Self>, listener: UnixListener) {
        for stream in listener.incoming() {
            if self.stopping.load(Ordering::Relaxed) {
                return;
            }
            let Ok(stream) = stream else { continue };
            let keeper = Arc::clone(&self);
            let _ = std::thread::Builder::new()
                .name("keeper-conn".into())
                .spawn(move || keeper.serve_connection(stream));
        }
    }

    fn serve_connection(self: Arc<Self>, mut stream: UnixStream) {
        if !dir::peer_is_owner(&stream) {
            return;
        }
        let hello = self.hello(&self.state.lock().expect("keeper state"));
        if wire::write_frame(&mut stream, &Frame::Hello(Box::new(hello))).is_err() {
            return;
        }
        let _ = stream.set_read_timeout(Some(CLAIM_TIMEOUT));
        let Ok(Some(Frame::Claim { takeover, from })) = wire::read_frame(&mut stream) else {
            return;
        };
        let _ = stream.set_read_timeout(None);
        let mut state = self.state.lock().expect("keeper state");
        if state.controller.is_some() && !takeover {
            drop(state);
            let _ = wire::write_frame(&mut stream, &Frame::Busy);
            return;
        }
        let oldest = state.end - u64::try_from(state.ring.len()).unwrap_or(0);
        self.install(&mut state, stream, from.unwrap_or(oldest), true);
    }

    fn wait_until_done(&self, retention: Duration) {
        let mut state = self.state.lock().expect("keeper state");
        let mut ended_at: Option<Instant> = None;
        loop {
            if state.exited.is_some() && state.output_ended {
                if state.acknowledged || state.closing {
                    return;
                }
                let since = *ended_at.get_or_insert_with(Instant::now);
                let Some(left) = retention.checked_sub(since.elapsed()) else {
                    return;
                };
                state = self
                    .changed
                    .wait_timeout(state, left)
                    .expect("keeper state")
                    .0;
            } else {
                state = self.changed.wait(state).expect("keeper state");
            }
        }
    }

    /// Remove the socket, end the acceptor and let the controller's writer
    /// send what it has.
    fn shut_down(&self) {
        self.stopping.store(true, Ordering::Relaxed);
        // Wakes the acceptor, which sees that the keeper stops.
        let _ = UnixStream::connect(&self.socket);
        let _ = std::fs::remove_file(&self.socket);
        let controller = self.state.lock().expect("keeper state").controller.take();
        if let Some(controller) = controller {
            controller.outbox.close();
            controller.outbox.wait_drained(Duration::from_secs(1));
        }
    }
}

/// Frames for one controller, sent by its own thread so a slow controller
/// never blocks the terminal's reader.
#[derive(Default)]
struct Outbox {
    state: Mutex<OutboxState>,
    ready: Condvar,
}

#[derive(Default)]
struct OutboxState {
    frames: VecDeque<Frame>,
    /// Output bytes queued and not yet sent.
    behind: usize,
    closed: bool,
    drained: bool,
}

impl Outbox {
    /// Queue a frame; false when closed or too far behind.
    fn push(&self, frame: Frame) -> bool {
        let mut state = self.state.lock().expect("outbox");
        if state.closed {
            return false;
        }
        if let Frame::Output { data, .. } = &frame {
            state.behind += data.len();
            if state.behind > MAX_BEHIND {
                return false;
            }
        }
        state.frames.push_back(frame);
        self.ready.notify_all();
        true
    }

    fn close(&self) {
        let mut state = self.state.lock().expect("outbox");
        state.closed = true;
        self.ready.notify_all();
    }

    /// Send queued frames until the outbox is closed and empty, or the
    /// connection fails.
    fn drain_into(&self, mut stream: UnixStream) {
        loop {
            let frame = {
                let mut state = self.state.lock().expect("outbox");
                loop {
                    if let Some(frame) = state.frames.pop_front() {
                        if let Frame::Output { data, .. } = &frame {
                            state.behind = state.behind.saturating_sub(data.len());
                        }
                        break Some(frame);
                    }
                    if state.closed {
                        break None;
                    }
                    state = self.ready.wait(state).expect("outbox");
                }
            };
            let Some(frame) = frame else { break };
            if wire::write_frame(&mut stream, &frame).is_err() {
                break;
            }
        }
        let _ = stream.shutdown(std::net::Shutdown::Both);
        let mut state = self.state.lock().expect("outbox");
        state.closed = true;
        state.drained = true;
        state.frames.clear();
        self.ready.notify_all();
    }

    fn wait_drained(&self, limit: Duration) {
        let deadline = Instant::now() + limit;
        let mut state = self.state.lock().expect("outbox");
        while !state.drained {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return;
            };
            state = self.ready.wait_timeout(state, left).expect("outbox").0;
        }
    }
}

#[cfg(test)]
mod tests;
