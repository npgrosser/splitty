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
        let runtime_dir = home
            .path()
            .canonicalize()
            .expect("canonical fixture")
            .join("run");
        Self { home, runtime_dir }
    }

    /// Common umask-0002 setup: existing shared data/config parents, while
    /// Splitty owns private subdirectories. Never repair these parents.
    fn shared_parents(&self) {
        use std::os::unix::fs::PermissionsExt;
        for parent in [
            self.config_dir().parent().unwrap(),
            self.run_dir().parent().unwrap().parent().unwrap(),
        ] {
            fs::create_dir_all(parent).unwrap();
            fs::set_permissions(parent, fs::Permissions::from_mode(0o775)).unwrap();
        }
    }

    fn assert_shared_parents_unchanged(&self) {
        use std::os::unix::fs::PermissionsExt;
        for parent in [
            self.config_dir().parent().unwrap(),
            self.run_dir().parent().unwrap().parent().unwrap(),
        ] {
            assert_eq!(
                fs::metadata(parent).unwrap().permissions().mode() & 0o777,
                0o775
            );
        }
    }

    /// Point a command at this sandbox instead of the invoking user's setup.
    fn isolate<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        cmd.env(
            "HOME",
            self.home.path().canonicalize().expect("canonical home"),
        )
        .env(
            "XDG_CONFIG_HOME",
            self.home.path().canonicalize().unwrap().join("config"),
        )
        .env(
            "XDG_DATA_HOME",
            self.home.path().canonicalize().unwrap().join("data"),
        )
        .env("XDG_RUNTIME_DIR", &self.runtime_dir)
        // Run from inside a Splitty pane, the tests would otherwise drive
        // the developer's own server.
        .env_remove(splitty_core::config::CONFIG_DIR_VARIABLE)
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
            self.home
                .path()
                .canonicalize()
                .unwrap()
                .join("Library/Application Support/splitty")
        } else {
            self.home
                .path()
                .canonicalize()
                .unwrap()
                .join("config/splitty")
        }
    }

    /// Where the daemon's control socket and its keepers live: below the
    /// data directory, which `dirs::data_dir()` takes from `XDG_DATA_HOME` on
    /// Linux only.
    fn run_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home
                .path()
                .canonicalize()
                .unwrap()
                .join("Library/Application Support/splitty/run")
        } else {
            self.home
                .path()
                .canonicalize()
                .unwrap()
                .join("data/splitty/run")
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

    fn describe(&self, out: &Output) -> String {
        format!(
            "status: {}\nstdout:\n{}\nstderr:\n{}\ndaemon.log:\n{}",
            out.status,
            stdout(out),
            stderr(out),
            self.daemon_log()
        )
    }

    fn start_daemon(&self) {
        let start = self.splitty(&["daemon", "start"]);
        assert!(start.status.success(), "{}", self.describe(&start));
        let info = self.read_info().expect("daemon discovery after readiness");
        assert_eq!(
            stdout(&start).trim(),
            format!(
                "SPLITTY_CLIENT_SOCKET={}\nSPLITTY_INSTANCE={}",
                info["clientSocket"].as_str().unwrap(),
                info["instance"].as_str().unwrap()
            )
        );
        assert!(
            !self.config_dir().join("auth-token").exists(),
            "native startup never resolves a browser token"
        );
    }

    fn native(&self, operation: splitty_server::service::catalog::Operation) -> serde_json::Value {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let info = serde_json::from_value(self.read_info().expect("native daemon")).unwrap();
            let mut client = splitty_server::native::client::Connector::default()
                .local(&info)
                .await
                .expect("verified native connect");
            let reply = client
                .request(operation, serde_json::Map::new())
                .await
                .expect("native reply");
            assert_eq!(reply.status, 200, "{}", reply.body);
            client.shutdown().await;
            serde_json::from_str(&reply.body).expect("reply JSON")
        })
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
        // cli, Scoped Administrative Commands Are Refused: an agent grant
        // never reaches a terminal.
        vec!["attach", "3f9a"],
        vec!["daemon", "stop"],
        vec![
            "desktop-backend",
            "--transport",
            "native-v1",
            "--backend",
            "embedded",
        ],
        vec!["remote-bridge", "--transport", "native-v1"],
        vec!["remote-handshake", "--json"],
        vec!["session-keeper"],
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
        .args(["daemon", "start"]);
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
        .args(["daemon", "start"]);
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
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    sandbox.shared_parents();
    // Only the real start command proves repair happens before discovery,
    // instance-lock acquisition and the detached child's startup.
    fs::create_dir_all(sandbox.config_dir()).unwrap();
    fs::set_permissions(sandbox.config_dir(), fs::Permissions::from_mode(0o755)).unwrap();

    let token_path = sandbox.config_dir().join("auth-token");
    fs::write(&token_path, b"unused-token-fixture").unwrap();
    fs::set_permissions(&token_path, fs::Permissions::from_mode(0o644)).unwrap();

    let start = sandbox.splitty(&["daemon", "start"]);
    assert!(
        start.status.success(),
        "daemon start failed\n{}",
        sandbox.describe(&start)
    );
    assert_eq!(
        fs::metadata(sandbox.config_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(fs::read(&token_path).unwrap(), b"unused-token-fixture");
    assert_eq!(
        fs::metadata(&token_path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    let info = sandbox
        .read_info()
        .unwrap_or_else(|| panic!("no daemon.json after start\n{}", sandbox.describe(&start)));
    let pid = info["pid"].as_i64().expect("pid in daemon.json");
    assert_no_tcp_listener(pid as u32);
    assert_eq!(info["format"], 2);
    assert_eq!(info["transportVersion"], 1);
    assert!(info.get("port").is_none());
    let client_socket = PathBuf::from(info["clientSocket"].as_str().unwrap());
    assert_eq!(
        stdout(&start).trim(),
        format!(
            "SPLITTY_CLIENT_SOCKET={}\nSPLITTY_INSTANCE={}",
            client_socket.display(),
            info["instance"].as_str().unwrap()
        )
    );
    assert!(client_socket.exists());
    assert_eq!(
        sandbox.native(splitty_server::service::catalog::Operation::MuxStatus)["serverMode"],
        "daemon"
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
    assert_eq!(json["clientSocket"], info["clientSocket"]);
    assert!(json.get("port").is_none());
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
    assert!(
        !client_socket.exists(),
        "native socket left behind after stop"
    );

    let status = sandbox.splitty(&["daemon", "status", "--json"]);
    assert!(status.status.success(), "{}", sandbox.describe(&status));
    assert_eq!(stdout(&status).trim(), r#"{"running":false}"#);

    // Dead discovery is removed only under the native instance lock.
    let mut stale = info.clone();
    stale["pid"] = serde_json::json!(i32::MAX);
    for args in [
        vec!["daemon", "status"],
        vec!["daemon", "status", "--json"],
        vec!["daemon", "stop"],
    ] {
        fs::write(sandbox.info_path(), serde_json::to_vec(&stale).unwrap()).unwrap();
        let absent = sandbox.splitty(&args);
        assert!(absent.status.success(), "{}", sandbox.describe(&absent));
        assert!(
            !sandbox.info_path().exists(),
            "verified dead publication is removed"
        );
        if args.contains(&"--json") {
            assert_eq!(stdout(&absent).trim(), r#"{"running":false}"#);
        } else {
            assert!(stdout(&absent).contains("stale info file cleaned up"));
        }
    }

    sandbox.assert_shared_parents_unchanged();
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

#[path = "support/listeners.rs"]
mod process_inventory;

/// A real process FD inventory, rather than an assertion about bind helpers.
fn assert_no_tcp_listener(pid: u32) {
    assert!(
        process_inventory::tcp_listeners(pid).is_empty(),
        "native process {pid} has a TCP listener"
    );
}

#[test]
fn native_discovery_loss_corruption_and_concurrent_start_cannot_replace_a_live_daemon() {
    let sandbox = Sandbox::new();
    // A real held instance lock models another launcher before publication.
    // Remote discovery must classify contention as retryable, never replace it.
    let starting = splitty_server::native::endpoint::InstanceLock::acquire(&sandbox.run_dir())
        .expect("hold the unpublished daemon instance");
    let handshake = sandbox.splitty(&["remote-handshake", "--json", "--ensure-daemon"]);
    assert_eq!(
        handshake.status.code(),
        Some(75),
        "{}",
        sandbox.describe(&handshake)
    );
    assert!(!sandbox.info_path().exists());
    assert!(!sandbox.run_dir().join("client.sock").exists());
    drop(starting);
    let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
    command.args(["daemon", "start"]);
    let first = sandbox
        .isolate(&mut command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let second = sandbox.splitty(&["daemon", "start"]);
    let first = first.wait_with_output().unwrap();
    assert!(
        first.status.success() || second.status.success(),
        "{}\n{}",
        sandbox.describe(&first),
        sandbox.describe(&second)
    );
    for started in [&first, &second] {
        assert!(
            started.status.success() || started.status.code() == Some(75),
            "concurrent start must succeed or request a discovery refresh: {}",
            sandbox.describe(started)
        );
    }
    let record = fs::read(sandbox.info_path()).unwrap();
    let info: splitty_core::daemon::DaemonInfo = serde_json::from_slice(&record).unwrap();
    assert_no_tcp_listener(info.pid);
    for corrupt in [Some(b"{broken".as_slice()), None] {
        if let Some(bytes) = corrupt {
            fs::write(sandbox.info_path(), bytes).unwrap();
        } else {
            fs::remove_file(sandbox.info_path()).unwrap();
        }
        let handshake = sandbox.splitty(&["remote-handshake", "--json", "--ensure-daemon"]);
        assert_eq!(handshake.status.code(), Some(if corrupt.is_some() { 1 } else { 75 }),
            "corrupt discovery remains fatal; missing publication with a live lock is retryable: {}", sandbox.describe(&handshake));
        let again = sandbox.splitty(&["daemon", "start"]);
        assert!(
            !again.status.success(),
            "must not create a replacement: {}",
            sandbox.describe(&again)
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let connector = splitty_server::native::client::Connector::default();
            let mut client = connector
                .local(&info)
                .await
                .expect("original service still owns endpoint");
            assert_eq!(
                client
                    .request(
                        splitty_server::service::catalog::Operation::MuxStatus,
                        serde_json::Map::new()
                    )
                    .await
                    .unwrap()
                    .status,
                200
            );
            client.shutdown().await;
            let mut stale = info.clone();
            stale.instance = Some("0".repeat(32));
            assert!(
                connector.local(&stale).await.is_err(),
                "stale discovery cannot admit a client"
            );
        });
        fs::write(sandbox.info_path(), &record).unwrap();
    }
    assert_eq!(
        sandbox.json(&["daemon", "status", "--json"])["pid"],
        info.pid
    );
    // A status/start process that saw a dead PID must respect the instance
    // still holding its native lock, even before a fresh publication appears.
    let mut stale = info.clone();
    stale.pid = i32::MAX as u32;
    let stale_record = serde_json::to_vec(&stale).unwrap();
    fs::write(sandbox.info_path(), &stale_record).unwrap();
    let status = sandbox.splitty(&["daemon", "status", "--json"]);
    let after_status = fs::read(sandbox.info_path()).unwrap();
    let start = sandbox.splitty(&["daemon", "start"]);
    let after_start = fs::read(sandbox.info_path()).unwrap();
    fs::write(sandbox.info_path(), &record).unwrap();
    assert!(status.status.success(), "{}", sandbox.describe(&status));
    assert_eq!(
        after_status, stale_record,
        "status must not unlink a lock holder's publication"
    );
    assert!(!start.status.success(), "a live lock blocks replacement");
    assert_eq!(
        after_start, stale_record,
        "start must acquire the instance lock before replacing stale publication"
    );
}

#[test]
fn a_live_legacy_record_is_diagnostic_and_never_selects_its_tcp_port() {
    let sandbox = Sandbox::new();
    let legacy = ChildGuard(Command::new("/bin/sleep").arg("60").spawn().unwrap());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let directory = sandbox.config_dir();
    splitty_core::config::PrivateDir::open(&directory, true).unwrap();
    fs::write(sandbox.info_path(), serde_json::to_vec(&serde_json::json!({
        "pid":legacy.0.id(), "port":listener.local_addr().unwrap().port(), "socket":"", "startedAt":"1", "version":"0.0.1", "token":"old-secret"
    })).unwrap()).unwrap();
    let status = sandbox.json(&["daemon", "status", "--json"]);
    assert_eq!(status["legacy"], true);
    assert_eq!(status["running"], true);
    for key in ["clientSocket", "instance", "transportVersion", "token"] {
        assert!(status.get(key).is_none());
    }
    let start = sandbox.splitty(&["daemon", "start"]);
    assert!(start.status.success());
    assert!(stdout(&start).contains("legacy"));
    let restart = sandbox.splitty(&["daemon", "restart"]);
    assert!(!restart.status.success());
    assert!(stderr(&restart).contains("--end-sessions"));
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    // The legacy process has no native instance lock. Corruption must not
    // allow restart to infer absence and overwrite its discovery record.
    fs::write(sandbox.info_path(), b"{broken").unwrap();
    for args in [
        vec!["daemon", "status"],
        vec!["daemon", "status", "--json"],
        vec!["daemon", "restart"],
        vec!["daemon", "restart", "--end-sessions"],
        vec!["daemon", "stop", "--end-sessions"],
    ] {
        let refused = sandbox.splitty(&args);
        assert!(!refused.status.success(), "{}", sandbox.describe(&refused));
        assert!(stderr(&refused).contains("cannot verify"));
        assert_eq!(fs::read(sandbox.info_path()).unwrap(), b"{broken");
        assert!(splitty_core::daemon::is_running(legacy.0.id()));
        assert!(!sandbox.run_dir().join("client.sock").exists());
    }
    fs::remove_file(sandbox.info_path()).unwrap();
}

// Smoke: inherited pipes and launcher death are OS lifetimes; an in-process
// stream cannot prove cleanup of a real backend and its PTY child.
#[test]
fn owned_native_child_pipes_end_only_their_embedded_sessions() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let (screen, pane, session) = sandbox.shell();
    assert_no_tcp_listener(
        sandbox.read_info().unwrap()["pid"]
            .as_u64()
            .unwrap()
            .try_into()
            .unwrap(),
    );
    let home = sandbox.home.path().display().to_string();
    sandbox.type_line(&screen, &pane, &format!("echo $$ > {home}/persistent-pid"));
    let persistent = sandbox.wait_for("persistent-pid");
    let record = fs::read(sandbox.info_path()).unwrap();
    // The independent daemon observes its own shell after screen creation.
    // Snapshot only after that metadata is persisted, so its background write
    // cannot be mistaken for an embedded helper changing the workspace.
    let state_path = sandbox.config_dir().join("state.json");
    let deadline = Instant::now() + READ_TIMEOUT;
    let state = loop {
        let bytes = fs::read(&state_path).unwrap();
        let workspace: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let entry = &workspace["screens"][&screen]["sessions"][&session];
        if entry["cwd"].as_str().is_some_and(|cwd| !cwd.is_empty())
            && entry["busy"] == false
            && entry["displayTitle"]
                .as_str()
                .is_some_and(|title| title != "Terminal")
        {
            break bytes;
        }
        assert!(
            Instant::now() < deadline,
            "daemon shell metadata was not persisted"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    struct Restore<'a>(&'a Sandbox, Vec<u8>);
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            let _ = fs::write(self.0.info_path(), &self.1);
        }
    }
    let _restore = Restore(&sandbox, record.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for mode in [
        "eof",
        "broken-output",
        "launcher-death",
        "eof-missing",
        "eof-stale",
        "eof-legacy",
        "eof-unreachable",
        "closed-pane",
        "blocked-eof",
        "blocked-output",
    ] {
        let mut discovery: serde_json::Value = serde_json::from_slice(&record).unwrap();
        let expected = match mode {
            "broken-output" => Some(b"{broken".to_vec()),
            "launcher-death" => {
                discovery["transportVersion"] = serde_json::json!(999);
                Some(serde_json::to_vec(&discovery).unwrap())
            }
            "eof-missing" => None,
            "eof-stale" => {
                discovery["pid"] = serde_json::json!(i32::MAX);
                Some(serde_json::to_vec(&discovery).unwrap())
            }
            "eof-unreachable" => {
                discovery["clientSocket"] =
                    serde_json::json!(sandbox.run_dir().join("missing.sock"));
                Some(serde_json::to_vec(&discovery).unwrap())
            }
            "eof-legacy" => {
                let object = discovery.as_object_mut().unwrap();
                for key in ["format", "clientSocket", "instance", "transportVersion"] {
                    object.remove(key);
                }
                object.insert("port".into(), serde_json::json!(9800));
                Some(serde_json::to_vec(&discovery).unwrap())
            }
            _ => Some(record.clone()),
        };
        match &expected {
            Some(bytes) => fs::write(sandbox.info_path(), bytes).unwrap(),
            None => fs::remove_file(sandbox.info_path()).unwrap(),
        }
        runtime.block_on(async {
            use splitty_transport::{budget::ConnectionBudget, codec::{self, Frame}, negotiation, wire::{self, Kind}};
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let pid_file = sandbox.home.path().join(format!("owned-{mode}"));
            let helper_pid_file = sandbox.home.path().join(format!("helper-{mode}"));
            let mut command = if mode == "launcher-death" {
                let mut command = Command::new("python3");
                command.args(["-c", r#"import os, subprocess, sys
p = subprocess.Popen(sys.argv[1:], stdin=subprocess.PIPE, stdout=sys.stdout.buffer, stderr=sys.stderr.buffer, close_fds=True)
with open(os.environ['HELPER_PID_FILE'], 'w') as f: f.write(str(p.pid))
while True:
    data = os.read(0, 65536)
    if not data: break
    p.stdin.write(data); p.stdin.flush()
p.stdin.close()
sys.exit(p.wait())
"#, env!("CARGO_BIN_EXE_splitty")]);
                command.env("HELPER_PID_FILE", &helper_pid_file); command
            } else { Command::new(env!("CARGO_BIN_EXE_splitty")) };
            command.args(["desktop-backend", "--transport", "native-v1", "--backend", "embedded"]);
            sandbox.isolate(&mut command).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = tokio::process::Command::from(command).kill_on_drop(true).spawn().unwrap();
            let mut input = child.stdin.take().unwrap(); let mut output = child.stdout.take().unwrap();
            let mut stderr_pipe = child.stderr.take().unwrap().take(4097);
            let budget = ConnectionBudget::default();
            negotiation::client(&mut output, &mut input, &budget).await.expect("byte-exact magic and hello on stdout");
            let ready = negotiation::accept(&mut output, &mut input, &budget, negotiation::Expected {
                instance: None, purpose: &wire::Purpose::Direct, mux_protocol: splitty_core::protocol::PROTOCOL_VERSION, preparation: Duration::from_secs(5),
            }, |_| {}).await.unwrap();
            let welcome = codec::read(&mut output, &budget).await.unwrap().unwrap();
            assert_eq!(welcome.value.kind, Kind::Text);
            let welcome: serde_json::Value = welcome.value.decode().unwrap();
            assert_eq!(welcome["type"], "mux.welcome"); assert_eq!(welcome["serverMode"], "embedded");
            let helper_pid = if mode == "launcher-death" { fs::read_to_string(&helper_pid_file).unwrap().parse::<u32>().unwrap() } else { child.id().unwrap() };
            assert_no_tcp_listener(helper_pid);
            let raw = if mode.starts_with("blocked-") { "stty raw -echo; " } else { "" };
            let script = format!("trap '' HUP TERM; {raw}echo $$ > '{}'; printf '\\000\\377\\200'; exec sleep 120", pid_file.display());
            let create = Frame::data_json(Kind::Text, wire::Delivery::new(ready.generation), &serde_json::json!({"type":"create", "command":script, "cols":80, "rows":24})).unwrap();
            codec::write(&mut input, &create).await.unwrap();
            let session_id = tokio::time::timeout(Duration::from_secs(5), async {
                let mut session_id = None;
                let mut received_bytes = false;
                loop {
                    let frame = codec::read(&mut output, &budget).await.unwrap().unwrap();
                    if frame.value.kind == Kind::Text {
                        let value: serde_json::Value = frame.value.decode().unwrap();
                        if value["type"] == "created" { session_id = value["sessionId"].as_str().map(str::to_owned); }
                    }
                    if frame.value.kind == Kind::Binary {
                        let (_, data) = frame.value.application().unwrap();
                        let (_, bytes) = splitty_server::client_session::codec::decode_binary_frame(data).unwrap();
                        received_bytes |= bytes.windows(3).any(|bytes| bytes == [0, 255, 128]);
                    }
                    if received_bytes {
                        if let Some(id) = session_id.take() { break id; }
                    }
                }
            }).await.expect("NUL and non-UTF8 PTY bytes survive native pipes");
            let owned_pid = fs::read_to_string(&pid_file).unwrap();
            assert!(alive(owned_pid.trim()));
            if mode == "closed-pane" {
                let close = Frame::data_json(Kind::Text, wire::Delivery::new(ready.generation), &serde_json::json!({"type":"close", "id":session_id})).unwrap();
                codec::write(&mut input, &close).await.unwrap();
                tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        let frame = codec::read(&mut output, &budget).await.unwrap().unwrap();
                        if frame.value.kind == Kind::Text {
                            let value: serde_json::Value = frame.value.decode().unwrap();
                            if value["type"] == "closed" { break; }
                        }
                    }
                }).await.expect("pane removed before backend shutdown");
                assert!(alive(owned_pid.trim()), "ignored HUP still requires owned termination");
            }
            if mode.starts_with("blocked-") {
                let request = Frame::data_json(Kind::Request, wire::Delivery::new(ready.generation), &wire::Request { id: 1, operation: "session.send".into(), args: serde_json::json!({"id":session_id, "body":{"data":"x".repeat(512 * 1024)}}).as_object().unwrap().clone() }).unwrap();
                codec::write(&mut input, &request).await.unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
                assert!(alive(owned_pid.trim()), "the input target must still be live");
            }
            if matches!(mode, "broken-output" | "blocked-output") {
                drop(output);
                let request = Frame::data_json(Kind::Request, wire::Delivery::new(ready.generation), &wire::Request { id: 2, operation: "mux.status".into(), args: serde_json::Map::new() }).unwrap();
                codec::write(&mut input, &request).await.unwrap();
                tokio::time::timeout(Duration::from_secs(3), child.wait()).await.expect("broken output ends the helper even with stdin open").unwrap();
                drop(input);
            } else if mode == "launcher-death" {
                child.kill().await.unwrap(); drop(input); drop(output);
                tokio::time::timeout(Duration::from_secs(3), async {
                    while splitty_core::daemon::is_running(helper_pid) { tokio::time::sleep(Duration::from_millis(20)).await; }
                }).await.expect("launcher death closes inherited stdin and ends the owned backend");
            } else {
                input.shutdown().await.unwrap(); drop(input);
                let result = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
                if result.is_err() {
                    child.kill().await.unwrap(); let mut errors = Vec::new(); stderr_pipe.read_to_end(&mut errors).await.unwrap();
                    panic!("{mode}: EOF did not end the backend: {}", String::from_utf8_lossy(&errors));
                }
                assert!(result.unwrap().unwrap().success()); drop(output);
            }
            let mut errors = Vec::new(); stderr_pipe.read_to_end(&mut errors).await.unwrap();
            assert!(errors.len() <= 4096, "stderr is diagnostic, not a terminal-output stream");
            tokio::time::timeout(Duration::from_secs(3), async { while alive(owned_pid.trim()) { tokio::time::sleep(Duration::from_millis(20)).await; } }).await.expect("owned PTY child ends");
            assert!(alive(&persistent));
        });
        assert_eq!(
            fs::read(sandbox.info_path()).ok(),
            expected,
            "{mode}: local app ownership never changes daemon discovery"
        );
        fs::write(sandbox.info_path(), &record).unwrap();
    }
    assert_eq!(
        fs::read(sandbox.info_path()).unwrap(),
        record,
        "embedded helpers never rewrite unrelated daemon discovery"
    );
    assert_eq!(
        fs::read(sandbox.config_dir().join("state.json")).unwrap(),
        state,
        "embedded helpers never persist a workspace"
    );
    assert_eq!(sandbox.keeper_sockets(), 1);
    assert!(!sandbox.config_dir().join("auth-token").exists());
}

/// Smoke: only the binary proves parsing refuses before creating a backend
/// or starting SSH, with no protocol or bootstrap bytes written to stdout.
#[test]
fn native_child_arguments_and_legacy_bootstrap_input_fail_before_backend_creation() {
    use std::io::Write;
    let sandbox = Sandbox::new();
    for args in [
        vec![
            "desktop-backend",
            "--transport",
            "native-v1",
            "--backend",
            "embedded",
            "--target",
            "vm1",
        ],
        vec![
            "desktop-backend",
            "--transport",
            "native-v1",
            "--backend",
            "embedded",
            "--update",
        ],
        vec![
            "desktop-backend",
            "--transport",
            "native-v1",
            "--backend",
            "embedded",
            "--update",
            "--target",
            "vm1",
        ],
        vec![
            "desktop-backend",
            "--transport",
            "native-v1",
            "--backend",
            "ssh",
            "--target",
            "vm1",
        ],
        vec![
            "desktop-backend",
            "--transport",
            "native-v1",
            "--backend",
            "embedded",
            "--remember-target",
        ],
        vec![
            "desktop-backend",
            "--transport",
            "native-v1",
            "--backend",
            "embedded",
            "--remember-target",
            "--target",
            "vm1",
        ],
        vec!["desktop-backend", "--backend", "embedded"],
        vec![
            "desktop-backend",
            "--transport",
            "native-v1",
            "--backend",
            "local",
        ],
        vec![
            "desktop-backend",
            "--transport",
            "native-v1",
            "--backend",
            "embedded",
            "--json",
        ],
        vec![
            "desktop-backend",
            "--transport",
            "native-v1",
            "--backend",
            "embedded",
            "--addr",
            "127.0.0.1:0",
        ],
    ] {
        let out = sandbox.splitty(&args);
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stdout.is_empty());
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
    command.args([
        "desktop-backend",
        "--transport",
        "native-v1",
        "--backend",
        "embedded",
    ]);
    let mut child = sandbox
        .isolate(&mut command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"bootstrap\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert!(!sandbox.config_dir().exists());
    assert!(!sandbox.run_dir().exists());
}

/// `daemon run` is the process `daemon start` spawns. Started by hand with a
/// instance lock, it binds its own endpoint and announces readiness on stdout.
#[test]
fn daemon_run_serves_after_inheriting_its_instance_lock() {
    let sandbox = Sandbox::new();
    let lock = splitty_server::native::endpoint::InstanceLock::acquire(&sandbox.run_dir()).unwrap();
    let lock_fd = lock.file().as_raw_fd();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_splitty"));
    cmd.args(["daemon", "run", "--lock-fd", &lock_fd.to_string()]);
    sandbox
        .isolate(&mut cmd)
        .stdout(Stdio::piped())
        .stderr(File::create(sandbox.home.path().join("run.stderr")).expect("stderr file"));
    // SAFETY: only `fcntl` runs between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            nix::fcntl::fcntl(
                lock_fd,
                nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::empty()),
            )?;
            Ok(())
        });
    }
    let mut child = ChildGuard(cmd.spawn().expect("spawn daemon run"));
    drop(lock);
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
    assert!(info.get("port").is_none());
    assert_eq!(
        sandbox.native(splitty_server::service::catalog::Operation::MuxStatus)["serverMode"],
        "daemon"
    );
    assert!(splitty_server::native::endpoint::InstanceLock::acquire(&sandbox.run_dir()).is_err());

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

/// An invalid instance-lock descriptor is refused with an error line, before
/// anything names the process in `daemon.json`.
#[test]
fn daemon_run_refuses_an_invalid_lock_descriptor() {
    let sandbox = Sandbox::new();
    let out = sandbox.splitty(&["daemon", "run", "--lock-fd", "901"]);
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

/// Occupied TCP ports cannot affect native startup; legacy listener flags
/// are explicit usage errors before endpoint/configuration side effects.
#[test]
fn a_busy_tcp_port_is_irrelevant_and_legacy_listener_flags_are_refused() {
    let sandbox = Sandbox::new();
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap().to_string();
    for flags in [["--addr", addr.as_str()], ["--token", "never-print"]] {
        let out = sandbox.splitty(&["daemon", "start", flags[0], flags[1]]);
        assert_eq!(out.status.code(), Some(2), "{}", sandbox.describe(&out));
        assert!(stderr(&out).contains("unexpected argument"));
        assert!(!sandbox.info_path().exists());
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
    command.args(["daemon", "start"]);
    let out = sandbox
        .isolate(&mut command)
        .env("SPLITTY_ADDR", &addr)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", sandbox.describe(&out));
    assert!(port_from(&out).is_none());
    assert!(!sandbox.config_dir().join("auth-token").exists());
    assert_eq!(
        sandbox.native(splitty_server::service::catalog::Operation::MuxStatus)["serverMode"],
        "daemon"
    );
}

/// Smoke: only an actual process descriptor limit can make the daemon's
/// Unix accept fail while existing descriptors and its instance remain live.
#[cfg(target_os = "linux")]
#[test]
fn descriptor_exhaustion_does_not_end_the_native_daemon() {
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let info = sandbox.read_info().unwrap();
    let pid = i32::try_from(info["pid"].as_u64().unwrap()).unwrap();
    let path = info["clientSocket"].as_str().unwrap();
    let mut original = nix::libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: pid names this fixture's child. Both rlimit pointers reference
    // live initialized objects, and only that child process is changed.
    assert_eq!(
        unsafe {
            nix::libc::prlimit(
                pid,
                nix::libc::RLIMIT_NOFILE,
                std::ptr::null(),
                &mut original,
            )
        },
        0,
        "cannot inspect fixture descriptor limit: {}",
        std::io::Error::last_os_error()
    );
    struct Restore {
        pid: i32,
        limits: nix::libc::rlimit,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            // SAFETY: this restores the unchanged hard limit and saved soft
            // limit of the same fixture child, including after an assertion.
            let _ = unsafe {
                nix::libc::prlimit(
                    self.pid,
                    nix::libc::RLIMIT_NOFILE,
                    &self.limits,
                    std::ptr::null_mut(),
                )
            };
        }
    }
    let restore = Restore {
        pid,
        limits: original,
    };
    let exhausted = nix::libc::rlimit {
        rlim_cur: 0,
        rlim_max: original.rlim_max,
    };
    // SAFETY: the initialized limit lowers only the child's soft ceiling;
    // restore above keeps the original hard and soft values available.
    assert_eq!(
        unsafe {
            nix::libc::prlimit(
                pid,
                nix::libc::RLIMIT_NOFILE,
                &exhausted,
                std::ptr::null_mut(),
            )
        },
        0
    );
    let pending = std::os::unix::net::UnixStream::connect(path).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !sandbox.daemon_log().contains("native accept failed") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let observed = sandbox.daemon_log().contains("native accept failed");
    let running = splitty_core::daemon::is_running(pid as u32);
    drop(restore);
    drop(pending);
    assert!(
        observed,
        "the fixture must actually force an accept failure: {}",
        sandbox.daemon_log()
    );
    assert!(running, "one failed accept must not stop the daemon");
    let status = sandbox.native(splitty_server::service::catalog::Operation::MuxStatus);
    assert_eq!(
        status["instance"], info["instance"],
        "resource recovery preserves the same service incarnation"
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
        .args(["-c", "\"$0\" daemon start", env!("CARGO_BIN_EXE_splitty")])
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

/// The owner-control socket remains optional; the private native endpoint
/// is mandatory and independent of it.
#[test]
fn daemon_without_control_socket_still_serves_native_clients() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.run_dir().join("daemon.sock")).unwrap();
    fs::set_permissions(sandbox.run_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    sandbox.start_daemon();
    let info = sandbox.read_info().unwrap();
    assert_eq!(info["socket"], "");
    assert_eq!(
        sandbox.native(splitty_server::service::catalog::Operation::MuxStatus)["serverMode"],
        "daemon"
    );
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
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(sandbox.log_path()).unwrap();
    fs::set_permissions(sandbox.config_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    let start = sandbox.splitty(&["daemon", "start"]);
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
        !sandbox.info_path().exists(),
        "daemon.json left behind by a failed start"
    );
    let status = sandbox.splitty(&["daemon", "status"]);
    assert!(
        stdout(&status).starts_with("No daemon running"),
        "{}",
        sandbox.describe(&status)
    );
    assert!(!sandbox.run_dir().join("client.sock").exists());
    splitty_server::native::endpoint::InstanceLock::acquire(&sandbox.run_dir())
        .expect("failed startup releases the lock");
}

/// server-modes, Daemon Graceful Shutdown: `daemon stop` ends the daemon while
/// a client is still connected and streaming a session. The daemon used to
/// wait for that session's shell forever.
#[test]
fn daemon_stop_exits_while_a_client_streams_a_session() {
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
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

    let window = stream_session(&sandbox, &session);
    wait_until("native client attaches", || {
        contains(&window, "\"attached\"")
    });

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
}

/// A native window's view, over the real daemon process and private endpoint.
fn stream_session(sandbox: &Sandbox, session: &str) -> std::sync::Arc<std::sync::Mutex<Vec<u8>>> {
    let info: splitty_core::daemon::DaemonInfo =
        serde_json::from_value(sandbox.read_info().unwrap()).unwrap();
    let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let collected = output.clone();
    let session = session.to_owned();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let mut client = splitty_server::native::client::Connector::default()
                .local(&info)
                .await
                .expect("native window");
            client
                .sender()
                .data(
                    splitty_transport::wire::Kind::Text,
                    serde_json::json!({"type":"attach", "id":session, "cols":80, "rows":24})
                        .to_string()
                        .as_bytes(),
                )
                .unwrap();
            while let Some(message) = client.recv().await {
                let (_, bytes) = message.frame.application().unwrap();
                match message.frame.kind {
                    splitty_transport::wire::Kind::Text => {
                        let mut out = collected.lock().unwrap();
                        out.push(b'\n');
                        out.extend_from_slice(bytes);
                        out.push(b'\n');
                    }
                    splitty_transport::wire::Kind::Binary => {
                        if let Some((id, bytes)) =
                            splitty_server::client_session::codec::decode_binary_frame(bytes)
                        {
                            if id == session {
                                collected.lock().unwrap().extend_from_slice(bytes);
                            }
                        }
                    }
                    _ => {}
                }
            }
            client.shutdown().await;
        });
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
// Smoke: only the real binary combines raw mode on its own terminal with
// verified private daemon discovery and native framing; the session loop, the targets
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

    sandbox.start_daemon();
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

    let window = stream_session(&sandbox, &session);

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
        !sandbox.config_dir().join("auth-token").exists(),
        "native attach never reads or creates a browser token"
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

fn json_of(out: &Output, sandbox: &Sandbox) -> serde_json::Value {
    assert!(out.status.success(), "{}", sandbox.describe(out));
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e})\n{}", sandbox.describe(out)))
}

/// What a client asks a host over SSH, run here without SSH: the real binary
/// shows the subcommand, its JSON and the daemon it starts.
#[test]
fn remote_handshake_reports_the_host_and_starts_its_daemon() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();

    let idle = json_of(&sandbox.splitty(&["remote-handshake", "--json"]), &sandbox);
    assert_eq!(idle["handshake"], 2, "{idle}");
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

    sandbox.shared_parents();
    fs::create_dir_all(sandbox.config_dir()).unwrap();
    fs::set_permissions(sandbox.config_dir(), fs::Permissions::from_mode(0o755)).unwrap();
    let readonly = sandbox.splitty(&["remote-handshake", "--json"]);
    assert!(
        !readonly.status.success(),
        "unsafe discovery must remain read-only"
    );
    assert_eq!(
        fs::metadata(sandbox.config_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert!(!sandbox.info_path().exists());

    let started = json_of(
        &sandbox.splitty(&["remote-handshake", "--json", "--ensure-daemon"]),
        &sandbox,
    );
    assert_eq!(
        fs::metadata(sandbox.config_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    sandbox.assert_shared_parents_unchanged();
    let daemon = &started["daemon"];
    assert_eq!(daemon["running"], true, "{started}");
    assert_eq!(daemon["started"], true, "{started}");
    assert!(daemon.get("port").is_none());
    assert_eq!(daemon["transportVersion"], 1);
    assert_eq!(daemon["instance"].as_str().unwrap().len(), 32);
    assert_eq!(
        started["server"]["transportVersions"],
        serde_json::json!([1])
    );
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
        &sandbox.splitty(&["remote-handshake", "--json", "--ensure-daemon"]),
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

// Smoke: these entry points must refuse unsafe configuration before any
// subsystem can repair it, write discovery or start a replacement process.
#[test]
fn daemon_start_and_ensure_refuse_writable_config_directories() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    sandbox.shared_parents();
    fs::create_dir_all(sandbox.config_dir()).unwrap();
    for mode in [0o775, 0o777] {
        fs::set_permissions(sandbox.config_dir(), fs::Permissions::from_mode(mode)).unwrap();
        for args in [
            vec!["daemon", "start"],
            vec!["daemon", "restart"],
            vec!["remote-handshake", "--json", "--ensure-daemon"],
        ] {
            let out = sandbox.splitty(&args);
            assert!(!out.status.success(), "{}", sandbox.describe(&out));
            assert!(
                stderr(&out).contains("daemon configuration"),
                "{}",
                sandbox.describe(&out)
            );
            assert_eq!(
                fs::metadata(sandbox.config_dir())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                mode
            );
            assert!(!sandbox.info_path().exists());
            assert!(!sandbox.run_dir().exists());
        }
    }
    sandbox.assert_shared_parents_unchanged();
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

    let started = json_of(
        &with_own(&["remote-handshake", "--json", "--ensure-daemon"]),
        &sandbox,
    );
    assert_eq!(started["daemon"]["started"], true, "{started}");

    let info: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(own.join("daemon.json")).expect("daemon.json"))
            .expect("daemon.json is JSON");
    assert!(info.get("port").is_none());
    assert!(info["clientSocket"]
        .as_str()
        .unwrap()
        .starts_with(own.join("run").to_str().unwrap()));
    assert!(
        info["socket"]
            .as_str()
            .is_some_and(|socket| socket.starts_with(own.join("run").to_str().expect("UTF-8"))),
        "the control socket lives below the directory: {info}"
    );
    assert!(
        !own.join("auth-token").exists(),
        "native daemon has no browser token"
    );

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
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.run_dir()).unwrap();
    fs::set_permissions(sandbox.run_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    let endpoint = sandbox.run_dir().join("client.sock");
    fs::write(&endpoint, b"unrelated owner file").unwrap();
    let out = sandbox.splitty(&["remote-handshake", "--json", "--ensure-daemon"]);
    assert_eq!(out.status.code(), Some(1), "{}", sandbox.describe(&out));
    assert!(stdout(&out).is_empty(), "no JSON on failure");
    assert!(!stderr(&out).is_empty(), "startup failure has a diagnostic");
    assert_eq!(fs::read(&endpoint).unwrap(), b"unrelated owner file");
    let legacy = sandbox.splitty(&[
        "remote-handshake",
        "--json",
        "--ensure-daemon",
        "--port",
        "9800",
    ]);
    assert_eq!(legacy.status.code(), Some(2));
    let usage: serde_json::Value = serde_json::from_slice(&legacy.stdout).unwrap();
    assert_eq!(usage["code"], "usage");
}

#[test]
fn remote_bridge_relays_native_bytes_and_detaches_without_ending_host_sessions() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let (screen, pane, session) = sandbox.shell();
    let pidfile = sandbox.home.path().join("remote-pid");
    sandbox.type_line(
        &screen,
        &pane,
        &format!("echo $$ > '{}'", pidfile.display()),
    );
    let pid = sandbox.wait_for("remote-pid");
    let record = fs::read(sandbox.info_path()).unwrap();
    let info: splitty_core::daemon::DaemonInfo = serde_json::from_slice(&record).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // The same typed client used by SSH/native helpers verifies discovery
    // before exposing application messages and retains its worker until exit.
    runtime.block_on(async {
        use splitty_transport::{
            admission, negotiation,
            wire::{self, Kind},
        };
        let stale = "0".repeat(32);
        for instance in [info.instance.as_deref().unwrap(), stale.as_str()] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
            command.args(["remote-bridge", "--transport", "native-v1"]);
            sandbox.isolate(&mut command);
            let assignment =
                admission::Assignment::new(admission::Backend::new(), admission::Domain::new());
            let opened = splitty_remote::bridge::NativeBridge::spawn(
                tokio::process::Command::from(command),
                assignment,
                negotiation::Expected {
                    instance: Some(instance),
                    purpose: &wire::Purpose::Direct,
                    mux_protocol: splitty_core::protocol::PROTOCOL_VERSION,
                    preparation: Duration::from_secs(5),
                },
                |_| {},
            )
            .await;
            if instance == stale {
                assert!(
                    opened.is_err(),
                    "stale discovery cannot admit the exec client"
                );
                continue;
            }
            let mut bridge = opened.unwrap();
            assert_eq!(bridge.ready().instance, instance);
            let welcome = tokio::time::timeout(READ_TIMEOUT, bridge.recv())
                .await
                .unwrap()
                .unwrap();
            let value: serde_json::Value = welcome.frame.decode().unwrap();
            assert_eq!(value["type"], "mux.welcome");
            drop(welcome);
            bridge
                .sender()
                .json(
                    Kind::Request,
                    &wire::Request {
                        id: 1,
                        operation: "mux.status".into(),
                        args: serde_json::Map::new(),
                    },
                )
                .unwrap();
            tokio::time::timeout(READ_TIMEOUT, async {
                loop {
                    let message = bridge.recv().await.unwrap();
                    if message.frame.kind == Kind::Reply {
                        let reply: wire::Reply = message.frame.decode().unwrap();
                        assert_eq!((reply.id, reply.status), (1, 200));
                        break;
                    }
                }
            })
            .await
            .unwrap();
            let (transport, ended) = bridge.finish().await;
            transport.unwrap();
            assert!(ended.status.unwrap().success(), "{}", ended.stderr);
        }
    });
    for mode in [
        "eof",
        "broken-output",
        "invalid-frame",
        "forbidden-operation",
    ] {
        runtime.block_on(async {
            use splitty_transport::{budget::ConnectionBudget, codec::{self, Frame}, negotiation, wire::{self, Kind}};
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
            command.args(["remote-bridge", "--transport", "native-v1"]);
            sandbox.isolate(&mut command).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = tokio::process::Command::from(command).kill_on_drop(true).spawn().unwrap();
            let mut input = child.stdin.take().unwrap(); let mut output = child.stdout.take().unwrap();
            let mut errors = child.stderr.take().unwrap().take(4097);
            let budget = ConnectionBudget::default();
            negotiation::client(&mut output, &mut input, &budget).await.unwrap();
            let ready = negotiation::accept(&mut output, &mut input, &budget, negotiation::Expected {
                instance: info.instance.as_deref(), purpose: &wire::Purpose::Direct,
                mux_protocol: splitty_core::protocol::PROTOCOL_VERSION, preparation: Duration::from_secs(5),
            }, |_| {}).await.unwrap();
            let welcome = codec::read(&mut output, &budget).await.unwrap().unwrap();
            assert_eq!(welcome.value.kind, Kind::Text);
            let welcome: serde_json::Value = welcome.value.decode().unwrap();
            assert_eq!(welcome["type"], "mux.welcome"); assert_eq!(welcome["serverMode"], "daemon");
            assert_no_tcp_listener(child.id().unwrap());
            let resync = Frame::data_json(Kind::Text, wire::Delivery::new(ready.generation), &serde_json::json!({"type":"mux.resync", "screenId":screen})).unwrap();
            codec::write(&mut input, &resync).await.unwrap();
            tokio::time::timeout(READ_TIMEOUT, async {
                loop {
                    let frame = codec::read(&mut output, &budget).await.unwrap().unwrap();
                    if frame.value.kind == Kind::Text {
                        let value: serde_json::Value = frame.value.decode().unwrap();
                        if value["type"] == "mux.screen_snapshot" { assert!(value.to_string().contains(&screen)); break; }
                    }
                }
            }).await.unwrap();
            let attach = Frame::data_json(Kind::Text, wire::Delivery::new(ready.generation), &serde_json::json!({"type":"attach", "id":session})).unwrap();
            codec::write(&mut input, &attach).await.unwrap();
            let request = Frame::data_json(Kind::Request, wire::Delivery::new(ready.generation), &wire::Request {
                id: 1, operation: "session.send".into(), args: serde_json::json!({"id":session, "body":{"data":"printf '\\000\\377\\200'\n"}}).as_object().unwrap().clone(),
            }).unwrap();
            codec::write(&mut input, &request).await.unwrap();
            tokio::time::timeout(READ_TIMEOUT, async {
                let mut replied = false; let mut binary = false;
                while !replied || !binary {
                    let frame = codec::read(&mut output, &budget).await.unwrap().unwrap();
                    if frame.value.kind == Kind::Reply {
                        let reply: wire::Reply = frame.value.decode().unwrap();
                        assert_eq!((reply.id, reply.status), (1, 200)); replied = true;
                    }
                    if frame.value.kind == Kind::Binary {
                        let (_, data) = frame.value.application().unwrap();
                        let (_, bytes) = splitty_server::client_session::codec::decode_binary_frame(data).unwrap();
                        binary |= bytes.windows(3).any(|bytes| bytes == [0, 255, 128]);
                    }
                }
            }).await.unwrap();
            match mode {
                "broken-output" => {
                    drop(output);
                    let frame = Frame::data_json(Kind::Request, wire::Delivery::new(ready.generation), &wire::Request { id:2, operation:"mux.status".into(), args:serde_json::Map::new() }).unwrap();
                    codec::write(&mut input, &frame).await.unwrap();
                }
                "invalid-frame" => { input.write_all(&[255, 0, 0, 0, 0]).await.unwrap(); }
                "forbidden-operation" => {
                    let frame = Frame::data_json(Kind::Request, wire::Delivery::new(ready.generation), &wire::Request { id:2, operation:"host.list".into(), args:serde_json::Map::new() }).unwrap();
                    codec::write(&mut input, &frame).await.unwrap();
                }
                _ => { input.shutdown().await.unwrap(); drop(input); }
            }
            let status = tokio::time::timeout(Duration::from_secs(3), child.wait()).await.expect("bridge exits without waiting for the daemon").unwrap();
            if mode == "eof" { assert!(status.success()); }
            else { assert!(!status.success(), "{mode} cannot report a clean EOF"); }
            let mut stderr = Vec::new(); errors.read_to_end(&mut stderr).await.unwrap();
            assert!(stderr.len() <= 4096);
            assert!(alive(&pid));
        });
        assert_eq!(fs::read(sandbox.info_path()).unwrap(), record);
    }
    assert!(!sandbox.config_dir().join("auth-token").exists());
}

#[test]
fn remote_bridge_without_a_daemon_fails() {
    let sandbox = Sandbox::new();
    let out = sandbox.splitty(&["remote-bridge", "--transport", "native-v1"]);
    assert_eq!(out.status.code(), Some(1), "{}", sandbox.describe(&out));
    assert!(
        stderr(&out).contains("No Splitty daemon is running"),
        "{}",
        sandbox.describe(&out)
    );
    assert!(out.stdout.is_empty());
    let old = sandbox.splitty(&["remote-bridge"]);
    assert_eq!(old.status.code(), Some(2));
    assert!(old.stdout.is_empty());
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
    sandbox.start_daemon();
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
    sandbox.start_daemon();
    let window = stream_session(&sandbox, &session);
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

    // "Update without ending sessions".
    let restart = sandbox.splitty(&["daemon", "restart"]);
    assert!(restart.status.success(), "{}", sandbox.describe(&restart));
    assert!(
        stdout(&restart).contains("SPLITTY_CLIENT_SOCKET="),
        "native endpoint after restart"
    );
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

    sandbox.start_daemon();
    let window = stream_session(&sandbox, &session);
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

    sandbox.start_daemon();
    let screens = sandbox.json(&["list-screens", "--json"]);
    assert!(
        screens.as_array().is_some_and(|screens| screens
            .iter()
            .all(|s| s["id"].as_str() != Some(screen.as_str()))),
        "the lost screen is not restored: {screens}"
    );
    let window = stream_session(&sandbox, &session);
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

/// Smoke: only inherited pipes to an actual child demonstrate app ownership,
/// reload survival, and helper/job exit; a same-process service cannot prove it.
#[test]
fn rust_embedded_owner_shares_runtime_across_reload_and_reaps_its_jobs() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.config_dir()).unwrap();
    let discovery = sandbox.info_path();
    fs::write(&discovery, b"independent corrupt discovery").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
    sandbox.isolate(&mut command).args([
        "desktop-backend",
        "--transport",
        "native-v1",
        "--backend",
        "embedded",
        "--owner",
    ]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        use splitty_remote::owner::{Connection, Owner};
        use splitty_transport::{queue, wire::{self, Kind}};
        async fn request(connection: &mut Connection, operation: &str, args: serde_json::Value) -> wire::Reply {
            (*connection.request(operation.into(), args.as_object().unwrap().clone(), Some(READ_TIMEOUT)).await.unwrap()).clone()
        }
        let owner = Owner::spawn(tokio::process::Command::from(command), 1).await.unwrap();
        let pid = i32::try_from(owner.id().unwrap()).unwrap();
        assert_no_tcp_listener(owner.id().unwrap());
        let first_context = queue::Context::default();
        let second_context = queue::Context::default();
        let mut first = owner.open_context(None, false, first_context.clone(), |_| {}).await.unwrap();
        let mut second = owner.open_context(None, false, second_context.clone(), |_| {}).await.unwrap();
        assert_eq!(owner.stream_count(), 2);
        let mut session = owner.open_context(None, false, queue::Context::default(), |_| {}).await.unwrap();
        // An API-only receiver must never consume this session's welcome/output.
        let welcome = tokio::time::timeout(READ_TIMEOUT, async {
            loop {
                let frame = session.recv().await.unwrap();
                if frame.frame.kind == Kind::Text {
                    let value: serde_json::Value = frame.frame.decode().unwrap();
                    if value["type"] == "mux.welcome" { break value; }
                }
            }
        }).await.unwrap();
        assert_eq!(welcome["type"], "mux.welcome");
        for operation in ["home.get", "shellProfiles.get", "template.list"] {
            assert_eq!(request(&mut first, operation, serde_json::json!({})).await.status, 200);
        }
        session.finish().await.unwrap();
        let instance = first.ready().instance.clone();
        assert_eq!(second.ready().instance, instance);
        let reply = request(&mut first, "mux.op", serde_json::json!({"body":{"screenId":"rust-owner-screen","op":{
            "type":"create_screen","screenId":"rust-owner-screen","name":"survives reload"
        }}})).await;
        assert_eq!(reply.status, 200, "{}", reply.body);
        assert!(request(&mut second, "mux.screen", serde_json::json!({"id":"rust-owner-screen"})).await.body.contains("survives reload"));
        first.finish().await.unwrap();
        assert_eq!(owner.stream_count(), 1);
        assert_eq!(first_context.budget.used(), 0);
        assert_eq!(request(&mut second, "mux.status", serde_json::json!({})).await.status, 200);
        for _ in 0..40 {
            let context = queue::Context::default();
            let mut reloaded = owner.open_context(None, false, context.clone(), |_| {}).await.unwrap();
            assert_eq!(reloaded.ready().instance, instance);
            assert_eq!(request(&mut reloaded, "mux.screen", serde_json::json!({"id":"rust-owner-screen"})).await.status, 200);
            reloaded.finish().await.unwrap();
            assert_eq!(context.budget.used(), 0);
        }
        let job_pid_path = sandbox.home.path().join("owned-job-pid");
        let reply = request(&mut second, "job.launch", serde_json::json!({"body":{
            "requestId":"d0267ff6-1234-4dfc-a01b-448f78f00223", "argv":["/bin/sh","-c",format!("echo $$ > {}; exec sleep 60", splitty_remote::ssh::quote(job_pid_path.to_str().unwrap()))],
            "cwd":sandbox.home.path(), "placement":{"kind":"screen"}, "background":true
        }})).await;
        assert_eq!(reply.status, 201, "{}", reply.body);
        let job_pid = tokio::time::timeout(READ_TIMEOUT, async {
            loop {
                if let Ok(text) = tokio::fs::read_to_string(&job_pid_path).await {
                    if let Ok(pid) = text.trim().parse::<i32>() { break pid; }
                }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        tokio::time::timeout(READ_TIMEOUT, owner.close_result()).await.unwrap().unwrap();
        assert!(kill(Pid::from_raw(pid), None).is_err(), "owner completion observes helper exit");
        assert!(kill(Pid::from_raw(job_pid), None).is_err(), "owner exit includes its managed job; remaining process: {:?}", fs::read_to_string(format!("/proc/{job_pid}/stat")));
        second.finish().await.unwrap();
        assert_eq!(second_context.budget.used(), 0);
        assert_eq!(owner.retained_bytes(), 0);
        assert_eq!(owner.stream_count(), 0);
        assert!(owner.is_closed());
        assert_eq!(fs::read(discovery).unwrap(), b"independent corrupt discovery");
    });
}

/// Smoke: two real children in the same configuration directory prove that
/// each owner has its own runtime and pipe EOF ends only that owner's session.
#[test]
fn two_embedded_owners_keep_their_sessions_independent() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let sandbox = Sandbox::new();
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
        sandbox.isolate(&mut command).args([
            "desktop-backend",
            "--transport",
            "native-v1",
            "--backend",
            "embedded",
            "--owner",
        ]);
        tokio::process::Command::from(command)
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        use splitty_remote::owner::{Connection, Owner};
        use splitty_transport::queue;

        async fn request(connection: &mut Connection, operation: &str, args: serde_json::Value) -> serde_json::Value {
            let reply = connection.request(operation.into(), args.as_object().unwrap().clone(), Some(READ_TIMEOUT)).await.unwrap();
            assert!((200..300).contains(&reply.status), "{}", reply.body);
            serde_json::from_str(&reply.body).unwrap()
        }

        let first = Owner::spawn(command(), 1).await.unwrap();
        let second = Owner::spawn(command(), 1).await.unwrap();
        let first_pid = i32::try_from(first.id().unwrap()).unwrap();
        let second_pid = i32::try_from(second.id().unwrap()).unwrap();
        assert_no_tcp_listener(first.id().unwrap());
        assert_no_tcp_listener(second.id().unwrap());
        let mut first_client = first.open_context(None, false, queue::Context::default(), |_| {}).await.unwrap();
        let mut second_client = second.open_context(None, false, queue::Context::default(), |_| {}).await.unwrap();
        assert_ne!(first_client.ready().instance, second_client.ready().instance);
        let mut sessions = Vec::new();
        for (name, client) in [("first", &mut first_client), ("second", &mut second_client)] {
            assert_eq!(request(client, "job.list", serde_json::json!({})).await["jobs"], serde_json::json!([]));
            let pid_path = sandbox.home.path().join(format!("{name}-session-pid"));
            let job = request(client, "job.launch", serde_json::json!({"body":{
                "requestId":"d0267ff6-1234-4dfc-a01b-448f78f00223",
                "argv":["/bin/sh", "-c", format!("echo $$ > {}; exec sleep 60", splitty_remote::ssh::quote(pid_path.to_str().unwrap()))],
                "cwd":sandbox.home.path(), "placement":{"kind":"screen"}, "background":true
            }})).await;
            assert_eq!(job["state"], "running");
            let pid = tokio::time::timeout(READ_TIMEOUT, async {
                loop {
                    if let Ok(text) = tokio::fs::read_to_string(&pid_path).await {
                        if let Ok(pid) = text.trim().parse::<i32>() {
                            break pid;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.unwrap();
            assert!(kill(Pid::from_raw(pid), None).is_ok());
            sessions.push((job["jobId"].as_str().unwrap().to_owned(), pid));
        }
        assert_eq!(request(&mut first_client, "job.list", serde_json::json!({})).await["jobs"].as_array().unwrap().len(), 1);
        assert_eq!(request(&mut second_client, "job.list", serde_json::json!({})).await["jobs"].as_array().unwrap().len(), 1);
        assert_ne!(sessions[0].1, sessions[1].1);
        tokio::time::timeout(Duration::from_secs(3), first.close_result()).await.unwrap().unwrap();
        first_client.finish().await.unwrap();
        assert!(kill(Pid::from_raw(first_pid), None).is_err());
        assert!(kill(Pid::from_raw(sessions[0].1), None).is_err(), "first owner reaps its session");
        assert!(kill(Pid::from_raw(second_pid), None).is_ok());
        assert!(kill(Pid::from_raw(sessions[1].1), None).is_ok(), "second owner's session remains alive");
        let job = request(&mut second_client, "job.inspect", serde_json::json!({"id":sessions[1].0})).await;
        assert_eq!(job["state"], "running");
        tokio::time::timeout(Duration::from_secs(3), second.close_result()).await.unwrap().unwrap();
        second_client.finish().await.unwrap();
        assert!(kill(Pid::from_raw(second_pid), None).is_err());
        assert!(kill(Pid::from_raw(sessions[1].1), None).is_err());
        assert!(!sandbox.info_path().exists());
        assert!(!sandbox.config_dir().join("auth-token").exists());
    });
}

/// Smoke: only two real released-format implementations can demonstrate that
/// rollback adopts the original keeper and shell instead of recreating them.
#[cfg(target_os = "linux")]
#[test]
fn a_kept_shell_survives_a_pinned_previous_binary_rollback_and_return() {
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    let Some(previous) = std::env::var_os("SPLITTY_TEST_PREVIOUS_BINARY") else {
        assert_ne!(
            std::env::var("SPLITTY_TEST_REQUIRE_PREVIOUS_BINARY").as_deref(),
            Ok("1"),
            "strict rollback proof requires SPLITTY_TEST_PREVIOUS_BINARY and its SHA256"
        );
        eprintln!("UNRUN historical rollback: no explicitly pinned previous binary");
        return;
    };
    // No daemon runs in the host PID namespace. Namespace PID1 exiting kills
    // every descendant, including a detached daemon whose start failed early.
    const NAMESPACE_CHILD: &str = "SPLITTY_TEST_ROLLBACK_NAMESPACE_CHILD";
    if std::env::var_os(NAMESPACE_CHILD).is_none() {
        let logs = tempfile::tempdir().unwrap();
        let out = logs.path().join("stdout");
        let err = logs.path().join("stderr");
        let previous_path = PathBuf::from(&previous).canonicalize().unwrap();
        let executable = std::env::current_exe().unwrap();
        let server = Path::new(env!("CARGO_BIN_EXE_splitty"))
            .canonicalize()
            .unwrap();
        let mut command = Command::new("bwrap");
        command.args([
            "--unshare-user",
            "--unshare-pid",
            "--unshare-net",
            "--as-pid-1",
            "--die-with-parent",
            "--ro-bind",
            "/usr",
            "/usr",
            "--ro-bind",
            "/etc",
            "/etc",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--chdir",
            "/",
        ]);
        // This Linux smoke requires a merged-/usr runtime. lib64 is optional
        // (e.g. absent on aarch64); never guess an architecture-specific loader.
        for (link, target) in [
            ("/bin", "usr/bin"),
            ("/lib", "usr/lib"),
            ("/lib64", "usr/lib64"),
        ] {
            if link == "/lib64" && !Path::new(link).exists() {
                continue;
            }
            assert_eq!(
                fs::read_link(link).expect("merged-/usr runtime prerequisite"),
                Path::new(target)
            );
            command.args(["--symlink", target, link]);
        }
        // No host HOME or repository directory is mounted. Only these three
        // artifacts are visible at their original paths, always read-only.
        for binary in [&executable, &server, &previous_path] {
            command.arg("--ro-bind").arg(binary).arg(binary);
        }
        let mut child = command
            .arg("--")
            .arg(&executable)
            .args([
                "a_kept_shell_survives_a_pinned_previous_binary_rollback_and_return",
                "--exact",
                "--nocapture",
            ])
            .env(NAMESPACE_CHILD, "1")
            .env("SPLITTY_TEST_PREVIOUS_BINARY", &previous_path)
            // Lifecycle proof uses a fixed POSIX shell, independent of the
            // invoking user's interactive shell and its first-run setup.
            .env("SHELL", "/bin/sh")
            .stdin(Stdio::null())
            .stdout(File::create(&out).unwrap())
            .stderr(File::create(&err).unwrap())
            .spawn()
            .expect("private PID namespace is required for rollback proof");
        let deadline = Instant::now() + Duration::from_secs(90);
        let result = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Ok(None) => break Err("rollback namespace exceeded 90 seconds".to_string()),
                Err(error) => break Err(format!("observing rollback namespace: {error}")),
            }
        };
        if result.is_err() {
            // Child is still unreaped: its identity cannot have been reused.
            let kill = child.kill();
            let deadline = Instant::now() + Duration::from_secs(5);
            let reaped = loop {
                match child.try_wait() {
                    Ok(Some(_)) => break true,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    _ => break false,
                }
            };
            assert!(
                reaped,
                "owned namespace supervisor cleanup failed: {kill:?}"
            );
        }
        let stdout = fs::read_to_string(out).unwrap();
        let stderr = fs::read_to_string(err).unwrap();
        print!("{stdout}");
        eprint!("{stderr}");
        assert!(
            result.as_ref().is_ok_and(|status| status.success()),
            "{result:?}\n{stdout}\n{stderr}"
        );
        return;
    }
    assert_eq!(
        unsafe { nix::libc::getpid() },
        1,
        "rollback child must be private namespace PID1"
    );
    let previous = PathBuf::from(previous).canonicalize().unwrap();
    let expected = std::env::var("SPLITTY_TEST_PREVIOUS_SHA256").expect("pinned artifact SHA256");
    assert_eq!(expected.len(), 64);
    let digest = Command::new("sha256sum")
        .arg("--")
        .arg(&previous)
        .output()
        .unwrap();
    assert!(digest.status.success());
    assert_eq!(
        String::from_utf8(digest.stdout)
            .unwrap()
            .split_whitespace()
            .next(),
        Some(expected.as_str())
    );
    let sandbox = Sandbox::new();
    sandbox.shared_parents();

    // pidfds hold exact fixture generations, including on assertion failure.
    struct Owned(Vec<File>);
    impl Owned {
        fn pin(&mut self, pid: u32, sandbox: &Sandbox) {
            // Acquire the generation before reading PID-indexed provenance.
            // An unverified descriptor is only closed on failure, never signalled.
            let fd = unsafe { nix::libc::syscall(nix::libc::SYS_pidfd_open, pid, 0) };
            assert!(fd >= 0, "pin candidate process {pid}");
            let file = unsafe { File::from_raw_fd(fd as i32) };
            assert!(
                !Self::exited(&file).expect("observe candidate generation"),
                "candidate generation is live"
            );
            let env = fs::read(format!("/proc/{pid}/environ")).expect("owned process environment");
            let home = format!(
                "HOME={}",
                sandbox.home.path().canonicalize().unwrap().display()
            );
            assert!(env
                .split(|byte| *byte == 0)
                .any(|entry| entry == home.as_bytes()));
            assert!(
                !Self::exited(&file).expect("observe candidate generation"),
                "same generation survived provenance validation"
            );
            self.0.push(file);
        }
        fn exited(file: &File) -> std::io::Result<bool> {
            loop {
                let mut event = nix::libc::pollfd {
                    fd: file.as_raw_fd(),
                    events: nix::libc::POLLIN,
                    revents: 0,
                };
                let result = unsafe { nix::libc::poll(&mut event, 1, 0) };
                if result >= 0 {
                    if event.revents & (nix::libc::POLLERR | nix::libc::POLLNVAL) != 0 {
                        return Err(std::io::Error::other("invalid pidfd observation"));
                    }
                    return Ok(event.revents & nix::libc::POLLIN != 0);
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            for file in &self.0 {
                if matches!(Self::exited(file), Ok(false)) {
                    unsafe {
                        nix::libc::syscall(
                            nix::libc::SYS_pidfd_send_signal,
                            file.as_raw_fd(),
                            nix::libc::SIGKILL,
                            std::ptr::null::<nix::libc::siginfo_t>(),
                            0,
                        );
                    }
                }
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while self
                .0
                .iter()
                .any(|file| !matches!(Self::exited(file), Ok(true)))
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(
                self.0
                    .iter()
                    .all(|file| matches!(Self::exited(file), Ok(true))),
                "all exact owned generations exited"
            );
        }
    }
    let mut owned = Owned(Vec::new());
    let mut unrelated_command = Command::new("/bin/sh");
    let mut unrelated = sandbox
        .isolate(&mut unrelated_command)
        .args(["-c", "read -r ignored"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let run_result = |binary: &Path, args: &[&str]| {
        eprintln!("ROLLBACK CLI binary={} args={args:?}", binary.display());
        let stdout_path = sandbox.path("rollback-cli.stdout");
        let stderr_path = sandbox.path("rollback-cli.stderr");
        let mut command = Command::new(binary);
        sandbox
            .isolate(&mut command)
            .args(args)
            .process_group(0)
            .stdout(File::create(&stdout_path).unwrap())
            .stderr(File::create(&stderr_path).unwrap());
        let mut child = command.spawn().expect("owned bounded CLI");
        let deadline = Instant::now() + READ_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let signal = killpg(Pid::from_raw(child.id() as i32), Signal::SIGKILL);
                let cleanup_deadline = Instant::now() + Duration::from_secs(2);
                let reaped = loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break true,
                        Ok(None) if Instant::now() < cleanup_deadline => {
                            std::thread::sleep(Duration::from_millis(20))
                        }
                        _ => break false,
                    }
                };
                panic!(
                    "bounded rollback CLI timed out: {args:?}; signal={signal:?}, reaped={reaped}"
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        Output {
            status,
            stdout: fs::read(stdout_path).unwrap(),
            stderr: fs::read(stderr_path).unwrap(),
        }
    };
    let run = |binary: &Path, args: &[&str]| {
        let output = run_result(binary, args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            sandbox.describe(&output)
        );
        output
    };
    let current = Path::new(env!("CARGO_BIN_EXE_splitty"));
    run(current, &["daemon", "start"]);
    owned.pin(
        sandbox.read_info().unwrap()["pid"].as_u64().unwrap() as u32,
        &sandbox,
    );
    run(current, &["new-screen"]);
    let screens: serde_json::Value =
        serde_json::from_slice(&run(current, &["list-screens", "--json"]).stdout).unwrap();
    let screen = screens.as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap();
    let panes: serde_json::Value =
        serde_json::from_slice(&run(current, &["list-panes", "--screen", screen, "--json"]).stdout)
            .unwrap();
    let (pane, record) = panes.as_object().unwrap().iter().next().unwrap();
    let session = record["sessionId"].as_str().unwrap();
    let random = || {
        use std::io::Read;
        let mut bytes = [0u8; 16];
        File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut bytes)
            .unwrap();
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let secret = random();
    let expected_shell = std::cell::Cell::new(None::<u32>);
    let tail = |path: &Path| -> String {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut file) = File::open(path) else {
            return "<absent>".into();
        };
        let length = file.metadata().unwrap().len();
        file.seek(SeekFrom::Start(length.saturating_sub(8192)))
            .unwrap();
        let mut bytes = Vec::new();
        file.take(8192).read_to_end(&mut bytes).unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    eprintln!(
        "ROLLBACK shell environment SHELL={:?} ENV={:?} BASH_ENV={:?} ZDOTDIR={:?}; settings={}",
        std::env::var_os("SHELL"),
        std::env::var_os("ENV"),
        std::env::var_os("BASH_ENV"),
        std::env::var_os("ZDOTDIR"),
        tail(&sandbox.config_dir().join("settings.json"))
    );
    let prove = |binary: &Path, stage: &str, initialize: bool| {
        eprintln!(
            "ROLLBACK PROOF stage={stage} initialize={initialize} expected_pid={:?}",
            expected_shell.get()
        );
        let nonce = random();
        let init = if initialize {
            format!("SPLITTY_ROLLBACK_PROOF={secret}; ")
        } else {
            String::new()
        };
        let line = format!(
            "{init}printf '%s:%s:%s\\n' '{nonce}' \"$$\" \"$SPLITTY_ROLLBACK_PROOF\" > {}\\n",
            sandbox.path(stage).display()
        );
        run(
            binary,
            &["send-keys", "--screen", screen, "--pane", pane, &line],
        );
        let proof = sandbox.wait_for(stage);
        let fields: Vec<_> = proof.split(':').collect();
        let pid = fields.get(1).and_then(|pid| pid.parse::<u32>().ok());
        let identities: Vec<_> = fs::read_dir(sandbox.run_dir().join("keepers"))
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), entry.metadata().unwrap().ino())
            })
            .collect();
        eprintln!("ROLLBACK PROOF stage={stage} proof={proof:?} current_pid={pid:?} expected_pid={:?} discovery={} keepers={identities:?} shell_exe={:?} shell_cmdline={} daemonlog(last8KiB)={}",
            expected_shell.get(), tail(&sandbox.info_path()),
            pid.and_then(|pid| fs::read_link(format!("/proc/{pid}/exe")).ok()),
            pid.map(|pid| tail(Path::new(&format!("/proc/{pid}/cmdline")))).unwrap_or_default(), tail(&sandbox.log_path()));
        assert_eq!(
            fs::read_link(format!("/proc/{}/exe", pid.expect("shell PID"))).unwrap(),
            Path::new("/bin/sh").canonicalize().unwrap(),
            "{stage}: explicitly selected fixture shell"
        );
        assert_eq!(fields.len(), 3, "{stage}: {proof}");
        assert_eq!(fields[0], nonce);
        assert_eq!(
            fields[2], secret,
            "{stage}: unexported original shell state; proof={proof}"
        );
        fields[1].parse::<u32>().unwrap()
    };
    let shell = prove(current, "rollback-before", true);
    expected_shell.set(Some(shell));
    let shell_generation = owned.0.len();
    owned.pin(shell, &sandbox);
    assert!(!Owned::exited(&owned.0[shell_generation]).expect("observe retained shell generation"));
    let stat = fs::read_to_string(format!("/proc/{shell}/stat")).unwrap();
    assert!(
        !Owned::exited(&owned.0[shell_generation]).expect("observe retained shell generation"),
        "shell generation survived parent lookup"
    );
    let keeper: u32 = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    owned.pin(keeper, &sandbox);
    assert!(
        !Owned::exited(&owned.0[shell_generation]).expect("observe retained shell generation"),
        "shell generation survived keeper validation"
    );
    let sockets = || {
        let mut entries: Vec<_> = fs::read_dir(sandbox.run_dir().join("keepers"))
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), entry.metadata().unwrap().ino())
            })
            .collect();
        entries.sort();
        entries
    };
    let original_sockets = sockets();
    assert!(!original_sockets.is_empty());
    run(current, &["daemon", "stop"]);
    assert!(alive(&shell.to_string()));
    // The historical server deliberately exposes only its explicit ephemeral
    // loopback browser listener; no current native client consumes that port.
    run(&previous, &["daemon", "start", "--addr", "127.0.0.1:0"]);
    let info = sandbox.read_info().unwrap();
    owned.pin(info["pid"].as_u64().unwrap() as u32, &sandbox);
    assert_eq!(
        fs::read_link(format!("/proc/{}/exe", info["pid"])).unwrap(),
        previous
    );
    let adopted: serde_json::Value = serde_json::from_slice(
        &run(&previous, &["list-panes", "--screen", screen, "--json"]).stdout,
    )
    .unwrap();
    assert_eq!(adopted[pane]["sessionId"], session);
    assert_eq!(prove(&previous, "rollback-previous", false), shell);
    assert_eq!(sockets(), original_sockets);

    // Also create a terminal while the historical daemon is running, so its
    // keeper is genuinely the pinned historical executable, not merely a
    // current keeper that the historical daemon adopted during rollback.
    run(&previous, &["new-screen"]);
    let historical_screens: serde_json::Value =
        serde_json::from_slice(&run(&previous, &["list-screens", "--json"]).stdout).unwrap();
    let historical_screen = historical_screens
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"].as_str() != Some(screen))
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let historical_panes: serde_json::Value = serde_json::from_slice(
        &run(
            &previous,
            &["list-panes", "--screen", historical_screen, "--json"],
        )
        .stdout,
    )
    .unwrap();
    let (historical_pane, historical_record) =
        historical_panes.as_object().unwrap().iter().next().unwrap();
    let historical_session = historical_record["sessionId"].as_str().unwrap();
    let historical_secret = random();
    let prove_historical = |binary: &Path, stage: &str, initialize: bool| {
        let nonce = random();
        let init = if initialize {
            format!("SPLITTY_HISTORICAL_PROOF={historical_secret}; ")
        } else {
            String::new()
        };
        let line = format!(
            "{init}printf '%s:%s:%s\\n' '{nonce}' \"$$\" \"$SPLITTY_HISTORICAL_PROOF\" > {}\\n",
            sandbox.path(stage).display()
        );
        run(
            binary,
            &[
                "send-keys",
                "--screen",
                historical_screen,
                "--pane",
                historical_pane,
                &line,
            ],
        );
        let proof = sandbox.wait_for(stage);
        let fields: Vec<_> = proof.split(':').collect();
        assert_eq!(fields.len(), 3, "{stage}: {proof}");
        assert_eq!(fields[0], nonce);
        assert_eq!(
            fields[2], historical_secret,
            "{stage}: historical shell private state"
        );
        fields[1].parse::<u32>().unwrap()
    };
    let historical_shell = prove_historical(&previous, "historical-before-migration", true);
    owned.pin(historical_shell, &sandbox);
    let historical_stat = fs::read_to_string(format!("/proc/{historical_shell}/stat")).unwrap();
    let historical_keeper: u32 = historical_stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    owned.pin(historical_keeper, &sandbox);
    assert_eq!(
        fs::read_link(format!("/proc/{historical_keeper}/exe")).unwrap(),
        previous,
        "new historical session is owned by the pinned historical keeper executable"
    );
    let original_sockets = sockets();
    assert_eq!(original_sockets.len(), 2);

    // The new binary's discovery only reads the old daemon's private owner
    // socket and keeper Hello. No claim, shutdown or TCP fallback occurs.
    let inspect = || -> serde_json::Value {
        serde_json::from_slice(&run(current, &["remote-handshake", "--json"]).stdout).unwrap()
    };
    let before = fs::read(sandbox.info_path()).unwrap();
    let offer = inspect();
    assert_eq!(offer["daemon"]["pid"], info["pid"]);
    assert_eq!(
        offer["daemon"]["legacyMigration"]["status"], "ready",
        "{offer}"
    );
    assert_eq!(offer["daemon"]["legacyMigration"]["sessions"], 2);
    let token = offer["daemon"]["legacyMigration"]["token"]
        .as_str()
        .unwrap();
    assert_eq!(fs::read(sandbox.info_path()).unwrap(), before);
    assert_eq!(prove(&previous, "migration-probe-read-only", false), shell);

    // A live mux session with undiscoverable keepers cannot be offered for
    // migration. Temporarily move only fixture socket directory entries; the
    // old daemon's established keeper connection and the program stay alive.
    let keeper_paths: Vec<_> = fs::read_dir(sandbox.run_dir().join("keepers"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    for path in &keeper_paths {
        fs::rename(path, path.with_extension("hidden")).unwrap();
    }
    let no_keepers = inspect();
    for path in &keeper_paths {
        fs::rename(path.with_extension("hidden"), path).unwrap();
    }
    assert_eq!(no_keepers["daemon"]["legacyMigration"]["status"], "blocked");
    assert_eq!(prove(&previous, "migration-missing-keeper", false), shell);

    // A valid-looking record for the wrong process must never authorize a
    // signal. The namespace supervisor is deliberately *not* the socket peer.
    let mut wrong_identity = info.clone();
    wrong_identity["pid"] = serde_json::json!(std::process::id());
    fs::write(
        sandbox.info_path(),
        serde_json::to_vec(&wrong_identity).unwrap(),
    )
    .unwrap();
    let rejected = inspect();
    assert_eq!(rejected["daemon"]["legacyMigration"]["status"], "blocked");
    let rejected = run_result(current, &["remote-migrate", "--expected", token]);
    assert!(!rejected.status.success());
    fs::write(sandbox.info_path(), &before).unwrap();
    assert_eq!(prove(&previous, "migration-wrong-identity", false), shell);

    // The token binds the exact metadata, even fields not used for transport.
    let mut changed = info.clone();
    changed["startedAt"] = serde_json::json!("changed-record");
    fs::write(sandbox.info_path(), serde_json::to_vec(&changed).unwrap()).unwrap();
    let rejected = run_result(current, &["remote-migrate", "--expected", token]);
    assert_eq!(rejected.status.code(), Some(75));
    fs::write(sandbox.info_path(), &before).unwrap();
    assert_eq!(prove(&previous, "migration-changed-record", false), shell);

    // This is the explicit app action: one checked SIGTERM, then normal keeper
    // adoption by the new binary. A lost successful response must not make
    // repeating the old intent restart the replacement daemon.
    run(current, &["remote-migrate", "--expected", token]);
    let replacement = fs::read(sandbox.info_path()).unwrap();
    let replay = run_result(current, &["remote-migrate", "--expected", token]);
    assert!(!replay.status.success());
    assert_eq!(fs::read(sandbox.info_path()).unwrap(), replacement);
    owned.pin(
        sandbox.read_info().unwrap()["pid"].as_u64().unwrap() as u32,
        &sandbox,
    );
    assert_eq!(prove(current, "rollback-return", false), shell);
    assert_eq!(
        prove_historical(current, "historical-after-migration", false),
        historical_shell
    );
    let historical_adopted: serde_json::Value = serde_json::from_slice(
        &run(
            current,
            &["list-panes", "--screen", historical_screen, "--json"],
        )
        .stdout,
    )
    .unwrap();
    assert_eq!(
        historical_adopted[historical_pane]["sessionId"],
        historical_session
    );
    assert_eq!(
        fs::read_link(format!("/proc/{historical_keeper}/exe")).unwrap(),
        previous,
        "migration retained the historical keeper process"
    );
    assert_eq!(sockets(), original_sockets);
    assert!(
        unrelated.try_wait().unwrap().is_none(),
        "migration and rejected replay leave the unrelated owned child running"
    );
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
    run(current, &["daemon", "stop", "--end-sessions"]);
    wait_until("all rollback process generations exit", || {
        owned
            .0
            .iter()
            .all(|file| matches!(Owned::exited(file), Ok(true)))
    });
    assert_eq!(sandbox.keeper_sockets(), 0);
    sandbox.assert_shared_parents_unchanged();
    eprintln!("PROVED historical rollback current→{expected}→current: current and historical keeper executables both retain original shell PID, private state, session and keeper inode; unrelated process survives; exact processes exited");
}

// ── access-control: the owner's control socket ──────────────────────────────
//
// Smoke: these run `splitty daemon start` and reach its control socket as a
// pane's CLI does; only the real process shows the socket's file modes, its
// peer check and the routes the binary serves there.

#[path = "support/command.rs"]
#[cfg(target_os = "linux")]
mod isolated_command;
#[path = "support/processes.rs"]
#[cfg(target_os = "linux")]
mod process_cleanup;

impl Sandbox {
    /// The running daemon's owner control socket, as daemon.json names it.
    fn control_socket(&self) -> PathBuf {
        PathBuf::from(
            self.read_info().expect("a running daemon")["socket"]
                .as_str()
                .expect("socket in daemon.json"),
        )
    }
}

/// Minimal HTTP-over-Unix-socket request, mirroring what the CLI does.
fn control_request(
    socket: &std::path::Path,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, String) {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(socket).expect("connect to control socket");
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();

    let payload = body.unwrap_or("");
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: splitty\r\nConnection: close\r\n");
    if body.is_some() {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    }
    request.push_str("\r\n");
    request.push_str(payload);

    stream.write_all(request.as_bytes()).expect("write");

    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok();

    let status = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    (status, raw)
}

/// The JSON body of a [`control_request`] response.
fn control_body(response: &str) -> serde_json::Value {
    serde_json::from_str(response.split_once("\r\n\r\n").expect("body").1).expect("JSON body")
}

/// The multiplexer operations the in-shell CLI needs are reachable, with no
/// credential of any kind — filesystem permissions are the authorisation.
#[test]
fn control_socket_serves_mux_operations() {
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let socket = sandbox.control_socket();

    let (status, body) = control_request(&socket, "GET", "/api/mux/status", None);
    assert_eq!(
        status, 200,
        "control socket should answer mux status: {body}"
    );
}

/// The heart of SR-05: this channel must not reach anything beyond the
/// multiplexer. Configuration and shell profiles are executable configuration —
/// reaching them would let a build script in a pane persist a change that runs
/// in every future pane, which is the escalation the channel exists to remove.
#[test]
fn control_socket_cannot_reach_privileged_endpoints() {
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let socket = sandbox.control_socket();

    let forbidden = [
        ("GET", "/api/home/settings"),
        ("GET", "/api/shell-profiles"),
        ("GET", "/api/home/theme"),
        ("GET", "/api/templates"),
        ("GET", "/api/build"),
        // Minting browser credentials must stay on the token-authenticated path.
        ("POST", "/api/bootstrap"),
        ("POST", "/api/ws-ticket"),
        ("POST", "/api/session"),
        // A pane process must not open SSH connections or read the user's hosts.
        ("GET", "/ws/remote?target=vm1"),
        ("GET", "/api/remote/hosts"),
    ];

    for (method, path) in forbidden {
        let (status, _) = control_request(&socket, method, path, Some("{}"));
        assert_eq!(
            status, 404,
            "{method} {path} must not be routable over the control socket",
        );
    }
}

/// The socket and its directory must be private to the owner. This is the
/// primary authorisation for the channel, so it is asserted rather than assumed.
#[test]
fn control_socket_is_private_to_the_owner() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let socket = sandbox.control_socket();

    let dir_mode = fs::metadata(socket.parent().unwrap())
        .expect("socket dir")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "the socket directory must not be traversable by others"
    );

    let socket_mode = fs::metadata(&socket).expect("socket").permissions().mode() & 0o777;
    assert_eq!(
        socket_mode, 0o600,
        "the socket itself must not be connectable by others"
    );
}

