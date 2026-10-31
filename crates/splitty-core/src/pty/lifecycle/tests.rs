use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
struct FailedSetupChild {
    kills: Arc<AtomicUsize>,
    waits: Arc<AtomicUsize>,
}
impl portable_pty::ChildKiller for FailedSetupChild {
    fn kill(&mut self) -> io::Result<()> {
        self.kills.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(Self {
            kills: Arc::clone(&self.kills),
            waits: Arc::clone(&self.waits),
        })
    }
}
impl portable_pty::Child for FailedSetupChild {
    fn process_id(&self) -> Option<u32> {
        None
    }
    fn try_wait(&mut self) -> io::Result<Option<portable_pty::ExitStatus>> {
        Ok(None)
    }
    fn wait(&mut self) -> io::Result<portable_pty::ExitStatus> {
        self.waits.fetch_add(1, Ordering::SeqCst);
        Ok(portable_pty::ExitStatus::with_exit_code(1))
    }
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        None
    }
}

#[test]
#[cfg(unix)]
fn setup_failure_kills_and_reaps_before_returning_error() {
    let kills = Arc::new(AtomicUsize::new(0));
    let waits = Arc::new(AtomicUsize::new(0));
    let child = Box::new(FailedSetupChild {
        kills: kills.clone(),
        waits: waits.clone(),
    });
    assert!(Lifecycle::start(child, Some(-1)).is_err());
    assert_eq!(kills.load(Ordering::SeqCst), 1);
    assert_eq!(waits.load(Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[derive(Default)]
struct KernelState {
    exited: bool,
    reaped: bool,
    signals: Vec<nix::sys::signal::Signal>,
    fail_signal: bool,
}
#[cfg(unix)]
struct FakeProcess(Arc<Mutex<KernelState>>);
#[cfg(unix)]
impl OwnedProcess for FakeProcess {
    fn observe(&mut self) -> Result<Option<ProcessResult>, String> {
        let kernel = self.0.lock().expect("kernel");
        assert!(!kernel.reaped, "must never inspect a reused child identity");
        Ok(kernel.exited.then(|| ProcessResult {
            exit_code: Some(7),
            ..Default::default()
        }))
    }
    fn signal(&mut self, signal: nix::sys::signal::Signal) -> Result<(), String> {
        let mut kernel = self.0.lock().expect("kernel");
        assert!(!kernel.reaped, "must never signal a reused child identity");
        kernel.signals.push(signal);
        if kernel.fail_signal {
            Err("permission denied".into())
        } else {
            Ok(())
        }
    }
    fn reap(&mut self) -> Result<(), String> {
        let mut kernel = self.0.lock().expect("kernel");
        assert!(kernel.exited && !kernel.reaped);
        kernel.reaped = true;
        Ok(())
    }
    fn abandon(&mut self) {
        self.0.lock().expect("kernel").reaped = true;
    }
}

#[test]
#[cfg(unix)]
fn observed_exit_wins_a_queued_stop_without_signalling_reused_identity() {
    let kernel = Arc::new(Mutex::new(KernelState {
        exited: true,
        ..Default::default()
    }));
    let (tx, rx) = mpsc::sync_channel(1);
    tx.send(Control::Stop(None)).expect("queued stop");
    let (result, replies) = drive(
        &mut FakeProcess(kernel.clone()),
        rx,
        Duration::ZERO,
        Duration::ZERO,
    );
    assert_eq!(result.exit_code, Some(7));
    assert!(replies.is_empty());
    let kernel = kernel.lock().expect("kernel");
    assert!(kernel.reaped);
    assert!(kernel.signals.is_empty());
}

#[test]
#[cfg(unix)]
fn failed_signalling_and_unconfirmed_stop_are_errors_not_success() {
    for fail_signal in [false, true] {
        let kernel = Arc::new(Mutex::new(KernelState {
            fail_signal,
            ..Default::default()
        }));
        let worker_kernel = kernel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let (reply, result) = oneshot::channel();
        tx.send(Control::Stop(Some(reply))).expect("stop");
        let worker = std::thread::spawn(move || {
            drive(
                &mut FakeProcess(worker_kernel),
                rx,
                Duration::ZERO,
                Duration::ZERO,
            )
        });
        let error = result
            .blocking_recv()
            .expect("stop replied")
            .expect_err("not confirmed");
        if fail_signal {
            assert!(error.contains("permission denied"));
        } else {
            assert!(error.contains("could not confirm"));
        }
        // The acknowledged failure is a barrier: the child can only now exit.
        kernel.lock().expect("kernel").exited = true;
        let (result, replies) = worker.join().expect("lifecycle worker");
        assert_eq!(result.exit_code, Some(7));
        assert!(replies.is_empty(), "the failed stop was answered already");
        assert!(kernel.lock().expect("kernel").reaped);
    }
}
