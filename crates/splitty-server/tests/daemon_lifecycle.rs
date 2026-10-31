//! The daemon's lifecycle, end to end, through the real `splitty` binary.
//!
//! `daemon start` once shipped broken: the forked daemon panicked before it
//! served anything, yet the command exited 0 and `daemon status` reported it
//! as running. Nothing ran the command the way a user does, so nothing noticed.
//! These tests do: every step is a separate `splitty` process, exactly as typed
//! in a shell.
//!
//! Each test gets a throwaway `HOME`/`XDG_*`, so it never touches the
//! developer's real `~/.config/splitty` or a daemon already running there.

#![cfg(unix)]

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;
use tempfile::TempDir;

/// Longer than `daemon start` waits for readiness, so its own timeout fires
/// first.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// An isolated user environment for `splitty` commands.
struct Sandbox {
    home: TempDir,
    /// `XDG_RUNTIME_DIR` for every command, where the control socket lives.
    runtime_dir: PathBuf,
}

impl Sandbox {
    fn json(&self, args: &[&str]) -> serde_json::Value {
        let output = self.splitty(args);
        assert!(output.status.success(), "{}", self.describe(&output));
        serde_json::from_slice(&output.stdout).expect("one JSON value on stdout")
    }

    fn scoped(&self, socket: &str, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
        command.args(args);
        self.isolate(&mut command)
            .env("SPLITTY_AGENT_SOCK", socket)
            .output()
            .expect("scoped CLI")
    }
    fn new() -> Self {
        // Below /tmp, not the platform's temporary directory: on macOS that
        // is so long that a keeper's socket path would exceed the 103 bytes
        // a Unix socket allows, and sessions would run without keepers.
        let home = tempfile::Builder::new()
            .prefix("sp")
            .tempdir_in("/tmp")
            .expect("tempdir");
        let runtime_dir = home.path().join("run");
        Self { home, runtime_dir }
    }

