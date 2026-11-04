//! Wait for the command itself, without waiting on output pipes inherited by
//! an orphan descendant. Callers redirect diagnostics into fixture files and
//! start an isolated process group on Unix; descendant checks follow this wait.
use std::{
    process::{Child, ExitStatus},
    time::{Duration, Instant},
};

pub fn wait(mut child: Child) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().expect("command exit") {
            return status;
        }
        if Instant::now() >= deadline {
            // The still-live leader anchors its dedicated group during kill.
            #[cfg(unix)]
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(child.id().try_into().expect("child PID")),
                nix::sys::signal::Signal::SIGKILL,
            );
            let _ = child.kill();
            let _ = child.wait();
            panic!("isolated command did not exit within 20 seconds");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
