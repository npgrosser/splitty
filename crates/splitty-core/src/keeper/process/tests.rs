//! In-process: the keeper runs on threads over a socket pair, with real PTYs
//! (skipped without one), and these tests speak its protocol directly.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::super::wire::{read_frame, write_frame, Exit, Frame, Hello, SpawnSpec};
use crate::pty::testing::pty_available;

const TIMEOUT: Duration = Duration::from_secs(10);

/// A keeper running a `/bin/sh -c` script, and its first controller.
struct Running {
    controller: UnixStream,
    socket: PathBuf,
    hello: Hello,
    finished: mpsc::Receiver<std::io::Result<()>>,
    _dir: tempfile::TempDir,
}

fn spawn_spec(
    script: &str,
    managed: bool,
    buffer_size: usize,
    socket: &std::path::Path,
) -> SpawnSpec {
    SpawnSpec {
        session_id: "keeper-test".into(),
        pane_id: Some("pane-1".into()),
        screen_id: None,
        argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        env: vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("TERM".into(), "xterm-256color".into()),
        ],
        cwd: Some("/".into()),
        cols: 80,
        rows: 24,
        managed,
        buffer_size,
        socket: socket.to_string_lossy().into_owned(),
    }
}

fn start(script: &str, managed: bool, buffer_size: usize, retention: Duration) -> Option<Running> {
    if !pty_available() {
        return None;
    }
    // Short: socket paths are limited, and temporary directories on macOS
    // are long already.
    let dir = tempfile::Builder::new()
        .prefix("k")
        .tempdir_in("/tmp")
        .expect("dir");
    let socket = dir.path().join("keepers").join("t.sock");
    let (mut controller, keeper) = UnixStream::pair().expect("pair");
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(super::run(keeper, retention));
    });
    write_frame(
        &mut controller,
        &Frame::Spawn(Box::new(spawn_spec(script, managed, buffer_size, &socket))),
    )
    .expect("spec");
    controller.set_read_timeout(Some(TIMEOUT)).expect("timeout");
    let Some(Frame::Hello(hello)) = read_frame(&mut controller).expect("hello") else {
        panic!("the keeper answers with hello");
    };
    Some(Running {
        controller,
        socket,
        hello: *hello,
        finished,
        _dir: dir,
    })
}

fn next(stream: &mut UnixStream) -> Frame {
    read_frame(stream)
        .expect("a frame")
        .expect("the connection stays open")
}

/// Frames until `stop` returns true for one, with the output they carried.
fn until(stream: &mut UnixStream, mut stop: impl FnMut(&Frame) -> bool) -> (Vec<Frame>, Vec<u8>) {
    let mut frames = Vec::new();
    let mut output = Vec::new();
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        let frame = next(stream);
        if let Frame::Output { data, .. } = &frame {
            output.extend_from_slice(data);
        }
        let done = stop(&frame);
        frames.push(frame);
        if done {
            return (frames, output);
        }
    }
    panic!("gave up waiting; got {frames:?}");
}

fn shows(output: &[u8], text: &str) -> bool {
    String::from_utf8_lossy(output).contains(text)
}

fn claim(socket: &std::path::Path, takeover: bool, from: Option<u64>) -> (UnixStream, Hello) {
    let mut stream = super::super::dir::connect(socket).expect("connect");
    stream.set_read_timeout(Some(TIMEOUT)).expect("timeout");
    let Frame::Hello(hello) = next(&mut stream) else {
        panic!("hello first");
    };
    write_frame(&mut stream, &Frame::Claim { takeover, from }).expect("claim");
    (stream, *hello)
}

fn finished(running: &Running) -> bool {
    running.finished.recv_timeout(TIMEOUT).is_ok()
}

#[test]
fn input_is_answered_and_an_exit_waits_for_nothing_but_its_acknowledgement() {
    let Some(mut running) = start(
        "printf ready; read x; printf \"got $x\"; exit 3",
        false,
        1 << 20,
        super::RETENTION,
    ) else {
        return;
    };
    assert!(running.hello.pid.is_some_and(|pid| pid > 0));
    assert_eq!(running.hello.program, "sh");
    assert_eq!(running.hello.pane_id.as_deref(), Some("pane-1"));
    let mut expected = 0u64;
    let (_, output) = until(&mut running.controller, |frame| {
        if let Frame::Output { offset, data } = frame {
            assert_eq!(*offset, expected, "contiguous offsets");
            expected += u64::try_from(data.len()).expect("len");
        }
        matches!(frame, Frame::Output { data, .. } if shows(data, "ready"))
    });
    assert!(shows(&output, "ready"));

    write_frame(
        &mut running.controller,
        &Frame::Input {
            seq: 7,
            data: b"hi\n".to_vec(),
        },
    )
    .expect("input");
    let (frames, output) = until(&mut running.controller, |frame| {
        matches!(frame, Frame::OutputEnded)
    });
    assert!(frames.contains(&Frame::InputResult {
        seq: 7,
        error: None
    }));
    assert!(
        shows(&output, "got hi"),
        "{:?}",
        String::from_utf8_lossy(&output)
    );
    let exited = frames.iter().find_map(|frame| match frame {
        Frame::Exited(exit) => Some(exit.clone()),
        _ => None,
    });
    let exited = exited.or_else(|| match next(&mut running.controller) {
        Frame::Exited(exit) => Some(exit),
        _ => None,
    });
    assert_eq!(
        exited,
        Some(Exit {
            exit_code: Some(3),
            signal: None,
            error: None
        })
    );
    assert!(
        running
            .finished
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "the keeper waits for the acknowledgement"
    );
    write_frame(&mut running.controller, &Frame::ExitAck).expect("ack");
    assert!(finished(&running));
    assert!(!running.socket.exists(), "the socket is removed");
}