    /// Point a command at this sandbox instead of the invoking user's setup.
    fn isolate<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        cmd.env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("XDG_RUNTIME_DIR", &self.runtime_dir)
            // Run from inside a Splitty pane, the tests would otherwise drive
            // the developer's own server.
            .env_remove("SPLITTY_SOCK")
            .env_remove("SPLITTY_AGENT_SOCK")
            .env_remove("SPLITTY_ADDR")
            .env_remove("SPLITTY_TOKEN")
            .env_remove("SPLITTY_SERVER")
            .env_remove("SPLITTY_PANE_ID")
            .env_remove("SPLITTY_SCREEN_ID")
            .env_remove("SPLITTY_SESSION_ID")
            .stdin(Stdio::null())
    }

    fn splitty(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_splitty"));
        cmd.args(args);
        self.isolate(&mut cmd).output().expect("run splitty")
    }

    /// `dirs::config_dir()` honours `XDG_CONFIG_HOME` on Linux only.
    fn config_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home.path().join("Library/Application Support/splitty")
        } else {
            self.home.path().join("config/splitty")
        }
    }

    /// Where the daemon's control socket and its keepers live: below the
    /// data directory, which `dirs::data_dir()` takes from `XDG_DATA_HOME` on
    /// Linux only.
    fn run_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home
                .path()
                .join("Library/Application Support/splitty/run")
        } else {
            self.home.path().join("data/splitty/run")
        }
    }

    fn info_path(&self) -> PathBuf {
        self.config_dir().join("daemon.json")
    }

    fn read_info(&self) -> Option<serde_json::Value> {
        let text = fs::read_to_string(self.info_path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn log_path(&self) -> PathBuf {
        self.config_dir().join("daemon.log")
    }

    fn daemon_log(&self) -> String {
        fs::read_to_string(self.log_path()).unwrap_or_else(|e| format!("<no daemon.log: {e}>"))
    }

    /// The process token the daemon resolved, for reaching it over TCP.
    fn token(&self) -> String {
        fs::read_to_string(self.config_dir().join("auth-token"))
            .map(|token| token.trim().to_string())
            .expect("the daemon should have persisted an auth token")
    }

    /// `daemon start` on an OS-assigned port; returns that port.
    fn start_daemon(&self) -> u16 {
        let start = self.splitty(&["daemon", "start", "--addr", "127.0.0.1:0"]);
        assert!(
            start.status.success(),
            "daemon start failed\n{}",
            self.describe(&start)
        );
        port_from(&start)
            .unwrap_or_else(|| panic!("no SPLITTY_PORT line\n{}", self.describe(&start)))
    }

    /// The server's status over TCP with the process token, bypassing the
    /// control socket.
    fn status_over_tcp(&self, port: u16) -> serde_json::Value {
        let addr = format!("127.0.0.1:{port}");
        let token = self.token();
        let status = self.splitty(&["status", "--json", "--addr", &addr, "--token", &token]);
        assert!(
            status.status.success(),
            "status over TCP failed\n{}",
            self.describe(&status)
        );
        serde_json::from_slice(&status.stdout).expect("status JSON")
    }

    /// Everything that helps diagnose a failed command, including the daemon's
    /// log: a daemon that dies in the background leaves its reason only there.
    fn describe(&self, out: &Output) -> String {
        format!(
            "status: {}\nstdout:\n{}\nstderr:\n{}\ndaemon.log:\n{}",
            out.status,
            stdout(out),
            stderr(out),
            self.daemon_log(),
        )
    }
}

// Smoke: only the real binary exercises the scoped main-entry guard before any
// discovery/config access, not merely the local guide renderer.
#[test]
fn agent_guide_is_offline_even_with_unusable_scope_and_configuration() {
    let sandbox = Sandbox::new();
    let missing = sandbox.home.path().join("unreachable-agent.sock");
    let socket = missing.to_str().expect("socket");
    for scoped in [false, true] {
        let args = ["agent", "guide", "--json"];
        let output = if scoped {
            sandbox.scoped(socket, &args)
        } else {
            sandbox.splitty(&args)
        };
        assert!(output.status.success(), "{}", sandbox.describe(&output));
        let value: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("one JSON object");
        assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
        assert!(value["guide"]
            .as_str()
            .expect("guide")
            .contains("python3 -m http.server"));
        assert!(output.stderr.is_empty());
    }
    let markdown = sandbox.scoped(socket, &["agent", "guide"]);
    assert!(markdown.status.success());
    assert!(stdout(&markdown).starts_with("# Splitty agent guide\n"));
    let help = sandbox.splitty(&["agent", "--help"]);
    assert!(help.status.success());
    assert!(stdout(&help).contains("guide"));
    assert!(sandbox.splitty(&["--version"]).status.success());
    assert_eq!(
        sandbox.splitty(&["agent-guide-unknown"]).status.code(),
        Some(2)
    );
    for args in [
        vec!["agent", "list", "--json"],
        vec!["agent", "create", "--name", "no", "--json"],
        vec!["agent", "revoke", "id", "--json"],
        vec!["agent", "assign", "id", "--job", "job", "--json"],
        vec!["agent", "reclaim", "--job", "job", "--json"],
        vec!["agent", "exec", "id", "--", "/bin/false"],
        vec!["issue-bootstrap"],
        // cli, Scoped Administrative Commands Are Refused: an agent grant
        // never reaches a terminal; refused before the token file is read.
        vec!["attach", "3f9a"],
        vec!["daemon", "stop"],
        vec!["serve", "--addr", "127.0.0.1:0"],
    ] {
        assert_eq!(sandbox.scoped(socket, &args).status.code(), Some(2));
    }
    assert!(
        !sandbox.config_dir().exists(),
        "local help or refused administration created config"
    );
    assert!(
        !sandbox.runtime_dir.exists(),
        "offline guide created a server socket directory"
    );
}

// Smoke: separate CLI/agent-exec processes and real sockets are the lifetime boundary.
// This fixture additionally needs a real executable in a version-specific directory:
// parser/path-helper tests alone cannot prove child command lookup after login.
// Only a real daemon writes the stable CLI link, and only a second installation's
// daemon adopting a kept shell shows that the shell follows an update
// (terminal-sessions, Host CLI Follows The Daemon).
#[test]
fn agent_cli_is_discoverable_in_packaged_sessions_and_exec_clients() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let sandbox = Sandbox::new();
    let directory = sandbox.home.path().join("desktop version ö");
    fs::create_dir(&directory).expect("bundle directory");
    let executable = directory.join("splitty");
    fs::copy(env!("CARGO_BIN_EXE_splitty"), &executable).expect("bundled CLI");
    let integration =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shell-integration");
    for name in ["bash/splitty.bash", "zsh/.zshenv", "zsh/splitty.zsh"] {
        let target = directory.join("shell-integration").join(name);
        fs::create_dir_all(target.parent().expect("parent")).expect("integration directory");
        fs::copy(integration.join(name), target).expect("bundled integration");
    }
    fs::write(
        sandbox.home.path().join(".bash_profile"),
        "export PATH=/usr/bin:/bin\nHISTFILE=/dev/null\n",
    )
    .expect("profile");
    let mut start = Command::new(&executable);
    sandbox
        .isolate(&mut start)
        .env("PATH", "/usr/bin:/bin")
        .env("SHELL", "/bin/bash")
        .env("SPLITTY_CLI_DIR", "/untrusted-hint")
        .env_remove("CARGO_MANIFEST_DIR")
        .args(["daemon", "start", "--addr", "127.0.0.1:0"]);
    let output = start.output().expect("start packaged backend");
    assert!(output.status.success(), "{}", sandbox.describe(&output));
    let stable = sandbox.run_dir().join("bin").join("splitty");
    assert_eq!(
        fs::canonicalize(&stable).expect("stable CLI link"),
        fs::canonicalize(&executable).expect("bundle")
    );
    assert!(sandbox
        .splitty(&["new-screen", "--name", "CLI shell"])
        .status
        .success());
    let screens = sandbox.json(&["list-screens", "--json"]);
    let screen = screens[0]["id"].as_str().expect("screen");
    let panes = sandbox.json(&["list-panes", "--screen", screen, "--json"]);
    let pane = panes
        .as_object()
        .expect("panes")
        .keys()
        .next()
        .expect("pane");
    let (first_screen, first_pane) = (screen, pane);
    let input="command -v splitty > \"$HOME/ordinary-cli.txt\"; splitty agent guide --json > \"$HOME/ordinary-guide.json\"\n";
    assert!(sandbox
        .splitty(&["send-keys", input, "--screen", screen, "--pane", pane])
        .status
        .success());
    let deadline = Instant::now() + Duration::from_secs(10);
    let ordinary = loop {
        if let Ok(bytes) = fs::read(sandbox.home.path().join("ordinary-guide.json")) {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                break value;
            }
        }
        assert!(
            Instant::now() < deadline,
            "ordinary shell cannot resolve CLI: {}",
            sandbox.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(ordinary["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        fs::read_to_string(sandbox.home.path().join("ordinary-cli.txt"))
            .expect("resolution")
            .trim(),
        stable.to_str().expect("CLI path")
    );
    let grant = sandbox.json(&[
        "agent",
        "create",
        "--name",
        "PATH agent",
        "--screen",
        screen,
        "--pane",
        pane,
        "--json",
    ]);
    let socket = grant["socket"].as_str().expect("grant socket");
    let grant_id = grant["agentId"].as_str().expect("grant");
    let mut run = Command::new(&executable);
    sandbox.isolate(&mut run).env("PATH","/usr/bin:/bin").env("SPLITTY_AGENT_SOCK",socket)
        .args(["run","--tab","--cwd",sandbox.home.path().to_str().expect("cwd"),"--json","--","/bin/sh","-c","command -v splitty; splitty agent guide --json; printf 'OWNER=%s\\n' \"${SPLITTY_SOCK-unset}\""]);
    let output = run.output().expect("scoped run");
    assert!(output.status.success(), "{}", sandbox.describe(&output));
    let job: serde_json::Value = serde_json::from_slice(&output.stdout).expect("job");
    let id = job["jobId"].as_str().expect("job id");
    let deadline = Instant::now() + Duration::from_secs(10);
    let logs = loop {
        let output = sandbox.scoped(socket, &["logs", id, "--wait", "1s", "--json"]);
        assert!(output.status.success(), "{}", sandbox.describe(&output));
        let logs: serde_json::Value = serde_json::from_slice(&output.stdout).expect("logs");
        if logs["outputComplete"] == true {
            break logs;
        }
        assert!(Instant::now() < deadline, "job did not finish");
    };
    let data = logs["data"].as_str().expect("text");
    assert!(data.contains(stable.to_str().expect("CLI path")), "{data}");
    assert!(data.contains("# Splitty agent guide"));
    assert!(data.contains("OWNER=unset"));
    let mut exec = Command::new(&executable);
    sandbox.isolate(&mut exec).env("PATH","/usr/bin:/bin").env("SPLITTY_TOKEN","must-not-be-inherited")
        .args(["agent","exec",grant_id,"--","/bin/sh","-c","test \"$SPLITTY_AGENT_SOCK\" = \"$1\" && test -z \"${SPLITTY_SOCK-}${SPLITTY_TOKEN-}\" && splitty agent guide --json || exit 91","sh",socket]);
    let output = exec.output().expect("scoped agent exec");
    assert!(output.status.success(), "{}", sandbox.describe(&output));
    let guide: serde_json::Value = serde_json::from_slice(&output.stdout).expect("one child guide");
    assert_eq!(guide["version"], env!("CARGO_PKG_VERSION"));
    // cli, Agent Exec And Resume: the client gets the stable directory too.
    let mut exec = Command::new(&executable);
    sandbox
        .isolate(&mut exec)
        .env("PATH", "/usr/bin:/bin")
        .args([
            "agent",
            "exec",
            grant_id,
            "--",
            "/bin/sh",
            "-c",
            "command -v splitty",
        ]);
    let output = exec.output().expect("agent exec lookup");
    assert!(output.status.success(), "{}", sandbox.describe(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        stable.to_str().expect("CLI path")
    );
    assert_eq!(
        fs::read_to_string(sandbox.home.path().join(".bash_profile")).expect("profile unchanged"),
        "export PATH=/usr/bin:/bin\nHISTFILE=/dev/null\n"
    );

    // A reinstall of the same version, as a host's install does it, moves the
    // running daemon's directory away, deletes it and puts the new one in its
    // place. Shells started afterwards still find the CLI there.
    let moved = sandbox.home.path().join("desktop version ö.old");
    fs::rename(&directory, &moved).expect("move the installation away");
    fs::create_dir(&directory).expect("new bundle directory");
    fs::copy(moved.join("splitty"), &executable).expect("new CLI");
    let status = Command::new("cp")
        .arg("-R")
        .arg(moved.join("shell-integration"))
        .arg(&directory)
        .status()
        .expect("copy the integration");
    assert!(status.success());
    fs::remove_dir_all(&moved).expect("delete the old installation");
    assert!(sandbox
        .splitty(&["new-screen", "--name", "After reinstall"])
        .status
        .success());
    let screens = sandbox.json(&["list-screens", "--json"]);
    let screen = screens
        .as_array()
        .expect("screens")
        .iter()
        .find(|s| s["name"] == "After reinstall")
        .and_then(|s| s["id"].as_str())
        .expect("new screen");
    let panes = sandbox.json(&["list-panes", "--screen", screen, "--json"]);
    let pane = panes
        .as_object()
        .expect("panes")
        .keys()
        .next()
        .expect("pane");
    let input = "{ command -v splitty || echo missing; } > \"$HOME/reinstalled-cli.txt\"\n";
    assert!(sandbox
        .splitty(&["send-keys", input, "--screen", screen, "--pane", pane])
        .status
        .success());
    let resolved = sandbox.home.path().join("reinstalled-cli.txt");
    let deadline = Instant::now() + Duration::from_secs(10);
    let found = loop {
        if let Ok(text) = fs::read_to_string(&resolved) {
            if text.ends_with('\n') {
                break text;
            }
        }
        assert!(
            Instant::now() < deadline,
            "shell after the reinstall wrote nothing: {}",
            sandbox.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(found.trim(), stable.to_str().expect("CLI path"));

    // An update: the daemon stops with its sessions kept and one of another
    // installation adopts them. The first pane's shell, still running, then
    // runs the other installation's CLI.
    let update = sandbox.home.path().join("remote 0.2.0");
    fs::create_dir(&update).expect("second installation");
    let updated = update.join("splitty");
    fs::copy(&executable, &updated).expect("second CLI");
    let status = Command::new("cp")
        .arg("-R")
        .arg(directory.join("shell-integration"))
        .arg(&update)
        .status()
        .expect("copy the integration");
    assert!(status.success());
    // Only the same shell still knows this variable after the update.
    assert!(sandbox
        .splitty(&[
            "send-keys",
            "KEPT=yes\n",
            "--screen",
            first_screen,
            "--pane",
            first_pane
        ])
        .status
        .success());
    let stop = sandbox.splitty(&["daemon", "stop"]);
    assert!(stop.status.success(), "{}", sandbox.describe(&stop));
    let mut start = Command::new(&updated);
    sandbox
        .isolate(&mut start)
        .env("PATH", "/usr/bin:/bin")
        .env("SHELL", "/bin/bash")
        .env_remove("CARGO_MANIFEST_DIR")
        .args(["daemon", "start", "--addr", "127.0.0.1:0"]);
    let output = start.output().expect("start the updated backend");
    assert!(output.status.success(), "{}", sandbox.describe(&output));
    let input = "printf '%s %s\\n' \"$KEPT\" \"$(readlink -f \"$(command -v splitty)\")\" > \"$HOME/updated-cli.txt\"\n";
    assert!(sandbox
        .splitty(&[
            "send-keys",
            input,
            "--screen",
            first_screen,
            "--pane",
            first_pane
        ])
        .status
        .success());
    let resolved = sandbox.home.path().join("updated-cli.txt");
    let deadline = Instant::now() + Duration::from_secs(10);
    let found = loop {
        if let Ok(text) = fs::read_to_string(&resolved) {
            if text.ends_with('\n') {
                break text;
            }
        }
        assert!(
            Instant::now() < deadline,
            "kept shell wrote nothing after the update: {}",
            sandbox.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        found.trim(),
        format!(
            "yes {}",
            fs::canonicalize(&updated).expect("second CLI").display()
        )
    );
}

// Smoke: separate CLI/agent-exec processes and real sockets are the lifetime boundary.
#[test]
fn managed_jobs_survive_cli_clients_and_resumed_grants() {
    use std::io::Write;
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    assert!(sandbox
        .splitty(&["new-screen", "--name", "user"])
        .status
        .success());
    let screens = sandbox.json(&["list-screens", "--json"]);
    let screen = screens[0]["id"].as_str().expect("screen");
    let panes = sandbox.json(&["list-panes", "--screen", screen, "--json"]);
    let pane = panes
        .as_object()
        .expect("panes")
        .keys()
        .next()
        .expect("pane");
    let grant = sandbox.json(&[
        "agent", "create", "--name", "claude", "--screen", screen, "--pane", pane, "--json",
    ]);
    let grant_id = grant["agentId"].as_str().expect("grant");
    let socket = grant["socket"].as_str().expect("socket");
    let cwd = sandbox.home.path().to_str().expect("cwd");
    let binary = env!("CARGO_BIN_EXE_splitty");
    let job = sandbox.json(&[
        "agent",
        "exec",
        grant_id,
        "--",
        binary,
        "run",
        "--tab",
        "--background",
        "--cwd",
        cwd,
        "--name",
        "backend",
        "--json",
        "--",
        "/bin/sh",
        "-c",
        "printf START; while read line; do printf 'MSG:%s\\n' \"$line\"; done",
    ]);
    let id = job["jobId"].as_str().expect("job");
    assert_eq!(job["screenId"], screen);
    let list = sandbox.json(&["agent", "exec", grant_id, "--", binary, "jobs", "--json"]);
    assert_eq!(list["jobs"][0]["jobId"], id);
    let logs = sandbox.json(&[
        "agent", "exec", grant_id, "--", binary, "logs", id, "--wait", "5s", "--json",
    ]);
    assert!(logs["data"].as_str().expect("output").contains("START"));
    let cursor = logs["nextCursor"].as_str().expect("cursor");
    let mut exec = Command::new(binary);
    sandbox.isolate(&mut exec).env("SPLITTY_SOCK",sandbox.read_info().expect("daemon")["socket"].as_str().expect("socket"))
        .env("SPLITTY_TOKEN","inherited-token").env("SPLITTY_SERVER","inherited-address")
        .args(["agent","exec",grant_id,"--","/bin/sh","-c","test -n \"$SPLITTY_AGENT_SOCK\" && test -z \"$SPLITTY_SOCK$SPLITTY_TOKEN$SPLITTY_SERVER\" || exit 99; exit 23"]);
    assert_eq!(
        exec.output().expect("exec environment").status.code(),
        Some(23)
    );
    let mut command = Command::new(binary);
    command.args(["input", id, "--json"]);
    sandbox
        .isolate(&mut command)
        .env("SPLITTY_AGENT_SOCK", socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("input CLI");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"hello\n")
        .expect("write");
    assert!(child
        .wait_with_output()
        .expect("input result")
        .status
        .success());
    let output = sandbox.scoped(
        socket,
        &["logs", id, "--since", cursor, "--wait", "5s", "--json"],
    );
    assert!(output.status.success(), "{}", sandbox.describe(&output));
    let logs: serde_json::Value = serde_json::from_slice(&output.stdout).expect("logs JSON");
    assert!(logs["data"].as_str().expect("new output").contains("hello"));
    let stopped = sandbox.scoped(socket, &["stop", id, "--json"]);
    assert!(stopped.status.success(), "{}", sandbox.describe(&stopped));
    assert_eq!(sandbox.json(&["inspect", id, "--json"])["state"], "exited");
    let after = sandbox.json(&["list-panes", "--screen", screen, "--json"]);
    assert_eq!(
        after[pane]["sessionId"], panes[pane]["sessionId"],
        "unrelated user shell changed"
    );
    assert!(!sandbox
        .scoped(socket, &["issue-bootstrap"])
        .status
        .success());
    assert!(!sandbox.scoped(socket, &["daemon", "stop"]).status.success());
    assert_eq!(
        sandbox
            .scoped(
                socket,
                &["jobs", "--addr", "127.0.0.1:1", "--token", "test", "--json"]
            )
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        sandbox
            .splitty(&["list-screens", "--addr", "127.0.0.1:1"])
            .status
            .code(),
        Some(2)
    );
    sandbox.json(&["agent", "reclaim", "--job", id, "--json"]);
    assert!(!sandbox
        .scoped(socket, &["inspect", id, "--json"])
        .status
        .success());
    sandbox.json(&["agent", "revoke", grant_id, "--json"]);
    assert!(!PathBuf::from(socket).exists());
    assert!(!sandbox.scoped(socket, &["jobs", "--json"]).status.success());
    sandbox.json(&["forget", id, "--json"]);
}

// Smoke: a new daemon incarnation must not repeat a real launch side effect.
#[test]
fn managed_jobs_restore_as_interrupted_without_replaying_commands() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    assert!(sandbox
        .splitty(&["new-screen", "--name", "ordinary"])
        .status
        .success());
    let cwd = sandbox.home.path().to_str().expect("cwd");
    let old_grant = sandbox.json(&["agent", "create", "--name", "before-restart", "--json"]);
    let job = sandbox.json(&[
        "run",
        "--cwd",
        cwd,
        "--json",
        "--",
        "/bin/sh",
        "-c",
        "echo started >> marker; printf READY; read line",
    ]);
    let id = job["jobId"].as_str().expect("job");
    let logs = sandbox.json(&["logs", id, "--wait", "5s", "--json"]);
    assert!(logs["data"].as_str().expect("output").contains("READY"));
    let cursor = logs["nextCursor"].as_str().expect("cursor");
    let completed = sandbox.json(&[
        "run", "--cwd", cwd, "--json", "--", "/bin/sh", "-c", "exit 7",
    ]);
    let completed_id = completed["jobId"].as_str().expect("completed job");
    let deadline = Instant::now() + Duration::from_secs(5);
    let persisted = loop {
        let bytes = fs::read(sandbox.config_dir().join("state.json")).expect("persisted state");
        let state: serde_json::Value = serde_json::from_slice(&bytes).expect("state JSON");
        if state["jobs"][completed_id]["state"] == "exited" {
            break state;
        }
        assert!(
            Instant::now() < deadline,
            "exit was not persisted without a UI"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(persisted["jobs"][completed_id]["result"]["exitCode"], 7);
    assert!(persisted["jobs"][completed_id].get("output").is_none());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(sandbox.config_dir().join("state.json"))
            .expect("state mode")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    // The job's keeper ends with the daemon here: a job whose keeper is gone
    // is interrupted, never replayed (`server-modes`, Session Respawn On
    // Daemon Start).
    assert!(sandbox
        .splitty(&["daemon", "stop", "--end-sessions"])
        .status
        .success());
    let deadline = Instant::now() + Duration::from_secs(15);
    while sandbox.info_path().exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !sandbox.info_path().exists(),
        "daemon did not stop: {}",
        sandbox.daemon_log()
    );
    sandbox.start_daemon();
    assert_eq!(
        sandbox.json(&["inspect", completed_id, "--json"])["exitCode"],
        7
    );
    assert_eq!(
        sandbox.json(&["inspect", id, "--json"])["state"],
        "interrupted"
    );
    let old_path = old_grant["socket"].as_str().expect("old path");
    assert!(!PathBuf::from(old_path).exists());
    assert!(!sandbox
        .scoped(old_path, &["jobs", "--json"])
        .status
        .success());
    let fresh = sandbox.json(&["agent", "create", "--name", "after-restart", "--json"]);
    sandbox.json(&[
        "agent",
        "assign",
        fresh["agentId"].as_str().expect("new grant"),
        "--job",
        id,
        "--json",
    ]);
    assert!(sandbox
        .scoped(
            fresh["socket"].as_str().expect("new socket"),
            &["inspect", id, "--json"]
        )
        .status
        .success());
    assert_eq!(
        sandbox.json(&["logs", id, "--json"])["outputAvailable"],
        false
    );
    assert!(!sandbox
        .splitty(&["logs", id, "--since", cursor, "--json"])
        .status
        .success());
    assert_eq!(
        fs::read_to_string(sandbox.home.path().join("marker")).expect("marker"),
        "started\n"
    );
    assert_eq!(
        sandbox
            .json(&["list-screens", "--json"])
            .as_array()
            .expect("screens")
            .len(),
        3
    );
}

impl Drop for Sandbox {
    /// A failed assertion must not leave a daemon or a session keeper running
    /// after the test.
    fn drop(&mut self) {
        let _ = self.splitty(&["daemon", "stop", "--end-sessions"]);
        if let Some(pid) = self.read_info().and_then(|info| info["pid"].as_i64()) {
            stop_process(pid);
        }
    }
}

/// SIGTERM, then SIGKILL if the process is still there after two seconds.
fn stop_process(pid: i64) {
    // `kill(0, …)` would signal the test runner's own process group.
    let Ok(raw) = i32::try_from(pid) else { return };
    if raw <= 0 {
        return;
    }
    let pid = Pid::from_raw(raw);
    if kill(pid, Signal::SIGTERM).is_err() {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if kill(pid, None).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = kill(pid, Signal::SIGKILL);
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn port_from(out: &Output) -> Option<u16> {
    stdout(out)
        .lines()
        .find_map(|line| line.strip_prefix("SPLITTY_PORT="))
        .and_then(|port| port.trim().parse().ok())
}

/// A loopback address nothing listens on right now.
fn free_loopback_addr() -> String {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("bind probe");
    probe.local_addr().expect("probe address").to_string()
}

/// Read one line, or `None` if none arrives within [`READ_TIMEOUT`]. An empty
/// line means end of file. The reader comes back for further reads.
fn read_line_within<R: BufRead + Send + 'static>(mut reader: R) -> Option<(String, R)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send((line, reader));
    });
    rx.recv_timeout(READ_TIMEOUT).ok()
}

/// Kills and reaps a child the test started itself, should the test fail
/// before it stops the child.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn start_status_control_command_stop() {
    let sandbox = Sandbox::new();

    let start = sandbox.splitty(&["daemon", "start", "--addr", "127.0.0.1:0"]);
    assert!(
        start.status.success(),
        "daemon start failed\n{}",
        sandbox.describe(&start)
    );
    let port = port_from(&start)
        .unwrap_or_else(|| panic!("no SPLITTY_PORT line\n{}", sandbox.describe(&start)));
    assert_ne!(
        port, 0,
        "the OS-assigned port must be reported, not the requested 0"
    );

    let info = sandbox
        .read_info()
        .unwrap_or_else(|| panic!("no daemon.json after start\n{}", sandbox.describe(&start)));
    let pid = info["pid"].as_i64().expect("pid in daemon.json");
    assert_eq!(
        info["port"].as_u64(),
        Some(u64::from(port)),
        "daemon.json: {info}"
    );
    assert!(
        info.get("token").is_none(),
        "daemon.json must hold no credential: {info}"
    );
    assert_eq!(
        info["protocol"], 1,
        "daemon.json records the protocol: {info}"
    );
    let socket = PathBuf::from(info["socket"].as_str().expect("socket in daemon.json"));
    assert!(
        socket.exists(),
        "daemon.json names a socket that does not exist: {info}\ndaemon.log:\n{}",
        sandbox.daemon_log(),
    );

    let status = sandbox.splitty(&["daemon", "status"]);
    assert!(
        stdout(&status).contains(&format!("PID:     {pid}")),
        "daemon status does not report the daemon\n{}",
        sandbox.describe(&status),
    );

    // What `splitty connect` asks a remote host: daemon.json plus `running`.
    let status = sandbox.splitty(&["daemon", "status", "--json"]);
    assert!(status.status.success(), "{}", sandbox.describe(&status));
    let json: serde_json::Value = serde_json::from_slice(&status.stdout)
        .unwrap_or_else(|e| panic!("daemon status --json: {e}\n{}", sandbox.describe(&status)));
    assert_eq!(json["running"], true, "{json}");
    assert_eq!(json["pid"].as_i64(), Some(pid), "{json}");
    assert_eq!(json["port"].as_u64(), Some(u64::from(port)), "{json}");
    assert_eq!(json["socket"], info["socket"], "{json}");
    assert!(
        json["startedAt"].is_string() && json["version"].is_string(),
        "{json}"
    );
    assert!(json.get("token").is_none(), "no credential: {json}");

    // Run right after `daemon start` returned, through the socket that
    // daemon.json names: the daemon has to be serving already.
    let list = sandbox.splitty(&["list-screens"]);
    assert!(
        list.status.success(),
        "list-screens failed\n{}",
        sandbox.describe(&list)
    );

    let stop = sandbox.splitty(&["daemon", "stop"]);
    assert!(
        stdout(&stop).contains(&format!("Daemon stopped (pid={pid})")),
        "daemon stop did not stop the daemon\n{}",
        sandbox.describe(&stop),
    );
    assert!(
        !sandbox.info_path().exists(),
        "daemon.json left behind after stop"
    );
    assert!(!socket.exists(), "control socket left behind after stop");

    let status = sandbox.splitty(&["daemon", "status", "--json"]);
    assert!(status.status.success(), "{}", sandbox.describe(&status));
    assert_eq!(stdout(&status).trim(), r#"{"running":false}"#);

    let log = sandbox.daemon_log();
    assert!(
        log.contains("daemon started"),
        "no startup line in daemon.log:\n{log}"
    );
    assert!(
        log.contains("daemon shut down"),
        "no shutdown line in daemon.log:\n{log}"
    );
}

/// `daemon run` is the process `daemon start` spawns. Started by hand with a
/// listener, it serves on its own and announces that on stdout.
#[test]
fn daemon_run_serves_an_inherited_listener() {
    let sandbox = Sandbox::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("local address").port();
    let fd = listener.as_raw_fd();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_splitty"));
    cmd.args(["daemon", "run", "--listen-fd", &fd.to_string()]);
    sandbox
        .isolate(&mut cmd)
        .stdout(Stdio::piped())
        .stderr(File::create(sandbox.home.path().join("run.stderr")).expect("stderr file"));
    // SAFETY: only `fcntl` runs between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            nix::fcntl::fcntl(
                fd,
                nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::empty()),
            )?;
            Ok(())
        });
    }
    let mut child = ChildGuard(cmd.spawn().expect("spawn daemon run"));
    drop(listener);
    let pid = child.0.id();
    let stderr_log =
        || fs::read_to_string(sandbox.home.path().join("run.stderr")).unwrap_or_default();

    let reader = BufReader::new(child.0.stdout.take().expect("stdout"));
    let (ready, reader) = read_line_within(reader).expect("no readiness line in time");
    assert_eq!(
        ready.trim_end(),
        format!("SPLITTY_DAEMON_READY pid={pid}"),
        "stderr:\n{}",
        stderr_log()
    );

    // Nothing follows on the handshake pipe: stdout now goes to the log.
    let (rest, _) = read_line_within(reader).expect("stdout still open after the ready line");
    assert_eq!(rest, "", "unexpected output on the handshake pipe");

    let info = sandbox.read_info().expect("daemon.json once ready");
    assert_eq!(
        info["pid"].as_u64(),
        Some(u64::from(pid)),
        "daemon.json: {info}"
    );
    assert_eq!(
        info["port"].as_u64(),
        Some(u64::from(port)),
        "daemon.json: {info}"
    );
    assert_eq!(sandbox.status_over_tcp(port)["serverMode"], "daemon");

    kill(Pid::from_raw(pid as i32), Signal::SIGTERM).expect("SIGTERM");
    let status = child.0.wait().expect("wait for daemon");
    assert!(
        status.success(),
        "daemon exited with {status}\nstderr:\n{}",
        stderr_log()
    );
    assert!(
        !sandbox.info_path().exists(),
        "daemon.json left behind after SIGTERM"
    );
}

