//! What a session's program runs on: the PTY master and the program's
//! lifecycle. The session layer (scrollback, OSC and prompt tracking, job log
//! text, subscriptions) works over any [`Terminal`]: a daemon's terminals live
//! in session keepers (`crate::keeper`), every other server's in this process
//! ([`LocalTerminal`]).

use std::ffi::OsString;
use std::io;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tokio::sync::watch;

use super::{ProcessResult, PtyError};

/// Bytes the reader thread asks for per read.
const READ_CHUNK: usize = 32 * 1024;

/// Where a terminal's output goes: the session's buffers and subscribers.
pub trait Feed: Send + Sync {
    /// The next chunk of output.
    fn record(&self, data: &[u8]);
    /// The terminal closed: no output follows.
    fn end(&self);
}

/// Everything a terminal needs to start a session's program, as the session
/// layer built it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSpec {
    pub session_id: String,
    pub pane_id: Option<String>,
    pub screen_id: Option<String>,
    pub argv: Vec<String>,
    /// The program's whole environment; nothing else is inherited.
    pub env: Vec<(String, String)>,
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// A managed job: input fails at once on a full terminal, and closing
    /// stops the program instead of hanging it up.
    pub managed: bool,
    pub buffer_size: usize,
}

/// A session's PTY master with its program.
#[async_trait]
pub trait Terminal: Send + Sync {
    /// The program's process id.
    fn pid(&self) -> Option<u32>;
    /// Write input; the result says whether the terminal took it.
    async fn write(&self, data: &[u8]) -> io::Result<()>;
    fn resize(&self, cols: u16, rows: u16) -> io::Result<()>;
    /// Hang up an ordinary session's terminal, or stop a managed one's
    /// program without waiting. Closing twice is a no-op.
    fn close(&self) -> io::Result<()>;
    /// Stop a managed program with escalation and await its result.
    async fn stop(&self) -> io::Result<ProcessResult>;
    fn begin_shutdown(&self) -> io::Result<()> {
        self.close()
    }
    async fn shutdown(&self) -> io::Result<ProcessResult> {
        self.stop().await
    }
    /// True once the program has ended.
    fn done(&self) -> watch::Receiver<bool>;
    fn result(&self) -> Option<ProcessResult>;
    /// The terminal's foreground process group.
    fn foreground_pgid(&self) -> Option<u32>;
    /// Let go of the program without ending it, where it outlives this
    /// process; afterwards closing is a no-op.
    fn release(&self) {}
}

/// Starts the terminals of a backend's sessions.
pub trait TerminalFactory: Send + Sync {
    fn start(&self, spec: &SessionSpec, feed: Arc<dyn Feed>)
        -> Result<Box<dyn Terminal>, PtyError>;
}

/// Terminals in this process.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalTerminals;

impl TerminalFactory for LocalTerminals {
    fn start(
        &self,
        spec: &SessionSpec,
        feed: Arc<dyn Feed>,
    ) -> Result<Box<dyn Terminal>, PtyError> {
        Ok(Box::new(LocalTerminal::start(spec, feed)?))
    }
}

/// What the reader polls: the master is non-blocking, so a
/// program that never reads its input cannot block the executor, and a dup'ed
/// descriptor shares that flag.
#[cfg(unix)]
pub type Readiness = Option<std::os::fd::OwnedFd>;
#[cfg(not(unix))]
pub type Readiness = Option<()>;

/// A started PTY with its program, before anything reads or supervises it.
pub struct Spawned {
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub reader: Box<dyn io::Read + Send>,
    pub writer: Box<dyn io::Write + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    pub pid: Option<u32>,
    /// The master's descriptor, for `tcgetpgrp` and the lifecycle's hang-up.
    pub master_fd: Option<i32>,
    pub readiness: Readiness,
}

/// Open a PTY of the spec's size and start the spec's program on it, as a
/// session leader with the PTY as its controlling terminal, with exactly the
/// spec's environment.
pub fn spawn(spec: &SessionSpec) -> Result<Spawned, PtyError> {
    spawn_with_input(spec, spec.managed)
}