#[test]
fn a_managed_write_to_a_full_terminal_fails_promptly() {
    // Raw mode: a canonical terminal discards what does not fit instead.
    let Some(mut running) = start(
        "stty raw -echo; printf READY; sleep 30",
        true,
        1 << 20,
        super::RETENTION,
    ) else {
        return;
    };
    until(
        &mut running.controller,
        |frame| matches!(frame, Frame::Output { data, .. } if shows(data, "READY")),
    );
    let started = Instant::now();
    let mut refused = None;
    for seq in 0..64u32 {
        write_frame(
            &mut running.controller,
            &Frame::Input {
                seq,
                data: vec![b'x'; 16384],
            },
        )
        .expect("input");
        let (frames, _) = until(&mut running.controller, |frame| {
            matches!(frame, Frame::InputResult { .. })
        });
        if let Some(Frame::InputResult {
            error: Some(error), ..
        }) = frames.last()
        {
            refused = Some(error.clone());
            break;
        }
    }
    assert_eq!(
        refused.as_deref(),
        Some("the program does not read its input")
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );

    // `close` stops a managed program and ends the keeper without waiting.
    write_frame(&mut running.controller, &Frame::Close).expect("close");
    assert!(finished(&running));
}

#[test]
fn a_second_claim_is_refused_and_a_takeover_moves_the_keeper() {
    let Some(mut running) = start("printf ready; cat", false, 1 << 20, super::RETENTION) else {
        return;
    };
    until(
        &mut running.controller,
        |frame| matches!(frame, Frame::Output { data, .. } if shows(data, "ready")),
    );

    let (mut refused, hello) = claim(&running.socket, false, None);
    assert!(hello.controlled);
    assert_eq!(next(&mut refused), Frame::Busy);

    let (mut taker, _) = claim(&running.socket, true, None);
    let Frame::Claimed { from, lost } = next(&mut taker) else {
        panic!("claimed");
    };
    assert_eq!((from, lost), (0, false));
    let (_, replay) = until(
        &mut taker,
        |frame| matches!(frame, Frame::Output { data, .. } if shows(data, "ready")),
    );
    assert!(shows(&replay, "ready"));
    let (frames, _) = until(&mut running.controller, |frame| {
        matches!(frame, Frame::TakenOver)
    });
    assert_eq!(frames.last(), Some(&Frame::TakenOver));
    assert_eq!(
        read_frame(&mut running.controller).expect("end"),
        None,
        "disconnected"
    );

    write_frame(
        &mut taker,
        &Frame::Input {
            seq: 2,
            data: b"echoed\n".to_vec(),
        },
    )
    .expect("input");
    let (frames, output) = until(
        &mut taker,
        |frame| matches!(frame, Frame::Output { data, .. } if shows(data, "echoed")),
    );
    assert!(
        frames.contains(&Frame::InputResult {
            seq: 2,
            error: None
        }) || shows(&output, "echoed")
    );

    write_frame(&mut taker, &Frame::Close).expect("close");
    assert!(finished(&running));
}

#[test]
fn a_resumed_controller_gets_every_byte_once_and_hears_of_loss() {
    let Some(mut running) = start(
        "printf aaaa; sleep 0.4; printf bbbb; read x",
        false,
        1 << 20,
        super::RETENTION,
    ) else {
        return;
    };
    let (_, output) = until(
        &mut running.controller,
        |frame| matches!(frame, Frame::Output { data, .. } if shows(data, "aaaa")),
    );
    let seen = u64::try_from(output.len()).expect("len");
    write_frame(&mut running.controller, &Frame::Release).expect("release");
    std::thread::sleep(Duration::from_millis(800));

    let (mut resumed, hello) = claim(&running.socket, false, Some(seen));
    assert!(!hello.controlled, "released");
    assert_eq!(
        next(&mut resumed),
        Frame::Claimed {
            from: seen,
            lost: false
        }
    );
    let (frames, output) = until(
        &mut resumed,
        |frame| matches!(frame, Frame::Output { data, .. } if shows(data, "bbbb")),
    );
    assert_eq!(String::from_utf8_lossy(&output), "bbbb", "{frames:?}");
    write_frame(&mut resumed, &Frame::Close).expect("close");
    assert!(finished(&running));

    // A buffer of four bytes cannot hold what came before.
    let Some(mut small) = start(
        "printf aaaa; printf bbbbbbbb; read x",
        false,
        4,
        super::RETENTION,
    ) else {
        return;
    };
    until(
        &mut small.controller,
        |frame| matches!(frame, Frame::Output { data, .. } if shows(data, "bbbb")),
    );
    std::thread::sleep(Duration::from_millis(200));
    let (mut late, hello) = claim(&small.socket, true, Some(0));
    assert_eq!(hello.buffer_end - hello.buffer_start, 4);
    assert_eq!(
        next(&mut late),
        Frame::Claimed {
            from: hello.buffer_start,
            lost: true
        }
    );
    write_frame(&mut late, &Frame::Close).expect("close");
    assert!(finished(&small));
}

