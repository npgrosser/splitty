//! The last released `splitty` against the current build, at the boundaries
//! where the two meet as different versions:
//!
//! - a client and a daemon: the `splitty` command's own clients (`attach` on
//!   the native endpoint, the CLI commands on the control socket) of one build
//!   against the daemon of the other, and the current build's native client,
//!   the desktop's, through the `remote-bridge` a host runs;
//! - a daemon and the keepers the other build started: stop one build's
//!   daemon, attach its keeper directly with the other build, start the other
//!   build's daemon, which adopts the keeper;
//! - the `splitty` command in a pane and the daemon behind its `SPLITTY_SOCK`,
//!   while one daemon runs and after the other build's daemon took over;
//! - the files one build writes and the other reads: `state.json` with ended
//!   sessions, `config.toml` and `daemon.json`.
//!
//! Each runs in both directions and creates, attaches, resizes and re-attaches
//! a session across the boundary.
//!
//! Smoke: an in-process test links one version of every type, so it holds the
//! newer side's idea of the older one; only two real binaries show a break
//! between them.
//!
//! `scripts/testing/fetch-last-release.sh` fetches the released build and
//! checks it against the release's `SHA256SUMS`; `SPLITTY_TEST_RELEASED_BINARY`
//! names it. Without it the tests are skipped at runtime and say so, as PTY
//! tests are without a PTY; `SPLITTY_TEST_REQUIRE_RELEASED_BINARY=1` (CI)
//! makes that a failure. Linux only: the release's static server is built for
//! Linux x86_64.
//!
//! Every process of both builds runs with a cleared environment, a throwaway
//! `HOME`, `XDG_*` and `TMPDIR`, and `/bin/sh` as its shell, so neither reaches
//! the invoking user's daemon, also when the tests run inside a Splitty pane.

#![cfg(target_os = "linux")]

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use serde_json::{json, Value};
use splitty_server::service::catalog::Operation;
use tempfile::TempDir;

/// How long one step may take.
const TIMEOUT: Duration = Duration::from_secs(15);
/// How long one command may run.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

const RELEASED: &str = "SPLITTY_TEST_RELEASED_BINARY";
const REQUIRE_RELEASED: &str = "SPLITTY_TEST_REQUIRE_RELEASED_BINARY";

/// A line that makes the shell print its terminal size, and what it prints
/// at `cols`x`rows`. Each call has a marker of its own, so neither the echo
/// of the typed line nor earlier output that a replay repeats can match.
fn size_probe(cols: u16, rows: u16) -> (String, String) {
    static PROBES: AtomicUsize = AtomicUsize::new(0);
    let probe = PROBES.fetch_add(1, Ordering::Relaxed);
    (
        format!("echo SIZE{probe}-$(stty size | tr ' ' x)"),
        format!("SIZE{probe}-{rows}x{cols}"),
    )
}

/// One of the two builds.
struct Build {
    /// `released` or `current`, for messages.
    name: &'static str,
    /// The canonical path of its `splitty`.
    path: PathBuf,
    version: String,
}