fn spawn_with_input(spec: &SessionSpec, nonblocking: bool) -> Result<Spawned, PtyError> {
    if spec.argv.is_empty() {
        return Err(PtyError::SpawnFailed("no program to start".into()));
    }
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: spec.rows,
            cols: spec.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| PtyError::SpawnFailed(e.to_string()))?;

    let mut cmd = CommandBuilder::from_argv(spec.argv.iter().map(OsString::from).collect());
    cmd.env_clear();
    for (key, value) in &spec.env {
        cmd.env(key, value);
    }
    if let Some(dir) = &spec.cwd {
        cmd.cwd(dir);
    }

    // The master's descriptor for TIOCGPGRP, taken before the master moves.
    #[cfg(unix)]
    let master_fd = pair.master.as_raw_fd();
    #[cfg(not(unix))]
    let master_fd = None;

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| PtyError::SpawnFailed(format!("clone reader: {}", e)))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| PtyError::SpawnFailed(format!("take writer: {}", e)))?;

    // In-process writes never block the async executor. A keeper has a
    // dedicated input thread; only its managed jobs need immediate refusal.
    // dup shares O_NONBLOCK, so the output reader polls the same descriptor.
    #[cfg(unix)]
    let readiness: Readiness = if nonblocking {
        use nix::fcntl::{fcntl, FcntlArg, OFlag};
        use std::os::fd::FromRawFd;
        let fd = master_fd.ok_or_else(|| PtyError::SpawnFailed("PTY has no descriptor".into()))?;
        let flags =
            fcntl(fd, FcntlArg::F_GETFL).map_err(|e| PtyError::SpawnFailed(e.to_string()))?;
        fcntl(
            fd,
            FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
        )
        .map_err(|e| PtyError::SpawnFailed(e.to_string()))?;
        let owned = fcntl(fd, FcntlArg::F_DUPFD_CLOEXEC(0))
            .map_err(|e| PtyError::SpawnFailed(e.to_string()))?;
        // SAFETY: fcntl produced this new, uniquely owned, close-on-exec fd.
        Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(owned) })
    } else {
        None
    };
    #[cfg(not(unix))]
    let readiness: Readiness = {
        let _ = nonblocking;
        None
    };

    // Allocate fallible PTY descriptors before starting a child.
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| PtyError::SpawnFailed(e.to_string()))?;
    let pid = child.process_id();
    // The child holds the only reference to the slave from here on.
    drop(pair.slave);

    Ok(Spawned {
        master: pair.master,
        reader,
        writer,
        child,
        pid,
        master_fd,
        readiness,
    })
}

/// Read a terminal until it closes.
///
/// It runs whether or not anyone streams the session, so output always reaches
/// the feed and a busy process never blocks on a full PTY buffer. Its own
/// descriptor keeps the PTY master open; it ends at the EOF (EIO on Linux)
/// that follows once the session's processes are gone.
pub fn read_output(mut reader: Box<dyn io::Read + Send>, feed: &dyn Feed, readiness: Readiness) {
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => feed.record(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                #[cfg(unix)]
                if let Some(fd) = &readiness {
                    use std::os::fd::AsFd;
                    let mut descriptors = [nix::poll::PollFd::new(
                        fd.as_fd(),
                        nix::poll::PollFlags::POLLIN,
                    )];
                    match nix::poll::poll(&mut descriptors, nix::poll::PollTimeout::NONE) {
                        Ok(_) | Err(nix::errno::Errno::EINTR) => continue,
                        Err(_) => break,
                    }
                }
                #[cfg(not(unix))]
                let _ = &readiness;
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(_) => break,
        }
    }
    feed.end();
}

/// The foreground process group of the terminal behind `master_fd`.
#[cfg(unix)]
pub fn foreground_of(master_fd: i32) -> Option<u32> {
    use std::os::unix::io::BorrowedFd;

    if master_fd < 0 {
        return None;
    }
    // SAFETY: the caller holds the PTY master open while it asks; the
    // descriptor is only borrowed for this one ioctl.
    let fd = unsafe { BorrowedFd::borrow_raw(master_fd) };
    nix::unistd::tcgetpgrp(fd)
        .ok()
        .and_then(|pgrp| u32::try_from(pgrp.as_raw()).ok())
        .filter(|pid| *pid > 0)
}

#[cfg(not(unix))]
pub fn foreground_of(_master_fd: i32) -> Option<u32> {
    None
}

