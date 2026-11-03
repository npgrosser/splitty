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

    #[cfg(target_os = "linux")]
    fn fake_ssh_to(&self, remote: &Sandbox) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = self.home.path().join("ssh-bin");
        fs::create_dir(&bin).unwrap();
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_splitty"), bin.join("splitty")).unwrap();
        let quote = splitty_remote::ssh::quote;
        let script = format!("#!/bin/sh\nfor argument do line=\"$argument\"; done\nprintf '%s\\n' \"$line\" >> {}\nexec env -u SPLITTY_SOCK -u SPLITTY_AGENT_SOCK -u SPLITTY_CONFIG_DIR HOME={} XDG_CONFIG_HOME={} XDG_DATA_HOME={} XDG_RUNTIME_DIR={} /bin/sh -c \"$line\"\n",
            quote(self.home.path().join("ssh-calls").to_str().unwrap()), quote(remote.home.path().to_str().unwrap()),
            quote(remote.home.path().join("config").to_str().unwrap()), quote(remote.home.path().join("data").to_str().unwrap()),
            quote(remote.runtime_dir.to_str().unwrap()));
        fs::write(bin.join("ssh"), script).unwrap();
        fs::set_permissions(bin.join("ssh"), fs::Permissions::from_mode(0o700)).unwrap();
        bin
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
        vec!["issue-bootstrap"],
        // cli, Scoped Administrative Commands Are Refused: an agent grant
        // never reaches a terminal; refused before the token file is read.
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

    let start = sandbox.splitty(&["daemon", "start"]);
    assert!(
        start.status.success(),
        "daemon start failed\n{}",
        sandbox.describe(&start)
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
    let record = fs::read(sandbox.info_path()).unwrap();
    let info: splitty_core::daemon::DaemonInfo = serde_json::from_slice(&record).unwrap();
    assert_no_tcp_listener(info.pid);
    for corrupt in [Some(b"{broken".as_slice()), None] {
        if let Some(bytes) = corrupt {
            fs::write(sandbox.info_path(), bytes).unwrap();
        } else {
            fs::remove_file(sandbox.info_path()).unwrap();
        }
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
    splitty_core::daemon::private_directory(&directory).unwrap();
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
    let (screen, pane, _) = sandbox.shell();
    let home = sandbox.home.path().display().to_string();
    sandbox.type_line(&screen, &pane, &format!("echo $$ > {home}/persistent-pid"));
    let persistent = sandbox.wait_for("persistent-pid");
    let record = fs::read(sandbox.info_path()).unwrap();
    let state = fs::read(sandbox.config_dir().join("state.json")).unwrap();
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
/// listener, it serves on its own and announces that on stdout.
#[test]
fn daemon_run_serves_an_inherited_listener() {
    let sandbox = Sandbox::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut bound = runtime
        .block_on(splitty_server::native::endpoint::bind(
            &sandbox.run_dir().join("client.sock"),
        ))
        .expect("bind native endpoint");
    let fd = bound.listener.as_raw_fd();
    let lock_fd = bound.guard.lock.file().as_raw_fd();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_splitty"));
    cmd.args([
        "daemon",
        "run",
        "--listen-fd",
        &fd.to_string(),
        "--lock-fd",
        &lock_fd.to_string(),
    ]);
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
            nix::fcntl::fcntl(
                lock_fd,
                nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::empty()),
            )?;
            Ok(())
        });
    }
    let mut child = ChildGuard(cmd.spawn().expect("spawn daemon run"));
    bound.guard.preserve_endpoint();
    drop(bound);
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

