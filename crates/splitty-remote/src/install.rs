//! Installing the client's own server on a host.
//!
//! One probe tells the host's kernel (Linux or macOS), architecture, home
//! directory and whether this version is installed. When it is not, the
//! payload goes over the SSH connection's standard input into an owner-only
//! directory, its checksum is verified on the host before anything is
//! unpacked, and the server is moved to
//! `~/.local/share/splitty/remote/<version>/` only once it has been seen to
//! run there. Nothing is downloaded by the host.
//!
//! Both scripts are constant single lines without single quotes, run as
//! `sh -c '<script>' <name> <quoted values>`: the host's login shell, which may
//! be fish or csh, only parses that outer line.

use std::io;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::ssh::{is_sha256, is_version, run_checked, CommandLine, Executor, SshError};

/// Where versions are installed, below the host's home directory.
pub const REMOTE_DIR: &str = ".local/share/splitty/remote";

/// The environment variable that names a payload archive (development, tests).
pub const PAYLOAD_VARIABLE: &str = "SPLITTY_REMOTE_PAYLOAD";

/// The kernels a host may run, as `uname -s` names them.
pub const KERNELS: [&str; 2] = ["Linux", "Darwin"];

/// The variable that gives the server a configuration directory of its own
/// (the server's `SPLITTY_CONFIG_DIR`).
pub const CONFIG_DIR_VARIABLE: &str = "SPLITTY_CONFIG_DIR";

/// The configuration directory of the server installed on a macOS host,
/// below the host's home directory: apart from the desktop app's
/// `~/Library/Application Support/splitty`.
pub const MACOS_CONFIG_DIR: &str = ".config/splitty";

/// The archives built into this client (`build.rs`).
mod built_in {
    include!(concat!(env!("OUT_DIR"), "/payloads.rs"));
}

/// `uname -m` as payloads name architectures.
pub fn normalize_arch(machine: &str) -> &str {
    match machine {
        "amd64" => "x86_64",
        "arm64" => "aarch64",
        other => other,
    }
}

/// Prints the kernel, the architecture, `$HOME`, and the marker of version
/// `$1` when that version's server is installed.
const PROBE: &str = r#"printf "%s\n" "$(uname -s)" "$(uname -m)" "$HOME"; d="$HOME/.local/share/splitty/remote/$1"; if [ -x "$d/splitty" ] && [ -f "$d/.payload-sha256" ]; then cat "$d/.payload-sha256"; fi"#;

/// Installs version `$1` from the archive on standard input, whose SHA-256 is
/// `$2`, checked with `sha256sum` or, where that is missing, `shasum -a 256`.
/// Exits with 3 when the checksum does not match, with 4 when the unpacked
/// server does not run and with 5 when the host has neither tool; then nothing
/// is moved into place. The versions processes run are kept, read from
/// `/proc` or, on a host without it (macOS), from `ps`.
const INSTALL: &str = r#"set -e; umask 077; base="$HOME/.local/share/splitty/remote"; mkdir -p "$base"; tmp=$(mktemp -d "$base/.install.XXXXXX"); cleanup() { rm -rf "$tmp" "$tmp.old"; }; trap cleanup EXIT; cat > "$tmp/payload"; if command -v sha256sum >/dev/null 2>&1; then sum=sha256sum; elif command -v shasum >/dev/null 2>&1; then sum="shasum -a 256"; else exit 5; fi; printf "%s  %s\n" "$2" "$tmp/payload" | $sum -c - >/dev/null 2>&1 || exit 3; tar -xzf "$tmp/payload" -C "$tmp"; rm -f "$tmp/payload"; chmod 755 "$tmp/splitty"; "$tmp/splitty" --version >/dev/null 2>&1 || exit 4; printf "%s\n" "$2" > "$tmp/.payload-sha256"; final="$base/$1"; if [ -e "$final" ]; then mv "$final" "$tmp.old"; fi; mv "$tmp" "$final"; rm -rf "$tmp.old"; trap - EXIT; if [ -d /proc/self ]; then exes=$(ls -l /proc/[0-9]*/exe 2>/dev/null || true); else exes=$(ps -axo comm= 2>/dev/null || true); fi; for d in "$base"/*/; do d=${d%/}; [ -d "$d" ] || continue; [ "$d" = "$final" ] && continue; case "$exes" in *"$d/splitty"*) continue;; esac; rm -rf "$d"; done"#;