#[test]
fn a_busy_program_without_a_controller_keeps_running() {
    let Some(mut running) = start(
        "head -c 2000000 /dev/zero | tr '\\0' x; printf END; read x",
        false,
        1 << 16,
        super::RETENTION,
    ) else {
        return;
    };
    write_frame(&mut running.controller, &Frame::Release).expect("release");
    let deadline = Instant::now() + TIMEOUT;
    let hello = loop {
        let mut stream = super::super::dir::connect(&running.socket).expect("connect");
        stream.set_read_timeout(Some(TIMEOUT)).expect("timeout");
        let Frame::Hello(hello) = next(&mut stream) else {
            panic!("hello");
        };
        if hello.buffer_end >= 2_000_003 || Instant::now() > deadline {
            break hello;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(
        hello.buffer_end >= 2_000_003,
        "all output read: {}",
        hello.buffer_end
    );
    let (mut late, _) = claim(&running.socket, false, None);
    let (_, output) = until(
        &mut late,
        |frame| matches!(frame, Frame::Output { data, .. } if shows(data, "END")),
    );
    assert!(output.len() <= 1 << 16, "only the buffer is replayed");
    write_frame(&mut late, &Frame::Close).expect("close");
    assert!(finished(&running));
}

#[test]
fn an_exit_without_a_controller_waits_for_one_then_for_the_retention() {
    let Some(mut running) = start(
        "sleep 0.3; printf done; exit 3",
        false,
        1 << 20,
        super::RETENTION,
    ) else {
        return;
    };
    write_frame(&mut running.controller, &Frame::Release).expect("release");
    drop(running.controller.try_clone());
    std::thread::sleep(Duration::from_millis(800));

    let (mut late, hello) = claim(&running.socket, false, None);
    assert_eq!(
        hello.exited.as_ref().and_then(|exit| exit.exit_code),
        Some(3)
    );
    let (frames, output) = until(&mut late, |frame| matches!(frame, Frame::OutputEnded));
    assert!(shows(&output, "done"));
    assert!(frames.iter().any(|frame| matches!(
        frame,
        Frame::Exited(Exit {
            exit_code: Some(3),
            ..
        })
    )));
    write_frame(&mut late, &Frame::ExitAck).expect("ack");
    assert!(finished(&running));

    // Nobody comes: the keeper ends after its retention.
    let Some(alone) = start("exit 0", false, 1 << 20, Duration::from_millis(300)) else {
        return;
    };
    let Running {
        controller,
        finished: done,
        ..
    } = alone;
    drop(controller);
    assert!(done.recv_timeout(TIMEOUT).is_ok());
}

#[test]
fn a_program_that_cannot_start_is_reported() {
    if !pty_available() {
        return;
    }
    let dir = tempfile::Builder::new()
        .prefix("k")
        .tempdir_in("/tmp")
        .expect("dir");
    let socket = dir.path().join("keepers").join("t.sock");
    let (mut controller, keeper) = UnixStream::pair().expect("pair");
    let worker = std::thread::spawn(move || super::run(keeper, super::RETENTION));
    let mut spec = spawn_spec("", false, 1024, &socket);
    spec.argv = vec!["/nonexistent/program".into()];
    write_frame(&mut controller, &Frame::Spawn(Box::new(spec))).expect("spec");
    controller.set_read_timeout(Some(TIMEOUT)).expect("timeout");
    assert!(matches!(
        next(&mut controller),
        Frame::SpawnFailed { program: true, .. }
    ));
    assert!(worker.join().expect("keeper thread").is_err());
    assert!(!socket.exists());
}

#[test]
fn the_foreground_process_group_is_reported() {
    let Some(mut running) = start("sleep 3", false, 1 << 20, super::RETENTION) else {
        return;
    };
    let pid = running.hello.pid.expect("pid");
    let (frames, _) = until(&mut running.controller, |frame| {
        matches!(frame, Frame::Foreground { .. })
    });
    assert_eq!(frames.last(), Some(&Frame::Foreground { pid: Some(pid) }));
    write_frame(&mut running.controller, &Frame::Close).expect("close");
    assert!(finished(&running));
}
