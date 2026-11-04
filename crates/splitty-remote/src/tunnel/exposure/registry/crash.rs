//! Library process smoke at the actual spawn/publication boundaries. This
//! exercises the real registry and OS children with a private test authority;
//! production policy and real SSH forwarding are checked separately by the
//! server's remote_connect smoke. No test flag exists in production binaries.
use super::*;
use std::os::unix::process::CommandExt;
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const FIXTURE: &str = "SPLITTY_EXPOSURE_UNIT_PROCESS_FIXTURE";
const PHASE: &str = "SPLITTY_EXPOSURE_UNIT_PROCESS_PHASE";
const WAIT: Duration = Duration::from_secs(10);

pub(super) fn pause(phase: &str, pid: Option<u32>) {
    if std::env::var(PHASE).as_deref() != Ok(phase) {
        return;
    }
    let directory = PathBuf::from(std::env::var_os(FIXTURE).expect("private process fixture"));
    let identity = pid.map(|pid| platform::identity(pid).expect("spawned worker identity"));
    std::fs::write(
        directory.join("paused.json"),
        serde_json::to_vec(&identity).unwrap(),
    )
    .unwrap();
    loop {
        std::thread::park_timeout(Duration::from_secs(1));
    }
}

fn wait(reason: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !ready() {
        assert!(Instant::now() < deadline, "{reason}");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn terminated(pid: u32) -> bool {
    platform::identity(pid).is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
}
struct Coordinator {
    child: Child,
    directory: Option<tempfile::TempDir>,
    worker: Option<Identity>,
    before_spawn_confirmed: bool,
}
impl Drop for Coordinator {
    fn drop(&mut self) {
        // The unreaped direct child pins this process-group number throughout
        // cleanup. Never signal a saved, already-reaped PID or scan by name.
        let group = nix::unistd::Pid::from_raw(i32::try_from(self.child.id()).unwrap());
        let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
        let deadline = Instant::now() + WAIT;
        let mut confirmed = self.before_spawn_confirmed;
        while !confirmed && Instant::now() < deadline {
            confirmed = self
                .worker
                .as_ref()
                .is_some_and(|worker| terminated(worker.pid));
            if !confirmed {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let _ = self.child.wait();
        if !confirmed {
            // Preserve uncertain claims rather than remove their authority.
            let path = self.directory.take().unwrap().keep();
            eprintln!(
                "unconfirmed fixture worker cleanup; reservation retained at {}",
                path.display()
            );
            assert!(std::thread::panicking(), "fixture worker did not terminate");
        }
    }
}
fn scope(uid: u32) -> Scope {
    Scope {
        machine: "process-smoke-machine".into(),
        boot: "process-smoke-boot".into(),
        network: 0,
        pid_namespace: 0,
        uid,
    }
}
fn worker_program(directory: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("python3");
    command.args(["-c", "import pathlib,sys,time\np=pathlib.Path(sys.argv[1])\n(p/'worker-ready').touch()\nwhile not (p/'stop-worker').exists(): time.sleep(0.01)"])
        .arg(directory).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    command
}

#[test]
fn crash_boundaries() {
    if let Some(directory) = std::env::var_os(FIXTURE) {
        // Only this test entry point injects a fixture authority, through the
        // private constructor. Registry::system has no bypass or override.
        let directory = PathBuf::from(directory);
        let owner = platform::identity(std::process::id()).unwrap();
        let registry = Registry::new(directory.join("registry"), scope(owner.uid), owner).unwrap();
        let port: u16 = std::fs::read_to_string(directory.join("port"))
            .unwrap()
            .parse()
            .unwrap();
        let share = tests::share(&registry, port);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let _worker = share
                .spawn(&mut worker_program(&directory), &Admission::default())
                .await
                .unwrap();
            panic!("requested process boundary was not reached");
        });
        return;
    }
    for phase in ["beforeSpawn", "beforePublication", "afterPublication"] {
        // /tmp may be tmpfs, intentionally unsupported by the production
        // durable-storage policy. Keep fixtures on the repository filesystem.
        let directory = tempfile::Builder::new()
            .prefix(".exposure-crash-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        let address = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        std::fs::write(directory.path().join("port"), address.port().to_string()).unwrap();
        let test_name = format!(
            "{}::crash_boundaries",
            module_path!().split_once("::").unwrap().1
        );
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name, "--nocapture"])
            .env(FIXTURE, directory.path())
            .env(PHASE, phase)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut coordinator = Coordinator {
            child,
            directory: Some(directory),
            worker: None,
            before_spawn_confirmed: false,
        };
        let directory = coordinator.directory.as_ref().unwrap().path().to_owned();
        let mut paused: Option<Option<Identity>> = None;
        wait(
            "coordinator never reached the real transaction boundary",
            || {
                paused = std::fs::read(directory.join("paused.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                paused.is_some()
            },
        );
        coordinator.worker = paused.unwrap();
        coordinator.before_spawn_confirmed = phase == "beforeSpawn" && coordinator.worker.is_none();
        let owner = platform::identity(std::process::id()).unwrap();
        let observer = Registry::new(directory.join("registry"), scope(owner.uid), owner).unwrap();
        observer
            .transaction(|records| {
                assert_eq!(records.entries.len(), 1);
                let entry = &records.entries[0];
                assert_eq!(entry.owner.pid, coordinator.child.id());
                assert_eq!(entry.workers.len(), 1);
                assert_eq!(
                    entry.workers[0].worker.is_some(),
                    phase == "afterPublication"
                );
                Ok(())
            })
            .unwrap();
        if phase != "beforeSpawn" {
            wait("real worker did not execute", || {
                directory.join("worker-ready").exists()
            });
            let worker = coordinator.worker.as_ref().unwrap();
            assert_eq!(&platform::identity(worker.pid).unwrap(), worker);
        }
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(coordinator.child.id()).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        )
        .unwrap();
        // Do not reap the coordinator until the owning guard finishes cleanup.
        wait("coordinator did not terminate", || {
            terminated(coordinator.child.id())
        });
        assert!(
            observer.bind(address).is_err(),
            "{phase}: coordinator death cannot release the destination"
        );
        if let Some(worker) = &coordinator.worker {
            assert_eq!(
                &platform::identity(worker.pid).unwrap(),
                worker,
                "worker survives its coordinator"
            );
            std::fs::write(directory.join("stop-worker"), b"stop").unwrap();
            wait("actual local worker did not exit", || {
                terminated(worker.pid)
            });
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        if phase == "afterPublication" {
            drop(
                observer
                    .bind(address)
                    .expect("observed published worker exit permits reuse"),
            );
        } else {
            assert!(
                observer.bind(address).is_err(),
                "an ambiguous unpublished generation stays reserved after the fixture worker exits"
            );
        }
    }
}