/// Why the server could not be installed.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Ssh(#[from] SshError),

    #[error("{host} runs {kernel}; Splitty supports Linux and macOS hosts only")]
    UnsupportedKernel { host: String, kernel: String },

    #[error("{host} gave an unexpected answer to the probe: {detail}")]
    InvalidProbe { host: String, detail: String },

    #[error("version {0:?} cannot be installed (unexpected characters)")]
    InvalidVersion(String),

    #[error("the payload {path} could not be read: {source}")]
    PayloadUnreadable { path: PathBuf, source: io::Error },

    #[error("this client could not pack itself as a server: {0}")]
    SelfPayload(String),

    #[error("the server arrived corrupted on {host} (checksum mismatch); nothing was installed")]
    Checksum { host: String },

    #[error(
        "{host} has neither sha256sum nor shasum to verify the server's checksum; \
         nothing was installed"
    )]
    NoChecksumTool { host: String },

    #[error(
        "the server does not run on {host} ({arch}); nothing was installed. \
         Is the payload built for that architecture?"
    )]
    DoesNotRun { host: String, arch: String },
}

/// What the probe found on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub kernel: String,
    pub arch: String,
    pub home: String,
    /// The checksum marker of the probed version, when it is installed.
    pub marker: Option<String>,
}

/// Parse the probe's output, checking `$HOME` before it is ever used.
pub fn parse_probe(host: &str, stdout: &[u8]) -> Result<Probe, InstallError> {
    let invalid = |detail: &str| InstallError::InvalidProbe {
        host: host.to_owned(),
        detail: detail.to_owned(),
    };
    let text = std::str::from_utf8(stdout).map_err(|_| invalid("not UTF-8"))?;
    let lines: Vec<&str> = text
        .strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .collect();
    let (kernel, arch, home, marker) = match lines[..] {
        [kernel, arch, home] => (kernel, arch, home, None),
        [kernel, arch, home, marker] => (kernel, arch, home, Some(marker)),
        _ => return Err(invalid("expected three or four lines")),
    };
    if kernel.is_empty() || arch.is_empty() {
        return Err(invalid("no kernel or architecture"));
    }
    if !home.starts_with('/') || home.len() > 4096 || home.chars().any(char::is_control) {
        return Err(invalid("the home directory is not an absolute path"));
    }
    Ok(Probe {
        kernel: kernel.to_owned(),
        arch: arch.to_owned(),
        home: home.to_owned(),
        marker: marker.map(str::to_owned),
    })
}

/// Where a payload comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadSource {
    /// The archive named by [`PAYLOAD_VARIABLE`].
    Environment(PathBuf),
    /// An archive built into this client, checksum computed at build time.
    BuiltIn,
    /// This client, a static Linux build of the host's architecture.
    Itself,
}

/// A server archive and the checksum of exactly these bytes.
pub struct Payload {
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub source: PayloadSource,
}

impl Payload {
    pub fn new(bytes: Vec<u8>, source: PayloadSource) -> Self {
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        Self {
            bytes,
            sha256,
            source,
        }
    }
}

/// The payload for a host of kernel `kernel` (`uname -s`) and architecture
/// `arch` (`uname -m`), if this client has one: [`PAYLOAD_VARIABLE`] first,
/// then an archive built in, then the client itself.
pub fn payload_for(kernel: &str, arch: &str) -> Result<Option<Payload>, InstallError> {
    choose_payload(
        kernel,
        arch,
        std::env::var_os(PAYLOAD_VARIABLE),
        built_in::BUILT_IN,
        itself,
    )
}

/// The order of the payload sources, with each source passed in.
fn choose_payload(
    kernel: &str,
    arch: &str,
    environment: Option<std::ffi::OsString>,
    built_in: &[(&str, &str, &[u8], &str)],
    itself: impl FnOnce(&str) -> Result<Option<Payload>, InstallError>,
) -> Result<Option<Payload>, InstallError> {
    if let Some(path) = environment.filter(|v| !v.is_empty()) {
        let path = PathBuf::from(path);
        let bytes = std::fs::read(&path).map_err(|source| InstallError::PayloadUnreadable {
            path: path.clone(),
            source,
        })?;
        return Ok(Some(Payload::new(bytes, PayloadSource::Environment(path))));
    }
    let arch = normalize_arch(arch);
    if let Some((.., bytes, sha256)) = built_in
        .iter()
        .find(|(for_kernel, for_arch, ..)| *for_kernel == kernel && *for_arch == arch)
    {
        return Ok(Some(Payload {
            bytes: bytes.to_vec(),
            sha256: (*sha256).to_owned(),
            source: PayloadSource::BuiltIn,
        }));
    }
    // A static Linux client serves only Linux hosts.
    if kernel != "Linux" {
        return Ok(None);
    }
    itself(arch)
}

