//! Strict native-OS reservation proof. Run explicitly on supported Linux/macOS:
//! SPLITTY_TEST_REQUIRE_NATIVE_EXPOSURE=1 cargo test -p splitty-remote --test native_exposure -- --exact native_policy_retains_published_worker_after_coordinator_death
//! WSL's intentional refusal is not a passing substitute for this test.
//! An in-process test cannot show that persisted production reservations keep
//! excluding endpoints after the coordinator is killed without running Drop.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use splitty_remote::tunnel::exposure::{bind_gateway, check_sharing_supported, Share};
use splitty_transport::admission::Admission;
use std::{
    fs, io,
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const FIXTURE: &str = "SPLITTY_NATIVE_RESERVATION_FIXTURE";
const WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, Eq, PartialEq)]
struct Identity {
    pid: u32,
    start: u64,
    subsecond: u64,
}
#[cfg(target_os = "linux")]
fn identity(pid: u32) -> Option<Identity> {
    let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => panic!("native worker identity unavailable without exit proof: {error}"),
    };
    let fields: Vec<_> = stat
        .rsplit_once(") ")
        .expect("native process identity format")
        .1
        .split_whitespace()
        .collect();
    if matches!(fields.first(), Some(&"Z" | &"X" | &"x")) {
        return None;
    }
    Some(Identity {
        pid,
        start: fields
            .get(19)
            .expect("native process start time")
            .parse()
            .expect("native process start value"),
        subsecond: 0,
    })
}
#[cfg(target_os = "macos")]
fn identity(pid: u32) -> Option<Identity> {
    use nix::libc;
    let deadline = Instant::now() + WAIT;
    loop {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).unwrap();
        // SAFETY: this flavor fills precisely proc_bsdinfo; no saved PID is signaled.
        let result = unsafe {
            libc::proc_pidinfo(
                i32::try_from(pid).unwrap(),
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if result != size {
            // An unavailable query must not be mistaken for observed process exit.
            let missing = unsafe { libc::kill(i32::try_from(pid).unwrap(), 0) } == -1
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            if missing {
                return None;
            }
            // proc_pidinfo may transiently fail while an orphan exits, before
            // kill(pid, 0) reports ESRCH. Unknown is not exit proof: retry boundedly.
            assert!(
                Instant::now() < deadline,
                "native worker identity unavailable without exit proof"
            );
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        // SAFETY: the full initialized structure was returned above.
        let info = unsafe { info.assume_init() };
        if info.pbi_status == libc::SZOMB {
            return None;
        }
        return Some(Identity {
            pid,
            start: info.pbi_start_tvsec,
            subsecond: info.pbi_start_tvusec,
        });
    }
}

fn wait(reason: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !ready() {
        assert!(Instant::now() < deadline, "{reason}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Coordinator {
    child: Child,
    directory: tempfile::TempDir,
}
impl Coordinator {
    fn worker_pid(&self) -> Option<u32> {
        fs::read_to_string(self.directory.path().join("worker-pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }
}
impl Drop for Coordinator {
    fn drop(&mut self) {
        // Stop only this fixture's cooperating worker, without signaling a
        // discovered/recycled grandchild PID or touching other reservations.
        let _ = fs::write(self.directory.path().join("stop-worker"), b"stop");
        let _ = self.child.kill();
        let _ = self.child.wait();
        let deadline = Instant::now() + WAIT;
        while self.worker_pid().and_then(identity).is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            self.worker_pid().and_then(identity).is_none() || std::thread::panicking(),
            "fixture worker cleanup not observed"
        );
    }
}

async fn coordinator(directory: &Path) {
    check_sharing_supported().expect("native production policy must support sharing");
    let port = fs::read_to_string(directory.join("port"))
        .unwrap()
        .parse()
        .unwrap();
    let share = Share::reserve(port)
        .await
        .expect("real native share admission");
    let mut command = tokio::process::Command::new("python3");
    command
        .args([
            "-c",
            "import os,pathlib,sys,time\np=pathlib.Path(sys.argv[1])\n(p/'worker-pid').write_text(str(os.getpid()))\nwhile not (p/'stop-worker').exists(): time.sleep(0.01)",
        ])
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _worker = share
        .spawn(&mut command, &Admission::default())
        .await
        .expect("native worker publication");
    fs::write(directory.join("published"), b"ready").unwrap();
    // The parent uses SIGKILL via Child::kill, not Drop/EOF/heartbeat, to end
    // this coordinator while the actual independently running worker remains.
    std::future::pending::<()>().await;
}

#[tokio::test]
async fn native_policy_retains_published_worker_after_coordinator_death() {
    if let Some(directory) = std::env::var_os(FIXTURE) {
        coordinator(&PathBuf::from(directory)).await;
        return;
    }
    if let Err(error) = check_sharing_supported() {
        assert_ne!(
            std::env::var("SPLITTY_TEST_REQUIRE_NATIVE_EXPOSURE").as_deref(),
            Ok("1"),
            "strict positive proof requires supported native Linux/macOS policy: {error}"
        );
        eprintln!("native reservation positive scenario UNRUN (not proof): {error}; set SPLITTY_TEST_REQUIRE_NATIVE_EXPOSURE=1 to require actual execution");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let selected = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = selected.local_addr().unwrap().port();
    drop(selected);
    fs::write(directory.path().join("port"), port.to_string()).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "native_policy_retains_published_worker_after_coordinator_death",
            "--nocapture",
        ])
        .env(FIXTURE, directory.path())
        // Application workspaces cannot split the OS-account authority.
        .env(
            "SPLITTY_CONFIG_DIR",
            directory.path().join("different-client-config"),
        )
        .env("HOME", directory.path().join("different-client-home"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut coordinator = Coordinator { child, directory };
    wait("native coordinator did not publish its worker", || {
        assert!(
            coordinator.child.try_wait().unwrap().is_none(),
            "coordinator exited before publication"
        );
        coordinator.directory.path().join("published").exists()
            && coordinator.worker_pid().is_some()
    });
    let worker =
        identity(coordinator.worker_pid().unwrap()).expect("actual published worker running");
    let addresses = [
        SocketAddr::from(([127, 0, 0, 1], port)),
        SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
    ];
    for address in addresses {
        assert_eq!(
            bind_gateway(address).await.err().unwrap().kind(),
            io::ErrorKind::AddrInUse
        );
    }
    coordinator.child.kill().unwrap();
    assert!(!coordinator.child.wait().unwrap().success());
    assert_eq!(
        identity(worker.pid).as_ref(),
        Some(&worker),
        "same actual worker survives coordinator SIGKILL"
    );
    for address in addresses {
        assert_eq!(
            bind_gateway(address).await.err().unwrap().kind(),
            io::ErrorKind::AddrInUse,
            "coordinator death must not release a surviving published generation"
        );
    }
    let alternate = bind_gateway(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    assert_ne!(alternate.local_addr().unwrap().port(), port);
    drop(alternate);
    fs::write(coordinator.directory.path().join("stop-worker"), b"stop").unwrap();
    wait("actual local worker did not exit", || {
        identity(worker.pid).is_none()
    });
    // On macOS, a transient orphan zombie may be reaped after our independent
    // observer sees SZOMB. Production authority must itself verify release.
    let deadline = Instant::now() + WAIT;
    for address in addresses {
        loop {
            match bind_gateway(address).await {
                Ok(replacement) => {
                    assert_eq!(replacement.local_addr().unwrap(), address);
                    assert_eq!(Share::reserve(port).await.err().unwrap().kind(), io::ErrorKind::AddrInUse,
                        "an admitted browser endpoint must exclude new shares in either address family");
                    drop(replacement);
                    break;
                }
                Err(error)
                    if error.kind() == io::ErrorKind::AddrInUse && Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!(
                    "native worker exit did not release its own endpoint reservation: {error}"
                ),
            }
        }
    }
    eprintln!("NATIVE_EXPOSURE_POSITIVE_EXERCISED: real native policy, worker {}, coordinator SIGKILL, IPv4/IPv6 exclusion and actual-exit reuse", worker.pid);
}