/// A real grant listener has its own ACL and routes, not a wrapper-side filter.
#[test]
fn agent_control_socket_is_private_and_only_routes_owned_jobs() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let owner = sandbox.control_socket();
    let (status, response) = control_request(
        &owner,
        "POST",
        "/api/agents",
        Some(r#"{"name":"test-agent"}"#),
    );
    assert_eq!(status, 200);
    let grant = control_body(&response);
    let socket = PathBuf::from(grant["socket"].as_str().expect("socket"));
    assert_ne!(socket, owner);
    assert_eq!(
        fs::metadata(&socket).expect("mode").permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(control_request(&socket, "GET", "/api/jobs", None).0, 200);
    for path in ["/api/mux/state", "/api/agents", "/api/bootstrap", "/ws"] {
        assert_eq!(control_request(&socket, "GET", path, None).0, 404);
    }
    let (status, _) = control_request(
        &owner,
        "POST",
        &format!(
            "/api/agents/{}/revoke",
            grant["agentId"].as_str().expect("id")
        ),
        Some("{}"),
    );
    assert_eq!(status, 200);
    assert!(!socket.exists());
}

#[test]
#[cfg(target_os = "linux")]
fn agent_socket_rejects_another_uid_even_if_file_modes_are_loosened() {
    use std::os::unix::fs::PermissionsExt;
    if !nix::unistd::geteuid().is_root() {
        eprintln!("skipping cross-UID subprocess: requires root; owner modes and route gates are tested separately");
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let owner = sandbox.control_socket();
    let (_, response) = control_request(
        &owner,
        "POST",
        "/api/agents",
        Some(r#"{"name":"peer-check"}"#),
    );
    let grant = control_body(&response);
    let socket = PathBuf::from(grant["socket"].as_str().expect("socket"));
    let home = sandbox.home.path().canonicalize().expect("canonical home");
    let mut parent = socket.parent();
    while let Some(path) = parent {
        if !path.starts_with(&home) {
            break;
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o711)).expect("test traversal");
        parent = path.parent();
    }
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o666)).expect("test socket mode");
    let script="import socket,sys\ns=socket.socket(socket.AF_UNIX);s.settimeout(5)\ntry:s.connect(sys.argv[1])\nexcept PermissionError:sys.exit(4)\ntry:\n s.sendall(b'GET /api/jobs HTTP/1.1\\r\\nHost: localhost\\r\\n\\r\\n');data=s.recv(1024)\nexcept (ConnectionResetError,BrokenPipeError):data=b''\nsys.exit(1 if data else 0)";
    let output = Command::new("python3")
        .args(["-c", script])
        .arg(&socket)
        .uid(65534)
        .gid(65534)
        .output()
        .expect("different uid");
    assert_eq!(
        output.status.code(),
        Some(0),
        "peer gate did not reject: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// control-api: `create_screen` for a screen that still has panes is refused
/// like any invalid op, and leaves the screen as it was (workspace-model,
/// Screen Creation).
#[test]
fn create_screen_over_a_screen_with_panes_is_rejected() {
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let socket = sandbox.control_socket();
    let create = |pane: &str| {
        control_request(
            &socket,
            "POST",
            "/api/mux/op",
            Some(
                &serde_json::json!({
                    "screenId": "s-api",
                    "op": { "type": "create_screen", "screenId": "s-api", "newPaneId": pane },
                })
                .to_string(),
            ),
        )
    };

    let (status, response) = create("p-first");
    assert_eq!(status, 200, "first create_screen: {response}");

    let (status, response) = create("p-second");
    assert_eq!(status, 400, "second create_screen: {response}");
    let body = control_body(&response);
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("already exists"),
        "body: {body}",
    );

    let (status, response) = control_request(&socket, "GET", "/api/mux/screen/s-api", None);
    assert_eq!(status, 200, "{response}");
    let screen = control_body(&response);
    let panes = screen["panes"].as_object().expect("panes");
    assert!(
        panes.contains_key("p-first") && !panes.contains_key("p-second"),
        "panes: {panes:?}"
    );
}

/// A leaking descendant can retain diagnostic descriptors after its command
/// exits. A real child is needed to prove the inventory remains reachable.
#[cfg(target_os = "linux")]
#[test]
fn inherited_diagnostics_do_not_block_failed_command_inventory() {
    let fixture = tempfile::tempdir().unwrap();
    let marker = fixture.path().join("descendant.pid");
    let child = Command::new("sh")
        .args(["-c", "sleep 30 & echo $! > \"$1\"; exit 1", "sh"])
        .arg(&marker)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(File::create(fixture.path().join("stdout")).unwrap())
        .stderr(File::create(fixture.path().join("stderr")).unwrap())
        .spawn()
        .unwrap();
    let group = child.id();
    let started = Instant::now();
    let status = isolated_command::wait(child);
    let elapsed = started.elapsed();
    let members = process_cleanup::live_group_members(group);
    let descendant: u32 = fs::read_to_string(marker).unwrap().trim().parse().unwrap();
    // End the deliberately leaked fixture before reporting its assertions.
    killpg(Pid::from_raw(group.try_into().unwrap()), Signal::SIGKILL).unwrap();
    process_cleanup::assert_group_exited(group);
    assert_eq!(status.code(), Some(1));
    assert!(
        elapsed < Duration::from_secs(5),
        "leader wait inherited a descendant's output lifetime"
    );
    assert!(
        members.contains(&descendant),
        "the orphan remains visible to the cleanup inventory"
    );
}

// ── cli: comments and reviews ───────────────────────────────────────────────

/// A window of the sandbox's daemon: a native connection over its private
/// client socket, past its welcome.
struct Window(splitty_server::native::client::Client);

impl Window {
    async fn open(sandbox: &Sandbox) -> Self {
        let info: splitty_core::daemon::DaemonInfo =
            serde_json::from_value(sandbox.read_info().expect("a running daemon")).unwrap();
        let client = splitty_server::native::client::Connector::default()
            .local(&info)
            .await
            .expect("native window");
        let mut window = Self(client);
        window.message_of("mux.welcome").await;
        window
    }

    /// The next message of type `kind`, past others.
    async fn message_of(&mut self, kind: &str) -> serde_json::Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let message = self.0.recv().await.expect("open window");
                if message.frame.kind != splitty_transport::wire::Kind::Text {
                    continue;
                }
                let value: serde_json::Value = message.frame.decode().expect("JSON");
                if value["type"] == kind {
                    return value;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no {kind} in time"))
    }

    fn send(&self, value: serde_json::Value) {
        self.0
            .sender()
            .data(
                splitty_transport::wire::Kind::Text,
                value.to_string().as_bytes(),
            )
            .expect("send");
    }

    /// Screen `s-cli` as this window sees it after everything it sent before.
    async fn screen(&mut self) -> serde_json::Value {
        self.send(serde_json::json!({"type": "mux.resync", "screenId": "s-cli"}));
        self.message_of("mux.screen_snapshot").await["screen"].clone()
    }

    /// The text viewer titled `Plan`, the wait for its review and its record,
    /// once a `splitty review` waits for it.
    async fn waiting_review(&mut self) -> (String, String, serde_json::Value) {
        loop {
            let screen = self.screen().await;
            let waiting =
                screen["panes"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .find_map(|(id, pane)| {
                        let viewer = &pane["viewer"];
                        let wait = viewer["reviewWait"]["id"].as_str()?;
                        (viewer["source"]["title"] == "Plan")
                            .then(|| (id.clone(), wait.to_owned(), viewer.clone()))
                    });
            if let Some(waiting) = waiting {
                return waiting;
            }
            self.message_of("mux.nudge").await;
        }
    }
}

/// `splitty` run in `dir` from the pane `p-agent` of screen `s-cli`, which
/// finds the daemon through `SPLITTY_SOCK` only.
fn pane_cli(sandbox: &Sandbox, dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
    command
        .current_dir(dir)
        .env_clear()
        .env("HOME", sandbox.home.path())
        .env("SPLITTY_SOCK", sandbox.control_socket())
        .env("SPLITTY_SCREEN_ID", "s-cli")
        .env("SPLITTY_PANE_ID", "p-agent")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// `command` with `args`, run to its end off the runtime's thread.
async fn run_cli(mut command: Command, args: &[&str]) -> Output {
    command.args(args);
    tokio::task::spawn_blocking(move || command.output().expect("splitty"))
        .await
        .expect("run")
}

/// `splitty review <args>` with `text` on standard input, left waiting.
fn review(sandbox: &Sandbox, dir: &std::path::Path, args: &[&str], text: &str) -> Child {
    use std::io::Write;
    let mut child = pane_cli(sandbox, dir)
        .arg("review")
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .expect("splitty review");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(text.as_bytes())
        .expect("text");
    child
}

async fn finished(child: Child) -> Output {
    tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || child.wait_with_output().expect("splitty review")),
    )
    .await
    .expect("the command ends in time")
    .expect("wait")
}

/// cli, Comments Command (Nothing sent yet, Read a sent review, Reply, Two
/// viewers of one file), Review Command (A plan reviewed, The next revision
/// of the plan, Approved, Outcome as JSON, The pane closed) and Review Hook
/// For Claude Code (Changes requested, Outside Splitty): the
/// real `splitty` in a pane's environment against a real daemon, answered by
/// a native window. Smoke: only here are the CLI's discovery of the server,
/// its output and its exit status visible; the endpoints are tested in
/// process (`http::tests::annotations`, `http::tests::reviews`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_reads_and_answers_a_review_through_the_cli() {
    use serde_json::json;
    let sandbox = Sandbox::new();
    sandbox.start_daemon();
    let (status, response) = control_request(
        &sandbox.control_socket(),
        "POST",
        "/api/mux/op",
        Some(
            &json!({
                "screenId": "s-cli",
                "op": {"type": "create_screen", "screenId": "s-cli", "newPaneId": "p-agent"},
            })
            .to_string(),
        ),
    );
    assert_eq!(status, 200, "create_screen: {response}");
    let dir = sandbox.home.path().join("app");
    fs::create_dir(&dir).expect("app");
    let dir = dir.canonicalize().expect("canonical");
    fs::write(
        dir.join("PLAN.md"),
        "# Plan\n\nOn failure, retry three times.\n\nThen cache the list.\n",
    )
    .expect("plan");
    let plan = dir.join("PLAN.md").to_str().expect("UTF-8").to_owned();
    let mut window = Window::open(&sandbox).await;

    let opened = run_cli(pane_cli(&sandbox, &dir), &["open", "PLAN.md"]).await;
    assert!(opened.status.success(), "{opened:?}");
    let stdout = String::from_utf8(opened.stdout).expect("UTF-8");
    let viewer = stdout.trim().rsplit(' ').next().expect("pane").to_owned();
    assert!(viewer.starts_with("viewer-"), "{stdout}");

    let listed = run_cli(pane_cli(&sandbox, &dir), &["comments", "PLAN.md"]).await;
    assert!(listed.status.success(), "{listed:?}");
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        format!("No comments on {plan}.\n"),
        "nothing sent yet"
    );

    let handed_over = json!([
        {"id": "ann-0000000000000001", "kind": "comment", "body": "Why not five times?",
         "target": {"view": "source", "quote": "retry three times", "prefix": "On failure, ",
                    "suffix": ".", "startLine": 3, "endLine": 3}},
        {"id": "ann-0000000000000002", "kind": "change", "replacement": "compute it on demand",
         "target": {"view": "source", "quote": "cache the list", "prefix": "Then ",
                    "suffix": ".", "startLine": 5, "endLine": 5}},
    ]);
    window.send(json!({"type": "mux.op", "screenId": "s-cli", "op": {
        "type": "put_annotations", "paneId": viewer, "annotations": handed_over,
    }}));
    // The review held both as they were put.
    window.send(json!({"type": "mux.op", "screenId": "s-cli", "op": {
        "type": "mark_sent", "paneId": viewer, "entries": handed_over,
    }}));
    let annotations = window.screen().await["panes"][&viewer]["viewer"]["annotations"].clone();
    assert_eq!(annotations[1]["sent"], true, "{annotations}");

    let listed = run_cli(pane_cli(&sandbox, &dir), &["comments", "PLAN.md"]).await;
    assert!(listed.status.success(), "{listed:?}");
    let listing = String::from_utf8(listed.stdout).expect("UTF-8");
    for part in [
        "# Review of PLAN.md".to_owned(),
        format!("Document: {plan}\nViewer pane: {viewer}"),
        "## #1 Line 3, comment\n> retry three times\n\nWhy not five times?".to_owned(),
        "## #2 Line 5, suggested change\n> cache the list\n\nReplace with:\n```\ncompute it on demand\n```"
            .to_owned(),
    ] {
        assert!(listing.contains(&part), "{part}\n---\n{listing}");
    }

    let replied = run_cli(
        pane_cli(&sandbox, &dir),
        &[
            "comments",
            "reply",
            "PLAN.md",
            "2",
            "Switched to on-demand computation.",
        ],
    )
    .await;
    assert!(replied.status.success(), "{replied:?}");
    assert_eq!(
        String::from_utf8_lossy(&replied.stdout),
        format!("Replied to #2 in {plan}\n")
    );
    let thread =
        window.screen().await["panes"][&viewer]["viewer"]["annotations"][1]["replies"].clone();
    assert_eq!(thread[0]["author"], "agent", "{thread}");
    assert_eq!(thread[0]["body"], "Switched to on-demand computation.");
    assert_eq!(thread[0]["from"]["paneId"], "p-agent", "{thread}");
    let listed = run_cli(pane_cli(&sandbox, &dir), &["comments", &viewer, "--json"]).await;
    assert!(listed.status.success(), "{listed:?}");
    let answer: serde_json::Value = serde_json::from_slice(&listed.stdout).expect("JSON");
    assert_eq!(
        answer["documents"][0]["annotations"][1]["replies"][0]["body"],
        "Switched to on-demand computation.",
        "{answer}"
    );

    window.send(json!({"type": "mux.op", "screenId": "s-cli", "op": {
        "type": "open_viewer", "paneId": "p-agent", "newPaneId": "viewer-00000000000000aa",
        "placement": "tab", "source": {"kind": "file", "path": plan},
    }}));
    window.screen().await;
    let refused = run_cli(
        pane_cli(&sandbox, &dir),
        &["comments", "reply", "PLAN.md", "1", "Which one?"],
    )
    .await;
    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    let error = String::from_utf8_lossy(&refused.stderr);
    assert!(
        error.contains(&viewer) && error.contains("viewer-00000000000000aa"),
        "two viewers of one file: {error}"
    );

    let first = "1. Retry three times.\n2. Cache the list.\n";
    let changes = "# Review of Plan\n\nVerdict: changes requested\n\n## #1 Line 1, comment\n> Retry three times.\n\nWhy not five times?\n";
    let child = review(&sandbox, &dir, &["--text", "--title", "Plan"], first);
    let (plan_pane, wait, record) = window.waiting_review().await;
    assert_eq!(record["source"]["kind"], "text", "{record}");
    let mut revision = record["revision"].as_u64().expect("revision");
    window.send(
        json!({"type": "viewer.review_answer", "screenId": "s-cli", "paneId": plan_pane,
               "waitId": wait, "outcome": "changes_requested", "review": changes}),
    );
    assert_eq!(
        window.message_of("viewer.review_answered").await["accepted"],
        true
    );
    let output = finished(child).await;
    // Only the binary's exit path shows the status of a request for changes.
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), changes);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(&plan_pane),
        "{output:?}"
    );

    let revised = "1. Retry five times with backoff.\n2. Cache the list.\n";
    for (json, printed) in [(false, ""), (true, "{\"outcome\":\"approved\"}\n")] {
        let args: &[&str] = if json {
            &["--text", "--title", "Plan", "--json"]
        } else {
            &["--text", "--title", "Plan"]
        };
        let child = review(&sandbox, &dir, args, revised);
        let (again, wait, record) = window.waiting_review().await;
        assert_eq!(again, plan_pane, "the next revision goes to the same pane");
        let shown = record["revision"].as_u64().expect("revision");
        assert!(shown > revision, "the pane shows the new text: {record}");
        revision = shown;
        window.send(
            json!({"type": "viewer.review_answer", "screenId": "s-cli", "paneId": again,
                   "waitId": wait, "outcome": "approved"}),
        );
        assert_eq!(
            window.message_of("viewer.review_answered").await["accepted"],
            true
        );
        let output = finished(child).await;
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), printed);
    }

    // Claude Code's hook: the plan of its event goes to the same pane, and a
    // request for changes is a denial with the review, with status 0.
    let event = json!({
        "hook_event_name": "PermissionRequest", "tool_name": "ExitPlanMode",
        "cwd": dir, "tool_input": {"plan": revised},
    })
    .to_string();
    let child = review(&sandbox, &dir, &["--hook", "claude-code"], &event);
    let (again, wait, record) = window.waiting_review().await;
    assert_eq!(again, plan_pane, "{record}");
    window.send(
        json!({"type": "viewer.review_answer", "screenId": "s-cli", "paneId": again,
               "waitId": wait, "outcome": "changes_requested", "review": changes}),
    );
    assert_eq!(
        window.message_of("viewer.review_answered").await["accepted"],
        true
    );
    let output = finished(child).await;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let decision: serde_json::Value = serde_json::from_slice(&output.stdout).expect("decision");
    assert_eq!(
        decision,
        json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest",
               "decision": {"behavior": "deny", "message": changes}}})
    );
    // Outside a Splitty terminal the hook decides nothing, and only the
    // binary's exit path shows that it still exits 0.
    let mut outside = pane_cli(&sandbox, &dir);
    outside.env_remove("SPLITTY_PANE_ID");
    let mut child = outside
        .args(["review", "--hook", "claude-code"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("splitty review");
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(event.as_bytes())
            .expect("event");
    }
    let output = finished(child).await;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Claude Code asks the user itself"),
        "{output:?}"
    );

    let child = review(&sandbox, &dir, &["--text", "--title", "Plan"], revised);
    let (pane, _, _) = window.waiting_review().await;
    window.send(
        json!({"type": "mux.op", "screenId": "s-cli", "op": {"type": "close_pane", "paneId": pane}}),
    );
    let output = finished(child).await;
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("closed without an answer"),
        "{output:?}"
    );
}