/// This client as a payload, when it is a static Linux build for `arch`.
fn itself(arch: &str) -> Result<Option<Payload>, InstallError> {
    #[cfg(all(target_os = "linux", target_env = "musl"))]
    if arch == std::env::consts::ARCH {
        let executable =
            std::env::current_exe().map_err(|e| InstallError::SelfPayload(e.to_string()))?;
        let integration = executable
            .parent()
            .map(|dir| dir.join("shell-integration"))
            .filter(|dir| dir.is_dir());
        let bytes = pack(&executable, integration.as_deref())
            .map_err(|e| InstallError::SelfPayload(e.to_string()))?;
        return Ok(Some(Payload::new(bytes, PayloadSource::Itself)));
    }
    #[cfg(not(all(target_os = "linux", target_env = "musl")))]
    let _ = arch;
    Ok(None)
}

/// A server archive in the layout the WSL payload uses: `splitty` and
/// `shell-integration/` at the top level, packed with the local `tar`.
#[cfg(unix)]
pub fn pack(
    executable: &std::path::Path,
    shell_integration: Option<&std::path::Path>,
) -> io::Result<Vec<u8>> {
    let staging = tempfile_dir()?;
    let result = (|| {
        std::os::unix::fs::symlink(executable, staging.join("splitty"))?;
        let mut entries = vec!["splitty"];
        if let Some(dir) = shell_integration {
            std::os::unix::fs::symlink(dir, staging.join("shell-integration"))?;
            entries.push("shell-integration");
        }
        // -h archives what the symlinks point to.
        let output = std::process::Command::new("tar")
            .args(["-czhf", "-", "-C"])
            .arg(&staging)
            .args(&entries)
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "tar failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(output.stdout)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// A fresh private directory below the system's temporary directory.
#[cfg(unix)]
fn tempfile_dir() -> io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let base = std::env::temp_dir();
    for attempt in 0..100u32 {
        let dir = base.join(format!("splitty-pack-{}-{attempt}", std::process::id()));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("no free temporary directory"))
}

/// The command that runs the host's server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Server {
    /// The server installed below [`REMOTE_DIR`], by its absolute path, and
    /// on a macOS host the configuration directory of its own it runs with.
    Installed {
        path: String,
        config_dir: Option<String>,
    },
    /// `splitty` from the host's `PATH`.
    Path,
}

impl Server {
    /// A command line that starts with this server.
    pub fn command(&self) -> CommandLine {
        match self {
            // `env` is a program, so the line works in any login shell, and
            // the daemon and its panes inherit the variable.
            Server::Installed {
                path,
                config_dir: Some(dir),
            } => CommandLine::new("env")
                .value(&format!("{CONFIG_DIR_VARIABLE}={dir}"))
                .value(path),
            Server::Installed {
                path,
                config_dir: None,
            } => CommandLine::program(path),
            Server::Path => CommandLine::new("splitty"),
        }
    }

    /// How to name it in a hint to the user.
    pub fn display(&self) -> &str {
        match self {
            Server::Installed { path, .. } => path,
            Server::Path => "splitty",
        }
    }
}

/// What [`prepare`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub server: Server,
    pub probe: Probe,
    /// Whether a payload was uploaded now.
    pub uploaded: bool,
    /// Whether this client had a payload for the host.
    pub had_payload: bool,
}

