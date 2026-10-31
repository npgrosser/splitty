//! The command search path of the user's login shell, for a desktop app that
//! was not started from a terminal.
//!
//! macOS starts an app from the Dock or the Finder with launchd's `PATH`
//! (`/usr/bin:/bin:/usr/sbin:/sbin`), and a Linux desktop with whatever its
//! session set up; neither has what the user's shell startup adds, such as
//! Homebrew's `/opt/homebrew/bin`. Programs the app starts without a shell
//! inherit that `PATH`: above all `ssh`, which runs the `ProxyCommand` of the
//! user's SSH configuration (`gcloud compute start-iap-tunnel …` for a cloud VM
//! behind IAP) through the user's shell, but without its startup files. So the
//! app asks the login shell once, before it starts anything, as VS Code does.
//!
//! Only `PATH` is taken. Taking the shell's whole environment would carry
//! variables of a shell session (an activated virtualenv, a terminal's
//! settings) into every process of the app.

use std::ffi::{OsStr, OsString};
use std::io::{IsTerminal, Read};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// How long the login shell may take to report its `PATH`. A slower startup
/// delays the app by this much and leaves the inherited `PATH`.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Printed before and after the shell's environment, to find it in whatever
/// the startup files print.
const MARKER: &str = "__SPLITTY_LOGIN_PATH__";

/// How much of the shell's output is read at most, should the startup files
/// print without end.
const MAX_OUTPUT: usize = 1 << 20;

/// Put the login shell's `PATH` in front of the inherited one.
///
/// Changes the process environment, so the app calls it before it starts a
/// thread that reads the environment. The only thread it may leave behind
/// reads a pipe, when a program the startup files left running holds it open.
pub fn adopt() {
    let inherited = std::env::var_os("PATH");
    if let Some(path) = adopted_path(
        std::io::stdin().is_terminal(),
        std::env::var_os("SHELL"),
        inherited.as_deref(),
        TIMEOUT,
    ) {
        std::env::set_var("PATH", path);
    }
}

/// The `PATH` the app should use instead of `inherited`, or `None` to keep it:
/// when standard input is a terminal (the app was started from a shell, whose
/// `PATH` it has), when `shell` is not an absolute path, or when the shell
/// reports no `PATH` within `timeout`.
fn adopted_path(
    stdin_is_terminal: bool,
    shell: Option<OsString>,
    inherited: Option<&OsStr>,
    timeout: Duration,
) -> Option<OsString> {
    if stdin_is_terminal {
        return None;
    }
    let Some(shell) = shell.map(PathBuf::from).filter(|shell| shell.is_absolute()) else {
        tracing::info!("SHELL is not an absolute path; keeping the inherited PATH");
        return None;
    };
    let Some(login) = login_path(&shell, timeout) else {
        tracing::warn!(
            shell = %shell.display(),
            "the login shell reported no PATH within {timeout:?}; keeping the inherited PATH"
        );
        return None;
    };
    let path = merge(&login, inherited);
    tracing::info!(path = %path.to_string_lossy(), "using the login shell's PATH");
    Some(path)
}

/// The `PATH` that `shell` has as an interactive login shell, or `None` when it
/// reports none within `timeout`.
///
/// Interactive as well, since many users extend `PATH` in the file only an
/// interactive shell reads (`.zshrc`, `.bashrc`). The shell gets no input, and
/// its errors are dropped. Its output is read up to the closing marker, not to
/// its end: a program the startup files leave running in the background (an
/// agent) can hold the pipe open long after the shell exited.
fn login_path(shell: &Path, timeout: Duration) -> Option<OsString> {
    let mut child = Command::new(shell)
        .args(login_args(shell))
        .arg(format!("echo {MARKER}; /usr/bin/env; echo {MARKER}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0u8; 4096];
        while let Ok(read @ 1..) = stdout.read(&mut buffer) {
            output.extend_from_slice(&buffer[..read]);
            if path_in(&output).is_some() || output.len() > MAX_OUTPUT {
                break;
            }
        }
        let _ = sender.send(output);
    });
    let output = receiver.recv_timeout(timeout).ok();
    let _ = child.kill();
    let _ = child.wait();
    path_in(&output?)
}

/// The arguments before the script. csh and tcsh take `-l` only as their sole
/// argument, so they run as interactive shells, which read `.cshrc` as well.
fn login_args(shell: &Path) -> &'static [&'static str] {
    match shell.file_name().and_then(OsStr::to_str) {
        Some("csh" | "tcsh") => &["-i", "-c"],
        _ => &["-l", "-i", "-c"],
    }
}

/// The value of `PATH` in the environment printed between two markers, when
/// both markers are there and the value is not empty.
fn path_in(output: &[u8]) -> Option<OsString> {
    let marker = MARKER.as_bytes();
    let mut lines = output.split(|&byte| byte == b'\n');
    lines.by_ref().find(|line| *line == marker)?;
    let mut path = None;
    for line in lines {
        if line == marker {
            return path
                .filter(|path: &Vec<u8>| !path.is_empty())
                .map(OsString::from_vec);
        }
        if let Some(value) = line.strip_prefix(b"PATH=") {
            path = Some(value.to_vec());
        }
    }
    None
}