/// A PTY in this process with its reader thread and lifecycle owner.
pub struct LocalTerminal {
    pid: Option<u32>,
    master_fd: i32,
    writer: Mutex<Option<Box<dyn io::Write + Send>>>,
    input: tokio::sync::Mutex<()>,
    master: Mutex<Option<Box<dyn portable_pty::MasterPty + Send>>>,
    lifecycle: super::lifecycle::Lifecycle,
    done: watch::Receiver<bool>,
    managed: bool,
}

impl LocalTerminal {
    pub fn start(spec: &SessionSpec, feed: Arc<dyn Feed>) -> Result<Self, PtyError> {
        let Spawned {
            master,
            reader,
            writer,
            mut child,
            pid,
            master_fd,
            readiness,
        } = spawn_with_input(spec, true)?;

        // Started before anything else, so not even the program's first bytes
        // can be missed. If the thread cannot start, the closure and with it
        // the reader are dropped and the PTY hangs up as usual.
        if let Err(error) = std::thread::Builder::new()
            .name(format!("pty-reader-{}", spec.session_id))
            .spawn(move || read_output(reader, feed.as_ref(), readiness))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PtyError::SpawnFailed(format!(
                "start reader thread: {error}"
            )));
        }

        let lifecycle = super::lifecycle::Lifecycle::start(child, master_fd)
            .map_err(|error| PtyError::SpawnFailed(format!("start lifecycle: {error}")))?;
        Ok(Self {
            pid,
            master_fd: master_fd.unwrap_or(-1),
            writer: Mutex::new(Some(writer)),
            input: tokio::sync::Mutex::new(()),
            master: Mutex::new(Some(master)),
            done: lifecycle.done(),
            lifecycle,
            managed: spec.managed,
        })
    }
}

#[async_trait]
impl Terminal for LocalTerminal {
    fn pid(&self) -> Option<u32> {
        self.pid
    }

    async fn write(&self, data: &[u8]) -> io::Result<()> {
        if self.managed {
            return match self.writer.lock().expect("terminal writer").as_mut() {
                Some(writer) => {
                    writer.write_all(data)?;
                    writer.flush()
                }
                None => Err(io::Error::new(io::ErrorKind::BrokenPipe, "writer closed")),
            };
        }
        // Keep order without copying input into another queue or occupying a
        // blocking worker. The caller retains its charged buffer until done.
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let _input = self.input.lock().await;
            let mut remaining = data;
            let mut done = self.done();
            while !remaining.is_empty() {
                let result = {
                    let mut writer = self.writer.lock().expect("terminal writer");
                    writer.as_mut().ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "writer closed"))?.write(remaining)
                };
                match result {
                    Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "terminal accepted no input")),
                    Ok(n) => remaining = &remaining[n..],
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        tokio::select! {
                            _ = done.wait_for(|done| *done) => return Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal ended during input")),
                            _ = tokio::time::sleep(std::time::Duration::from_millis(5)) => {},
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        }).await.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "terminal input timed out; part may already have been written"))?
    }

    fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        match self.master.lock().expect("terminal master").as_ref() {
            Some(master) => master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| io::Error::other(e.to_string())),
            None => Err(io::Error::new(io::ErrorKind::BrokenPipe, "master closed")),
        }
    }

    fn close(&self) -> io::Result<()> {
        let mut master = self.master.lock().expect("terminal master");
        if master.is_none() {
            return Ok(());
        }
        self.lifecycle.close(self.managed)?;
        *self.writer.lock().expect("terminal writer") = None;
        *master = None;
        Ok(())
    }

    async fn stop(&self) -> io::Result<ProcessResult> {
        self.lifecycle.stop().await
    }

    fn begin_shutdown(&self) -> io::Result<()> {
        self.lifecycle.begin_shutdown();
        Ok(())
    }

    async fn shutdown(&self) -> io::Result<ProcessResult> {
        let result = self.lifecycle.shutdown().await;
        self.close()?;
        result
    }

    fn done(&self) -> watch::Receiver<bool> {
        self.done.clone()
    }

    fn result(&self) -> Option<ProcessResult> {
        self.lifecycle.result()
    }

    fn foreground_pgid(&self) -> Option<u32> {
        // The descriptor is only valid while the master is held.
        let master = self.master.lock().expect("terminal master");
        master.as_ref()?;
        foreground_of(self.master_fd)
    }
}