/// Smoke: process exit, filesystem inventory and executable launch probes require
/// the binary; router tests cannot show that a CLI entry point starts no service.
#[test]
fn browser_command_stubs_exit_without_workspace_sockets_credentials_or_ssh() {
    use std::os::unix::fs::PermissionsExt;
    for args in [
        vec!["serve", "--addr", "127.0.0.1:9800", "--open"],
        vec!["serve", "--desktop-stdio"],
        vec!["connect", "me@192.0.2.10", "--json"],
        vec!["issue-bootstrap"],
    ] {
        let sandbox = Sandbox::new();
        let bin = sandbox.home.path().join("probes");
        fs::create_dir(&bin).unwrap();
        let called = sandbox.home.path().join("called");
        for program in ["ssh", "xdg-open", "open", "wslview", "powershell.exe"] {
            let path = bin.join(program);
            fs::write(
                &path,
                format!("#!/bin/sh\n: > '{}'\nexit 99\n", called.display()),
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
        sandbox.isolate(&mut command);
        command
            .args(&args)
            .env("PATH", &bin)
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = command.spawn().expect("stub");
        let group = Pid::from_raw(child.id().try_into().unwrap());
        let output = child.wait_with_output().expect("stub exit");
        assert_eq!(
            killpg(group, None),
            Err(nix::errno::Errno::ESRCH),
            "stub left a process that could retain a listener"
        );
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}",
            sandbox.describe(&output)
        );
        let message = if args.contains(&"--json") {
            assert_eq!(stdout(&output).lines().count(), 1);
            assert!(output.stderr.is_empty());
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["code"], "removed");
            value["error"].as_str().unwrap().to_owned()
        } else {
            assert!(output.stdout.is_empty());
            assert_eq!(stderr(&output).lines().count(), 1);
            stderr(&output)
        };
        for hint in [
            "splitty daemon start",
            "desktop app",
            "splitty attach",
            "ssh -t <host> splitty attach",
        ] {
            assert!(message.contains(hint), "{message}");
        }
        if args.contains(&"--desktop-stdio") {
            assert!(message.contains("desktop-backend --transport native-v1 --backend embedded"));
        }
        assert!(!called.exists(), "stub launched an external program");
        assert!(
            !sandbox.config_dir().exists(),
            "stub created configuration or workspace"
        );
        assert!(
            !sandbox.runtime_dir.exists(),
            "stub created socket directory"
        );
        assert!(!sandbox.run_dir().exists(), "stub created daemon endpoints");
    }
}