/// A descriptor that is not a listener is refused with an error line, before
/// anything names the process in `daemon.json`.
#[test]
fn daemon_run_refuses_a_descriptor_that_is_not_a_listener() {
    let sandbox = Sandbox::new();
    let out = sandbox.splitty(&["daemon", "run", "--listen-fd", "900"]);
    assert_eq!(out.status.code(), Some(1), "{}", sandbox.describe(&out));
    assert!(
        stdout(&out).starts_with("SPLITTY_DAEMON_ERROR "),
        "no error line\n{}",
        sandbox.describe(&out),
    );
    assert!(
        !sandbox.info_path().exists(),
        "daemon.json written by a daemon that never served"
    );
}

/// cli, Port in use: the bind fails in the foreground and nothing is spawned.
#[test]
fn busy_port_fails_in_the_foreground() {
    let sandbox = Sandbox::new();
    let taken = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = taken.local_addr().expect("local address").to_string();

    let start = sandbox.splitty(&["daemon", "start", "--addr", &addr]);
    assert_eq!(start.status.code(), Some(1), "{}", sandbox.describe(&start));
    assert!(
        stderr(&start).contains(&format!("Failed to bind {addr}: ")),
        "{}",
        sandbox.describe(&start),
    );
    assert_eq!(
        port_from(&start),
        None,
        "SPLITTY_PORT printed for a failed bind"
    );
    // `daemon start` opens daemon.log only to hand it to the daemon it spawns.
    assert!(
        !sandbox.log_path().exists(),
        "a daemon was spawned despite the bind failure"
    );
    assert!(
        !sandbox.info_path().exists(),
        "daemon.json written despite the bind failure"
    );
}

