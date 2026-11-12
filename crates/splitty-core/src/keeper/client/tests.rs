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

/// Also program-status, Output Read Again After Adoption (Restart after an
/// answered probe, Record rebuilt after a restart, the session half): the
/// program probes, reads the answer and reports `blocked` before the release;
/// after the adoption it reads no second answer, and the adopted session holds
/// the `blocked` record, marked as read again.
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
        "stty raw -echo; printf '\\033]7501;?\\007'; \
         a=$(dd bs=9 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); printf 'first:%s;' \"$a\"; \
         printf '\\033]7501;state=blocked:kind=permission\\007'; printf before; \
         read x; printf 'after:'; printf '%s' \"$x\" | od -An -tx1 | tr -d ' \\n'; printf ':end'; \
         exit 4",
    )
    .await;
    assert!(wait_until(TIMEOUT, || shows(&session, "before")).await);
    assert!(
        shows(&session, "first:1b5d373530313b3f07;"),
        "answered once: {}",
        String::from_utf8_lossy(&session.scrollback())
    );
    let pid = session.shell_pid();
    first.release_all();
    drop(session);

    // Release closes the client side synchronously; the keeper processes the
    // frame/EOF on its controller thread. Discovery can race that detach.
    let mut found = Vec::new();
    assert!(
        wait_until(TIMEOUT, || {
            found = find(&keepers);
            found.len() == 1 && !found[0].hello.controlled
        })
        .await,
        "keeper observes the released controller before adoption"
    );
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
    let status = adopted.program_status();
    let record = status.record.expect("the record rebuilt from the replay");
    assert_eq!(record.state, crate::pty::status::StatusState::Blocked);
    assert_eq!(record.kind, Some(crate::pty::status::BlockKind::Permission));
    assert!(status.read_again, "marked as read again");
    assert!(!shows(&adopted, "7501;?"), "the probe is not in the replay");

    // A second answer would reach the program ahead of this input.
    tokio::time::sleep(Duration::from_secs(1)).await;
    adopted.write(b"again\n").await.expect("input");
    assert!(
        wait_until(TIMEOUT, || shows(&adopted, ":end")).await,
        "{}",
        String::from_utf8_lossy(&adopted.scrollback())
    );
    assert!(
        shows(&adopted, "after:616761696e:end"),
        "no second answer: {}",
        String::from_utf8_lossy(&adopted.scrollback())
    );
    assert!(wait_until(TIMEOUT, || *adopted.done().borrow()).await);
    assert_eq!(adopted.process_result().and_then(|r| r.exit_code), Some(4));
    assert_eq!(
        adopted.program_status().record,
        None,
        "blocked ends with the output"
    );
}

/// What the kept terminal handed its feed: each chunk, and whether it was
/// read again.
#[derive(Default)]
struct Chunks(std::sync::Mutex<Vec<(bool, Vec<u8>)>>);

impl crate::pty::terminal::Feed for Chunks {
    fn record(&self, data: &[u8]) {
        self.0.lock().unwrap().push((false, data.to_vec()));
    }
    fn record_again(&self, data: &[u8]) {
        self.0.lock().unwrap().push((true, data.to_vec()));
    }
    fn end(&self) {}
}

/// program-status, Output Read Again After Adoption: a frame that straddles
/// the keeper's `buffer_end` is split there; the bytes below it are read
/// again, a probe in the live part is live output. Frames resent below the
/// offset already recorded are skipped as before.
#[test]
fn a_frame_straddling_the_buffer_end_is_split_there() {
    let chunks = Arc::new(Chunks::default());
    let shared = super::Shared {
        session_id: "straddle".into(),
        socket: PathBuf::new(),
        pid: None,
        stream: std::sync::Mutex::new(None),
        next_seq: std::sync::atomic::AtomicU32::new(1),
        pending: std::sync::Mutex::new(Default::default()),
        foreground: std::sync::atomic::AtomicU32::new(0),
        done_tx: tokio::sync::watch::channel(false).0,
        result: std::sync::Mutex::new(None),
        state: std::sync::Mutex::new(super::ClientState {
            offset: 100,
            again_end: 110,
            ..Default::default()
        }),
        feed: chunks.clone(),
    };
    let frame = |offset: u64, data: &[u8]| super::Frame::Output {
        offset,
        data: data.to_vec(),
    };
    // 95..100 was recorded already; 100..110 is buffered; the probe is live.
    shared.handle(frame(95, b"old!!buffered\x1b]7501;?\x07"));
    shared.handle(frame(105, b"resent"));
    shared.handle(frame(120, b"live"));
    assert_eq!(
        *chunks.0.lock().unwrap(),
        vec![
            (true, b"buffered\x1b]".to_vec()),
            (false, b"7501;?\x07".to_vec()),
            (false, b"live".to_vec()),
        ]
    );
}

/// program-status, Probe Answer Order (Large input) through a kept terminal:
/// input larger than a frame goes to the keeper in several frames, all within
/// one hold of the session's input gate, so a probe answer queued meanwhile
/// is written after them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_lands_around_large_input_in_several_frames() {
    if !pty_available() {
        return;
    }
    let (dir, keepers) = keepers_dir();
    let stored = dir.path().join("stored");
    let size = 2 * super::MAX_INPUT_FRAME + 4096;
    let answer = b"\x1b]7501;?\x07";
    let session = crate::pty::session::Session::start(
        sh_opts(
            "kept-large-input",
            &format!(
                "stty raw -echo; printf ready; sleep 1; printf '\\033]7501;?\\007'; \
                 head -c {} > '{}'; printf stored; sleep 5",
                size + answer.len(),
                stored.display()
            ),
        ),
        None,
        &Keepers::new(keepers.clone(), Arc::new(ThreadLauncher)),
    )
    .expect("ordinary session");
    assert!(
        wait_until(TIMEOUT, || String::from_utf8_lossy(&session.scrollback())
            .contains("ready"))
        .await
    );
    let input = vec![b'x'; size];
    session.write(&input).await.expect("large input");
    assert!(
        wait_until(Duration::from_secs(20), || String::from_utf8_lossy(
            &session.scrollback()
        )
        .contains("stored"))
        .await
    );
    let read = std::fs::read(&stored).expect("what the program read");
    assert_eq!(read.len(), size + answer.len());
    assert!(
        (read.starts_with(answer) && read[answer.len()..] == input[..])
            || (read.ends_with(answer) && read[..size] == input[..]),
        "the answer inside the input"
    );
    assert!(read.ends_with(answer), "queued while the input was written");
    session.close().expect("close");
}
