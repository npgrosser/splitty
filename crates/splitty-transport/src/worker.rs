//! Owned transport children keep their admission slot through verified exit,
//! including cancellation and Drop. Merely requesting kill releases nothing.
use crate::{admission::Admission, budget::Permit};
use std::{io, process::ExitStatus};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

struct UntilExit {
    slot: Option<Permit>,
    observed: Option<Box<dyn FnOnce() + Send>>,
}
impl UntilExit {
    fn observed(mut self) {
        self.slot.take();
        if let Some(observed) = self.observed.take() {
            observed();
        }
    }
}
impl Drop for UntilExit {
    fn drop(&mut self) {
        // Dropping an observer (including runtime cancellation) is not proof of
        // process exit. Keep capacity reserved in every ambiguous path.
        if let Some(slot) = self.slot.take() {
            std::mem::forget(slot);
        }
    }
}
pub struct Worker {
    child: Option<Child>,
    slot: Option<UntilExit>,
}
impl Worker {
    pub fn spawn(command: &mut Command, admission: &Admission) -> io::Result<Self> {
        let slot = admission
            .worker()
            .map_err(|e| io::Error::new(io::ErrorKind::WouldBlock, e))?;
        let child = command.kill_on_drop(true).spawn()?;
        Ok(Self {
            child: Some(child),
            slot: Some(UntilExit {
                slot: Some(slot),
                observed: None,
            }),
        })
    }
    /// The callback runs only after wait/try_wait proved termination. Drop or
    /// canceled reaping discards it without making an exit assertion.
    pub fn on_exit(&mut self, observed: impl FnOnce() + Send + 'static) {
        if let Some(slot) = &mut self.slot {
            let previous = slot.observed.take();
            slot.observed = Some(Box::new(move || {
                if let Some(previous) = previous {
                    previous();
                }
                observed();
            }));
        } else {
            observed();
        } // this worker's exit was already observed
    }
    fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("owned worker")
    }
    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }
    pub fn stdin(&mut self) -> Option<ChildStdin> {
        self.child().stdin.take()
    }
    pub fn stdout(&mut self) -> Option<ChildStdout> {
        self.child().stdout.take()
    }
    pub fn stderr(&mut self) -> Option<ChildStderr> {
        self.child().stderr.take()
    }
    pub fn start_kill(&mut self) -> io::Result<()> {
        self.child().start_kill()
    }
    pub async fn kill(&mut self) -> io::Result<()> {
        self.start_kill()?;
        self.wait().await.map(|_| ())
    }
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        let result = self.child().wait().await?;
        if let Some(slot) = self.slot.take() {
            slot.observed();
        }
        Ok(result)
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let result = self.child().try_wait()?;
        if result.is_some() {
            if let Some(slot) = self.slot.take() {
                slot.observed();
            }
        }
        Ok(result)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let Some(slot) = self.slot.take() else {
            return;
        };
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.start_kill();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if child.wait().await.is_ok() {
                    slot.observed();
                }
            });
        }
    }
}

/// The same process-wide worker admission for synchronous provisioning.
/// Use only from blocking work: Drop kills and waits for this owned child.
/// A failed wait retains the reservation rather than asserting termination.
pub struct BlockingWorker {
    child: std::process::Child,
    slot: Option<UntilExit>,
}
impl BlockingWorker {
    pub fn spawn(command: &mut std::process::Command, admission: &Admission) -> io::Result<Self> {
        let slot = admission
            .worker()
            .map_err(|e| io::Error::new(io::ErrorKind::WouldBlock, e))?;
        let child = command.spawn()?;
        Ok(Self {
            child,
            slot: Some(UntilExit {
                slot: Some(slot),
                observed: None,
            }),
        })
    }
    pub fn stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.child.stdin.take()
    }
    pub fn stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.stdout.take()
    }
    pub fn stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.stderr.take()
    }
    pub fn kill(&mut self) -> io::Result<()> {
        self.child.kill()
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = self.child.wait()?;
        if let Some(slot) = self.slot.take() {
            slot.observed();
        }
        Ok(status)
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            if let Some(slot) = self.slot.take() {
                slot.observed();
            }
        }
        Ok(status)
    }
}
impl Drop for BlockingWorker {
    fn drop(&mut self) {
        if self.slot.is_some() {
            let _ = self.child.kill();
            let _ = self.wait();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn kill_request_keeps_its_slot_until_exit_is_observed() {
        let admission = Admission::default();
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let mut worker = Worker::spawn(&mut command, &admission).unwrap();
        let observed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callback = observed.clone();
        worker.on_exit(move || {
            callback.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        assert_eq!(admission.counts().workers, 1);
        worker.start_kill().unwrap();
        assert_eq!(admission.counts().workers, 1);
        assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 0);
        worker.wait().await.unwrap();
        assert_eq!(admission.counts().workers, 0);
        assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 1);
        worker.try_wait().unwrap();
        assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 1);
        drop(worker);
        assert_eq!(admission.counts().workers, 0);
    }
    #[tokio::test]
    async fn blocking_and_async_workers_share_capacity_until_observed_exit() {
        let admission = Admission::default();
        let retained: Vec<_> = (1..crate::admission::WORKERS)
            .map(|_| admission.worker().unwrap())
            .collect();
        let mut command = std::process::Command::new("sleep");
        command.arg("30");
        let mut blocking = BlockingWorker::spawn(&mut command, &admission).unwrap();
        blocking.kill().unwrap();
        assert_eq!(admission.counts().workers, crate::admission::WORKERS);
        let error = Worker::spawn(&mut Command::new("true"), &admission)
            .err()
            .expect("worker ceiling");
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        blocking.wait().unwrap();
        let mut next = Worker::spawn(&mut Command::new("true"), &admission).unwrap();
        next.wait().await.unwrap();
        drop(blocking);
        drop(retained);
        assert_eq!(admission.counts().workers, 0);
        let blocking = BlockingWorker::spawn(&mut command, &admission).unwrap();
        assert_eq!(admission.counts().workers, 1);
        drop(blocking); // synchronous cleanup also observes exit
        assert_eq!(admission.counts().workers, 0);
    }
}
