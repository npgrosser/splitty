//! One owner of a spawned child's PID: it alone observes/reaps and signals it.
//! A stopping Unix child is observed with WNOWAIT until group escalation ends,
//! so its PID (also the launch process-group ID) cannot be reused in that interval.

use std::io;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{oneshot, watch};

use super::ProcessResult;

const TICK: Duration = Duration::from_millis(20);
const GRACE: Duration = Duration::from_secs(3);
const CONFIRM: Duration = Duration::from_secs(2);

type StopReply = oneshot::Sender<Result<ProcessResult, String>>;

/// A child's result and the stops still waiting for it, which are answered
/// once the result is recorded.
type Ended = (ProcessResult, Vec<StopReply>);

#[cfg(unix)]
type Terminal = Option<std::os::fd::OwnedFd>;
#[cfg(not(unix))]
type Terminal = Option<()>;

#[cfg(unix)]
fn own_terminal(fd: Option<i32>) -> io::Result<Terminal> {
    use std::os::fd::FromRawFd;
    fd.map(|fd| {
        let owned = nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_DUPFD_CLOEXEC(0))
            .map_err(io::Error::from)?;
        // SAFETY: fcntl returned a new descriptor whose ownership is transferred once.
        Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(owned) })
    })
    .transpose()
}
#[cfg(not(unix))]
fn own_terminal(_fd: Option<i32>) -> io::Result<Terminal> {
    Ok(None)
}

enum Control {
    Hangup,
    Stop(Option<StopReply>),
}

pub(crate) struct Lifecycle {
    control: mpsc::SyncSender<Control>,
    result: Arc<Mutex<Option<ProcessResult>>>,
    done: watch::Receiver<bool>,
}