/// server-modes, Terminal closed after start: closing a terminal hangs up the
/// process group of the job it ran. The daemon has a session of its own, so it
/// survives both the shell that started it and that hangup.
#[test]
fn daemon_survives_the_terminal_it_was_started_from() {
    let sandbox = Sandbox::new();
    let mut shell = Command::new("/bin/sh");
    shell
        .args([
            "-c",
            "\"$0\" daemon start --addr 127.0.0.1:0",
            env!("CARGO_BIN_EXE_splitty"),
        ])
        // The shell's own process group stands in for the terminal's job.
        .process_group(0);
    let shell = sandbox
        .isolate(&mut shell)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let shell = shell.spawn().expect("spawn sh");
    let job = Pid::from_raw(shell.id() as i32);
    let out = shell.wait_with_output().expect("wait for sh");
    assert!(
        out.status.success(),
        "daemon start failed\n{}",
        sandbox.describe(&out)
    );

    // What the kernel sends the job when its terminal goes away. ESRCH, once
    // nothing is left in the group, is the expected outcome.
    let _ = killpg(job, Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(300));

    let list = sandbox.splitty(&["list-screens"]);
    assert!(
        list.status.success(),
        "daemon gone after the hangup\n{}",
        sandbox.describe(&list)
    );
    let status = sandbox.splitty(&["daemon", "status"]);
    assert!(
        stdout(&status).contains("Daemon running"),
        "{}",
        sandbox.describe(&status)
    );
}

