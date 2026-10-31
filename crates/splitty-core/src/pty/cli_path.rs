//! Discover an installation-local CLI and amend child PATHs, never global state.
//!
//! A daemon points `bin/splitty` of its run directory at its own CLI when it
//! starts ([`publish_stable_cli`]); sessions get that directory while it names
//! their server's CLI, so a shell that outlives an update finds the new one.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
#[cfg(unix)]
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[cfg(windows)]
const CLI_NAME: &str = "splitty.exe";
#[cfg(not(windows))]
const CLI_NAME: &str = "splitty";

/// Where this process was started from, resolved once. A reinstall of the
/// same version moves a running server's directory away and deletes it, after
/// which `current_exe` names the deleted file; the path it started from holds
/// the new installation, with its CLI and shell integration.
static STARTED_FROM: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Resolve the executable now, before anything can replace it; `main` calls
/// this first. Later calls of [`started_from`] return the same path.
pub fn remember_executable() {
    started_from();
}

pub fn started_from() -> Option<&'static Path> {
    STARTED_FROM
        .get_or_init(|| env::current_exe().ok())
        .as_deref()
}

/// The directory to put a child's CLI in: the stable directory while its
/// link names this process's CLI, otherwise the CLI next to this process,
/// without treating a GUI as the CLI or searching the working directory.
/// Existing PATH lookup otherwise stays intact.
pub fn current_cli_dir() -> Option<PathBuf> {
    let own = cli_dir_for(started_from()?)?;
    Some(prefer_stable(own, &stable_dir()))
}

/// `bin/` of the run directory, whose `splitty` names the CLI of the daemon
/// that started last.
pub fn stable_dir() -> PathBuf {
    crate::config::run_dir().join("bin")
}

/// `stable` while its CLI resolves to the same file as the one in `own`,
/// otherwise `own`.
fn prefer_stable(own: PathBuf, stable: &Path) -> PathBuf {
    match (
        fs::canonicalize(stable.join(CLI_NAME)),
        fs::canonicalize(own.join(CLI_NAME)),
    ) {
        (Ok(linked), Ok(cli)) if linked == cli => stable.to_path_buf(),
        _ => own,
    }
}

/// Point the stable directory's `splitty` at the CLI next to this process,
/// if it has one. A starting daemon calls this before it starts sessions.
#[cfg(unix)]
pub fn publish_stable_cli() -> io::Result<()> {
    let Some(own) = started_from().and_then(cli_dir_for) else {
        return Ok(());
    };
    publish_in(&stable_dir(), &own.join(CLI_NAME))
}