/// Owns cleanup until the lifecycle worker has consumed the child.
struct ChildGuard(Option<Box<dyn portable_pty::Child + Send + Sync>>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            #[cfg(unix)]
            if let Some(pid) = child
                .process_id()
                .and_then(|pid| i32::try_from(pid).ok())
                .filter(|pid| *pid > 0)
                .map(nix::unistd::Pid::from_raw)
            {
                if nix::unistd::getpgid(Some(pid)).ok() == Some(pid) {
                    let _ = nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGKILL);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests;

impl Lifecycle {
    pub fn start(
        child: Box<dyn portable_pty::Child + Send + Sync>,
        terminal: Option<i32>,
    ) -> io::Result<Self> {
        Self::start_notifying(child, terminal, |_| {})
    }

    /// Like [`Lifecycle::start`], calling `on_done` with the result once the
    /// process has ended, for a caller that waits without a runtime.
    pub fn start_notifying(
        child: Box<dyn portable_pty::Child + Send + Sync>,
        terminal: Option<i32>,
        on_done: impl FnOnce(&ProcessResult) + Send + 'static,
    ) -> io::Result<Self> {
        let child = ChildGuard(Some(child));
        let terminal = own_terminal(terminal)?;
        let (tx, rx) = mpsc::sync_channel(16);
        let result = Arc::new(Mutex::new(None));
        let result_out = Arc::clone(&result);
        let (done_tx, done) = watch::channel(false);
        std::thread::Builder::new()
            .name("pty-lifecycle".into())
            .spawn(move || {
                let (outcome, replies) = supervise(child, rx, terminal);
                *result_out.lock().expect("process result lock") = Some(outcome.clone());
                done_tx.send_replace(true);
                for reply in replies {
                    let _ = reply.send(match &outcome.error {
                        Some(error) => Err(error.clone()),
                        None => Ok(outcome.clone()),
                    });
                }
                on_done(&outcome);
            })?;
        Ok(Self {
            control: tx,
            result,
            done,
        })
    }

    pub fn done(&self) -> watch::Receiver<bool> {
        self.done.clone()
    }

    pub fn result(&self) -> Option<ProcessResult> {
        self.result.lock().expect("process result lock").clone()
    }

    pub fn close(&self, managed: bool) -> io::Result<()> {
        if self.result().is_some() {
            return Ok(());
        }
        let command = if managed {
            Control::Stop(None)
        } else {
            Control::Hangup
        };
        self.control
            .try_send(command)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub async fn stop(&self) -> io::Result<ProcessResult> {
        if let Some(result) = self.result() {
            return Ok(result);
        }
        let (tx, rx) = oneshot::channel();
        if let Err(error) = self.control.try_send(Control::Stop(Some(tx))) {
            return self
                .result()
                .ok_or_else(|| io::Error::other(error.to_string()));
        }
        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => Err(io::Error::other(error)),
            Err(error) => {
                // The worker can finish naturally between the initial check and the send.
                let mut done = self.done();
                let _ = done.wait_for(|finished| *finished).await;
                self.result()
                    .ok_or_else(|| io::Error::other(error.to_string()))
            }
        }
    }
}

#[cfg(unix)]
trait OwnedProcess {
    fn observe(&mut self) -> Result<Option<ProcessResult>, String>;
    fn signal(&mut self, signal: nix::sys::signal::Signal) -> Result<(), String>;
    fn reap(&mut self) -> Result<(), String>;
    fn abandon(&mut self);
}

#[cfg(unix)]
struct UnixChild {
    child: ChildGuard,
    pid: nix::unistd::Pid,
    terminal: Terminal,
}

#[cfg(unix)]
impl OwnedProcess for UnixChild {
    fn observe(&mut self) -> Result<Option<ProcessResult>, String> {
        // nix 0.29 exposes waitid on Linux but not macOS. Both provide the POSIX
        // syscall and siginfo accessors through libc; use that shared interface.
        let pid: nix::libc::id_t = self
            .pid
            .as_raw()
            .try_into()
            .map_err(|_| "invalid child PID".to_owned())?;
        // SAFETY: siginfo_t is a C output structure; zero initialization also covers
        // WNOHANG implementations that leave it untouched. The pointer stays valid
        // for the call and the sole lifecycle owner retains this child's identity.
        let (status, info) = unsafe {
            let mut info: nix::libc::siginfo_t = std::mem::zeroed();
            let result = nix::libc::waitid(
                nix::libc::P_PID,
                pid,
                &mut info,
                nix::libc::WEXITED | nix::libc::WNOHANG | nix::libc::WNOWAIT,
            );
            (result, info)
        };
        if status < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::Interrupted {
                Ok(None)
            } else {
                Err(format!("wait failed: {error}"))
            };
        }
        // SAFETY: waitid initialized these child-status fields, or left the zeroed
        // no-event result. We read si_status only for terminal child-status codes.
        if unsafe { info.si_pid() } == 0 {
            return Ok(None);
        }
        if info.si_code == nix::libc::CLD_EXITED {
            Ok(Some(ProcessResult {
                exit_code: u32::try_from(unsafe { info.si_status() }).ok(),
                ..Default::default()
            }))
        } else if info.si_code == nix::libc::CLD_KILLED || info.si_code == nix::libc::CLD_DUMPED {
            let number = unsafe { info.si_status() };
            let signal = nix::sys::signal::Signal::try_from(number)
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|_| format!("signal {number}"));
            Ok(Some(ProcessResult {
                signal: Some(signal),
                ..Default::default()
            }))
        } else {
            Ok(None)
        }
    }

    fn signal(&mut self, sig: nix::sys::signal::Signal) -> Result<(), String> {
        use nix::sys::signal::{kill, killpg, Signal};
        use nix::unistd::{getpgid, getsid, tcgetpgrp};
        let pid = self.pid;
        // Ordinary terminal close also hangs up a foreground job. Resolve it now
        // from our owned terminal, not from a PID saved by a UI/proc-info poll.
        if sig == Signal::SIGHUP {
            if let Some(foreground) = self.terminal.as_ref().and_then(|fd| tcgetpgrp(fd).ok()) {
                if foreground != pid && getsid(Some(foreground)).ok() == Some(pid) {
                    let _ = killpg(foreground, sig);
                }
            }
        }
        // The process before its group: macOS signals a group member by member,
        // newest first, so a shell could see its child killed and exit with the
        // child's status before its own signal arrives.
        let leader = getpgid(Some(pid)).ok() == Some(pid);
        match kill(pid, sig) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => return Err(error.to_string()),
        }
        if leader {
            match killpg(pid, sig) {
                Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(())
    }

    fn reap(&mut self) -> Result<(), String> {
        loop {
            match nix::sys::wait::waitpid(self.pid, None) {
                Err(nix::errno::Errno::EINTR) => continue,
                result => {
                    self.child.0.take();
                    return result.map(|_| ()).map_err(|error| error.to_string());
                }
            }
        }
    }

    fn abandon(&mut self) {
        self.child.0.take();
    }
}

#[cfg(unix)]
fn supervise(child: ChildGuard, control: mpsc::Receiver<Control>, terminal: Terminal) -> Ended {
    let Some(pid) = child
        .0
        .as_ref()
        .and_then(|c| c.process_id())
        .and_then(|pid| i32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .map(nix::unistd::Pid::from_raw)
    else {
        let outcome = ProcessResult {
            error: Some("spawned child has no valid PID".into()),
            ..Default::default()
        };
        return (outcome, Vec::new());
    };
    drive(
        &mut UnixChild {
            child,
            pid,
            terminal,
        },
        control,
        GRACE,
        CONFIRM,
    )
}

#[cfg(unix)]
fn drive(
    process: &mut impl OwnedProcess,
    control: mpsc::Receiver<Control>,
    grace: Duration,
    confirm: Duration,
) -> Ended {
    use nix::sys::signal::Signal;
    let mut stopping: Option<Instant> = None;
    let mut escalated = false;
    let mut replies: Vec<StopReply> = Vec::new();
    let mut disconnected = false;
    loop {
        let observed = match process.observe() {
            Ok(observed) => observed,
            Err(error) => {
                // We no longer have reliable evidence of ownership. Do not signal again.
                process.abandon();
                let outcome = ProcessResult {
                    error: Some(error),
                    ..Default::default()
                };
                return (outcome, replies);
            }
        };
        if let Some(started) = stopping {
            if !escalated && started.elapsed() >= grace {
                if let Err(error) = process.signal(Signal::SIGKILL) {
                    for reply in replies.drain(..) {
                        let _ = reply.send(Err(error.clone()));
                    }
                }
                escalated = true;
            }
            if observed.is_none() && started.elapsed() >= grace + confirm {
                for reply in replies.drain(..) {
                    let _ = reply.send(Err("could not confirm job termination".into()));
                }
            }
        }
        if let Some(outcome) = observed {
            if stopping.is_none() || escalated {
                let outcome = match process.reap() {
                    Ok(()) => outcome,
                    Err(error) => ProcessResult {
                        error: Some(error),
                        ..Default::default()
                    },
                };
                return (outcome, replies);
            }
        }
        let request = if disconnected {
            std::thread::sleep(TICK);
            None
        } else {
            match control.recv_timeout(TICK) {
                Ok(request) => Some(request),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    disconnected = true;
                    Some(Control::Stop(None))
                }
            }
        };
        match request {
            Some(Control::Hangup) => {
                let _ = process.signal(Signal::SIGHUP);
            }
            Some(Control::Stop(reply)) => {
                if let Some(reply) = reply {
                    replies.push(reply);
                }
                if stopping.is_none() {
                    if let Err(error) = process.signal(Signal::SIGTERM) {
                        for reply in replies.drain(..) {
                            let _ = reply.send(Err(error.clone()));
                        }
                    }
                    stopping = Some(Instant::now());
                }
            }
            None => {}
        }
    }
}

#[cfg(not(unix))]
fn supervise(
    mut child: ChildGuard,
    control: mpsc::Receiver<Control>,
    _terminal: Terminal,
) -> Ended {
    let mut replies: Vec<StopReply> = Vec::new();
    loop {
        let result = child.0.as_mut().expect("owned child").try_wait();
        match result {
            Ok(Some(status)) => {
                let outcome = ProcessResult {
                    exit_code: Some(status.exit_code()),
                    ..Default::default()
                };
                child.0.take();
                return (outcome, replies);
            }
            Err(error) => {
                let outcome = ProcessResult {
                    error: Some(error.to_string()),
                    ..Default::default()
                };
                return (outcome, replies);
            }
            Ok(None) => {}
        }
        match control.recv_timeout(TICK) {
            Ok(Control::Stop(reply)) => {
                if let Some(reply) = reply {
                    replies.push(reply);
                }
                let _ = child.0.as_mut().expect("owned child").kill();
            }
            Ok(Control::Hangup) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = child.0.as_mut().expect("owned child").kill();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}