/// The entries of `login` in their order, then those of `inherited` that
/// `login` lacks, so nothing the launcher put in `PATH` gets lost. Empty
/// entries, which stand for the current directory, are dropped.
fn merge(login: &OsStr, inherited: Option<&OsStr>) -> OsString {
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in
        std::env::split_paths(login).chain(inherited.into_iter().flat_map(std::env::split_paths))
    {
        if !entry.as_os_str().is_empty() && !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    std::env::join_paths(entries).unwrap_or_else(|_| login.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    /// A stand-in for the user's shell: runs `startup`, as a login shell runs
    /// its startup files, and then its last argument, the script.
    fn fake_shell(dir: &Path, startup: &str) -> PathBuf {
        let shell = dir.join("shell");
        std::fs::write(
            &shell,
            format!("#!/bin/sh\n{startup}\nshift $(($# - 1))\neval \"$1\"\n"),
        )
        .expect("shell");
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        shell
    }

    #[test]
    fn an_app_started_from_the_desktop_puts_the_login_shells_path_first() {
        let dir = tempfile::tempdir().expect("dir");
        let shell = fake_shell(
            dir.path(),
            "echo 'Welcome back!'\nexport PATH=/opt/homebrew/bin:/usr/bin:/bin",
        );

        let path = adopted_path(
            false,
            Some(shell.into()),
            Some(OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin")),
            TIMEOUT,
        );

        assert_eq!(
            path,
            Some("/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin".into())
        );
    }

    #[test]
    fn an_app_started_from_a_terminal_keeps_its_path() {
        let dir = tempfile::tempdir().expect("dir");
        let shell = fake_shell(dir.path(), "export PATH=/login/bin");

        assert_eq!(
            adopted_path(
                true,
                Some(shell.into()),
                Some(OsStr::new("/usr/bin")),
                TIMEOUT
            ),
            None
        );
    }

    #[test]
    fn without_an_absolute_shell_the_path_is_kept() {
        let inherited = Some(OsStr::new("/usr/bin"));
        assert_eq!(adopted_path(false, None, inherited, TIMEOUT), None);
        assert_eq!(
            adopted_path(false, Some("zsh".into()), inherited, TIMEOUT),
            None
        );
    }

    #[test]
    fn a_shell_that_does_not_answer_in_time_reports_nothing() {
        let dir = tempfile::tempdir().expect("dir");
        let shell = fake_shell(dir.path(), "sleep 10");

        let start = Instant::now();
        assert_eq!(login_path(&shell, Duration::from_millis(300)), None);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_background_program_holding_the_output_does_not_hold_up_the_answer() {
        let dir = tempfile::tempdir().expect("dir");
        let shell = fake_shell(
            dir.path(),
            "export PATH=/login/bin:/usr/bin:/bin\nsleep 10 &",
        );

        let start = Instant::now();
        assert_eq!(
            login_path(&shell, TIMEOUT),
            Some("/login/bin:/usr/bin:/bin".into())
        );
        assert!(start.elapsed() < TIMEOUT, "{:?}", start.elapsed());
    }

    #[test]
    fn a_shell_without_a_path_or_that_fails_reports_nothing() {
        let dir = tempfile::tempdir().expect("dir");
        assert_eq!(
            login_path(&fake_shell(dir.path(), "unset PATH"), TIMEOUT),
            None
        );
        assert_eq!(login_path(&fake_shell(dir.path(), "exit 1"), TIMEOUT), None);
        assert_eq!(login_path(&dir.path().join("missing"), TIMEOUT), None);
    }

    #[test]
    fn the_path_is_read_between_the_markers_only() {
        let output = format!(
            "PATH=/from/startup\n{MARKER}\nHOME=/home/me\nPATH=/a:/b\nSHELL=/bin/zsh\n{MARKER}\nPATH=/after\n"
        );
        assert_eq!(path_in(output.as_bytes()), Some("/a:/b".into()));

        let unfinished = format!("{MARKER}\nPATH=/a:/b\n");
        assert_eq!(path_in(unfinished.as_bytes()), None);

        let empty = format!("{MARKER}\nPATH=\n{MARKER}\n");
        assert_eq!(path_in(empty.as_bytes()), None);
    }

    #[test]
    fn csh_and_tcsh_run_as_interactive_shells_only() {
        assert_eq!(login_args(Path::new("/bin/zsh")), ["-l", "-i", "-c"]);
        assert_eq!(
            login_args(Path::new("/usr/local/bin/fish")),
            ["-l", "-i", "-c"]
        );
        assert_eq!(login_args(Path::new("/bin/tcsh")), ["-i", "-c"]);
    }

    #[test]
    fn merging_keeps_the_inherited_entries_the_login_path_lacks() {
        assert_eq!(
            merge(
                OsStr::new("/opt/homebrew/bin:/usr/bin::/bin"),
                Some(OsStr::new("/usr/bin:/tmp/.mount_x/usr/bin:/bin:"))
            ),
            "/opt/homebrew/bin:/usr/bin:/bin:/tmp/.mount_x/usr/bin"
        );
        assert_eq!(merge(OsStr::new("/a:/b:/a"), None), "/a:/b");
    }
}