/// server-modes, Control socket unavailable: the daemon still serves HTTP and
/// `daemon.json` names no socket.
#[test]
fn daemon_without_control_socket_still_serves_http() {
    let sandbox = Sandbox::new();
    // A regular file where the run directory should be, so the socket's
    // directory cannot be created.
    let blocker = sandbox.run_dir();
    fs::create_dir_all(blocker.parent().expect("parent")).expect("data dir");
    fs::write(&blocker, b"").expect("write blocker");

    let port = sandbox.start_daemon();
    let info = sandbox.read_info().expect("daemon.json after start");
    assert_eq!(info["socket"].as_str(), Some(""), "daemon.json: {info}");
    assert_eq!(sandbox.status_over_tcp(port)["serverMode"], "daemon");

    let stop = sandbox.splitty(&["daemon", "stop"]);
    assert!(
        stdout(&stop).contains("Daemon stopped"),
        "{}",
        sandbox.describe(&stop)
    );
}

/// server-modes, Daemon fails during startup, and cli, Daemon fails to start:
/// the command reports the failure, and neither a `daemon.json` nor a daemon
/// process is left.
#[test]
fn startup_failure_is_reported_and_leaves_nothing_behind() {
    let sandbox = Sandbox::new();
    // `daemon.json` is written through `daemon.tmp`; a directory in its place
    // makes that write fail. Making the config directory read-only would not:
    // its mode is repaired on every write, and root ignores it anyway.
    fs::create_dir_all(sandbox.config_dir().join("daemon.tmp")).expect("create blocker");
    let addr = free_loopback_addr();

    let start = sandbox.splitty(&["daemon", "start", "--addr", &addr]);
    assert_eq!(start.status.code(), Some(1), "{}", sandbox.describe(&start));
    let message = stderr(&start);
    assert!(
        message.contains("Failed to start daemon: ") && message.contains("daemon.log"),
        "{}",
        sandbox.describe(&start),
    );
    assert_eq!(
        port_from(&start),
        None,
        "SPLITTY_PORT printed for a daemon that never served"
    );
    assert!(
        sandbox.daemon_log().contains("daemon failed to start"),
        "reason missing from daemon.log"
    );

    assert!(
        !sandbox.info_path().exists(),
        "daemon.json left behind by a failed start"
    );
    let status = sandbox.splitty(&["daemon", "status"]);
    assert!(
        stdout(&status).starts_with("No daemon running"),
        "{}",
        sandbox.describe(&status)
    );
    // A surviving daemon process would still hold the listener.
    std::net::TcpListener::bind(&addr).expect("port still held after the failed start");
}

/// One HTTP/1.1 request on a fresh connection; returns the response body.
fn http_request(port: u16, request: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(READ_TIMEOUT)).ok();
    stream.write_all(request.as_bytes()).expect("send request");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default()
}

/// A masked client text frame, as RFC 6455 requires from clients.
fn masked_text_frame(text: &str) -> Vec<u8> {
    let payload = text.as_bytes();
    let mask = [0x12, 0x34, 0x56, 0x78];
    let len = u8::try_from(payload.len())
        .ok()
        .filter(|&n| n < 126)
        .expect("short frame");
    let mut frame = vec![0x81, 0x80 | len];
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(i, byte)| byte ^ mask[i % 4]),
    );
    frame
}

/// server-modes, Daemon Graceful Shutdown: `daemon stop` ends the daemon while
/// a client is still connected and streaming a session. The daemon used to
/// wait for that session's shell forever.
#[test]
fn daemon_stop_exits_while_a_client_streams_a_session() {
    use std::io::{Read, Write};

    let sandbox = Sandbox::new();
    let port = sandbox.start_daemon();
    let pid = sandbox
        .read_info()
        .and_then(|info| info["pid"].as_i64())
        .expect("daemon pid");
    let new_screen = sandbox.splitty(&["new-screen"]);
    assert!(
        new_screen.status.success(),
        "{}",
        sandbox.describe(&new_screen)
    );
    let screens: serde_json::Value =
        serde_json::from_slice(&sandbox.splitty(&["list-screens", "--json"]).stdout)
            .expect("screens");
    let screen = screens[0]["id"].as_str().expect("screen id").to_string();
    let panes: serde_json::Value = serde_json::from_slice(
        &sandbox
            .splitty(&["list-panes", "--screen", &screen, "--json"])
            .stdout,
    )
    .expect("panes");
    let session = panes
        .as_object()
        .and_then(|p| p.values().next())
        .and_then(|p| p["sessionId"].as_str())
        .expect("session id")
        .to_string();

    let origin = format!("http://127.0.0.1:{port}");
    let body = http_request(port, &format!(
        "POST /api/ws-ticket HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nAuthorization: Bearer {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        sandbox.token(),
    ));
    let ticket: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("ticket response {body:?}: {e}"));
    let ticket = ticket["ticket"].as_str().expect("ticket").to_string();

    let mut ws = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    ws.set_read_timeout(Some(READ_TIMEOUT)).ok();
    write!(ws, "GET /ws?ticket={ticket} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").expect("handshake");
    let mut buf = [0u8; 4096];
    let n = ws.read(&mut buf).expect("handshake response");
    assert!(
        String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 101"),
        "upgrade refused: {}",
        String::from_utf8_lossy(&buf[..n])
    );
    ws.write_all(&masked_text_frame(&format!(
        r#"{{"type":"attach","id":"{session}","cols":80,"rows":24}}"#
    )))
    .expect("attach");
    // Wait for the replay and `attached`, so the session is streamed when the
    // daemon is asked to stop.
    let mut received = Vec::new();
    while !String::from_utf8_lossy(&received).contains("\"attached\"") {
        match ws.read(&mut buf) {
            Ok(0) | Err(_) => panic!(
                "connection ended before `attached`: {}",
                String::from_utf8_lossy(&received)
            ),
            Ok(n) => received.extend_from_slice(&buf[..n]),
        }
    }

    let stop = sandbox.splitty(&["daemon", "stop"]);
    let stopped = stdout(&stop).contains(&format!("Daemon stopped (pid={pid})"));
    if !stopped {
        stop_process(pid);
    }
    assert!(
        stopped,
        "daemon did not exit while a client was connected\n{}",
        sandbox.describe(&stop)
    );
    drop(ws);
}