impl Build {
    fn current() -> Self {
        Self {
            name: "current",
            path: Path::new(env!("CARGO_BIN_EXE_splitty"))
                .canonicalize()
                .expect("current build"),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    fn released() -> Option<Self> {
        let Some(path) = std::env::var_os(RELEASED).filter(|path| !path.is_empty()) else {
            assert_ne!(
                std::env::var(REQUIRE_RELEASED).as_deref(),
                Ok("1"),
                "{REQUIRE_RELEASED}=1 needs {RELEASED}; scripts/testing/fetch-last-release.sh provides it"
            );
            eprintln!(
                "SKIPPED version skew: {RELEASED} names no released splitty; \
                 scripts/testing/fetch-last-release.sh fetches and verifies one"
            );
            return None;
        };
        let path = PathBuf::from(path)
            .canonicalize()
            .unwrap_or_else(|error| panic!("{RELEASED}: {error}"));
        let output = Command::new(&path)
            .arg("--version")
            .env_clear()
            .stdin(Stdio::null())
            .output()
            .expect("released splitty --version");
        assert!(output.status.success(), "released splitty --version");
        let version = String::from_utf8_lossy(&output.stdout)
            .trim()
            .strip_prefix("splitty ")
            .expect("`splitty <version>`")
            .to_owned();
        Some(Self {
            name: "released",
            path,
            version,
        })
    }

    /// The path as a word of a shell line.
    fn word(&self) -> String {
        let path = self.path.to_str().expect("UTF-8 path");
        assert!(!path.contains('\''), "a path a shell line can quote");
        format!("'{path}'")
    }
}

/// Both builds, `(released, current)`, or `None` when the test is skipped.
fn builds() -> Option<(Build, Build)> {
    let released = Build::released()?;
    if !splitty_core::pty::testing::pty_available() {
        eprintln!("SKIPPED version skew: no PTY");
        return None;
    }
    let current = Build::current();
    eprintln!(
        "version skew: released {} ({}) against current {}",
        released.version,
        released.path.display(),
        current.version
    );
    Some((released, current))
}

/// A screen's first pane and its session.
struct Pane {
    screen: String,
    pane: String,
    session: String,
}

/// A throwaway user for both builds: the same `HOME`, `XDG_*` and runtime
/// directory for every command, nothing of the invoking user's.
struct Sandbox {
    _dir: TempDir,
    /// The canonical `HOME`.
    home: PathBuf,
    /// Both builds' `splitty`, for the cleanup.
    builds: [PathBuf; 2],
    commands: AtomicUsize,
}

impl Sandbox {
    fn new((released, current): &(Build, Build)) -> Self {
        // Below /tmp, short: a keeper's socket path must stay within the
        // 108 bytes a Unix socket allows.
        let dir = tempfile::Builder::new()
            .prefix("vs")
            .tempdir_in("/tmp")
            .expect("tempdir");
        let home = dir.path().canonicalize().expect("canonical home");
        for sub in ["tmp", "logs"] {
            fs::create_dir(home.join(sub)).expect("sandbox directory");
        }
        Self {
            _dir: dir,
            home,
            builds: [released.path.clone(), current.path.clone()],
            commands: AtomicUsize::new(0),
        }
    }

    /// A cleared environment with an explicit allowlist: no `SPLITTY_*` of
    /// the invoking shell, no `HOME` or `XDG_*` of the invoking user.
    fn isolate<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        let home = &self.home;
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home)
            .env("SHELL", "/bin/sh")
            .env("TMPDIR", home.join("tmp"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_RUNTIME_DIR", home.join("run"))
            .current_dir(home)
            .stdin(Stdio::null())
    }

    /// One command of `build` to its end. Its output goes to files: a daemon
    /// it starts may hold on to inherited pipes.
    fn run(&self, build: &Build, args: &[&str]) -> Output {
        let number = self.commands.fetch_add(1, Ordering::Relaxed);
        let out = self.home.join(format!("logs/{number}.out"));
        let err = self.home.join(format!("logs/{number}.err"));
        let mut command = Command::new(&build.path);
        command.args(args);
        self.isolate(&mut command)
            .stdout(File::create(&out).expect("stdout file"))
            .stderr(File::create(&err).expect("stderr file"));
        let mut child = command.spawn().expect("splitty");
        let Some(status) = wait_within(&mut child, COMMAND_TIMEOUT) else {
            panic!(
                "`splitty {}` of the {} build did not exit\n{}",
                args.join(" "),
                build.name,
                self.daemon_log()
            );
        };
        Output {
            status,
            stdout: fs::read(out).expect("stdout"),
            stderr: fs::read(err).expect("stderr"),
        }
    }

    /// Its standard output, after asserting that it succeeded.
    fn ok(&self, build: &Build, args: &[&str]) -> String {
        let output = self.run(build, args);
        assert!(
            output.status.success(),
            "`splitty {}` of the {} build\n{}",
            args.join(" "),
            build.name,
            self.describe(&output)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn json(&self, build: &Build, args: &[&str]) -> Value {
        let text = self.ok(build, args);
        serde_json::from_str(&text).unwrap_or_else(|error| {
            panic!(
                "`splitty {}` of the {} build is not JSON ({error}): {text}",
                args.join(" "),
                build.name
            )
        })
    }

    fn describe(&self, output: &Output) -> String {
        format!(
            "status: {}\nstdout:\n{}\nstderr:\n{}\ndaemon.log:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            self.daemon_log()
        )
    }

    fn config_dir(&self) -> PathBuf {
        self.home.join("config/splitty")
    }

    fn keeper_dir(&self) -> PathBuf {
        self.home.join("data/splitty/run/keepers")
    }

    fn info(&self) -> Option<Value> {
        let text = fs::read_to_string(self.config_dir().join("daemon.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn daemon_pid(&self) -> Option<u32> {
        self.info()?["pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
    }

    /// The last 8 KiB of the daemon's log.
    fn daemon_log(&self) -> String {
        match fs::read(self.config_dir().join("daemon.log")) {
            Ok(bytes) => {
                String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(8192)..]).into_owned()
            }
            Err(error) => format!("<no daemon.log: {error}>"),
        }
    }

    fn start_daemon(&self, build: &Build) {
        self.ok(build, &["daemon", "start"]);
        let pid = self.daemon_pid().expect("daemon.json after `daemon start`");
        assert_eq!(
            executable(pid),
            build.path,
            "the {} build runs the daemon",
            build.name
        );
    }

    /// Stops the running daemon with `build`'s CLI, which may be the other
    /// build's.
    fn stop_daemon(&self, build: &Build, end_sessions: bool) {
        let pid = self.daemon_pid().expect("a running daemon");
        let mut args = vec!["daemon", "stop"];
        if end_sessions {
            args.push("--end-sessions");
        }
        let stopped = self.ok(build, &args);
        assert!(stopped.contains("Daemon stopped"), "{stopped}");
        self.wait_until(&format!("daemon {pid} exits"), || !alive(pid));
    }

    /// A new screen through `build`'s CLI.
    fn new_screen(&self, build: &Build, name: &str) -> Pane {
        self.ok(build, &["new-screen", "--name", name]);
        let screens = self.json(build, &["list-screens", "--json"]);
        let screen = screens
            .as_array()
            .and_then(|screens| screens.iter().find(|screen| screen["name"] == name))
            .and_then(|screen| screen["id"].as_str())
            .unwrap_or_else(|| panic!("screen {name} in {screens}"))
            .to_owned();
        let panes = self.panes(build, &screen);
        let (pane, record) = panes
            .as_object()
            .and_then(|panes| panes.iter().next())
            .expect("a pane");
        Pane {
            pane: pane.clone(),
            session: record["sessionId"].as_str().expect("session").to_owned(),
            screen,
        }
    }

    fn panes(&self, build: &Build, screen: &str) -> Value {
        self.json(build, &["list-panes", "--screen", screen, "--json"])
    }

    fn screen_name(&self, build: &Build, screen: &str) -> String {
        let screens = self.json(build, &["list-screens", "--json"]);
        screens
            .as_array()
            .and_then(|screens| screens.iter().find(|s| s["id"] == screen))
            .and_then(|screen| screen["name"].as_str())
            .unwrap_or_else(|| panic!("screen {screen} in {screens}"))
            .to_owned()
    }

    /// Types a line into a pane's shell with `build`'s `send-keys`.
    fn type_line(&self, build: &Build, pane: &Pane, line: &str) {
        let keys = format!("{line}\\n");
        self.ok(
            build,
            &[
                "send-keys",
                "--screen",
                &pane.screen,
                "--pane",
                &pane.pane,
                &keys,
            ],
        );
    }

    /// The trimmed contents of a file a shell writes below `HOME`, once it
    /// has written a whole line.
    fn wait_for_file(&self, name: &str) -> String {
        let path = self.home.join(name);
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Ok(text) = fs::read_to_string(&path) {
                if text.ends_with('\n') {
                    return text.trim().to_owned();
                }
            }
            assert!(
                Instant::now() < deadline,
                "{name} never appeared\n{}",
                self.daemon_log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn keeper_sockets(&self) -> usize {
        fs::read_dir(self.keeper_dir())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "sock"))
                    .count()
            })
            .unwrap_or(0)
    }

    fn wait_until(&self, what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "gave up waiting: {what}\n{}",
                self.daemon_log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// One request of the current build's native client, the desktop's, to
    /// the running daemon, found through the `daemon.json` it wrote.
    fn native_request(&self, operation: Operation, args: serde_json::Map<String, Value>) -> Value {
        let info: splitty_core::daemon::DaemonInfo =
            serde_json::from_value(self.info().expect("daemon.json"))
                .expect("the current build reads daemon.json");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let mut client = splitty_server::native::client::Connector::default()
                .local(&info)
                .await
                .expect("the current native client reaches the daemon");
            let reply = client
                .request(operation, args)
                .await
                .expect("a native reply");
            let (status, body) = (reply.status, reply.body.clone());
            drop(reply);
            client.shutdown().await;
            assert_eq!(status, 200, "{body}");
            serde_json::from_str(&body).expect("a JSON reply")
        })
    }

    /// The processes of this sandbox: those whose environment names its
    /// `HOME`. Nothing else of this user has it.
    fn processes(&self) -> Vec<u32> {
        let entry = format!("HOME={}", self.home.display()).into_bytes();
        let own = std::process::id();
        let mut found = Vec::new();
        for process in fs::read_dir("/proc").expect("process list").flatten() {
            let Some(pid) = process
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if pid == own {
                continue;
            }
            // Another user's process or one that has just ended.
            let Ok(environment) = fs::read(process.path().join("environ")) else {
                continue;
            };
            if environment.split(|byte| *byte == 0).any(|e| e == entry) {
                found.push(pid);
            }
        }
        found
    }
}

impl Drop for Sandbox {
    /// A failed assertion must not leave a daemon, a keeper or a shell of
    /// either build running. The daemons end their sessions first; what is
    /// left is stopped by PID, only after it shows this sandbox's `HOME`.
    fn drop(&mut self) {
        for path in &self.builds {
            let mut command = Command::new(path);
            command.args(["daemon", "stop", "--end-sessions"]);
            self.isolate(&mut command)
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Ok(mut child) = command.spawn() {
                let _ = wait_within(&mut child, COMMAND_TIMEOUT);
            }
        }
        for signal in [Signal::SIGTERM, Signal::SIGKILL] {
            let left = self.processes();
            if left.is_empty() {
                return;
            }
            for pid in left {
                let shown = Command::new("ps")
                    .args(["-p", &pid.to_string(), "-o", "args="])
                    .output()
                    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                    .unwrap_or_default();
                eprintln!("version skew cleanup: {signal} to {pid} ({shown})");
                if let Ok(raw) = i32::try_from(pid) {
                    let _ = kill(Pid::from_raw(raw), signal);
                }
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while !self.processes().is_empty() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// The exit status of a child, or `None` (after killing it) once `limit` has
/// passed.
fn wait_within(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().expect("child status") {
            return Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn alive(pid: u32) -> bool {
    i32::try_from(pid).is_ok_and(|pid| pid > 0 && kill(Pid::from_raw(pid), None).is_ok())
}

/// The executable a process runs.
fn executable(pid: u32) -> PathBuf {
    fs::read_link(format!("/proc/{pid}/exe")).expect("process executable")
}

fn parent(pid: u32) -> u32 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).expect("process stat");
    stat.rsplit_once(") ")
        .and_then(|(_, fields)| fields.split_whitespace().nth(1))
        .and_then(|ppid| ppid.parse().ok())
        .expect("parent PID")
}

nix::ioctl_write_ptr_bad!(set_window_size, nix::libc::TIOCSWINSZ, nix::libc::winsize);

/// `splitty attach` of one build in a terminal of its own: a PTY this test
/// reads, types into and resizes, as a terminal emulator would.
struct Terminal {
    child: Option<Child>,
    master: File,
    output: Arc<Mutex<Vec<u8>>>,
    what: String,
}

impl Terminal {
    fn attach(sandbox: &Sandbox, build: &Build, target: &str, cols: u16, rows: u16) -> Self {
        let size = nix::pty::Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = nix::pty::openpty(Some(&size), None).expect("openpty");
        let slave = File::from(pty.slave);
        let mut command = Command::new(&build.path);
        command.args(["attach", target]);
        sandbox
            .isolate(&mut command)
            .stdin(slave.try_clone().expect("stdin"))
            .stdout(slave.try_clone().expect("stdout"))
            .stderr(slave);
        let child = command.spawn().expect("splitty attach");
        drop(command);
        let master = File::from(pty.master);
        let output = Arc::new(Mutex::new(Vec::new()));
        {
            let output = output.clone();
            let mut reader = master.try_clone().expect("master");
            std::thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                while let Ok(count) = reader.read(&mut buffer) {
                    if count == 0 {
                        return;
                    }
                    output
                        .lock()
                        .expect("terminal")
                        .extend_from_slice(&buffer[..count]);
                }
            });
        }
        let terminal = Self {
            child: Some(child),
            master,
            output,
            what: format!("the {} build's attach", build.name),
        };
        // Typed before it is in raw mode, a line would go through the
        // terminal's line editing first.
        terminal.wait(sandbox, "the attached session's first output", 0, "");
        terminal
    }

    fn mark(&self) -> usize {
        self.output.lock().expect("terminal").len()
    }

    fn since(&self, mark: usize) -> String {
        String::from_utf8_lossy(&self.output.lock().expect("terminal")[mark..]).into_owned()
    }

    /// Waits for `needle` in what the terminal showed after `mark`; an empty
    /// needle waits for any output.
    fn wait(&self, sandbox: &Sandbox, what: &str, mark: usize, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let shown = self.since(mark);
            if !shown.is_empty() && shown.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{}: {what} ({needle:?})\nterminal: {shown:?}\n{}",
                self.what,
                sandbox.daemon_log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn type_text(&mut self, text: &str) {
        self.master.write_all(text.as_bytes()).expect("type");
    }

    /// Types a line and waits for its output.
    fn run_line(&mut self, sandbox: &Sandbox, line: &str, expect: &str) {
        let mark = self.mark();
        self.type_text(&format!("{line}\r"));
        self.wait(sandbox, line, mark, expect);
    }

    /// Asks the shell for its terminal size until it reports `cols`x`rows`:
    /// a resize reaches the PTY on a path of its own, which a typed line may
    /// overtake.
    fn expect_size(&mut self, sandbox: &Sandbox, cols: u16, rows: u16) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let (line, needle) = size_probe(cols, rows);
            let mark = self.mark();
            self.type_text(&format!("{line}\r"));
            let attempt = Instant::now() + Duration::from_secs(1);
            while Instant::now() < attempt {
                if self.since(mark).contains(&needle) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            assert!(
                Instant::now() < deadline,
                "{}: the shell never saw {cols}x{rows}\nterminal: {:?}\n{}",
                self.what,
                self.since(0),
                sandbox.daemon_log()
            );
        }
    }

    /// Resizes the terminal. It is not the attach's controlling terminal, so
    /// the kernel sends no SIGWINCH; this does, as the terminal's would.
    fn resize(&mut self, cols: u16, rows: u16) {
        let size = nix::libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: TIOCSWINSZ reads one `winsize` from the pointer, which lives
        // for the whole call; the descriptor is open.
        unsafe { set_window_size(self.master.as_raw_fd(), &size) }.expect("TIOCSWINSZ");
        let child = self.child.as_ref().expect("attached");
        let pid = i32::try_from(child.id()).expect("attach PID");
        kill(Pid::from_raw(pid), Signal::SIGWINCH).expect("SIGWINCH");
    }

    /// Ctrl+\ detaches: the attach exits 0 with its closing line.
    fn detach(mut self, sandbox: &Sandbox) {
        let mark = self.mark();
        self.type_text("\x1c");
        let mut child = self.child.take().expect("attached");
        let status = wait_within(&mut child, TIMEOUT)
            .unwrap_or_else(|| panic!("{} did not detach: {:?}", self.what, self.since(0)));
        assert!(status.success(), "{} detached with {status}", self.what);
        self.wait(sandbox, "the closing line", mark, "[detached]");
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Attaches a pane with `build`'s `attach` at 100x30 and checks the size,
/// resizes it to 120x40 and checks again, types a line, detaches; then
/// attaches again at 90x28 and finds that line in the replay. `seen` must be
/// in the first replay already.
fn attach_resize_reattach(sandbox: &Sandbox, build: &Build, pane: &Pane, seen: &str, mark: &str) {
    let mut terminal = Terminal::attach(sandbox, build, &pane.pane, 100, 30);
    terminal.wait(sandbox, "the replay", 0, seen);
    terminal.expect_size(sandbox, 100, 30);
    terminal.resize(120, 40);
    terminal.expect_size(sandbox, 120, 40);
    terminal.run_line(
        sandbox,
        &format!("echo {mark}-$((40+2))"),
        &format!("{mark}-42"),
    );
    terminal.detach(sandbox);

    let mut again = Terminal::attach(sandbox, build, &pane.pane, 90, 28);
    again.wait(
        sandbox,
        "the replay after re-attaching",
        0,
        &format!("{mark}-42"),
    );
    again.expect_size(sandbox, 90, 28);
    again.detach(sandbox);
}

/// A session the current build's native client shows through a bridge.
struct Bridged<'a> {
    link: splitty_remote::bridge::NativeBridge,
    session: &'a str,
    output: Vec<u8>,
    /// Whether the reply to the `mux.status` request arrived.
    replied: bool,
    what: String,
}

impl Bridged<'_> {
    fn send(&self, message: Value) {
        self.link
            .sender()
            .data(
                splitty_transport::wire::Kind::Text,
                message.to_string().as_bytes(),
            )
            .expect("send");
    }

    fn type_line(&self, line: &str) {
        self.send(json!({"type": "input", "id": self.session, "data": format!("{line}\n")}));
    }

    /// Reads until `needle` shows in the session's output after `mark`, or
    /// `within` has passed; notes the request's reply on the way.
    async fn read_until(&mut self, mark: usize, needle: &str, within: Duration) -> bool {
        use splitty_transport::wire::{self, Kind};
        let deadline = tokio::time::Instant::now() + within;
        loop {
            if String::from_utf8_lossy(&self.output[mark..]).contains(needle) {
                return true;
            }
            let Ok(message) = tokio::time::timeout_at(deadline, self.link.recv()).await else {
                return false;
            };
            let message = message.unwrap_or_else(|| panic!("{}: the bridge ended", self.what));
            match message.frame.kind {
                Kind::Reply => {
                    let reply: wire::Reply = message.frame.decode().expect("reply");
                    assert_eq!((reply.id, reply.status), (1, 200), "{}", reply.body);
                    self.replied = true;
                }
                Kind::Binary => {
                    let (_, bytes) = message.frame.application().expect("frame");
                    if let Some((id, data)) =
                        splitty_server::client_session::codec::decode_binary_frame(bytes)
                    {
                        if id == self.session {
                            self.output.extend_from_slice(data);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    async fn expect(&mut self, sandbox: &Sandbox, mark: usize, needle: &str) {
        if !self.read_until(mark, needle, TIMEOUT).await {
            panic!(
                "{}: {needle:?}\noutput: {:?}\n{}",
                self.what,
                String::from_utf8_lossy(&self.output),
                sandbox.daemon_log()
            );
        }
    }

    /// As [`Terminal::expect_size`], through the bridge.
    async fn expect_size(&mut self, sandbox: &Sandbox, cols: u16, rows: u16) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let (line, needle) = size_probe(cols, rows);
            let mark = self.output.len();
            self.type_line(&line);
            if self.read_until(mark, &needle, Duration::from_secs(1)).await {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{}: the shell never saw {cols}x{rows}\noutput: {:?}\n{}",
                self.what,
                String::from_utf8_lossy(&self.output),
                sandbox.daemon_log()
            );
        }
    }
}

/// The current build's native client attaches a session through `bridge`'s
/// `remote-bridge`, as it reaches a host's daemon over SSH: the welcome, a
/// request, the session attached at 100x30, resized to 120x40, detached and
/// attached again at 90x28.
fn bridge_session(sandbox: &Sandbox, bridge: &Build, pane: &Pane) {
    use splitty_transport::{
        admission, negotiation,
        wire::{self, Kind},
    };

    let info: splitty_core::daemon::DaemonInfo =
        serde_json::from_value(sandbox.info().expect("daemon.json"))
            .expect("the current build reads daemon.json");
    let instance = info.instance.clone().expect("daemon instance");
    let mut command = Command::new(&bridge.path);
    command.args(["remote-bridge", "--transport", "native-v1"]);
    sandbox.isolate(&mut command);
    let what = format!(
        "the current client through the {} build's bridge",
        bridge.name
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let assignment =
            admission::Assignment::new(admission::Backend::new(), admission::Domain::new());
        let mut link = splitty_remote::bridge::NativeBridge::spawn(
            tokio::process::Command::from(command),
            assignment,
            negotiation::Expected {
                instance: Some(&instance),
                purpose: &wire::Purpose::Direct,
                mux_protocol: splitty_core::protocol::PROTOCOL_VERSION,
                preparation: Duration::from_secs(5),
            },
            |_| {},
        )
        .await
        .unwrap_or_else(|error| panic!("{what}: {error}\n{}", sandbox.daemon_log()));
        assert_eq!(link.ready().instance, instance);
        let welcome = tokio::time::timeout(TIMEOUT, link.recv())
            .await
            .expect("a welcome in time")
            .expect("a welcome");
        let value: Value = welcome.frame.decode().expect("welcome JSON");
        drop(welcome);
        assert_eq!(value["type"], "mux.welcome", "{value}");
        eprintln!("{what}: capabilities {}", value["capabilities"]);
        link.sender()
            .json(
                Kind::Request,
                &wire::Request {
                    id: 1,
                    operation: "mux.status".into(),
                    args: serde_json::Map::new(),
                },
            )
            .expect("request");

        let session = pane.session.as_str();
        let mut view = Bridged {
            link,
            session,
            output: Vec::new(),
            replied: false,
            what: what.clone(),
        };
        // An attach shows the session; the resize after it reports the
        // client's size, as every client sends it.
        let attach = |view: &Bridged, cols: u16, rows: u16| {
            view.send(json!({"type": "attach", "id": session, "cols": cols, "rows": rows}));
            view.send(json!({"type": "resize", "id": session, "cols": cols, "rows": rows}));
        };
        let typed = format!("bridged-{}", bridge.name);
        attach(&view, 100, 30);
        view.expect_size(sandbox, 100, 30).await;
        view.send(json!({"type": "resize", "id": session, "cols": 120, "rows": 40}));
        view.expect_size(sandbox, 120, 40).await;
        let mark = view.output.len();
        view.type_line(&format!("echo {typed}-$((40+2))"));
        view.expect(sandbox, mark, &format!("{typed}-42")).await;
        view.send(json!({"type": "detach", "id": session}));

        let mark = view.output.len();
        attach(&view, 90, 28);
        view.expect(sandbox, mark, &format!("{typed}-42")).await;
        view.expect_size(sandbox, 90, 28).await;
        view.send(json!({"type": "detach", "id": session}));
        assert!(view.replied, "{what}: no reply to mux.status");

        let (transport, ended) = view.link.finish().await;
        transport.unwrap_or_else(|error| panic!("{what}: {error}"));
        assert!(
            ended.status.as_ref().is_ok_and(ExitStatus::success),
            "{what}: the bridge's exit\n{}",
            ended.stderr
        );
    });
}

/// A client and a daemon: the CLI commands, `attach` and `daemon stop` of one
/// build against the daemon of the other; and the current build's native
/// client through each build's `remote-bridge`, as it reaches a host's daemon.
#[test]
fn the_clients_of_one_build_use_the_daemon_of_the_other() {
    let Some(builds) = builds() else { return };
    let (released, current) = &builds;
    for (daemon, client) in [(released, current), (current, released)] {
        eprintln!("clients: {} daemon, {} client", daemon.name, client.name);
        let sandbox = Sandbox::new(&builds);
        sandbox.start_daemon(daemon);
        let pid = sandbox.daemon_pid().expect("daemon PID");
        let status = sandbox.ok(client, &["daemon", "status"]);
        assert!(status.contains(&pid.to_string()), "{status}");
        let server = sandbox.json(client, &["status", "--json"]);
        assert!(server.is_object(), "{server}");

        // The control socket: screens, panes, splits, names, keys.
        let pane = sandbox.new_screen(client, "clients");
        sandbox.ok(
            client,
            &["split", "--screen", &pane.screen, "--pane", &pane.pane],
        );
        let panes = sandbox.panes(client, &pane.screen);
        assert_eq!(panes.as_object().map(|p| p.len()), Some(2), "{panes}");
        let split = panes
            .as_object()
            .and_then(|panes| panes.keys().find(|id| **id != pane.pane))
            .expect("the split pane")
            .clone();
        sandbox.ok(
            client,
            &["close-pane", "--screen", &pane.screen, "--pane", &split],
        );
        sandbox.ok(
            client,
            &["rename-screen", "renamed", "--screen", &pane.screen],
        );
        assert_eq!(sandbox.screen_name(daemon, &pane.screen), "renamed");
        sandbox.type_line(client, &pane, "echo typed-$((6*7))");

        // The native endpoint: a session attached, resized and re-attached.
        attach_resize_reattach(&sandbox, client, &pane, "typed-42", "attached");

        // The desktop's client through a host's bridge.
        let bridges: &[&Build] = if daemon.name == "released" {
            &[released, current]
        } else {
            &[released]
        };
        for bridge in bridges {
            bridge_session(&sandbox, bridge, &pane);
        }

        sandbox.stop_daemon(client, true);
    }
}

/// A daemon and the keepers the other build started: one build's daemon
/// stops and keeps its shell; the other build's `attach` controls that keeper
/// without a daemon; the other build's daemon adopts it with the same shell,
/// workspace and output, and attaches, resizes and re-attaches it.
#[test]
fn a_daemon_adopts_the_keepers_the_other_build_started() {
    let Some(builds) = builds() else { return };
    let (released, current) = &builds;
    for (first, second) in [(released, current), (current, released)] {
        eprintln!("keepers: {} keeper, {} daemon", first.name, second.name);
        let sandbox = Sandbox::new(&builds);
        sandbox.start_daemon(first);
        let pane = sandbox.new_screen(first, "kept");
        sandbox.type_line(
            first,
            &pane,
            "echo $$ > \"$HOME/shell\"; echo before-$((40+2))",
        );
        let shell: u32 = sandbox.wait_for_file("shell").parse().expect("shell PID");
        let keeper = parent(shell);
        assert_eq!(
            executable(keeper),
            first.path,
            "the {} build keeps the shell",
            first.name
        );
        assert_eq!(sandbox.keeper_sockets(), 1);

        sandbox.stop_daemon(first, false);
        assert!(
            alive(shell) && alive(keeper),
            "the shell outlives its daemon"
        );

        // Attach without a daemon: the other build controls the keeper.
        let mut direct = Terminal::attach(&sandbox, second, &pane.pane, 100, 30);
        direct.wait(&sandbox, "the keeper's replay", 0, "before-42");
        direct.expect_size(&sandbox, 100, 30);
        direct.run_line(&sandbox, "echo direct-$((2+2))", "direct-4");
        direct.detach(&sandbox);

        sandbox.start_daemon(second);
        let panes = sandbox.panes(second, &pane.screen);
        assert_eq!(
            panes[pane.pane.as_str()]["sessionId"],
            pane.session,
            "the same pane and session: {panes}"
        );
        assert!(
            alive(shell) && parent(shell) == keeper,
            "adopted, not respawned"
        );
        assert_eq!(executable(keeper), first.path);
        sandbox.type_line(second, &pane, "echo $$ > \"$HOME/shell-after\"");
        assert_eq!(
            sandbox.wait_for_file("shell-after"),
            shell.to_string(),
            "the same shell"
        );
        attach_resize_reattach(&sandbox, second, &pane, "direct-4", "adopted");
        assert_eq!(sandbox.keeper_sockets(), 1, "no second shell");

        sandbox.stop_daemon(second, true);
        sandbox.wait_until("the kept shell ends", || !alive(shell));
    }
}

/// The `splitty` command in a pane and the daemon behind its `SPLITTY_SOCK`:
/// the other build's CLI, run in a pane, targets the pane and its screen
/// implicitly; after the other build's daemon took over the pane's keeper,
/// the shell reaches it through the `SPLITTY_SOCK` and the CLI directory the
/// first daemon gave it, with either build's CLI.
#[test]
fn the_command_in_a_pane_reaches_the_daemon_of_the_other_build() {
    let Some(builds) = builds() else { return };
    let (released, current) = &builds;
    for (daemon, other) in [(released, current), (current, released)] {
        eprintln!("pane: {} daemon, {} command", daemon.name, other.name);
        let sandbox = Sandbox::new(&builds);
        sandbox.start_daemon(daemon);
        let pane = sandbox.new_screen(daemon, "pane");
        let cli = other.word();
        sandbox.type_line(
            daemon,
            &pane,
            &format!(
                "{cli} list-screens --json > \"$HOME/screens.json\" 2>&1; \
                 {cli} rename-screen from-pane > \"$HOME/rename.out\" 2>&1; \
                 {cli} split > \"$HOME/split.out\" 2>&1; \
                 echo \"$SPLITTY_CLI_DIR\" > \"$HOME/cli-dir\"; echo done > \"$HOME/first\""
            ),
        );
        sandbox.wait_for_file("first");
        let screens = fs::read_to_string(sandbox.home.join("screens.json")).expect("screens");
        assert!(screens.contains(&pane.screen), "{screens}");
        assert_eq!(
            sandbox.screen_name(daemon, &pane.screen),
            "from-pane",
            "{}",
            fs::read_to_string(sandbox.home.join("rename.out")).unwrap_or_default()
        );
        let panes = sandbox.panes(daemon, &pane.screen);
        assert_eq!(
            panes.as_object().map(|p| p.len()),
            Some(2),
            "split from the pane: {panes}\n{}",
            fs::read_to_string(sandbox.home.join("split.out")).unwrap_or_default()
        );
        let split = Pane {
            screen: pane.screen.clone(),
            pane: panes
                .as_object()
                .and_then(|panes| panes.keys().find(|id| **id != pane.pane))
                .expect("the split pane")
                .clone(),
            session: String::new(),
        };
        // The split's shell is the daemon's: the other build attaches it.
        sandbox.type_line(daemon, &split, "echo split-$((3*3))");
        attach_resize_reattach(&sandbox, other, &split, "split-9", "split");

        // The other build's daemon takes over the first pane's keeper. The
        // shell keeps the environment the first daemon gave it.
        assert!(
            !sandbox.wait_for_file("cli-dir").is_empty(),
            "a CLI directory"
        );
        sandbox.stop_daemon(daemon, false);
        sandbox.start_daemon(other);
        let own = daemon.word();
        sandbox.type_line(
            other,
            &pane,
            &format!(
                "\"$SPLITTY_CLI_DIR/splitty\" --version > \"$HOME/linked\" 2>&1; \
                 \"$SPLITTY_CLI_DIR/splitty\" rename-screen via-link > \"$HOME/linked.out\" 2>&1; \
                 {own} list-screens --json > \"$HOME/after.json\" 2>&1; \
                 {own} split > \"$HOME/split-after.out\" 2>&1; \
                 echo done > \"$HOME/second\""
            ),
        );
        sandbox.wait_for_file("second");
        assert_eq!(
            sandbox.wait_for_file("linked"),
            format!("splitty {}", other.version),
            "the CLI directory follows the daemon that started last"
        );
        assert_eq!(
            sandbox.screen_name(other, &pane.screen),
            "via-link",
            "{}",
            fs::read_to_string(sandbox.home.join("linked.out")).unwrap_or_default()
        );
        let after = fs::read_to_string(sandbox.home.join("after.json")).expect("after");
        assert!(after.contains(&pane.screen), "{after}");
        let panes = sandbox.panes(other, &pane.screen);
        assert_eq!(
            panes.as_object().map(|p| p.len()),
            Some(3),
            "split from the adopted pane: {panes}\n{}",
            fs::read_to_string(sandbox.home.join("split-after.out")).unwrap_or_default()
        );

        sandbox.stop_daemon(other, true);
    }
}

/// The files one build writes and the other reads: one build's daemon saves a
/// workspace and settings and ends its sessions; the other build's daemon
/// restores the screen with its panes, respawns their shells and serves the
/// settings, also beside a setting it does not know; a restored pane is
/// attached, resized and re-attached.
#[test]
fn the_files_one_build_writes_are_read_by_the_other() {
    let Some(builds) = builds() else { return };
    let (released, current) = &builds;
    for (writer, reader) in [(released, current), (current, released)] {
        eprintln!("files: written by {}, read by {}", writer.name, reader.name);
        let sandbox = Sandbox::new(&builds);
        sandbox.start_daemon(writer);
        let pane = sandbox.new_screen(writer, "files");
        sandbox.ok(
            writer,
            &["split", "--screen", &pane.screen, "--pane", &pane.pane],
        );
        let written = sandbox.panes(writer, &pane.screen);
        let mut panes: Vec<String> = written
            .as_object()
            .expect("panes")
            .keys()
            .cloned()
            .collect();
        panes.sort();
        assert_eq!(panes.len(), 2, "{written}");
        let settings = json!({"terminal": {"sizePolicy": "latest", "sizeIdleSeconds": 30}});
        sandbox.native_request(
            Operation::HomeUpdate,
            serde_json::Map::from_iter([("body".to_owned(), settings)]),
        );
        sandbox.stop_daemon(writer, true);
        assert!(
            sandbox.config_dir().join("state.json").is_file(),
            "the {} build wrote state.json",
            writer.name
        );
        // A setting neither build knows, as a later build would write one:
        // the reader keeps the values it knows.
        let config = sandbox.config_dir().join("config.toml");
        let toml = fs::read_to_string(&config).expect("config.toml");
        assert!(toml.contains("\n[terminal]\n"), "{toml}");
        fs::write(
            &config,
            toml.replacen("\n[terminal]\n", "\n[terminal]\nlater_setting = true\n", 1),
        )
        .expect("config.toml");

        sandbox.start_daemon(reader);
        assert_eq!(sandbox.screen_name(reader, &pane.screen), "files");
        let restored = sandbox.panes(reader, &pane.screen);
        let mut ids: Vec<String> = restored
            .as_object()
            .expect("panes")
            .keys()
            .cloned()
            .collect();
        ids.sort();
        assert_eq!(ids, panes, "the same panes: {restored}");
        let home = sandbox.native_request(Operation::HomeGet, serde_json::Map::new());
        assert_eq!(home["terminal"]["sizePolicy"], "latest", "{home}");
        assert_eq!(home["terminal"]["sizeIdleSeconds"], 30, "{home}");

        sandbox.type_line(reader, &pane, "echo restored-$((5+5))");
        attach_resize_reattach(&sandbox, reader, &pane, "restored-10", "reread");

        sandbox.stop_daemon(reader, true);
    }
}
