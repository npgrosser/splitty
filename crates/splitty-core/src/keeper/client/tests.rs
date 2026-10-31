//! In-process: sessions of a backend whose terminals are keepers, the keepers
//! running on threads instead of processes, with real PTYs (skipped without
//! one).

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::{find, Keepers, Launch};
use crate::pty::manager::Manager;
use crate::pty::testing::{pty_available, sh_opts, wait_until};
use crate::pty::{Backend, SessionHandle};
use crate::shell_profiles::{state_from_settings, ShellSettings};

const TIMEOUT: Duration = Duration::from_secs(10);

/// Keepers on threads of this process.
struct ThreadLauncher;

impl Launch for ThreadLauncher {
    fn launch(&self, keeper_end: UnixStream) -> std::io::Result<()> {
        std::thread::Builder::new()
            .name("test-keeper".into())
            .spawn(move || {
                let _ = super::super::process::run(keeper_end, super::super::process::RETENTION);
            })?;
        Ok(())
    }
}

/// A short directory: socket paths are limited, and macOS temporary
/// directories are long already.
fn keepers_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("kc")
        .tempdir_in("/tmp")
        .expect("dir");
    let keepers = dir.path().join("keepers");
    (dir, keepers)
}

fn manager(keepers: &Path) -> Manager {
    Manager::with_terminals(
        Arc::new(std::sync::RwLock::new(state_from_settings(
            &ShellSettings::default(),
        ))),
        Arc::new(Keepers::new(
            keepers.to_path_buf(),
            Arc::new(ThreadLauncher),
        )),
    )
}

async fn program(manager: &Manager, id: &str, script: &str) -> Arc<dyn SessionHandle> {
    let mut opts = sh_opts(id, "");
    opts.dir = "/".into();
    manager
        .create_program(opts, vec!["/bin/sh".into(), "-c".into(), script.into()])
        .await
        .expect("session")
}

fn shows(session: &Arc<dyn SessionHandle>, text: &str) -> bool {
    String::from_utf8_lossy(&session.scrollback()).contains(text)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_session_round_trips_input_and_reports_its_exit() {
    if !pty_available() {
        return;
    }
    let (_dir, keepers) = keepers_dir();
    let manager = manager(&keepers);
    let session = program(
        &manager,
        "kept-1",
        "printf ready; read x; printf \"got $x\"; exit 7",
    )
    .await;
    assert!(session.shell_pid().is_some());
    assert_eq!(
        super::super::dir::sockets(&keepers).len(),
        1,
        "a keeper serves the session"
    );

    assert!(wait_until(TIMEOUT, || shows(&session, "ready")).await);
    session.write(b"hi\n").await.expect("input");
    assert!(wait_until(TIMEOUT, || shows(&session, "got hi")).await);
    assert!(wait_until(TIMEOUT, || *session.done().borrow()).await);
    assert_eq!(
        session.process_result().and_then(|result| result.exit_code),
        Some(7)
    );
    assert!(
        wait_until(TIMEOUT, || super::super::dir::sockets(&keepers).is_empty()).await,
        "the acknowledged keeper leaves"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signals_close_and_a_managed_stop_behave_as_in_process() {
    if !pty_available() {
        return;
    }
    let (_dir, keepers) = keepers_dir();
    let manager = manager(&keepers);

    let killed = program(&manager, "kept-kill", "kill -9 $$").await;
    assert!(wait_until(TIMEOUT, || *killed.done().borrow()).await);
    let result = killed.process_result().expect("result");
    assert_eq!(result.exit_code, None, "never a fabricated code");
    assert_eq!(result.signal.as_deref(), Some("SIGKILL"));

    // An ordinary session hangs up on close.
    let ordinary = crate::pty::session::Session::start(
        sh_opts("kept-close", "printf ready; sleep 30"),
        None,
        &Keepers::new(keepers.clone(), Arc::new(ThreadLauncher)),
    )
    .expect("ordinary");
    assert!(
        wait_until(TIMEOUT, || String::from_utf8_lossy(&ordinary.scrollback())
            .contains("ready"))
        .await
    );
    ordinary.close().expect("close");
    assert!(wait_until(TIMEOUT, || *ordinary.done().borrow()).await);

    // A managed program that ignores SIGTERM is stopped with escalation.
    let stubborn = program(
        &manager,
        "kept-stop",
        "trap '' TERM; printf ready; sleep 30",
    )
    .await;
    assert!(wait_until(TIMEOUT, || shows(&stubborn, "ready")).await);
    let result = stubborn.stop().await.expect("stop");
    assert_eq!(result.signal.as_deref(), Some("SIGKILL"), "{result:?}");
    assert!(shows(&stubborn, "ready"), "output survives the stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_program_that_cannot_start_is_an_error_and_an_unusable_directory_falls_back() {
    if !pty_available() {
        return;
    }
    let (_dir, keepers) = keepers_dir();
    let manager = manager(&keepers);
    let mut opts = sh_opts("kept-missing", "");
    opts.dir = "/".into();
    let missing = manager
        .create_program(opts, vec!["/nonexistent/program".into()])
        .await;
    assert!(missing.is_err(), "the program's own error, not a fallback");

    // "Keeper cannot start": the session runs in this process instead.
    let fallback = Manager::with_terminals(
        Arc::new(std::sync::RwLock::new(state_from_settings(
            &ShellSettings::default(),
        ))),
        Arc::new(Keepers::new(
            PathBuf::from("/mnt/unusable/keepers"),
            Arc::new(ThreadLauncher),
        )),
    );
    let session = program(&fallback, "kept-fallback", "printf local; read x").await;
    assert!(wait_until(TIMEOUT, || shows(&session, "local")).await);
    session.close().expect("close");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn released_keepers_are_adopted_with_their_output_and_process() {
    if !pty_available() {
        return;
    }
    let (_dir, keepers) = keepers_dir();
    let first = manager(&keepers);
    let session = program(
        &first,
        "kept-adopt",
        "printf before; read x; printf \"after $x\"; exit 4",
    )
    .await;
    assert!(wait_until(TIMEOUT, || shows(&session, "before")).await);
    let pid = session.shell_pid();
    first.release_all();
    drop(session);

    let found = find(&keepers);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].hello.session_id, "kept-adopt");
    assert!(!found[0].hello.controlled, "released");

    let second = manager(&keepers);
    let found = found.into_iter().next().expect("keeper");
    let adopted = second
        .adopt("kept-adopt", found.hello.managed, |feed| {
            found
                .adopt(feed)
                .map(|terminal| Box::new(terminal) as Box<dyn crate::pty::terminal::Terminal>)
                .map_err(|e| crate::pty::PtyError::Other(e.to_string()))
        })
        .expect("adopted");
    assert_eq!(adopted.shell_pid(), pid, "the same process");
    assert!(
        wait_until(TIMEOUT, || shows(&adopted, "before")).await,
        "the replay"
    );
    assert!(second.get("kept-adopt").is_some());

    adopted.write(b"again\n").await.expect("input");
    assert!(wait_until(TIMEOUT, || shows(&adopted, "after again")).await);
    assert!(wait_until(TIMEOUT, || *adopted.done().borrow()).await);
    assert_eq!(adopted.process_result().and_then(|r| r.exit_code), Some(4));
}