/// A window's view of a session: the daemon's WebSocket, authenticated with
/// the token as the UI would be, attached to `session`; its output and the
/// JSON events collect in the returned buffer.
fn stream_session(
    port: u16,
    token: &str,
    session: &str,
) -> std::sync::Arc<std::sync::Mutex<Vec<u8>>> {
    use std::io::{Read, Write};

    let origin = format!("http://127.0.0.1:{port}");
    let body = http_request(port, &format!(
        "POST /api/ws-ticket HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ));
    let ticket: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("ticket response {body:?}: {e}"));
    let ticket = ticket["ticket"].as_str().expect("ticket").to_string();

    let mut ws = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    write!(ws, "GET /ws?ticket={ticket} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").expect("handshake");
    // Byte by byte up to the blank line: the first frames may follow at once.
    let mut response = Vec::new();
    let mut byte = [0u8; 1];
    while !response.ends_with(b"\r\n\r\n") {
        ws.read_exact(&mut byte).expect("handshake response");
        response.push(byte[0]);
    }
    assert!(
        response.starts_with(b"HTTP/1.1 101"),
        "upgrade refused: {}",
        String::from_utf8_lossy(&response)
    );
    ws.write_all(&masked_text_frame(&format!(
        r#"{{"type":"attach","id":"{session}","cols":80,"rows":24}}"#
    )))
    .expect("attach");

    let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let collected = output.clone();
    let session = session.to_string();
    std::thread::spawn(move || {
        let mut read = |count: usize| {
            let mut bytes = vec![0u8; count];
            ws.read_exact(&mut bytes).ok().map(|()| bytes)
        };
        while let Some(header) = read(2) {
            let length = match header[1] & 0x7f {
                126 => read(2).map(|b| u64::from(u16::from_be_bytes([b[0], b[1]]))),
                127 => read(8)
                    .and_then(|b| b.try_into().ok())
                    .map(u64::from_be_bytes),
                short => Some(u64::from(short)),
            };
            let Some(payload) = length
                .and_then(|n| usize::try_from(n).ok())
                .and_then(&mut read)
            else {
                return;
            };
            // Text frames, the JSON events (`closed` among them), go to the
            // same buffer, each on a line of its own.
            if header[0] & 0x0f == 1 {
                let mut collected = collected.lock().expect("output");
                collected.push(b'\n');
                collected.extend_from_slice(&payload);
                collected.push(b'\n');
            }
            // Binary frames: [id length][session id][output].
            if header[0] & 0x0f == 2 {
                if let Some((&id_length, rest)) = payload.split_first() {
                    let id_length = usize::from(id_length);
                    if rest.get(..id_length) == Some(session.as_bytes()) {
                        collected
                            .lock()
                            .expect("output")
                            .extend_from_slice(&rest[id_length..]);
                    }
                }
            }
        }
    });
    output
}

fn contains(haystack: &std::sync::Mutex<Vec<u8>>, needle: &str) -> bool {
    String::from_utf8_lossy(&haystack.lock().expect("output")).contains(needle)
}

/// cli, Attach Command, Attach Server And Credential, Attach Detach Key:
/// `splitty attach` shows a daemon's session in a terminal while a window
/// streams it too; a typed line reaches the process, Ctrl+\ detaches, and the
/// session keeps running for the window.
// Smoke: only the real binary combines raw mode on its own terminal, the token
// file, the ticket and the daemon's WebSocket; the session loop, the targets
// and the terminal handling have their in-process tests in `cli::attach`.
#[test]
fn attach_shows_a_session_in_a_terminal_next_to_a_window() {
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    let sandbox = Sandbox::new();
    // cli, Attach Command, "No terminal": the terminal check comes first, so a
    // command without one (`ssh host splitty attach`) contacts nothing.
    let no_terminal = sandbox.splitty(&["attach", "3f9a"]);
    assert_eq!(
        no_terminal.status.code(),
        Some(1),
        "{}",
        sandbox.describe(&no_terminal)
    );
    assert!(stderr(&no_terminal).contains("Error: splitty attach needs a terminal"));
    assert!(
        !sandbox.config_dir().exists(),
        "nothing was read or created"
    );

    let port = sandbox.start_daemon();
    let pid = sandbox
        .read_info()
        .and_then(|info| info["pid"].as_i64())
        .expect("daemon pid");
    let new_screen = sandbox.splitty(&["new-screen"]);
    assert!(
        new_screen.status.success(),
        "{}",
        sandbox.describe(&new_screen)
    );
    let screens: serde_json::Value =
        serde_json::from_slice(&sandbox.splitty(&["list-screens", "--json"]).stdout)
            .expect("screens");
    let screen = screens[0]["id"].as_str().expect("screen id").to_string();
    let panes: serde_json::Value = serde_json::from_slice(
        &sandbox
            .splitty(&["list-panes", "--screen", &screen, "--json"])
            .stdout,
    )
    .expect("panes");
    let (pane, session) = panes
        .as_object()
        .and_then(|p| p.iter().next())
        .map(|(id, p)| {
            (
                id.clone(),
                p["sessionId"].as_str().expect("session id").to_string(),
            )
        })
        .expect("a pane");

    let window = stream_session(port, &sandbox.token(), &session);

    // The terminal: a PTY whose master side this test reads and types into.
    let pty = nix::pty::openpty(None, None).expect("openpty");
    let slave = File::from(pty.slave);
    let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
    command.args(["attach", &pane[..8]]);
    sandbox
        .isolate(&mut command)
        .stdin(slave.try_clone().expect("stdin"))
        .stdout(slave.try_clone().expect("stdout"))
        .stderr(slave);
    let mut attach = Reaped(command.spawn().expect("splitty attach"));
    let mut master = File::from(pty.master);
    let terminal = Arc::new(Mutex::new(Vec::new()));
    {
        let terminal = terminal.clone();
        let mut reader = master.try_clone().expect("master");
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    return;
                }
                terminal
                    .lock()
                    .expect("terminal")
                    .extend_from_slice(&buffer[..count]);
            }
        });
    }
    let wait = |what: &str, condition: &dyn Fn() -> bool| {
        let deadline = Instant::now() + READ_TIMEOUT;
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "{what}\nterminal: {:?}\nwindow: {:?}\n{}",
                String::from_utf8_lossy(&terminal.lock().expect("terminal")),
                String::from_utf8_lossy(&window.lock().expect("window")),
                sandbox.daemon_log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    // The replay shows the shell's prompt before anything is typed.
    wait("the terminal shows the session", &|| {
        !terminal.lock().expect("terminal").is_empty() && !window.lock().expect("window").is_empty()
    });
    master.write_all(b"echo attach-$((40+2))\r").expect("type");
    wait("the typed line reaches the process and both views", &|| {
        contains(&terminal, "attach-42") && contains(&window, "attach-42")
    });

    master.write_all(b"\x1c").expect("Ctrl+\\");
    let deadline = Instant::now() + READ_TIMEOUT;
    let status = loop {
        if let Some(status) = attach.0.try_wait().expect("attach status") {
            break status;
        }
        if Instant::now() >= deadline {
            panic!(
                "splitty attach did not detach\n{}",
                String::from_utf8_lossy(&terminal.lock().expect("terminal"))
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    wait("the closing line", &|| contains(&terminal, "[detached]"));
    assert!(status.success(), "detach exits 0: {status}");
    assert!(
        !contains(&terminal, &sandbox.token()),
        "the token never reaches the terminal"
    );

    // The session keeps running, and the window keeps receiving its output.
    let keys = sandbox.splitty(&[
        "send-keys",
        "--screen",
        &screen,
        "--pane",
        &pane,
        "echo still-$((1+1))\\n",
    ]);
    assert!(keys.status.success(), "{}", sandbox.describe(&keys));
    wait("the session still runs for the window", &|| {
        contains(&window, "still-2")
    });

    let stop = sandbox.splitty(&["daemon", "stop"]);
    if !stdout(&stop).contains("Daemon stopped") {
        stop_process(pid);
    }
}

/// A port that was free a moment ago.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .expect("free port")
}

fn json_of(out: &Output, sandbox: &Sandbox) -> serde_json::Value {
    assert!(out.status.success(), "{}", sandbox.describe(out));
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e})\n{}", sandbox.describe(out)))
}

/// What a client asks a host over SSH, run here without SSH: the real binary
/// shows the subcommand, its JSON and the daemon it starts.
#[test]
fn remote_handshake_reports_the_host_and_starts_its_daemon() {
    let sandbox = Sandbox::new();

    let idle = json_of(&sandbox.splitty(&["remote-handshake", "--json"]), &sandbox);
    assert_eq!(idle["handshake"], 1, "{idle}");
    assert_eq!(idle["server"]["protocol"], 1, "{idle}");
    assert_eq!(
        idle["server"]["version"],
        env!("CARGO_PKG_VERSION"),
        "{idle}"
    );
    assert!(
        idle["server"]["path"]
            .as_str()
            .is_some_and(|p| p.starts_with('/')),
        "{idle}"
    );
    assert_eq!(
        idle["daemon"],
        serde_json::json!({"running": false}),
        "{idle}"
    );
    assert_eq!(idle["screens"], serde_json::json!([]), "{idle}");

    let port = free_port();
    let started = json_of(
        &sandbox.splitty(&[
            "remote-handshake",
            "--json",
            "--ensure-daemon",
            "--port",
            &port.to_string(),
        ]),
        &sandbox,
    );
    let daemon = &started["daemon"];
    assert_eq!(daemon["running"], true, "{started}");
    assert_eq!(daemon["started"], true, "{started}");
    assert_eq!(daemon["port"].as_u64(), Some(u64::from(port)), "{started}");
    assert_eq!(daemon["protocol"], 1, "{started}");
    assert!(daemon["startedAt"].is_string(), "{started}");
    // remote-hosts, Remote Handshake, "Daemon that keeps its sessions".
    assert_eq!(daemon["keepsSessions"], true, "{started}");
    assert!(
        started.to_string().find("token").is_none(),
        "no credential: {started}"
    );

    // A running daemon is reported, never started again, and lists its screens.
    let screen = sandbox.splitty(&["new-screen", "--name", "work"]);
    assert!(screen.status.success(), "{}", sandbox.describe(&screen));
    let again = json_of(
        &sandbox.splitty(&[
            "remote-handshake",
            "--json",
            "--ensure-daemon",
            "--port",
            "1",
        ]),
        &sandbox,
    );
    assert_eq!(again["daemon"]["started"], false, "{again}");
    assert_eq!(again["daemon"]["pid"], daemon["pid"], "{again}");
    let screens = again["screens"].as_array().expect("screens");
    assert!(
        screens
            .iter()
            .any(|s| s["name"] == "work" && s["panes"].as_u64() >= Some(1)),
        "{again}"
    );

    // An info file from before the protocol field counts as protocol 1.
    let mut info = sandbox.read_info().expect("daemon.json");
    info.as_object_mut().expect("object").remove("protocol");
    fs::write(sandbox.info_path(), info.to_string()).expect("rewrite daemon.json");
    let older = json_of(&sandbox.splitty(&["remote-handshake", "--json"]), &sandbox);
    assert_eq!(older["daemon"]["protocol"], 1, "{older}");
}

/// Stops the daemon a configuration directory of its own names, after a test.
struct OwnDaemon(PathBuf);

impl Drop for OwnDaemon {
    fn drop(&mut self) {
        let info = fs::read_to_string(self.0.join("daemon.json")).ok();
        let info: Option<serde_json::Value> =
            info.and_then(|text| serde_json::from_str(&text).ok());
        if let Some(pid) = info.and_then(|info| info["pid"].as_i64()) {
            stop_process(pid);
        }
    }
}

/// The server a hub installed on a macOS host runs with `SPLITTY_CONFIG_DIR`,
/// so its daemon keeps apart from the desktop app's directory. A real detached
/// daemon and its info file show that; no in-process test starts one.
#[test]
fn a_configuration_directory_of_its_own_holds_a_separate_daemon() {
    let sandbox = Sandbox::new();
    let own = sandbox.home.path().join("own-config");
    let _guard = OwnDaemon(own.clone());
    let with_own = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
        command.args(args);
        sandbox
            .isolate(&mut command)
            .env("SPLITTY_CONFIG_DIR", &own)
            .output()
            .expect("run splitty with its own configuration directory")
    };

    let port = free_port();
    let started = json_of(
        &with_own(&[
            "remote-handshake",
            "--json",
            "--ensure-daemon",
            "--port",
            &port.to_string(),
        ]),
        &sandbox,
    );
    assert_eq!(started["daemon"]["started"], true, "{started}");

    let info: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(own.join("daemon.json")).expect("daemon.json"))
            .expect("daemon.json is JSON");
    assert_eq!(info["port"].as_u64(), Some(u64::from(port)), "{info}");
    assert!(
        info["socket"]
            .as_str()
            .is_some_and(|socket| socket.starts_with(own.join("run").to_str().expect("UTF-8"))),
        "the control socket lives below the directory: {info}"
    );
    assert!(own.join("auth-token").is_file(), "its own token");

    // The default directory knows nothing of it, and a server there would
    // start a daemon of its own.
    assert!(
        sandbox.read_info().is_none(),
        "no daemon.json in the default directory"
    );
    let default = json_of(&sandbox.splitty(&["daemon", "status", "--json"]), &sandbox);
    assert_eq!(default["running"], false, "{default}");
    let separate = json_of(&with_own(&["daemon", "status", "--json"]), &sandbox);
    assert_eq!(separate["running"], true, "{separate}");
    assert_eq!(separate["pid"], info["pid"], "{separate}");

    let stop = with_own(&["daemon", "stop"]);
    assert!(stop.status.success(), "{}", sandbox.describe(&stop));
    assert!(!own.join("daemon.json").exists(), "removed at shutdown");
}