/// Create `dir` private to the user, refusing a symbolic link, and point its
/// `splitty` at `cli`: a new link under a temporary name renamed over the old
/// one, so the name is never missing.
#[cfg(unix)]
pub(super) fn publish_in(dir: &Path, cli: &Path) -> io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(io::Error::other(format!(
                "{} is a symbolic link",
                dir.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    crate::config::ensure_private_dir(dir)?;
    let temporary = dir.join(format!(".{CLI_NAME}.{}", std::process::id()));
    match fs::remove_file(&temporary) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    std::os::unix::fs::symlink(cli, &temporary)?;
    fs::rename(&temporary, dir.join(CLI_NAME)).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

fn cli_dir_for(executable: &Path) -> Option<PathBuf> {
    if !executable.is_absolute() {
        return None;
    }
    let directory = executable.parent()?;
    let candidate = directory.join(CLI_NAME);
    executable_file(&candidate).then(|| directory.to_path_buf())
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Keep every unrelated entry in its original order, including empty entries
/// deliberately present in inherited PATH. A directory unrepresentable in a
/// platform path list returns an error instead of corrupting the search path.
pub fn prepend(
    directory: &Path,
    inherited: Option<&OsStr>,
) -> Result<OsString, env::JoinPathsError> {
    let entries = std::iter::once(directory.to_path_buf()).chain(
        inherited
            .into_iter()
            .flat_map(env::split_paths)
            .filter(|path| path != directory),
    );
    env::join_paths(entries)
}

pub(super) fn configure_command(
    command: &mut portable_pty::CommandBuilder,
    directory: Option<&Path>,
) {
    // Neither an inherited hint nor client-like extra environment can select a CLI.
    command.env_remove("SPLITTY_CLI_DIR");
    let Some(directory) = directory.filter(|dir| executable_file(&dir.join(CLI_NAME))) else {
        return;
    };
    match prepend(directory, command.get_env("PATH")) {
        Ok(path) => {
            command.env("PATH", path);
            command.env("SPLITTY_CLI_DIR", directory);
        }
        Err(error) => tracing::warn!(%error, "cannot add Splitty CLI directory to session PATH"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executable(path: &Path) {
        std::fs::write(path, "fixture").expect("file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).expect("mode");
        }
    }

    #[test]
    fn discovers_only_an_executable_installation_local_cli() {
        let home = tempfile::tempdir().expect("fixture");
        let directory = home.path().join("CLI with spaces ö");
        std::fs::create_dir(&directory).expect("directory");
        let gui = directory.join("splitty-gui");
        executable(&gui);
        assert!(cli_dir_for(&gui).is_none());
        assert!(cli_dir_for(Path::new("splitty")).is_none());
        let cli = directory.join(CLI_NAME);
        executable(&cli);
        assert_eq!(cli_dir_for(&cli).as_deref(), Some(directory.as_path()));
        assert_eq!(cli_dir_for(&gui).as_deref(), Some(directory.as_path()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o600)).expect("mode");
            assert!(cli_dir_for(&gui).is_none());
        }
        assert!(cli_dir_for(&home.path().join("missing/splitty")).is_none());
    }

    #[test]
    fn prepends_once_preserving_other_entries_and_supports_absent_path() {
        let directory = Path::new("/available CLI ö");
        let inherited = env::join_paths([
            Path::new("/first"),
            directory,
            Path::new(""),
            Path::new("/last"),
            directory,
        ])
        .expect("path");
        let joined = prepend(directory, Some(&inherited)).expect("path");
        assert_eq!(
            env::split_paths(&joined).collect::<Vec<_>>(),
            vec![
                directory,
                Path::new("/first"),
                Path::new(""),
                Path::new("/last")
            ]
        );
        assert_eq!(prepend(directory, Some(&joined)).expect("repeat"), joined);
        assert_eq!(
            env::split_paths(&prepend(directory, None).expect("absent")).collect::<Vec<_>>(),
            vec![directory]
        );
    }

    #[test]
    fn missing_cli_preserves_path_and_discards_a_stale_hint() {
        let directory = tempfile::tempdir().expect("fixture");
        let mut command = portable_pty::CommandBuilder::new("program");
        command.env("PATH", "/first:/second");
        command.env("SPLITTY_CLI_DIR", "/inherited-wrong-cli");
        configure_command(&mut command, Some(directory.path()));
        assert_eq!(command.get_env("PATH"), Some(OsStr::new("/first:/second")));
        assert!(command.get_env("SPLITTY_CLI_DIR").is_none());
        executable(&directory.path().join(CLI_NAME));
        configure_command(&mut command, Some(directory.path()));
        assert_eq!(
            command.get_env("SPLITTY_CLI_DIR"),
            Some(directory.path().as_os_str())
        );
    }

    /// terminal-sessions, Host CLI Follows The Daemon: the link is created and
    /// replaced in place; only a link to the own CLI selects the stable
    /// directory.
    #[test]
    #[cfg(unix)]
    fn the_stable_directory_names_only_the_own_cli() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().expect("fixture");
        let installation = |name: &str| {
            let directory = fixture.path().join(name);
            std::fs::create_dir(&directory).expect("installation");
            executable(&directory.join(CLI_NAME));
            directory
        };
        let (old, new) = (installation("0.1.21"), installation("0.1.22 ö"));
        let stable = fixture.path().join("run").join("bin");
        assert_eq!(prefer_stable(old.clone(), &stable), old, "no link yet");

        publish_in(&stable, &old.join(CLI_NAME)).expect("publish");
        assert_eq!(
            std::fs::metadata(&stable)
                .expect("bin")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(prefer_stable(old.clone(), &stable), stable);
        assert_eq!(
            prefer_stable(new.clone(), &stable),
            new,
            "another installation"
        );

        publish_in(&stable, &new.join(CLI_NAME)).expect("replace");
        assert_eq!(
            std::fs::read_link(stable.join(CLI_NAME)).expect("link"),
            new.join(CLI_NAME)
        );
        assert_eq!(prefer_stable(new.clone(), &stable), stable);
        assert_eq!(prefer_stable(old.clone(), &stable), old);
        assert_eq!(std::fs::read_dir(&stable).expect("bin").count(), 1);

        std::fs::remove_dir_all(&new).expect("remove the installation");
        assert_eq!(prefer_stable(old.clone(), &stable), old, "dangling link");

        let elsewhere = fixture.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).expect("elsewhere");
        let linked = fixture.path().join("linked-bin");
        std::os::unix::fs::symlink(&elsewhere, &linked).expect("link");
        assert!(publish_in(&linked, &old.join(CLI_NAME)).is_err());
        assert_eq!(std::fs::read_dir(&elsewhere).expect("elsewhere").count(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn invalid_path_list_directory_is_not_partially_applied() {
        assert!(prepend(Path::new("/colon:directory"), Some(OsStr::new("/bin"))).is_err());
    }
}
