//! Windows desktop bridge for running Splitty entirely inside WSL 2.
//!
//! The parser and compatibility checks are platform independent so they can be
//! unit-tested on Linux. Process launching is compiled only for Windows.

use serde::{Deserialize, Serialize};

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslDistribution {
    pub name: String,
    pub version: u32,
    pub is_default: bool,
    pub supported: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsDesktopState {
    pub selected_distro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_distro: Option<String>,
}

fn saved_distribution(state: &WindowsDesktopState) -> Option<&str> {
    state
        .pending_distro
        .as_deref()
        .or(state.selected_distro.as_deref())
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SetupStage {
    Detecting,
    Provisioning,
    Starting,
    Ready,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WslSetupProgress {
    pub stage: SetupStage,
    pub message: String,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WslStartResult {
    pub distro: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListedDistribution {
    name: String,
    version: u32,
    is_default: bool,
}

/// `wsl.exe` commonly emits UTF-16LE even when stdout is redirected, but newer
/// versions may emit UTF-8. Accept both forms and strip a Unicode BOM.
fn decode_wsl_output(bytes: &[u8]) -> String {
    let looks_utf16 = bytes.starts_with(&[0xff, 0xfe])
        || (bytes.len() > 1 && bytes.iter().skip(1).step_by(2).any(|byte| *byte == 0));
    if !looks_utf16 {
        return String::from_utf8_lossy(bytes)
            .trim_start_matches('\u{feff}')
            .to_string();
    }

    let offset = usize::from(bytes.starts_with(&[0xff, 0xfe])) * 2;
    let words = bytes[offset..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair));
    char::decode_utf16(words)
        .map(|value| value.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn parse_wsl_list(bytes: &[u8]) -> Vec<ListedDistribution> {
    decode_wsl_output(bytes)
        .lines()
        .filter_map(|line| {
            let line = line.trim_matches(['\r', '\0', ' ']);
            if line.is_empty() {
                return None;
            }
            let (is_default, line) = match line.strip_prefix('*') {
                Some(rest) => (true, rest.trim_start()),
                None => (false, line),
            };
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 3 {
                return None;
            }
            let version = fields.last()?.parse::<u32>().ok()?;
            // The state column is always the final column before VERSION. This
            // keeps distribution names containing spaces or Unicode intact.
            let name = fields[..fields.len() - 2].join(" ");
            (!name.is_empty()).then_some(ListedDistribution {
                name,
                version,
                is_default,
            })
        })
        .collect()
}

fn os_release_id(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim() != "ID" {
            return None;
        }
        Some(value.trim().trim_matches(['\'', '"']).to_ascii_lowercase())
    })
}

fn compatibility_reason(version: u32, architecture: &str, os_id: Option<&str>) -> Option<String> {
    if version != 2 {
        return Some(format!(
            "WSL {version} is not supported; convert this distribution to WSL 2"
        ));
    }
    if architecture.trim() != "x86_64" {
        return Some(format!(
            "architecture {} is not supported; Splitty currently requires x86_64",
            architecture.trim()
        ));
    }
    match os_id {
        Some("ubuntu" | "debian") => None,
        Some(id) => Some(format!(
            "distribution ID '{id}' is not supported; use Ubuntu or Debian"
        )),
        None => Some("could not read ID from /etc/os-release".to_string()),
    }
}

#[cfg(target_os = "windows")]
pub(crate) mod platform {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use tauri::{AppHandle, Emitter, Manager};

    const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
    const START_TIMEOUT: Duration = Duration::from_secs(30);
    const FORWARDING_TIMEOUT: Duration = Duration::from_secs(5);
    const STDERR_LIMIT: usize = 8 * 1024;
    const ARCHIVE_NAME: &str = "splitty-wsl-x86_64.tar.gz";
    const CHECKSUM_NAME: &str = "splitty-wsl-x86_64.tar.gz.sha256";

    struct ProcessOutput {
        success: bool,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    }

    trait WslExecutor {
        fn execute(
            &self,
            arguments: &[OsString],
            input: Option<&[u8]>,
            timeout: Duration,
        ) -> Result<ProcessOutput, String>;
    }

    struct SystemWsl;

    fn wsl_command() -> Command {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

        let mut command = Command::new("wsl.exe");
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }

    impl WslExecutor for SystemWsl {
        fn execute(
            &self,
            arguments: &[OsString],
            input: Option<&[u8]>,
            timeout: Duration,
        ) -> Result<ProcessOutput, String> {
            let mut command = wsl_command();
            command
                .args(arguments)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if input.is_some() {
                command.stdin(Stdio::piped());
            } else {
                command.stdin(Stdio::null());
            }

            let mut child = command.spawn().map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    "WSL is not installed. Install it from an elevated PowerShell with `wsl --install -d Ubuntu`.".to_string()
                } else {
                    format!("could not start wsl.exe: {error}")
                }
            })?;

            let stdout = child.stdout.take().expect("piped stdout");
            let stderr = child.stderr.take().expect("piped stderr");
            let stdout_reader = std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = BufReader::new(stdout).read_to_end(&mut bytes);
                bytes
            });
            let stderr_reader =
                std::thread::spawn(move || read_bounded(BufReader::new(stderr), STDERR_LIMIT + 1));

            let stdin_writer = input.map(|bytes| {
                let mut stdin = child.stdin.take().expect("piped stdin");
                let bytes = bytes.to_vec();
                std::thread::spawn(move || stdin.write_all(&bytes))
            });

            let deadline = Instant::now() + timeout;
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Ok(None) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        if let Some(writer) = stdin_writer {
                            let _ = writer.join();
                        }
                        return Err(format!(
                            "wsl.exe timed out after {} seconds",
                            timeout.as_secs()
                        ));
                    }
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        if let Some(writer) = stdin_writer {
                            let _ = writer.join();
                        }
                        return Err(format!("could not wait for wsl.exe: {error}"));
                    }
                }
            };

            let stdout = stdout_reader.join().unwrap_or_default();
            let stderr = stderr_reader.join().unwrap_or_default();
            if let Some(writer) = stdin_writer {
                match writer.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        return Err(format!(
                            "could not send payload to WSL: {error}: {}",
                            limited_text(&stderr)
                        ));
                    }
                    Err(_) => return Err("WSL payload writer panicked".to_string()),
                }
            }

            Ok(ProcessOutput {
                success: status.success(),
                stdout,
                stderr,
            })
        }
    }

    fn read_bounded(mut reader: impl Read, limit: usize) -> Vec<u8> {
        let mut captured = Vec::with_capacity(limit.min(1024));
        let mut buffer = [0_u8; 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => return captured,
                Ok(count) if captured.len() < limit => {
                    let remaining = limit - captured.len();
                    captured.extend_from_slice(&buffer[..count.min(remaining)]);
                }
                Ok(_) => {}
            }
        }
    }

    fn distro_arguments(name: &str, command: &[&str]) -> Vec<OsString> {
        let mut arguments = vec![
            OsString::from("-d"),
            OsString::from(name),
            OsString::from("--exec"),
        ];
        arguments.extend(command.iter().map(OsString::from));
        arguments
    }

    fn execute_checked(
        executor: &dyn WslExecutor,
        name: &str,
        command: &[&str],
        input: Option<&[u8]>,
    ) -> Result<ProcessOutput, String> {
        let output = executor.execute(&distro_arguments(name, command), input, COMMAND_TIMEOUT)?;
        if output.success {
            Ok(output)
        } else {
            Err(format!(
                "WSL command `{}` failed: {}",
                command.first().copied().unwrap_or("unknown"),
                limited_text(&output.stderr)
            ))
        }
    }

    fn limited_text(bytes: &[u8]) -> String {
        let end = bytes.len().min(STDERR_LIMIT);
        let mut text = decode_wsl_output(&bytes[..end]).trim().to_string();
        if bytes.len() > STDERR_LIMIT {
            text.push_str("\n… output truncated");
        }
        if text.is_empty() {
            "no diagnostic output".to_string()
        } else {
            text
        }
    }

    fn detect_with(executor: &dyn WslExecutor) -> Result<Vec<WslDistribution>, String> {
        let output = executor.execute(
            &[OsString::from("--list"), OsString::from("--verbose")],
            None,
            COMMAND_TIMEOUT,
        )?;
        if !output.success {
            return Err(format!(
                "WSL distribution detection failed: {}",
                limited_text(&output.stderr)
            ));
        }

        let listed = parse_wsl_list(&output.stdout);
        let mut distributions = Vec::with_capacity(listed.len());
        for item in listed {
            if item.version != 2 {
                distributions.push(WslDistribution {
                    name: item.name,
                    version: item.version,
                    is_default: item.is_default,
                    supported: false,
                    reason: compatibility_reason(item.version, "x86_64", Some("ubuntu")),
                });
                continue;
            }
            let architecture = execute_checked(executor, &item.name, &["uname", "-m"], None)
                .map(|result| decode_wsl_output(&result.stdout).trim().to_string());
            let release = execute_checked(executor, &item.name, &["cat", "/etc/os-release"], None)
                .map(|result| decode_wsl_output(&result.stdout));

            let reason = match (architecture, release) {
                (Ok(architecture), Ok(release)) => compatibility_reason(
                    item.version,
                    &architecture,
                    os_release_id(&release).as_deref(),
                ),
                (Err(error), _) | (_, Err(error)) => {
                    Some(format!("could not inspect distribution: {error}"))
                }
            };
            distributions.push(WslDistribution {
                name: item.name,
                version: item.version,
                is_default: item.is_default,
                supported: reason.is_none(),
                reason,
            });
        }
        Ok(distributions)
    }

    fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
        app.path()
            .app_data_dir()
            .map(|path| path.join("windows.json"))
            .map_err(|error| format!("could not resolve Windows configuration directory: {error}"))
    }

    fn read_config(app: &AppHandle) -> Result<WindowsDesktopState, String> {
        let path = config_path(app)?;
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("{} is invalid: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(WindowsDesktopState::default())
            }
            Err(error) => Err(format!("could not read {}: {error}", path.display())),
        }
    }

    fn write_config(app: &AppHandle, state: &WindowsDesktopState) -> Result<(), String> {
        let path = config_path(app)?;
        let parent = path
            .parent()
            .ok_or("Windows configuration path has no parent")?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        let temporary = parent.join(format!(".windows.json.{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|error| format!("could not serialize Windows configuration: {error}"))?;
        let mut file = fs::File::create(&temporary)
            .map_err(|error| format!("could not create {}: {error}", temporary.display()))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
        replace_file(&temporary, &path)
    }

    fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };

        let destination_label = destination.display().to_string();
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let result = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            Err(format!(
                "could not atomically replace {}: {}",
                destination_label,
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(())
        }
    }

    fn resource_file(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
        let root = app
            .path()
            .resource_dir()
            .map_err(|error| format!("could not resolve bundled resources: {error}"))?;
        let direct = root.join(name);
        let nested = root.join("resources").join(name);
        if direct.is_file() {
            Ok(direct)
        } else if nested.is_file() {
            Ok(nested)
        } else {
            Err(format!("bundled WSL resource is missing: {name}"))
        }
    }

    fn installed_backend(
        executor: &dyn WslExecutor,
        distro: &str,
        expected_checksum: &str,
    ) -> Result<Option<String>, String> {
        // Keep the script constant and pass every dynamic value as a positional
        // argument. In particular, the distribution name is always a distinct
        // wsl.exe argument and is never interpolated into shell input.
        const CHECK_SCRIPT: &str = r#"
base="$HOME/.local/share/splitty/desktop/$1"
binary="$base/splitty"
marker="$base/.archive-sha256"
if [ -x "$binary" ] && [ -f "$marker" ] && [ "$(cat "$marker")" = "$2" ]; then
    printf '%s\n' "$binary"
fi
"#;

        let output = execute_checked(
            executor,
            distro,
            &[
                "sh",
                "-c",
                CHECK_SCRIPT,
                "splitty-backend-check",
                env!("CARGO_PKG_VERSION"),
                expected_checksum,
            ],
            None,
        )?;
        let binary = decode_wsl_output(&output.stdout).trim().to_string();
        if binary.is_empty() {
            return Ok(None);
        }
        if !binary.starts_with('/') || binary.contains(['\0', '\r', '\n']) {
            return Err(format!(
                "WSL returned an invalid installed backend path: {binary:?}"
            ));
        }
        Ok(Some(binary))
    }

    /// Refuse to operate in a distribution whose default user is root.
    ///
    /// The launcher never passes a user to `wsl.exe`, so everything — the
    /// provisioning commands, the backend process and every shell it spawns —
    /// runs as whatever the distribution has configured as its default
    /// (security review SR-19). On a root-default distribution
    /// that is UID 0, and the review's test system merely happened to have a
    /// normal user. Non-root execution was an observation about one machine, not
    /// a property of the product.
    ///
    /// This does not confer Windows administrator rights, but within the
    /// distribution it turns every finding into a root-level one: all users'
    /// files, system configuration, package installation and persistence.
    ///
    /// Checked before provisioning *and* before each start, because the default
    /// user can be changed between the two.
    fn ensure_non_root(executor: &dyn WslExecutor, distro: &str) -> Result<(), String> {
        let output = execute_checked(executor, distro, &["id", "-u"], None)?;
        let uid = decode_wsl_output(&output.stdout).trim().to_string();

        match uid.parse::<u32>() {
            Ok(0) => Err(format!(
                "The WSL distribution '{distro}' is configured to run as root.\n\n\
                 Splitty refuses to install or run as root: the backend and every \
                 shell it starts would have full control of the distribution.\n\n\
                 Set a normal default user, for example:\n\
                 \x20   {distro} config --default-user <your-user>"
            )),
            Ok(_) => Ok(()),
            Err(_) => Err(format!(
                "Could not determine the WSL user for '{distro}' (got {uid:?})."
            )),
        }
    }

    fn provision_backend(app: &AppHandle, distro: &str) -> Result<String, String> {
        let executor = SystemWsl;
        ensure_non_root(&executor, distro)?;
        let archive_path = resource_file(app, ARCHIVE_NAME)?;
        let checksum_path = resource_file(app, CHECKSUM_NAME)?;
        let expected = fs::read_to_string(&checksum_path)
            .map_err(|error| format!("could not read {}: {error}", checksum_path.display()))?
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("bundled WSL backend checksum is invalid".to_string());
        }

        if let Some(binary) = installed_backend(&executor, distro, &expected)? {
            return Ok(binary);
        }

        let archive = fs::read(&archive_path)
            .map_err(|error| format!("could not read {}: {error}", archive_path.display()))?;
        let actual = format!("{:x}", Sha256::digest(&archive));
        if expected != actual {
            return Err("bundled WSL backend checksum verification failed".to_string());
        }

        let home_output = execute_checked(&executor, distro, &["printenv", "HOME"], None)?;
        let home = decode_wsl_output(&home_output.stdout).trim().to_string();
        if !home.starts_with('/') || home.contains('\0') {
            return Err(format!("WSL returned an invalid HOME path: {home:?}"));
        }

        let base = format!("{home}/.local/share/splitty/desktop");
        let final_path = format!("{base}/{}", env!("CARGO_PKG_VERSION"));
        let binary_path = format!("{final_path}/splitty");

        emit_progress(
            app,
            SetupStage::Provisioning,
            format!("Installing backend in {distro}…"),
        );

        execute_checked(&executor, distro, &["mkdir", "-p", &base], None)?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let payload_name = format!(
            ".{}-{}-{}-{nonce}",
            env!("CARGO_PKG_VERSION"),
            &actual[..12],
            std::process::id()
        );
        let payload_path = format!("{base}/{payload_name}");
        let link_path = format!("{base}/.{payload_name}.link");

        let provision = (|| {
            execute_checked(&executor, distro, &["mkdir", &payload_path], None)?;
            execute_checked(
                &executor,
                distro,
                &["tar", "-xzf", "-", "-C", &payload_path],
                Some(&archive),
            )?;
            let payload_binary = format!("{payload_path}/splitty");
            let payload_marker = format!("{payload_path}/.archive-sha256");
            execute_checked(&executor, distro, &["chmod", "755", &payload_binary], None)?;
            execute_checked(
                &executor,
                distro,
                &["tee", &payload_marker],
                Some(expected.as_bytes()),
            )?;
            execute_checked(&executor, distro, &["test", "-x", &payload_binary], None)?;
            execute_checked(
                &executor,
                distro,
                &["ln", "-s", &payload_name, &link_path],
                None,
            )?;
            execute_checked(
                &executor,
                distro,
                &["mv", "-Tf", &link_path, &final_path],
                None,
            )?;
            Ok(binary_path)
        })();

        if provision.is_err() {
            let _ = execute_checked(&executor, distro, &["rm", "-rf", &payload_path], None);
            let _ = execute_checked(&executor, distro, &["rm", "-f", &link_path], None);
        }
        provision
    }

    struct BackendBridge {
        child: Child,
        stdin: Option<ChildStdin>,
        _stderr_thread: std::thread::JoinHandle<()>,
        url: String,
        port: u16,
        distro: String,
    }

    pub struct WindowsWslState {
        bridge: Mutex<Option<BackendBridge>>,
        startup: Mutex<()>,
        /// Origins the main window may navigate to, i.e. the running backend's.
        /// Empty until the backend is up, so the setup page cannot be swapped
        /// for an arbitrary loopback page in the meantime (see `navigation`).
        backend_origins: Mutex<Vec<String>>,
        /// The backend ports whose origin the main window's capability names.
        granted_ports: Mutex<Vec<u16>>,
    }

    impl WindowsWslState {
        pub fn new() -> Self {
            Self {
                bridge: Mutex::new(None),
                startup: Mutex::new(()),
                backend_origins: Mutex::new(Vec::new()),
                granted_ports: Mutex::new(Vec::new()),
            }
        }

        /// The origins of the running backend, for the navigation guard.
        pub fn backend_origins(&self) -> Vec<String> {
            self.backend_origins
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }

        /// Admit the backend on `port`: the main window's native commands on
        /// its exact origin (once per port, see `capability`), and its origins
        /// for the navigation guard.
        fn set_backend_port(&self, app: &AppHandle, port: u16) -> Result<(), String> {
            let mut granted = self
                .granted_ports
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !granted.contains(&port) {
                app.add_capability(crate::capability::main_window(port))
                    .map_err(|error| {
                        format!("could not grant the Splitty UI its native commands: {error}")
                    })?;
                granted.push(port);
            }
            *self
                .backend_origins
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                crate::navigation::backend_origins(port);
            Ok(())
        }

        pub fn shutdown(&self) {
            let mut slot = self
                .bridge
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(mut bridge) = slot.take() else {
                return;
            };

            // Closing the pipe sends EOF to `--desktop-stdio` in WSL.
            drop(bridge.stdin.take());
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match bridge.child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    _ => break,
                }
            }
            // Kill only the remaining wsl.exe bridge process. Never terminate
            // the distribution, which could contain unrelated user processes.
            let _ = bridge.child.kill();
            let _ = bridge.child.wait();
        }
    }

    fn emit_progress(app: &AppHandle, stage: SetupStage, message: impl Into<String>) {
        let _ = app.emit_to(
            "main",
            "wsl-setup-progress",
            WslSetupProgress {
                stage,
                message: message.into(),
            },
        );
    }

    fn stderr_reader(
        stderr: impl Read + Send + 'static,
        captured: Arc<Mutex<VecDeque<u8>>>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut buffer = [0_u8; 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        let mut target = captured
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        for byte in &buffer[..count] {
                            if target.len() == STDERR_LIMIT {
                                target.pop_front();
                            }
                            target.push_back(*byte);
                        }
                    }
                }
            }
        })
    }

    fn bridge_stderr(captured: &Arc<Mutex<VecDeque<u8>>>) -> String {
        let bytes: Vec<u8> = captured
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .copied()
            .collect();
        limited_text(&bytes)
    }

    /// Strip credential values out of a backend stdout line before logging it.
    ///
    /// The bridge mirrors the backend's stdout into `tracing::debug!`, and that
    /// stream ends up in support bundles and issue reports. It carries the
    /// bootstrap code and the launch URL, so it was a standing credential
    /// disclosure (security review SR-04).
    ///
    /// Redacting by key rather than dropping the lines keeps the log useful for
    /// diagnosing startup: which markers arrived, and in what order, is exactly
    /// what one needs to see, and the values never are.
    fn redact_credentials(line: &str) -> String {
        const SENSITIVE_PREFIXES: [&str; 3] =
            ["SPLITTY_BOOTSTRAP=", "SPLITTY_TOKEN=", "SPLITTY_URL="];

        for prefix in SENSITIVE_PREFIXES {
            if line.starts_with(prefix) {
                return format!("{prefix}<redacted>");
            }
        }
        line.to_string()
    }

    fn start_bridge(distro: &str, binary: &str) -> Result<BackendBridge, String> {
        // Re-checked here and not only at provisioning time: the distribution's
        // default user can be changed between setup and a later launch (SR-19).
        ensure_non_root(&SystemWsl, distro)?;

        let mut child = wsl_command()
            .args([
                "-d",
                distro,
                "--exec",
                binary,
                "serve",
                "--addr",
                "127.0.0.1:0",
                "--oneshot-token",
                "--desktop-stdio",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not start Splitty in WSL: {error}"))?;
        let stdin = child.stdin.take().ok_or("wsl.exe did not expose stdin")?;
        let stdout = child.stdout.take().ok_or("wsl.exe did not expose stdout")?;
        let stderr = child.stderr.take().ok_or("wsl.exe did not expose stderr")?;
        let captured_stderr = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_thread = stderr_reader(stderr, captured_stderr.clone());

        let (line_sender, line_receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        tracing::debug!(target: "splitty_wsl", "{}", redact_credentials(&line));
                        let _ = line_sender.send(line);
                    }
                    Err(_) => break,
                }
            }
        });

        let deadline = Instant::now() + START_TIMEOUT;
        let mut port = None;
        let mut bootstrap = None;
        let mut url = None;
        while Instant::now() < deadline && (port.is_none() || bootstrap.is_none() || url.is_none())
        {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("could not inspect WSL backend: {error}"))?
            {
                let _ = stderr_thread.join();
                return Err(format!(
                    "Splitty exited before it was ready ({status}): {}",
                    bridge_stderr(&captured_stderr)
                ));
            }
            match line_receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    if let Some(value) = line.strip_prefix("SPLITTY_PORT=") {
                        port = value.trim().parse::<u16>().ok();
                    } else if let Some(value) = line.strip_prefix("SPLITTY_BOOTSTRAP=") {
                        bootstrap = Some(value.trim().to_string());
                    } else if let Some(value) = line.strip_prefix("SPLITTY_URL=") {
                        url = Some(value.trim().to_string());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        let (port, bootstrap, url) = match (port, bootstrap, url) {
            (Some(port), Some(bootstrap), Some(url)) => (port, bootstrap, url),
            _ => {
                drop(stdin);
                let _ = child.kill();
                let _ = child.wait();
                let _ = stderr_thread.join();
                return Err(format!(
                    "Splitty did not report a listening address within {} seconds: {}",
                    START_TIMEOUT.as_secs(),
                    bridge_stderr(&captured_stderr)
                ));
            }
        };
        // The URL carries a single-use bootstrap code, not the process token
        // (security review SR-04). Verifying it byte for byte
        // keeps the existing guarantee that the backend cannot talk this
        // launcher into navigating anywhere but its own loopback port.
        let expected_url = format!("http://127.0.0.1:{port}/?bootstrap={bootstrap}&desktop=1");
        if url != expected_url {
            drop(stdin);
            let _ = child.kill();
            let _ = child.wait();
            let _ = stderr_thread.join();
            return Err("Splitty returned an invalid localhost URL".to_string());
        }

        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let forwarding_deadline = Instant::now() + FORWARDING_TIMEOUT;
        while TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_err() {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("could not inspect WSL backend: {error}"))?
            {
                let _ = stderr_thread.join();
                return Err(format!(
                    "Splitty exited before Windows localhost forwarding was ready ({status}): {}",
                    bridge_stderr(&captured_stderr)
                ));
            }
            if Instant::now() >= forwarding_deadline {
                drop(stdin);
                let _ = child.kill();
                let _ = child.wait();
                let _ = stderr_thread.join();
                return Err(
                    "The backend is running, but Windows cannot reach WSL localhost. Enable WSL localhost forwarding and try again."
                        .to_string(),
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        Ok(BackendBridge {
            child,
            stdin: Some(stdin),
            _stderr_thread: stderr_thread,
            url,
            port,
            distro: distro.to_string(),
        })
    }

    #[tauri::command]
    pub fn wsl_setup_state(app: AppHandle) -> Result<WindowsDesktopState, String> {
        read_config(&app)
    }

    #[tauri::command]
    pub async fn wsl_list_distributions(app: AppHandle) -> Result<Vec<WslDistribution>, String> {
        emit_progress(&app, SetupStage::Detecting, "Detecting WSL distributions…");
        tauri::async_runtime::spawn_blocking(move || detect_with(&SystemWsl))
            .await
            .map_err(|error| format!("WSL detection task failed: {error}"))?
    }

    #[tauri::command]
    pub async fn wsl_start_backend(
        app: AppHandle,
        state: tauri::State<'_, Arc<WindowsWslState>>,
        distro: Option<String>,
    ) -> Result<WslStartResult, String> {
        let state = Arc::clone(state.inner());
        tauri::async_runtime::spawn_blocking(move || {
            let _startup = state
                .startup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let config = read_config(&app)?;
            let distro = distro
                .or(config.pending_distro.clone())
                .or(config.selected_distro.clone())
                .ok_or("Select a supported WSL distribution first")?;

            if let Some(existing) = state
                .bridge
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_mut()
            {
                if existing.distro == distro && existing.child.try_wait().ok().flatten().is_none() {
                    state.set_backend_port(&app, existing.port)?;
                    return Ok(WslStartResult {
                        distro,
                        url: existing.url.clone(),
                    });
                }
            }

            // A saved selection was fully validated when it was chosen. Avoid
            // enumerating every distribution (and starting several short-lived
            // WSL processes) on every ordinary application launch.
            if saved_distribution(&config) != Some(distro.as_str()) {
                emit_progress(&app, SetupStage::Detecting, format!("Checking {distro}…"));
                let distributions = detect_with(&SystemWsl)?;
                let selected = distributions
                    .iter()
                    .find(|candidate| candidate.name == distro)
                    .ok_or_else(|| format!("WSL distribution '{distro}' is no longer installed"))?;
                if !selected.supported {
                    return Err(selected
                        .reason
                        .clone()
                        .unwrap_or_else(|| "distribution is unsupported".to_string()));
                }
            }

            emit_progress(
                &app,
                SetupStage::Starting,
                format!("Preparing backend in {distro}…"),
            );
            let binary = provision_backend(&app, &distro)?;
            emit_progress(
                &app,
                SetupStage::Starting,
                format!("Starting backend in {distro}…"),
            );
            let bridge = start_bridge(&distro, &binary)?;
            let url = bridge.url.clone();
            // Before the URL goes back to the page: the setup page navigates to
            // it as soon as it arrives, and the guard and the capability must
            // already know it.
            state.set_backend_port(&app, bridge.port)?;

            let mut promoted = config;
            promoted.selected_distro = Some(distro.clone());
            promoted.pending_distro = None;
            write_config(&app, &promoted)?;

            let mut slot = state
                .bridge
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(mut old) = slot.replace(bridge) {
                drop(old.stdin.take());
                let _ = old.child.kill();
                let _ = old.child.wait();
            }
            emit_progress(&app, SetupStage::Ready, format!("{distro} is ready"));
            Ok(WslStartResult { distro, url })
        })
        .await
        .map_err(|error| format!("WSL startup task failed: {error}"))?
    }

    #[tauri::command]
    pub async fn wsl_switch_distribution(app: AppHandle, distro: String) -> Result<(), String> {
        let restart_handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let distributions = detect_with(&SystemWsl)?;
            let selected = distributions
                .iter()
                .find(|candidate| candidate.name == distro)
                .ok_or_else(|| format!("WSL distribution '{distro}' is not installed"))?;
            if !selected.supported {
                return Err(selected
                    .reason
                    .clone()
                    .unwrap_or_else(|| "distribution is unsupported".to_string()));
            }
            let mut config = read_config(&app)?;
            config.pending_distro = Some(distro);
            write_config(&app, &config)
        })
        .await
        .map_err(|error| format!("WSL switch task failed: {error}"))??;
        restart_handle.restart();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        struct FakeWsl {
            outputs: Mutex<VecDeque<Result<ProcessOutput, String>>>,
        }

        impl FakeWsl {
            fn new(outputs: Vec<Result<ProcessOutput, String>>) -> Self {
                Self {
                    outputs: Mutex::new(outputs.into()),
                }
            }
        }

        impl WslExecutor for FakeWsl {
            fn execute(
                &self,
                _arguments: &[OsString],
                _input: Option<&[u8]>,
                _timeout: Duration,
            ) -> Result<ProcessOutput, String> {
                self.outputs.lock().unwrap().pop_front().unwrap()
            }
        }

        /// SR-19: a distribution whose default user is root must be refused.
        ///
        /// Without this, the backend, its shells and the installed files would
        /// all be root-owned inside the distribution — turning every other
        /// finding into a root-level one.
        #[test]
        fn root_default_user_is_rejected() {
            let fake = FakeWsl::new(vec![success("0\n")]);
            let error = ensure_non_root(&fake, "Ubuntu").expect_err("root must be refused");
            assert!(error.contains("root"), "error should explain why: {error}");
            assert!(
                error.contains("--default-user"),
                "error should say how to fix it: {error}",
            );
        }

        #[test]
        fn normal_default_user_is_accepted() {
            let fake = FakeWsl::new(vec![success("1000\n")]);
            assert!(ensure_non_root(&fake, "Ubuntu").is_ok());
        }

        /// An unreadable uid must fail closed rather than be assumed non-root.
        #[test]
        fn unparseable_uid_is_rejected() {
            let fake = FakeWsl::new(vec![success("not-a-number\n")]);
            assert!(ensure_non_root(&fake, "Ubuntu").is_err());
        }

        /// SR-04: the bridge mirrors backend stdout into the debug log, which
        /// ends up in support bundles. Credential-bearing lines must be redacted.
        #[test]
        fn backend_stdout_credentials_are_redacted_in_logs() {
            assert_eq!(
                redact_credentials("SPLITTY_BOOTSTRAP=abc123"),
                "SPLITTY_BOOTSTRAP=<redacted>",
            );
            assert_eq!(
                redact_credentials("SPLITTY_URL=http://127.0.0.1:1/?bootstrap=abc"),
                "SPLITTY_URL=<redacted>",
            );
            assert_eq!(
                redact_credentials("SPLITTY_TOKEN=abc"),
                "SPLITTY_TOKEN=<redacted>"
            );
            // Non-credential diagnostics stay readable.
            assert_eq!(
                redact_credentials("SPLITTY_PORT=45299"),
                "SPLITTY_PORT=45299"
            );
        }

        fn success(stdout: impl Into<Vec<u8>>) -> Result<ProcessOutput, String> {
            Ok(ProcessOutput {
                success: true,
                stdout: stdout.into(),
                stderr: Vec::new(),
            })
        }

        #[test]
        fn abstract_executor_detects_supported_and_wsl1_distributions() {
            let list = b"NAME STATE VERSION\r\n* Ubuntu Running 2\r\nLegacy Stopped 1\r\n".to_vec();
            let fake = FakeWsl::new(vec![
                success(list),
                success(b"x86_64\n".to_vec()),
                success(b"ID=ubuntu\n".to_vec()),
            ]);

            let detected = detect_with(&fake).unwrap();
            assert_eq!(detected.len(), 2);
            assert!(detected[0].supported);
            assert!(detected[0].is_default);
            assert!(!detected[1].supported);
            assert!(detected[1].reason.as_deref().unwrap().contains("WSL 1"));
        }

        #[test]
        fn abstract_executor_preserves_detection_failures() {
            let fake = FakeWsl::new(vec![Err("wsl.exe timed out after 15 seconds".into())]);
            assert!(detect_with(&fake).unwrap_err().contains("timed out"));
        }

        #[test]
        fn installed_backend_uses_single_probe_and_accepts_absolute_path() {
            let fake = FakeWsl::new(vec![success(
                b"/home/user/.local/share/splitty/desktop/0.1.0/splitty\n".to_vec(),
            )]);
            let checksum = "0".repeat(64);

            assert_eq!(
                installed_backend(&fake, "Ubuntu Dev", &checksum).unwrap(),
                Some("/home/user/.local/share/splitty/desktop/0.1.0/splitty".into())
            );
            assert!(fake.outputs.lock().unwrap().is_empty());
        }

        #[test]
        fn installed_backend_treats_empty_probe_as_cache_miss() {
            let fake = FakeWsl::new(vec![success(Vec::new())]);
            let checksum = "0".repeat(64);

            assert_eq!(installed_backend(&fake, "Debian", &checksum).unwrap(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xfe];
        for word in text.encode_utf16() {
            bytes.extend(word.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn parses_utf16_default_and_names_with_spaces_or_unicode() {
        let output = utf16(
            "  NAME                   STATE           VERSION\r\n\
             * Ubuntu Dev            Running         2\r\n\
               Débian-测试            Stopped         1\r\n",
        );
        assert_eq!(
            parse_wsl_list(&output),
            vec![
                ListedDistribution {
                    name: "Ubuntu Dev".into(),
                    version: 2,
                    is_default: true,
                },
                ListedDistribution {
                    name: "Débian-测试".into(),
                    version: 1,
                    is_default: false,
                },
            ]
        );
    }

    #[test]
    fn accepts_utf8_wsl_output() {
        let parsed = parse_wsl_list(b"NAME STATE VERSION\r\nDebian Stopped 2\r\n");
        assert_eq!(parsed[0].name, "Debian");
        assert_eq!(parsed[0].version, 2);
    }

    #[test]
    fn rejects_wsl1_arm_and_unsupported_os() {
        assert!(compatibility_reason(1, "x86_64", Some("ubuntu"))
            .unwrap()
            .contains("WSL 1"));
        assert!(compatibility_reason(2, "aarch64", Some("debian"))
            .unwrap()
            .contains("aarch64"));
        assert!(compatibility_reason(2, "x86_64", Some("alpine"))
            .unwrap()
            .contains("alpine"));
        assert_eq!(compatibility_reason(2, "x86_64", Some("ubuntu")), None);
        assert_eq!(compatibility_reason(2, "x86_64", Some("debian")), None);
    }

    #[test]
    fn parses_quoted_os_release_id() {
        assert_eq!(
            os_release_id("NAME=Ubuntu\nID=\"ubuntu\"\n"),
            Some("ubuntu".into())
        );
        assert_eq!(os_release_id("ID='debian'\n"), Some("debian".into()));
        assert_eq!(os_release_id("NAME=Unknown\n"), None);
    }

    #[test]
    fn saved_distribution_prefers_pending_selection() {
        let state = WindowsDesktopState {
            selected_distro: Some("Ubuntu".into()),
            pending_distro: Some("Debian".into()),
        };
        assert_eq!(saved_distribution(&state), Some("Debian"));

        let state = WindowsDesktopState {
            selected_distro: Some("Ubuntu".into()),
            pending_distro: None,
        };
        assert_eq!(saved_distribution(&state), Some("Ubuntu"));
    }
}