#[test]
fn remote_handshake_reports_a_daemon_that_cannot_start() {
    let sandbox = Sandbox::new();
    let busy = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = busy.local_addr().expect("addr").port().to_string();
    let out = sandbox.splitty(&[
        "remote-handshake",
        "--json",
        "--ensure-daemon",
        "--port",
        &port,
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", sandbox.describe(&out));
    assert!(stdout(&out).is_empty(), "no JSON on failure");
    assert!(
        stderr(&out).contains(&format!("127.0.0.1:{port}")),
        "the reason names the port\n{}",
        sandbox.describe(&out)
    );
}

/// One frame of `splitty remote-bridge`: kind byte, big-endian length, payload.
fn bridge_frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![kind];
    frame.extend_from_slice(&u32::try_from(payload.len()).expect("length").to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn read_bridge_frame(reader: &mut impl std::io::Read) -> Option<(u8, Vec<u8>)> {
    let mut header = [0u8; 5];
    reader.read_exact(&mut header).ok()?;
    let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload).ok()?;
    Some((header[0], payload))
}

/// The bridge with its real stdio against a real daemon.
#[test]
fn remote_bridge_relays_the_daemons_websocket() {
    use std::io::Write;

    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let screen = sandbox.splitty(&["new-screen", "--name", "bridged"]);
    assert!(screen.status.success(), "{}", sandbox.describe(&screen));

    let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
    command.arg("remote-bridge");
    let mut bridge = sandbox
        .isolate(&mut command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the bridge");
    let mut stdin = bridge.stdin.take().expect("stdin");
    let stdout = bridge.stdout.take().expect("stdout");
    let (frames_tx, frames) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stdout = BufReader::new(stdout);
        while let Some(frame) = read_bridge_frame(&mut stdout) {
            if frames_tx.send(frame).is_err() {
                break;
            }
        }
    });
    let next_json = || -> serde_json::Value {
        let (kind, payload) = frames
            .recv_timeout(READ_TIMEOUT)
            .expect("a frame from the bridge");
        assert_eq!(kind, 1, "a text frame");
        serde_json::from_slice(&payload).expect("JSON message")
    };

    let welcome = next_json();
    assert_eq!(welcome["type"], "mux.welcome", "{welcome}");
    let screen_id = welcome["screens"]
        .as_array()
        .and_then(|screens| screens.iter().find(|s| s["name"] == "bridged"))
        .and_then(|s| s["id"].as_str())
        .unwrap_or_else(|| panic!("the welcome lists the screen: {welcome}"))
        .to_owned();

    let resync = serde_json::json!({"type": "mux.resync", "screenId": screen_id}).to_string();
    stdin
        .write_all(&bridge_frame(1, resync.as_bytes()))
        .expect("send resync");
    stdin.flush().expect("flush");
    let snapshot = loop {
        let message = next_json();
        if message["type"] == "mux.screen_snapshot" {
            break message;
        }
    };
    assert!(snapshot.to_string().contains(&screen_id), "{snapshot}");

    // The end of its input ends the bridge cleanly.
    drop(stdin);
    let deadline = Instant::now() + READ_TIMEOUT;
    let status = loop {
        if let Some(status) = bridge.try_wait().expect("wait") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the bridge did not exit at the end of its input"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "bridge exit status {status}");
}

#[test]
fn remote_bridge_without_a_daemon_fails() {
    let sandbox = Sandbox::new();
    let out = sandbox.splitty(&["remote-bridge"]);
    assert_eq!(out.status.code(), Some(1), "{}", sandbox.describe(&out));
    assert!(
        stderr(&out).contains("No Splitty daemon is running"),
        "{}",
        sandbox.describe(&out)
    );
}

impl Sandbox {
    /// A new screen's shell: its screen, pane and session ids.
    fn shell(&self) -> (String, String, String) {
        let new_screen = self.splitty(&["new-screen"]);
        assert!(
            new_screen.status.success(),
            "{}",
            self.describe(&new_screen)
        );
        let screens = self.json(&["list-screens", "--json"]);
        let screen = screens
            .as_array()
            .and_then(|screens| screens.last())
            .and_then(|screen| screen["id"].as_str())
            .expect("screen id")
            .to_string();
        let panes = self.json(&["list-panes", "--screen", &screen, "--json"]);
        let (pane, session) = panes
            .as_object()
            .and_then(|panes| panes.iter().next())
            .map(|(id, pane)| {
                (
                    id.clone(),
                    pane["sessionId"].as_str().expect("session").to_string(),
                )
            })
            .expect("a pane");
        (screen, pane, session)
    }

    /// Type a line into a pane's shell.
    fn type_line(&self, screen: &str, pane: &str, line: &str) {
        let keys = format!("{line}\\n");
        let out = self.splitty(&["send-keys", "--screen", screen, "--pane", pane, &keys]);
        assert!(out.status.success(), "{}", self.describe(&out));
    }

    fn path(&self, name: &str) -> PathBuf {
        self.home.path().join(name)
    }

    /// The trimmed contents of a file a shell writes, once it is there.
    fn wait_for(&self, name: &str) -> String {
        let path = self.path(name);
        let deadline = Instant::now() + READ_TIMEOUT;
        loop {
            if let Ok(text) = fs::read_to_string(&path) {
                if text.ends_with('\n') {
                    return text.trim().to_string();
                }
            }
            assert!(
                Instant::now() < deadline,
                "{} never appeared\n{}",
                path.display(),
                self.daemon_log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn keeper_sockets(&self) -> usize {
        fs::read_dir(self.run_dir().join("keepers"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "sock"))
                    .count()
            })
            .unwrap_or(0)
    }
}

/// A child process that a failing test must not leave running.
struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn alive(pid: &str) -> bool {
    pid.parse::<i32>()
        .ok()
        .filter(|pid| *pid > 0)
        .is_some_and(|pid| kill(Pid::from_raw(pid), None).is_ok())
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + READ_TIMEOUT;
    while !condition() {
        assert!(Instant::now() < deadline, "gave up waiting: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// session-keepers, Adoption On Daemon Start; server-modes, Daemon Graceful
/// Shutdown, Session Respawn On Daemon Start, Session Lifetime Across Modes;
/// terminal-sessions, Session Lifetime Independent of Connections; cli, Daemon
/// Stop and Status Commands, Daemon Restart Command; access-control, Owner-Only
/// Control Socket and Owner-Only Keeper Sockets: a pane's shell outlives
/// `daemon stop`, a restart, `daemon restart` and SIGKILL with its PID and
/// output, and `--end-sessions` ends it.
// Smoke: only real processes show a keeper outliving the daemon process.
#[test]
fn a_shell_outlives_stop_restart_update_and_crash_of_its_daemon() {
    let sandbox = Sandbox::new();
    let port = sandbox.start_daemon();
    let (screen, pane, session) = sandbox.shell();
    let home = sandbox.home.path().display().to_string();
    sandbox.type_line(
        &screen,
        &pane,
        &format!("echo before-$((40+2)); echo $$ > {home}/pid1; env > {home}/env1"),
    );
    let pid = sandbox.wait_for("pid1");
    assert!(alive(&pid));
    assert_eq!(sandbox.keeper_sockets(), 1, "a keeper serves the shell");
    let env = sandbox.wait_for("env1");
    assert!(!env.contains("keepers"), "Pane environment: {env}");

    // "Stop keeps the sessions", "Kept sessions without a daemon".
    let stop = sandbox.splitty(&["daemon", "stop"]);
    assert!(
        stdout(&stop).contains("Daemon stopped"),
        "{}",
        sandbox.describe(&stop)
    );
    assert!(alive(&pid), "the shell outlives the daemon");
    let status = sandbox.splitty(&["daemon", "status"]);
    assert!(
        stdout(&status).contains("No daemon running"),
        "{}",
        stdout(&status)
    );
    assert!(
        stdout(&status).contains("1 session(s) kept"),
        "{}",
        stdout(&status)
    );

    // "Restart keeps the process and its output", "Adopted shell is not
    // respawned", "The daemon's socket survives a restart".
    let port2 = sandbox.start_daemon();
    let window = stream_session(port2, &sandbox.token(), &session);
    wait_until("the replay holds the output from before", || {
        contains(&window, "before-42")
    });
    sandbox.type_line(
        &screen,
        &pane,
        &format!("echo $$ > {home}/pid2; splitty list-screens > {home}/screens 2>&1"),
    );
    assert_eq!(sandbox.wait_for("pid2"), pid, "the same shell");
    assert!(
        sandbox.wait_for("screens").contains(&screen),
        "SPLITTY_SOCK reaches the new daemon"
    );
    assert_eq!(sandbox.keeper_sockets(), 1, "no second shell");
    let _ = port;

    // "Update without ending sessions".
    let restart = sandbox.splitty(&["daemon", "restart"]);
    assert!(restart.status.success(), "{}", sandbox.describe(&restart));
    assert_eq!(port_from(&restart), Some(port2), "the same port");
    sandbox.type_line(&screen, &pane, &format!("echo $$ > {home}/pid3"));
    assert_eq!(sandbox.wait_for("pid3"), pid);

    // "Daemon killed".
    let daemon = sandbox
        .read_info()
        .and_then(|info| info["pid"].as_i64())
        .expect("daemon pid");
    let _ = kill(
        Pid::from_raw(i32::try_from(daemon).expect("pid")),
        Signal::SIGKILL,
    );
    wait_until("the daemon is gone", || {
        kill(Pid::from_raw(i32::try_from(daemon).expect("pid")), None).is_err()
    });
    assert!(alive(&pid), "the shell outlives a crash");
    sandbox.start_daemon();
    sandbox.type_line(&screen, &pane, &format!("echo $$ > {home}/pid4"));
    assert_eq!(sandbox.wait_for("pid4"), pid);

    // "Stop and end the sessions".
    let end = sandbox.splitty(&["daemon", "stop", "--end-sessions"]);
    assert!(
        stdout(&end).contains("Ended 1 kept session(s)"),
        "{}",
        sandbox.describe(&end)
    );
    wait_until("the shell ends", || !alive(&pid));
    assert_eq!(sandbox.keeper_sockets(), 0);
}

/// session-keepers, Exit Results Survive The Daemon: a program that exits while
/// no daemon runs reports its code to the next daemon's clients.
// Smoke: the exit happens between two daemon processes.
#[test]
fn an_exit_while_no_daemon_runs_reaches_the_next_daemon() {
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let (screen, pane, session) = sandbox.shell();
    sandbox.type_line(&screen, &pane, "sleep 1; echo last-words; exit 3");
    let stop = sandbox.splitty(&["daemon", "stop"]);
    assert!(
        stdout(&stop).contains("Daemon stopped"),
        "{}",
        sandbox.describe(&stop)
    );
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        sandbox.keeper_sockets(),
        1,
        "the keeper waits with the result"
    );

    let port = sandbox.start_daemon();
    let window = stream_session(port, &sandbox.token(), &session);
    wait_until("the final output and the exit code", || {
        contains(&window, "last-words") && contains(&window, "\"exitCode\":3")
    });
    wait_until("the acknowledged keeper leaves", || {
        sandbox.keeper_sockets() == 0
    });
}

/// session-keepers, Sessions Without A Pane: a keeper the workspace does not
/// know is adopted and reachable by its session id.
// Smoke: the workspace file goes between two daemon processes.
#[test]
fn a_keeper_the_workspace_lost_is_still_reachable() {
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let (screen, pane, session) = sandbox.shell();
    sandbox.type_line(&screen, &pane, "echo orphan-$((6*7))");
    std::thread::sleep(Duration::from_millis(500));
    assert!(sandbox.splitty(&["daemon", "stop"]).status.success());
    fs::remove_file(sandbox.config_dir().join("state.json")).expect("lose the workspace");

    let port = sandbox.start_daemon();
    let screens = sandbox.json(&["list-screens", "--json"]);
    assert!(
        screens.as_array().is_some_and(|screens| screens
            .iter()
            .all(|s| s["id"].as_str() != Some(screen.as_str()))),
        "the lost screen is not restored: {screens}"
    );
    let window = stream_session(port, &sandbox.token(), &session);
    wait_until("the orphan's replay", || contains(&window, "orphan-42"));
}

/// server-modes, Grant And Client Lifetimes Are Independent; control-api, Job
/// Log Responses: a job outlives a daemon restart under trusted
/// administration, with its retained output readable.
// Smoke: the job's process lives across two daemon processes.
#[test]
fn a_job_outlives_a_daemon_restart() {
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let cwd = sandbox.home.path().to_string_lossy().into_owned();
    let job = sandbox.json(&[
        "run",
        "--background",
        "--name",
        "dev",
        "--cwd",
        &cwd,
        "--json",
        "--",
        "/bin/sh",
        "-c",
        "printf ready; sleep 100",
    ]);
    let id = job["jobId"].as_str().expect("job id").to_string();
    wait_until("the job prints", || {
        sandbox.json(&["logs", &id, "--json"])["data"]
            .as_str()
            .is_some_and(|data| data.contains("ready"))
    });
    let old_cursor = sandbox.json(&["logs", &id, "--json"])["nextCursor"].clone();

    let restart = sandbox.splitty(&["daemon", "restart"]);
    assert!(restart.status.success(), "{}", sandbox.describe(&restart));
    assert_eq!(
        sandbox.json(&["inspect", &id, "--json"])["state"],
        "running"
    );
    let logs = sandbox.json(&["logs", &id, "--json"]);
    assert_eq!(logs["outputAvailable"], true, "{logs}");
    assert!(
        logs["data"]
            .as_str()
            .is_some_and(|data| data.contains("ready")),
        "{logs}"
    );
    if let Some(cursor) = old_cursor.as_str() {
        let stale = sandbox.splitty(&["logs", &id, "--cursor", cursor, "--json"]);
        assert!(
            !stale.status.success(),
            "a cursor from before the restart is invalid"
        );
    }
    let stop = sandbox.json(&["stop", &id, "--json"]);
    assert_ne!(stop["state"], "running", "{stop}");
}

/// cli, Attach Without A Daemon: with the daemon stopped, a terminal controls
/// the keeper directly, a second one is refused, and a daemon start takes
/// over without the terminal showing the output again.
// Smoke: only real processes show the handover between a keeper and a
// starting daemon.
#[test]
fn attach_without_a_daemon_hands_over_to_a_starting_daemon() {
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let (screen, pane, _) = sandbox.shell();
    sandbox.type_line(&screen, &pane, "echo before-$((40+2))");
    std::thread::sleep(Duration::from_millis(500));
    assert!(sandbox.splitty(&["daemon", "stop"]).status.success());

    let pty = nix::pty::openpty(None, None).expect("openpty");
    let slave = File::from(pty.slave);
    let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
    command.args(["attach", &pane]);
    sandbox
        .isolate(&mut command)
        .stdin(slave.try_clone().expect("stdin"))
        .stdout(slave.try_clone().expect("stdout"))
        .stderr(slave);
    let mut attach = Reaped(command.spawn().expect("splitty attach"));
    let mut master = File::from(pty.master);
    let terminal = Arc::new(Mutex::new(Vec::new()));
    {
        let terminal = terminal.clone();
        let mut reader = master.try_clone().expect("master");
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    return;
                }
                terminal
                    .lock()
                    .expect("terminal")
                    .extend_from_slice(&buffer[..count]);
            }
        });
    }
    wait_until("the replay without a daemon", || {
        contains(&terminal, "before-42")
    });
    master.write_all(b"echo kept-$((2+3))\r").expect("type");
    wait_until("typing reaches the keeper's shell", || {
        contains(&terminal, "kept-5")
    });

    // "Controlled by another terminal".
    let pty2 = nix::pty::openpty(None, None).expect("openpty");
    let slave2 = File::from(pty2.slave);
    // macOS discards unread output when a terminal's last slave closes.
    let _open = slave2.try_clone().expect("keep the terminal open");
    let mut second = Command::new(env!("CARGO_BIN_EXE_splitty"));
    second.args(["attach", &pane]);
    sandbox
        .isolate(&mut second)
        .stdin(slave2.try_clone().expect("stdin"))
        .stdout(slave2.try_clone().expect("stdout"))
        .stderr(slave2);
    // Its terminal is read all along, as a terminal emulator would.
    let message = Arc::new(Mutex::new(Vec::new()));
    {
        let message = message.clone();
        let mut reader = File::from(pty2.master);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    return;
                }
                message
                    .lock()
                    .expect("message")
                    .extend_from_slice(&buffer[..count]);
            }
        });
    }
    let refused = second.output().expect("second attach");
    assert_eq!(refused.status.code(), Some(1));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !contains(&message, "another client controls the session") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        contains(&message, "another client controls the session"),
        "terminal: {:?}\nstdout: {:?}\nstderr: {:?}",
        String::from_utf8_lossy(&message.lock().expect("message")),
        stdout(&refused),
        stderr(&refused)
    );

    // "The daemon starts while attached".
    let mark = terminal.lock().expect("terminal").len();
    sandbox.start_daemon();
    std::thread::sleep(Duration::from_secs(2));
    master
        .write_all(b"echo via-daemon-$((3+4))\r")
        .expect("type");
    wait_until("typing reaches the shell through the daemon", || {
        contains(&terminal, "via-daemon-7")
    });
    let after = String::from_utf8_lossy(&terminal.lock().expect("terminal")[mark..]).into_owned();
    assert!(
        !after.contains("before-42"),
        "no repeated output: {after:?}"
    );

    master.write_all(b"\x1c").expect("Ctrl+\\");
    let deadline = Instant::now() + READ_TIMEOUT;
    let status = loop {
        if let Some(status) = attach.0.try_wait().expect("status") {
            break status;
        }
        assert!(Instant::now() < deadline, "attach did not detach");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success());
}