/// A descriptor that is not a listener is refused with an error line, before
/// anything names the process in `daemon.json`.
#[test]
fn daemon_run_refuses_a_descriptor_that_is_not_a_listener() {
    let sandbox = Sandbox::new();
    let out = sandbox.splitty(&["daemon", "run", "--listen-fd", "900", "--lock-fd", "901"]);
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
        assert!(stderr(&out).contains("serve --daemon"));
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

    let started = json_of(
        &sandbox.splitty(&["remote-handshake", "--json", "--ensure-daemon"]),
        &sandbox,
    );
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

// Smoke: real inherited pipes must detach the SSH bridge while the host's
// keeper remains live; an in-process duplex stream cannot prove process exit.
#[test]
fn an_explicit_daemon_browser_gateway_survives_daemon_replacement_and_detaches_cleanly() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let sandbox = Sandbox::new();
    let absent = sandbox.splitty(&[
        "serve",
        "--daemon",
        "--addr",
        "127.0.0.1:0",
        "--oneshot-token",
    ]);
    assert_eq!(absent.status.code(), Some(1));
    assert!(stderr(&absent).contains("daemon start"));
    assert!(sandbox.read_info().is_none());
    assert!(!sandbox.config_dir().join("auth-token").exists());
    sandbox.start_daemon();
    let (screen, pane, _) = sandbox.shell();
    sandbox.type_line(
        &screen,
        &pane,
        &format!("echo $$ > '{}'", sandbox.path("gateway-shell").display()),
    );
    let shell = sandbox.wait_for("gateway-shell");
    let original = sandbox.read_info().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
    sandbox
        .isolate(&mut command)
        .args([
            "serve",
            "--daemon",
            "--addr",
            "127.0.0.1:0",
            "--token",
            "gateway-owner-token",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut gateway = ChildGuard(command.spawn().unwrap());
    let mut reader = BufReader::new(gateway.0.stdout.take().unwrap());
    let mut port = None;
    let mut bootstrap = None;
    let mut url = None;
    for _ in 0..10 {
        let (line, returned) = read_line_within(reader).expect("browser gateway startup output");
        reader = returned;
        if let Some(value) = line.trim().strip_prefix("SPLITTY_PORT=") {
            port = Some(value.parse::<u16>().unwrap());
        }
        if let Some(value) = line.trim().strip_prefix("SPLITTY_BOOTSTRAP=") {
            bootstrap = Some(value.to_owned());
        }
        if let Some(value) = line.trim().strip_prefix("SPLITTY_URL=") {
            url = Some(value.to_owned());
        }
        if port.is_some() && bootstrap.is_some() && url.is_some() {
            break;
        }
    }
    let port = port.unwrap();
    assert_eq!(
        url.unwrap(),
        format!(
            "http://127.0.0.1:{port}/?bootstrap={}&desktop=1",
            bootstrap.as_ref().unwrap()
        )
    );
    assert_eq!(
        process_inventory::tcp_listeners(gateway.0.id()),
        [std::net::SocketAddr::from(([127, 0, 0, 1], port))]
    );
    assert_no_tcp_listener(original["pid"].as_u64().unwrap().try_into().unwrap());
    drop(reader); // Text readiness output has no ownership of this gateway.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let missing_address = sandbox.splitty(&["issue-bootstrap"]);
    assert_eq!(missing_address.status.code(), Some(1));
    assert!(stderr(&missing_address).contains("--addr"));
    let minted = sandbox.splitty(&[
        "issue-bootstrap",
        "--addr",
        &format!("127.0.0.1:{port}"),
        "--token",
        "gateway-owner-token",
    ]);
    assert!(minted.status.success(), "{}", sandbox.describe(&minted));
    let minted = stdout(&minted)
        .trim()
        .strip_prefix("SPLITTY_BOOTSTRAP=")
        .unwrap()
        .to_owned();
    assert_ne!(Some(&minted), bootstrap.as_ref());
    runtime.block_on(async {
        let response = http
            .post(format!("{base}/api/session"))
            .json(&serde_json::json!({"bootstrap":minted}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    });
    let cookie = runtime.block_on(async {
        let response = http
            .post(format!("{base}/api/session"))
            .json(&serde_json::json!({"bootstrap":bootstrap.unwrap()}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    });
    let status = || {
        runtime.block_on(async {
            let response = http
                .get(format!("{base}/api/mux/status"))
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap();
            let code = response.status();
            (code, response.json::<serde_json::Value>().await.unwrap())
        })
    };
    assert_eq!(status().1["instance"], original["instance"]);
    let wrong = sandbox.splitty(&["daemon", "stop", "--if-instance", &"0".repeat(32)]);
    assert_eq!(wrong.status.code(), Some(75));
    assert_eq!(
        sandbox.read_info().unwrap()["instance"],
        original["instance"]
    );
    let stopped = sandbox.splitty(&[
        "daemon",
        "stop",
        "--if-instance",
        original["instance"].as_str().unwrap(),
    ]);
    assert!(stopped.status.success(), "{}", sandbox.describe(&stopped));
    sandbox.start_daemon();
    let replacement = sandbox.read_info().unwrap();
    assert_ne!(replacement["instance"], original["instance"]);
    let deadline = Instant::now() + READ_TIMEOUT;
    loop {
        let (code, body) = status();
        if code == reqwest::StatusCode::OK {
            assert_eq!(body["instance"], replacement["instance"]);
            break;
        }
        assert_eq!(code, reqwest::StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(
            Instant::now() < deadline,
            "gateway did not rediscover the daemon"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        process_inventory::tcp_listeners(gateway.0.id()),
        [std::net::SocketAddr::from(([127, 0, 0, 1], port))]
    );
    assert_no_tcp_listener(replacement["pid"].as_u64().unwrap().try_into().unwrap());
    kill(
        Pid::from_raw(i32::try_from(gateway.0.id()).unwrap()),
        Signal::SIGTERM,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = gateway.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "gateway did not stop");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        sandbox.read_info().unwrap()["instance"],
        replacement["instance"]
    );
    assert!(
        kill(Pid::from_raw(shell.trim().parse().unwrap()), None).is_ok(),
        "gateway shutdown leaves the kept shell running"
    );
    assert!(!sandbox.config_dir().join("auth-token").exists());
    assert!(
        std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).is_err(),
        "the browser listener ends with its command"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn owned_ssh_helper_detaches_its_real_exec_pipes_and_leaves_both_daemons_untouched() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let local = Sandbox::new();
    local.start_daemon();
    let remote = Sandbox::new();
    remote.start_daemon();
    let (screen, pane, _) = remote.shell();
    remote.type_line(
        &screen,
        &pane,
        &format!(
            "echo $$ > '{}'",
            remote.home.path().join("host-shell").display()
        ),
    );
    let shell = remote.wait_for("host-shell");
    let local_record = fs::read(local.info_path()).unwrap();
    let remote_record = fs::read(remote.info_path()).unwrap();
    let info: splitty_core::daemon::DaemonInfo = serde_json::from_slice(&remote_record).unwrap();
    let bin = local.fake_ssh_to(&remote);
    let quote = splitty_remote::ssh::quote;
    let helper_command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
        local
            .isolate(&mut command)
            .args([
                "desktop-backend",
                "--transport",
                "native-v1",
                "--backend",
                "ssh",
                "--target",
                "test-host",
            ])
            .env(
                "PATH",
                std::env::join_paths(
                    std::iter::once(bin.clone())
                        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                )
                .unwrap(),
            );
        tokio::process::Command::from(command)
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        use splitty_transport::{
            admission, negotiation,
            wire::{self, Kind},
        };
        let mut helper = splitty_remote::bridge::NativeBridge::spawn(
            helper_command(),
            admission::Assignment::new(admission::Backend::new(), admission::Domain::new()),
            negotiation::Expected {
                instance: info.instance.as_deref(),
                purpose: &wire::Purpose::Host {
                    target: "test-host".into(),
                },
                mux_protocol: 1,
                preparation: READ_TIMEOUT,
            },
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(
            helper
                .recv()
                .await
                .unwrap()
                .frame
                .decode::<serde_json::Value>()
                .unwrap()["type"],
            "mux.welcome"
        );
        assert_no_tcp_listener(helper.id().unwrap());
        assert_no_tcp_listener(info.pid);
        helper
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
        let reply = tokio::time::timeout(READ_TIMEOUT, async {
            loop {
                let message = helper.recv().await.unwrap();
                if message.frame.kind == Kind::Reply {
                    break message.frame.decode::<wire::Reply>().unwrap();
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&reply.body).unwrap()["instance"],
            info.instance.as_ref().unwrap().as_str()
        );
        let started = Instant::now();
        let (transport, ended) = helper.finish().await;
        transport.unwrap();
        assert!(ended.status.unwrap().success(), "{}", ended.stderr);
        assert!(started.elapsed() < Duration::from_secs(3));
        // Exercise EOF during real setup exec, before there is any Ready or
        // live host bridge. The helper must reap that command before exiting.
        let pending_pid = local.home.path().join("pending-ssh-pid");
        fs::write(
            bin.join("ssh"),
            format!(
                "#!/bin/sh\necho $$ > {}\nexec sleep 30\n",
                quote(pending_pid.to_str().unwrap())
            ),
        )
        .unwrap();
        let mut child = helper_command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        negotiation::client(&mut output, &mut input, &Default::default())
            .await
            .unwrap();
        let pid = tokio::time::timeout(READ_TIMEOUT, async {
            loop {
                if let Ok(text) = tokio::fs::read_to_string(&pending_pid).await {
                    if let Ok(pid) = text.trim().parse::<i32>() {
                        break pid;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_no_tcp_listener(child.id().unwrap());
        drop((input, output));
        assert!(tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
        assert!(
            kill(Pid::from_raw(pid), None).is_err(),
            "helper exit precedes pending SSH cleanup"
        );
        for (message, kind) in [
            (
                "Permission denied (publickey).",
                std::io::ErrorKind::PermissionDenied,
            ),
            (
                "Host key verification failed.",
                std::io::ErrorKind::PermissionDenied,
            ),
            ("Connection refused", std::io::ErrorKind::ConnectionAborted),
        ] {
            fs::write(
                bin.join("ssh"),
                format!(
                    "#!/bin/sh\nprintf '%s\\n' {} >&2\nexit 255\n",
                    quote(message)
                ),
            )
            .unwrap();
            let error = match splitty_remote::bridge::NativeBridge::spawn(
                helper_command(),
                admission::Assignment::new(admission::Backend::new(), admission::Domain::new()),
                negotiation::Expected {
                    instance: info.instance.as_deref(),
                    purpose: &wire::Purpose::Host {
                        target: "test-host".into(),
                    },
                    mux_protocol: 1,
                    preparation: READ_TIMEOUT,
                },
                |_| {},
            )
            .await
            {
                Ok(_) => panic!("SSH failure admitted a backend"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), kind, "{message}: {error}");
            assert!(error.to_string().contains(message), "{error}");
        }
    });
    assert_eq!(fs::read(local.info_path()).unwrap(), local_record);
    assert_eq!(fs::read(remote.info_path()).unwrap(), remote_record);
    let shell: i32 = shell.trim().parse().unwrap();
    assert!(
        kill(Pid::from_raw(shell), None).is_ok(),
        "SSH helper EOF must not end the kept host shell"
    );
    assert!(!local.config_dir().join("auth-token").exists());
    assert!(!remote.config_dir().join("auth-token").exists());
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

// Smoke: the actual CLI owns stdout, its HTTP listener and multiple exec
// workers; in-process routing cannot prove stable cookies/URL across their
// replacement or that browser credentials never reach host commands.
#[test]
#[cfg(target_os = "linux")]
fn connect_keeps_its_local_gateway_and_cookie_when_the_native_host_is_replaced() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let local = Sandbox::new();
    local.start_daemon();
    let remote = Sandbox::new();
    remote.start_daemon();
    let local_record = fs::read(local.info_path()).unwrap();
    let first = remote.read_info().unwrap();
    let (screen, pane, _) = remote.shell();
    remote.type_line(
        &screen,
        &pane,
        &format!(
            "echo $$ > '{}'",
            remote.home.path().join("connect-shell").display()
        ),
    );
    let shell = remote.wait_for("connect-shell");
    let bin = local.fake_ssh_to(&remote);
    let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
    local
        .isolate(&mut command)
        .args(["connect", "test-host", "--no-open", "--json"])
        .env(
            "PATH",
            std::env::join_paths(
                std::iter::once(bin.clone())
                    .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
            )
            .unwrap(),
        )
        .stdout(Stdio::piped())
        .stderr(File::create(local.home.path().join("connect.stderr")).unwrap());
    let mut connect = ChildGuard(command.spawn().unwrap());
    let (send, lines) = mpsc::channel();
    let stdout = connect.0.stdout.take().unwrap();
    let output = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut events = Vec::<serde_json::Value>::new();
    let ready = loop {
        let line = lines
            .recv_timeout(READ_TIMEOUT)
            .expect("connect startup event");
        let event: serde_json::Value = serde_json::from_str(&line).unwrap();
        events.push(event.clone());
        assert_ne!(
            event["event"],
            "failed",
            "{event}; {}",
            fs::read_to_string(local.home.path().join("connect.stderr")).unwrap()
        );
        if event["event"] == "ready" {
            break event;
        }
    };
    let url = reqwest::Url::parse(ready["url"].as_str().unwrap()).unwrap();
    let port = url.port().unwrap();
    assert_eq!(ready["port"], port);
    assert_eq!(
        process_inventory::tcp_listeners(connect.0.id()),
        [std::net::SocketAddr::from(([127, 0, 0, 1], port))]
    );
    let bootstrap = url
        .query_pairs()
        .find(|(name, _)| name == "bootstrap")
        .unwrap()
        .1
        .into_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let cookie = runtime.block_on(async {
        let response = http
            .post(format!("{base}/api/session"))
            .json(&serde_json::json!({"bootstrap":bootstrap}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    });
    let status = || {
        runtime.block_on(async {
            let response = http
                .get(format!("{base}/api/mux/status"))
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap();
            (
                response.status(),
                response.json::<serde_json::Value>().await.unwrap(),
            )
        })
    };
    assert_eq!(status().1["instance"], first["instance"]);
    let local_pid = local.read_info().unwrap()["pid"].as_u64().unwrap();
    assert_no_tcp_listener(u32::try_from(local_pid).unwrap());
    assert_no_tcp_listener(u32::try_from(first["pid"].as_u64().unwrap()).unwrap());
    let stopped = remote.splitty(&[
        "daemon",
        "stop",
        "--if-instance",
        first["instance"].as_str().unwrap(),
    ]);
    assert!(stopped.status.success(), "{}", remote.describe(&stopped));
    // Reconnect itself must start the absent host daemon, without a second URL.
    let restored = loop {
        let line = lines
            .recv_timeout(READ_TIMEOUT)
            .expect("connect restoration event");
        let event: serde_json::Value = serde_json::from_str(&line).unwrap();
        events.push(event.clone());
        assert_ne!(event["event"], "failed", "{event}");
        assert_ne!(event["event"], "ready", "gateway must not be replaced");
        assert_ne!(
            event["event"], "url",
            "existing browser credentials remain valid"
        );
        if event["event"] == "restored" {
            break event;
        }
    };
    assert_eq!(restored["event"], "restored");
    let replacement = remote.read_info().unwrap();
    assert_ne!(replacement["instance"], first["instance"]);
    let deadline = Instant::now() + READ_TIMEOUT;
    loop {
        let (code, body) = status();
        if code == reqwest::StatusCode::OK {
            assert_eq!(body["instance"], replacement["instance"]);
            break;
        }
        assert_eq!(code, reqwest::StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        process_inventory::tcp_listeners(connect.0.id()),
        [std::net::SocketAddr::from(([127, 0, 0, 1], port))]
    );
    assert_no_tcp_listener(replacement["pid"].as_u64().unwrap().try_into().unwrap());
    kill(
        Pid::from_raw(i32::try_from(connect.0.id()).unwrap()),
        Signal::SIGTERM,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = connect.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "connect did not stop");
        std::thread::sleep(Duration::from_millis(20));
    }
    output.join().unwrap();
    events.extend(
        lines
            .try_iter()
            .map(|line| serde_json::from_str(&line).unwrap()),
    );
    assert_eq!(events.last().unwrap()["event"], "closed");
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event"] == "ready")
            .count(),
        1
    );
    let calls = fs::read_to_string(local.home.path().join("ssh-calls")).unwrap();
    for forbidden in [
        "issue-bootstrap",
        "/api/bootstrap",
        "/api/ws-ticket",
        "--port",
        &bootstrap,
        &cookie,
    ] {
        assert!(
            !calls.contains(forbidden),
            "browser credentials and port hints do not reach host commands"
        );
    }
    assert_eq!(fs::read(local.info_path()).unwrap(), local_record);
    assert_eq!(
        remote.read_info().unwrap()["instance"],
        replacement["instance"]
    );
    assert!(kill(Pid::from_raw(shell.trim().parse().unwrap()), None).is_ok());
    assert!(!local.config_dir().join("auth-token").exists());
    assert!(!remote.config_dir().join("auth-token").exists());
    assert!(std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).is_err());
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
            static REQUEST_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
            let id = REQUEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            connection.sender().json(Kind::Request, &wire::Request { id, operation: operation.into(), args: args.as_object().unwrap().clone() }).unwrap();
            tokio::time::timeout(READ_TIMEOUT, async {
                loop {
                    let message = connection.recv().await.expect("live owner stream");
                    if message.frame.kind == Kind::Reply { break message.frame.decode::<wire::Reply>().unwrap(); }
                }
            }).await.unwrap()
        }
        let owner = Owner::spawn(tokio::process::Command::from(command), 1).await.unwrap();
        let pid = i32::try_from(owner.id().unwrap()).unwrap();
        assert_no_tcp_listener(owner.id().unwrap());
        let first_context = queue::Context::default();
        let second_context = queue::Context::default();
        let mut first = owner.open_context(None, false, first_context.clone(), |_| {}).await.unwrap();
        let mut second = owner.open_context(None, false, second_context.clone(), |_| {}).await.unwrap();
        assert_eq!(owner.stream_count(), 2);
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
        tokio::time::timeout(Duration::from_secs(3), owner.close()).await.unwrap();
        assert!(kill(Pid::from_raw(pid), None).is_err(), "owner completion observes helper exit");
        assert!(kill(Pid::from_raw(job_pid), None).is_err(), "owner exit includes its managed job");
        second.finish().await.unwrap();
        assert_eq!(second_context.budget.used(), 0);
        assert_eq!(owner.retained_bytes(), 0);
        assert_eq!(owner.stream_count(), 0);
        assert!(owner.is_closed());
        assert_eq!(fs::read(discovery).unwrap(), b"independent corrupt discovery");
    });
}