/// Make sure the host has this client's server, installing it when a payload
/// for the host is available, and say how to run it.
pub async fn prepare<E: Executor>(
    executor: &E,
    version: &str,
    payload_for: impl FnOnce(&str, &str) -> Result<Option<Payload>, InstallError>,
    on_install: impl FnOnce(&Probe),
) -> Result<Prepared, InstallError> {
    if !is_version(version) {
        return Err(InstallError::InvalidVersion(version.to_owned()));
    }
    let probe_line = CommandLine::new("sh")
        .word("-c")
        .value(PROBE)
        .word("splitty-probe")
        .value(version);
    let output = run_checked(executor, &probe_line, None).await?;
    let probe = parse_probe(executor.host(), &output.stdout)?;
    if !KERNELS.contains(&probe.kernel.as_str()) {
        return Err(InstallError::UnsupportedKernel {
            host: executor.host().to_owned(),
            kernel: probe.kernel,
        });
    }
    let installed = Server::Installed {
        path: format!("{}/{REMOTE_DIR}/{version}/splitty", probe.home),
        config_dir: (probe.kernel == "Darwin")
            .then(|| format!("{}/{MACOS_CONFIG_DIR}", probe.home)),
    };

    let Some(payload) = payload_for(&probe.kernel, &probe.arch)? else {
        // Without a payload, a server of this version that another client
        // installed (and verified) is still the best choice.
        let server = if probe.marker.as_deref().is_some_and(is_sha256) {
            installed
        } else {
            Server::Path
        };
        return Ok(Prepared {
            server,
            probe,
            uploaded: false,
            had_payload: false,
        });
    };
    if probe.marker.as_deref() == Some(payload.sha256.as_str()) {
        return Ok(Prepared {
            server: installed,
            probe,
            uploaded: false,
            had_payload: true,
        });
    }

    on_install(&probe);
    let install_line = CommandLine::new("sh")
        .word("-c")
        .value(INSTALL)
        .word("splitty-install")
        .value(version)
        .value(&payload.sha256);
    match run_checked(executor, &install_line, Some(payload.bytes)).await {
        Ok(_) => Ok(Prepared {
            server: installed,
            probe,
            uploaded: true,
            had_payload: true,
        }),
        Err(SshError::Failed {
            status: Some(3), ..
        }) => Err(InstallError::Checksum {
            host: executor.host().to_owned(),
        }),
        Err(SshError::Failed {
            status: Some(4), ..
        }) => Err(InstallError::DoesNotRun {
            host: executor.host().to_owned(),
            arch: probe.arch,
        }),
        Err(SshError::Failed {
            status: Some(5), ..
        }) => Err(InstallError::NoChecksumTool {
            host: executor.host().to_owned(),
        }),
        Err(error) => Err(error.into()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Output;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A "host" that is this machine's `sh` with its own `HOME`: the real
    /// scripts and the quoting run against a real POSIX shell.
    struct LocalShell {
        home: PathBuf,
        uploads: AtomicUsize,
        /// Replaces the payload in transit, to simulate corruption.
        corrupt: bool,
        /// A directory put first in `PATH`, for a fake `uname`.
        bin: Option<PathBuf>,
        /// Replaces `PATH` altogether, for a host that lacks a tool.
        path: Option<PathBuf>,
    }

    impl LocalShell {
        fn new(home: PathBuf) -> Self {
            Self {
                home,
                uploads: AtomicUsize::new(0),
                corrupt: false,
                bin: None,
                path: None,
            }
        }
    }

    impl Executor for LocalShell {
        fn host(&self) -> &str {
            "user@vm"
        }

        async fn run(&self, line: &CommandLine, input: Option<Vec<u8>>) -> io::Result<Output> {
            let input = input.map(|mut bytes| {
                self.uploads.fetch_add(1, Ordering::SeqCst);
                if self.corrupt {
                    bytes.push(0);
                }
                bytes
            });
            let mut command = tokio::process::Command::new("sh");
            command.arg("-c").arg(line.as_str()).env("HOME", &self.home);
            if let Some(bin) = &self.bin {
                let mut path = vec![bin.clone()];
                path.extend(std::env::split_paths(
                    &std::env::var_os("PATH").unwrap_or_default(),
                ));
                command.env("PATH", std::env::join_paths(path).expect("PATH"));
            }
            if let Some(path) = &self.path {
                command.env("PATH", path);
            }
            crate::ssh::run_with_input(command, input).await
        }
    }

    /// An archive with a `splitty` script that runs (`ok`) or fails.
    fn archive(ok: bool) -> Vec<u8> {
        archive_of(if ok {
            "#!/bin/sh\necho splitty 0.0.0-test\n"
        } else {
            "#!/bin/sh\nexit 1\n"
        })
    }

    /// An archive whose `splitty` is `script`.
    fn archive_of(script: &str) -> Vec<u8> {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("server");
        fs::write(&exe, script).expect("write");
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).expect("chmod");
        let integration = dir.path().join("shell-integration");
        fs::create_dir_all(integration.join("bash")).expect("mkdir");
        fs::write(integration.join("bash/splitty.bash"), "# hook\n").expect("write");
        pack(&exe, Some(&integration)).expect("pack")
    }

    fn payload(ok: bool) -> Payload {
        Payload::new(archive(ok), PayloadSource::Itself)
    }

    fn home() -> tempfile::TempDir {
        tempfile::tempdir().expect("home")
    }

    fn version_dir(home: &Path, version: &str) -> PathBuf {
        home.join(REMOTE_DIR).join(version)
    }

    #[test]
    fn the_scripts_are_single_lines_without_single_quotes() {
        for script in [PROBE, INSTALL] {
            assert!(!script.contains('\n'), "one line");
            assert!(!script.contains('\''), "no single quotes");
            for (i, _) in script.match_indices('\\') {
                assert_eq!(&script[i..i + 2], "\\n", "only \\n escapes: {script}");
            }
        }
    }

    #[test]
    fn the_probe_names_kernel_architecture_home_and_marker() {
        let probe = parse_probe("vm", b"Linux\nx86_64\n/home/user\n").expect("probe");
        assert_eq!(
            probe,
            Probe {
                kernel: "Linux".into(),
                arch: "x86_64".into(),
                home: "/home/user".into(),
                marker: None
            }
        );
        let sum = "a".repeat(64);
        let probe =
            parse_probe("vm", format!("Linux\naarch64\n/root\n{sum}\n").as_bytes()).expect("probe");
        assert_eq!(probe.marker.as_deref(), Some(sum.as_str()));

        for bad in [
            &b"Linux\nx86_64\n"[..],
            b"Linux\nx86_64\nrelative/home\n",
            b"Linux\nx86_64\n/home/a\rb\n",
            b"Linux\nx86_64\n/home/user\nmarker\nextra\n",
            b"\nx86_64\n/home/user\n",
        ] {
            assert!(
                matches!(
                    parse_probe("vm", bad),
                    Err(InstallError::InvalidProbe { .. })
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_packed_server_holds_the_binary_and_its_shell_integration() {
        let bytes = archive(true);
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("payload.tar.gz");
        fs::write(&file, &bytes).expect("write");
        let listing = std::process::Command::new("tar")
            .arg("-tzf")
            .arg(&file)
            .output()
            .expect("tar");
        let listing = String::from_utf8_lossy(&listing.stdout);
        assert!(listing.lines().any(|l| l == "splitty"), "{listing}");
        assert!(
            listing.contains("shell-integration/bash/splitty.bash"),
            "{listing}"
        );
        let payload = Payload::new(bytes.clone(), PayloadSource::Itself);
        assert_eq!(payload.sha256, format!("{:x}", Sha256::digest(&bytes)));
        assert!(is_sha256(&payload.sha256));
    }

    #[tokio::test]
    async fn a_host_without_splitty_gets_the_server_installed() {
        let home = home();
        let host = LocalShell::new(home.path().to_path_buf());
        let mut announced = false;
        let prepared = prepare(
            &host,
            "0.1.0",
            |_, _| Ok(Some(payload(true))),
            |_| announced = true,
        )
        .await
        .expect("prepare");
        assert!(prepared.uploaded && announced);
        let dir = version_dir(home.path(), "0.1.0");
        assert_eq!(
            prepared.server,
            Server::Installed {
                path: dir.join("splitty").display().to_string(),
                config_dir: None,
            }
        );
        assert!(dir.join("shell-integration/bash/splitty.bash").is_file());
        let mode = fs::metadata(home.path().join(REMOTE_DIR))
            .expect("dir")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "owner-only: {mode:o}");
        let leftovers: Vec<_> = fs::read_dir(home.path().join(REMOTE_DIR))
            .expect("dir")
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .collect();
        assert_eq!(leftovers, ["0.1.0"], "no temporary directories left");
        let run = std::process::Command::new(dir.join("splitty"))
            .output()
            .expect("run");
        assert_eq!(String::from_utf8_lossy(&run.stdout), "splitty 0.0.0-test\n");
    }

    #[tokio::test]
    async fn an_installed_version_is_reused_without_an_upload() {
        let home = home();
        let host = LocalShell::new(home.path().to_path_buf());
        let bytes = archive(true);
        let first = prepare(
            &host,
            "0.1.0",
            |_, _| Ok(Some(Payload::new(bytes.clone(), PayloadSource::Itself))),
            |_| {},
        )
        .await
        .expect("install");
        let second = prepare(
            &host,
            "0.1.0",
            |_, _| Ok(Some(Payload::new(bytes, PayloadSource::Itself))),
            |_| panic!("no second install"),
        )
        .await
        .expect("reuse");
        assert!(first.uploaded && !second.uploaded);
        assert_eq!(first.server, second.server);
        assert_eq!(host.uploads.load(Ordering::SeqCst), 1);

        // A client of the same version without a payload uses it too.
        let third = prepare(&host, "0.1.0", |_, _| Ok(None), |_| {})
            .await
            .expect("no payload");
        assert_eq!(third.server, first.server);
    }

    #[tokio::test]
    async fn a_corrupted_upload_installs_nothing() {
        let home = home();
        let mut host = LocalShell::new(home.path().to_path_buf());
        host.corrupt = true;
        let error = prepare(&host, "0.1.0", |_, _| Ok(Some(payload(true))), |_| {})
            .await
            .expect_err("checksum failure");
        assert!(matches!(error, InstallError::Checksum { .. }), "{error:?}");
        let entries = fs::read_dir(home.path().join(REMOTE_DIR))
            .expect("dir")
            .count();
        assert_eq!(
            entries, 0,
            "nothing moved into place, no temporary directory left"
        );
    }

    #[tokio::test]
    async fn a_server_that_does_not_run_is_not_moved_into_place() {
        let home = home();
        let host = LocalShell::new(home.path().to_path_buf());
        let error = prepare(&host, "0.1.0", |_, _| Ok(Some(payload(false))), |_| {})
            .await
            .expect_err("does not run");
        assert!(
            matches!(error, InstallError::DoesNotRun { .. }),
            "{error:?}"
        );
        assert!(!version_dir(home.path(), "0.1.0").exists());
    }

    #[tokio::test]
    async fn a_broken_install_of_the_same_version_is_replaced() {
        let home = home();
        let dir = version_dir(home.path(), "0.1.0");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join(".payload-sha256"), "0".repeat(64)).expect("marker");
        let host = LocalShell::new(home.path().to_path_buf());
        let prepared = prepare(&host, "0.1.0", |_, _| Ok(Some(payload(true))), |_| {})
            .await
            .expect("install");
        assert!(prepared.uploaded);
        assert!(dir.join("splitty").is_file());
    }

    #[tokio::test]
    async fn versions_no_process_runs_are_removed_and_running_ones_kept() {
        let home = home();
        // A copy of `sh` runs under any name; `; :` keeps it from exec'ing
        // `sleep`, so /proc/<pid>/exe stays the copy.
        let shell = which("sh");
        let mut running = None;
        for version in ["0.0.1", "0.0.2"] {
            let dir = version_dir(home.path(), version);
            fs::create_dir_all(&dir).expect("mkdir");
            fs::copy(&shell, dir.join("splitty")).expect("copy sh");
            if version == "0.0.1" {
                // The daemon of an older version keeps running from here.
                running = Some(spawn_copied(&dir.join("splitty"), &["-c", "sleep 30; :"]));
            }
        }
        let host = LocalShell::new(home.path().to_path_buf());
        let result = prepare(&host, "0.1.0", |_, _| Ok(Some(payload(true))), |_| {}).await;
        let mut running = running.expect("running");
        let _ = running.kill();
        let _ = running.wait();
        result.expect("install");
        assert!(
            version_dir(home.path(), "0.0.1").join("splitty").exists(),
            "in use: kept"
        );
        assert!(
            !version_dir(home.path(), "0.0.2").exists(),
            "unused: removed"
        );
        assert!(version_dir(home.path(), "0.1.0").join("splitty").exists());
    }

    /// Start a just-copied executable. Another test thread may fork while the
    /// copy is still open for writing; the child then holds that descriptor
    /// for a moment and exec fails with ETXTBSY, so retry briefly.
    fn spawn_copied(path: &Path, args: &[&str]) -> std::process::Child {
        const ETXTBSY: i32 = 26;
        for _ in 0..100 {
            match std::process::Command::new(path).args(args).spawn() {
                Ok(child) => return child,
                Err(e) if e.raw_os_error() == Some(ETXTBSY) => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) => panic!("start {}: {e}", path.display()),
            }
        }
        panic!("{} stayed busy", path.display());
    }

    fn which(name: &str) -> PathBuf {
        std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
            .unwrap_or_else(|| panic!("{name} not found"))
    }

    #[tokio::test]
    async fn a_hostile_home_directory_is_only_ever_used_quoted() {
        let base = tempfile::tempdir().expect("base");
        let marker = base.path().join("pwned");
        let hostile = base.path().join(format!(
            "it's $(touch {m}) `touch {m}` \"x\" -n",
            m = marker.display()
        ));
        fs::create_dir_all(&hostile).expect("hostile home");
        let host = LocalShell::new(hostile.clone());
        let prepared = prepare(&host, "0.1.0", |_, _| Ok(Some(payload(true))), |_| {})
            .await
            .expect("install");
        assert!(!marker.exists(), "nothing from $HOME was executed");
        let line = prepared.server.command().word("--version");
        let output = host.run(&line, None).await.expect("run installed server");
        assert!(output.status.success(), "{:?}", output);
        assert!(!marker.exists(), "nothing from $HOME was executed");
    }

    /// A directory with a fake `uname` for a host of `kernel` and `machine`.
    fn fake_uname(kernel: &str, machine: &str) -> tempfile::TempDir {
        let bin = tempfile::tempdir().expect("bin");
        let uname = bin.path().join("uname");
        fs::write(
            &uname,
            format!("#!/bin/sh\ncase \"$1\" in -s) echo {kernel};; -m) echo {machine};; esac\n"),
        )
        .expect("fake uname");
        fs::set_permissions(&uname, fs::Permissions::from_mode(0o755)).expect("chmod");
        bin
    }

    /// A Mac gets the macOS server, which then runs with a configuration
    /// directory of its own, quoted however odd the home directory is.
    #[tokio::test]
    async fn a_mac_gets_its_server_with_a_configuration_directory_of_its_own() {
        let base = tempfile::tempdir().expect("base");
        let marker = base.path().join("pwned");
        let home = base
            .path()
            .join(format!("it's $(touch {m}) me", m = marker.display()));
        fs::create_dir_all(&home).expect("home");
        let bin = fake_uname("Darwin", "arm64");
        let mut host = LocalShell::new(home.clone());
        host.bin = Some(bin.path().to_path_buf());

        let bytes = archive_of("#!/bin/sh\nprintf \"%s\\n\" \"$SPLITTY_CONFIG_DIR\"\n");
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let linux = archive(false);
        let built_in: &[(&str, &str, &[u8], &str)] = &[
            ("Linux", "aarch64", linux.as_slice(), "0".repeat(64).leak()),
            ("Darwin", "aarch64", bytes.as_slice(), sha256.as_str()),
        ];
        let prepared = prepare(
            &host,
            "0.1.0",
            |kernel, arch| choose_payload(kernel, arch, None, built_in, |_| Ok(None)),
            |_| {},
        )
        .await
        .expect("install");
        let config_dir = home.join(".config/splitty").display().to_string();
        assert_eq!(
            prepared.server,
            Server::Installed {
                path: version_dir(&home, "0.1.0")
                    .join("splitty")
                    .display()
                    .to_string(),
                config_dir: Some(config_dir.clone()),
            }
        );
        let output = host
            .run(&prepared.server.command().word("--version"), None)
            .await
            .expect("run the installed server");
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("{config_dir}\n")
        );
        assert!(!marker.exists(), "nothing from $HOME was executed");
    }

    #[tokio::test]
    async fn a_linux_host_gets_no_configuration_directory() {
        let home = home();
        let host = LocalShell::new(home.path().to_path_buf());
        let prepared = prepare(&host, "0.1.0", |_, _| Ok(Some(payload(true))), |_| {})
            .await
            .expect("install");
        assert!(
            matches!(
                prepared.server,
                Server::Installed {
                    config_dir: None,
                    ..
                }
            ),
            "{:?}",
            prepared.server
        );
        assert!(!prepared
            .server
            .command()
            .as_str()
            .contains(CONFIG_DIR_VARIABLE));
    }

    /// A `PATH` with the tools the scripts need and, when given, `extra`;
    /// without `sha256sum`.
    fn tools_without_sha256sum(extra: &[(&str, &str)]) -> tempfile::TempDir {
        let bin = tempfile::tempdir().expect("bin");
        for tool in [
            "sh", "uname", "mkdir", "mktemp", "cat", "tar", "gzip", "rm", "mv", "chmod", "ls",
        ] {
            std::os::unix::fs::symlink(which(tool), bin.path().join(tool)).expect("link tool");
        }
        for (name, script) in extra {
            let path = bin.path().join(name);
            fs::write(&path, script).expect("write tool");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        bin
    }

    #[tokio::test]
    async fn shasum_verifies_the_upload_where_sha256sum_is_missing() {
        let home = home();
        let used = home.path().join("shasum-used");
        // `shasum -a 256` as macOS has it, by way of this machine's sha256sum.
        let shasum = format!(
            "#!/bin/sh\n[ \"$1 $2\" = \"-a 256\" ] || exit 9\nshift 2\n: > {used}\nexec {sum} \"$@\"\n",
            used = used.display(),
            sum = which("sha256sum").display()
        );
        let bin = tools_without_sha256sum(&[("shasum", &shasum)]);
        let mut host = LocalShell::new(home.path().to_path_buf());
        host.path = Some(bin.path().to_path_buf());
        let prepared = prepare(&host, "0.1.0", |_, _| Ok(Some(payload(true))), |_| {})
            .await
            .expect("install");
        assert!(prepared.uploaded);
        assert!(used.exists(), "verified with shasum");
        assert!(version_dir(home.path(), "0.1.0").join("splitty").is_file());
    }

    #[tokio::test]
    async fn a_host_without_a_checksum_tool_gets_nothing_installed() {
        let home = home();
        let bin = tools_without_sha256sum(&[]);
        let mut host = LocalShell::new(home.path().to_path_buf());
        host.path = Some(bin.path().to_path_buf());
        let error = prepare(&host, "0.1.0", |_, _| Ok(Some(payload(true))), |_| {})
            .await
            .expect_err("no tool");
        assert!(
            matches!(error, InstallError::NoChecksumTool { .. }),
            "{error:?}"
        );
        assert!(!version_dir(home.path(), "0.1.0").exists());
    }

    /// A host whose `uname -s` is neither Linux nor Darwin.
    struct OtherKernel;

    impl Executor for OtherKernel {
        fn host(&self) -> &str {
            "mac"
        }

        async fn run(&self, line: &CommandLine, input: Option<Vec<u8>>) -> io::Result<Output> {
            assert!(input.is_none(), "nothing may be uploaded");
            assert!(line.as_str().contains("splitty-probe"));
            let mut command = tokio::process::Command::new("printf");
            command.arg("FreeBSD\\namd64\\n/home/user\\n");
            crate::ssh::run_with_input(command, None).await
        }
    }

    #[tokio::test]
    async fn another_kernel_is_reported_before_anything_is_uploaded() {
        let error = prepare(
            &OtherKernel,
            "0.1.0",
            |_, _| Ok(Some(payload(true))),
            |_| {},
        )
        .await
        .expect_err("unsupported");
        assert!(
            matches!(error, InstallError::UnsupportedKernel { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("FreeBSD"), "{error}");
        assert!(error.to_string().contains("Linux and macOS"), "{error}");
    }

    #[tokio::test]
    async fn without_a_payload_the_hosts_path_is_used() {
        let home = home();
        let host = LocalShell::new(home.path().to_path_buf());
        let prepared = prepare(
            &host,
            "0.1.0",
            |_, _| Ok(None),
            |_| panic!("nothing to install"),
        )
        .await
        .expect("prepare");
        assert_eq!(prepared.server, Server::Path);
        assert!(!prepared.had_payload);
    }

    #[test]
    fn uname_aliases_name_the_payload_architectures() {
        assert_eq!(normalize_arch("x86_64"), "x86_64");
        assert_eq!(normalize_arch("amd64"), "x86_64");
        assert_eq!(normalize_arch("aarch64"), "aarch64");
        assert_eq!(normalize_arch("arm64"), "aarch64");
        assert_eq!(normalize_arch("riscv64"), "riscv64");
    }

    #[test]
    fn the_payload_sources_come_in_order() {
        let built_in: &[(&str, &str, &[u8], &str)] =
            &[("Linux", "aarch64", b"built-in", "b".repeat(64).leak())];
        let file = tempfile::NamedTempFile::new().expect("file");
        fs::write(file.path(), b"from the environment").expect("write");

        let environment =
            choose_payload("Linux", "arm64", Some(file.path().into()), built_in, |_| {
                panic!("the environment comes first")
            })
            .expect("payload")
            .expect("some");
        assert_eq!(
            environment.source,
            PayloadSource::Environment(file.path().to_path_buf())
        );
        assert_eq!(environment.bytes, b"from the environment");

        let built = choose_payload("Linux", "arm64", None, built_in, |_| {
            panic!("built-in comes second")
        })
        .expect("payload")
        .expect("some");
        assert_eq!(
            (built.source, built.bytes.as_slice()),
            (PayloadSource::BuiltIn, &b"built-in"[..])
        );
        assert_eq!(built.sha256, "b".repeat(64), "the build-time checksum");

        let mut asked = None;
        let none = choose_payload("Linux", "x86_64", None, built_in, |arch| {
            asked = Some(arch.to_owned());
            Ok(None)
        })
        .expect("no payload");
        assert!(none.is_none());
        assert_eq!(asked.as_deref(), Some("x86_64"), "then the client itself");
    }

    #[test]
    fn a_linux_server_is_not_offered_to_a_mac() {
        let built_in: &[(&str, &str, &[u8], &str)] = &[
            ("Linux", "aarch64", b"linux", "l".repeat(64).leak()),
            ("Darwin", "aarch64", b"darwin", "d".repeat(64).leak()),
        ];
        let mac = choose_payload("Darwin", "arm64", None, built_in, |_| {
            panic!("a Linux client is no server for a Mac")
        })
        .expect("payload")
        .expect("some");
        assert_eq!(mac.bytes, b"darwin");
        let none = choose_payload("Darwin", "x86_64", None, built_in, |_| {
            panic!("a Linux client is no server for a Mac")
        })
        .expect("no payload");
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn an_aarch64_host_gets_the_aarch64_payload() {
        let home = home();
        let bin = fake_uname("Linux", "arm64");
        let mut host = LocalShell::new(home.path().to_path_buf());
        host.bin = Some(bin.path().to_path_buf());

        let bytes = archive(true);
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let built_in: &[(&str, &str, &[u8], &str)] =
            &[("Linux", "aarch64", bytes.as_slice(), sha256.as_str())];
        let prepared = prepare(
            &host,
            "0.1.0",
            |kernel, arch| choose_payload(kernel, arch, None, built_in, |_| Ok(None)),
            |_| {},
        )
        .await
        .expect("install");
        assert!(prepared.uploaded);
        assert_eq!(prepared.probe.arch, "arm64");
        assert!(version_dir(home.path(), "0.1.0").join("splitty").is_file());
    }
}
