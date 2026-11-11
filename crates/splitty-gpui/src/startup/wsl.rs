//! Windows startup without options: choose a WSL distribution, make sure the
//! backend that ships beside the executable is installed in it, and name it as
//! the launch target. Every dynamic value is a separate `wsl.exe` argument or a
//! positional parameter of a constant script; nothing is interpolated into
//! shell input. WSL itself is never installed, changed or shut down.
//!
//! The backend lives at `~/.local/share/splitty/desktop/<version>/splitty` with
//! its archive checksum beside it, the layout the web app uses, so both apps of
//! one version share an installation. The remembered distribution is read from
//! the web app's state file for the same reason.

#[cfg(target_os = "windows")]
const ARCHIVE_NAME: &str = "splitty-wsl-x86_64.tar.gz";
#[cfg(target_os = "windows")]
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Prints the facts a start needs in one `wsl.exe` round trip.
/// Parameters: `$1` version, `$2` expected archive checksum.
const INSPECT_SCRIPT: &str = r#"printf 'uid=%s\n' "$(id -u)"; printf 'arch=%s\n' "$(uname -m)"; printf 'id=%s\n' "$(sed -n 's/^ID=//p' /etc/os-release 2>/dev/null | head -n 1 | tr -d "\"'")"; base="$HOME/.local/share/splitty/desktop/$1"; if [ -x "$base/splitty" ] && [ -f "$base/.archive-sha256" ] && [ "$(cat "$base/.archive-sha256")" = "$2" ]; then printf 'backend=%s\n' "$base/splitty"; fi"#;

/// Unpacks the archive on stdin beside the final path and swaps it in with one
/// rename, so a start never sees a partial installation.
/// Parameters: `$1` version, `$2` archive checksum, `$3` unique suffix.
const INSTALL_SCRIPT: &str = r#"base="$HOME/.local/share/splitty/desktop"; name=".$1-$3"; dir="$base/$name"; link="$base/$name.link"; fail() { rm -rf "$dir"; rm -f "$link"; exit 1; }; mkdir -p "$base" && mkdir "$dir" || exit 1; tar -xzf - -C "$dir" || fail; chmod 755 "$dir/splitty" || fail; printf '%s' "$2" > "$dir/.archive-sha256" || fail; test -x "$dir/splitty" || fail; ln -s "$name" "$link" || fail; mv -Tf "$link" "$base/$1" || fail; printf 'backend=%s\n' "$base/$1/splitty""#;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    name: String,
    version: u32,
    is_default: bool,
}

/// `wsl.exe` commonly emits UTF-16LE even when stdout is redirected, but newer
/// versions may emit UTF-8. Accept both forms and strip a Unicode BOM.
fn decode(bytes: &[u8]) -> String {
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

/// The rows of `wsl.exe --list --verbose`. The state column is always the one
/// before VERSION, which keeps names with spaces intact.
fn parse_list(bytes: &[u8]) -> Vec<Listed> {
    decode(bytes)
        .lines()
        .filter_map(|line| {
            let line = line.trim_matches(['\r', '\0', ' ']);
            let (is_default, line) = match line.strip_prefix('*') {
                Some(rest) => (true, rest.trim_start()),
                None => (false, line),
            };
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 3 {
                return None;
            }
            let version = fields.last()?.parse::<u32>().ok()?;
            let name = fields[..fields.len() - 2].join(" ");
            (!name.is_empty()).then_some(Listed {
                name,
                version,
                is_default,
            })
        })
        .collect()
}

/// WSL 2 distributions a user works in, in the order they are offered: the
/// default one first. The distributions container tools keep for themselves
/// are left out; they are never started to be inspected.
fn candidates(listed: &[Listed]) -> Vec<&str> {
    let mut names: Vec<&Listed> = listed
        .iter()
        .filter(|item| item.version == 2 && !is_tool_distribution(&item.name))
        .collect();
    names.sort_by_key(|item| !item.is_default);
    names.into_iter().map(|item| item.name.as_str()).collect()
}

fn is_tool_distribution(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["docker-desktop", "rancher-desktop", "podman-machine"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// The state file with `distro` as the selected distribution and no pending
/// switch; its other fields are kept.
fn selected_state(existing: &str, distro: &str) -> String {
    let mut state = match serde_json::from_str(existing) {
        Ok(serde_json::Value::Object(state)) => state,
        _ => serde_json::Map::new(),
    };
    state.remove("pendingDistro");
    state.insert("selectedDistro".to_owned(), distro.into());
    serde_json::Value::Object(state).to_string()
}

/// The distribution the web app remembered, a pending switch before the
/// selected one.
fn remembered(state: &str) -> Option<String> {
    let state: serde_json::Value = serde_json::from_str(state).ok()?;
    ["pendingDistro", "selectedDistro"]
        .iter()
        .find_map(|key| state.get(key)?.as_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// The first field of a `sha256sum` line, when it is a SHA-256.
fn checksum(contents: &str) -> Option<String> {
    let sum = contents.split_whitespace().next()?.to_ascii_lowercase();
    (sum.len() == 64 && sum.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(sum)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Inspection {
    uid: String,
    arch: String,
    os_id: String,
    backend: Option<String>,
}

fn field<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output.lines().find_map(|line| {
        line.trim_end_matches('\r')
            .strip_prefix(key)?
            .strip_prefix('=')
    })
}

/// An absolute Linux path without control characters, as a script printed it.
fn backend_path(output: &str) -> Option<String> {
    field(output, "backend")
        .filter(|path| path.starts_with('/') && !path.chars().any(char::is_control))
        .map(str::to_owned)
}

fn parse_inspection(output: &str) -> Inspection {
    let text = |key| field(output, key).unwrap_or_default().trim().to_owned();
    Inspection {
        uid: text("uid"),
        arch: text("arch"),
        os_id: text("id").to_ascii_lowercase(),
        backend: backend_path(output),
    }
}

/// Why a distribution cannot run the backend, if it cannot. A root default
/// user is refused: the backend and every shell it starts would have full
/// control of the distribution.
fn unsupported(distro: &str, inspection: &Inspection) -> Option<String> {
    if inspection.arch != "x86_64" {
        return Some(format!(
            "architecture '{}' is not supported; Splitty requires x86_64",
            inspection.arch
        ));
    }
    if !matches!(inspection.os_id.as_str(), "ubuntu" | "debian") {
        return Some(format!(
            "distribution ID '{}' is not supported; use Ubuntu or Debian",
            inspection.os_id
        ));
    }
    match inspection.uid.parse::<u32>() {
        Ok(0) => Some(format!(
            "its default user is root. Set a normal default user, for example:\n    {distro} config --default-user <your-user>"
        )),
        Ok(_) => None,
        Err(_) => Some("its default user could not be determined".to_owned()),
    }
}

#[cfg(target_os = "windows")]
pub use windows::{distributions, prepare, report_failure, select};

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use sha2::{Digest, Sha256};
    use splitty_client_core::owned_service::WslLaunch;
    use std::io::{Read, Write};
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const QUERY_TIMEOUT: Duration = Duration::from_secs(20);
    const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);
    const OUTPUT_LIMIT: u64 = 64 * 1024;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    struct Output {
        success: bool,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    }

    /// Runs `wsl.exe` with its own bounded pipes and kills only this child on a
    /// timeout.
    fn wsl(
        arguments: &[&str],
        input: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<Output, String> {
        let mut child = Command::new("wsl.exe")
            .args(arguments)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    "WSL is not installed. Install it from an elevated PowerShell with\n    wsl --install -d Ubuntu\nand start Splitty again.".to_owned()
                } else {
                    format!("Could not start wsl.exe: {error}")
                }
            })?;
        let capture = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut captured = Vec::new();
                if let Some(pipe) = pipe {
                    let mut pipe = pipe.take(OUTPUT_LIMIT);
                    let _ = pipe.read_to_end(&mut captured);
                    // Drain what exceeds the limit so the writer cannot block.
                    let _ = std::io::copy(&mut pipe.into_inner(), &mut std::io::sink());
                }
                captured
            })
        };
        let stdout = capture(child.stdout.take().map(|pipe| Box::new(pipe) as _));
        let stderr = capture(child.stderr.take().map(|pipe| Box::new(pipe) as _));
        let writer = child.stdin.take().zip(input).map(|(mut stdin, input)| {
            std::thread::spawn(move || {
                let _ = stdin.write_all(&input);
            })
        });
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(15));
                }
                Ok(None) => {
                    break Err(format!(
                        "wsl.exe timed out after {} seconds",
                        timeout.as_secs()
                    ))
                }
                Err(error) => break Err(format!("Could not wait for wsl.exe: {error}")),
            }
        };
        if status.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(writer) = writer {
            let _ = writer.join();
        }
        let stdout = stdout.join().unwrap_or_default();
        let stderr = stderr.join().unwrap_or_default();
        Ok(Output {
            success: status?.success(),
            stdout,
            stderr,
        })
    }

    fn reason(output: &Output) -> String {
        let text = decode(&output.stderr);
        let text = text.trim();
        if text.is_empty() {
            "no error output".to_owned()
        } else {
            text.chars().take(600).collect()
        }
    }

    fn inspect(distro: &str, expected: &str) -> Result<Inspection, String> {
        let output = wsl(
            &[
                "-d",
                distro,
                "--exec",
                "sh",
                "-c",
                INSPECT_SCRIPT,
                "splitty-inspect",
                VERSION,
                expected,
            ],
            None,
            QUERY_TIMEOUT,
        )?;
        if !output.success {
            return Err(format!(
                "The WSL distribution '{distro}' could not be inspected: {}",
                reason(&output)
            ));
        }
        Ok(parse_inspection(&decode(&output.stdout)))
    }

    fn resource(name: &str) -> Result<PathBuf, String> {
        let exe = std::env::current_exe()
            .map_err(|error| format!("Could not locate the application: {error}"))?;
        let root = exe.parent().unwrap_or(&exe);
        [root.join("resources").join(name), root.join(name)]
            .into_iter()
            .find(|path| path.is_file())
            .ok_or_else(|| format!("The WSL backend that ships with Splitty is missing: {name}\nInstall Splitty again."))
    }

    fn state_path() -> Option<PathBuf> {
        Some(
            PathBuf::from(std::env::var_os("APPDATA")?)
                .join("com.splitty.app")
                .join("windows.json"),
        )
    }

    fn remembered_distro() -> Option<String> {
        remembered(&std::fs::read_to_string(state_path()?).ok()?)
    }

    /// Stores the choice through a temporary file and one rename.
    fn remember(distro: &str) -> Result<(), String> {
        let path = state_path().ok_or("The application data directory is unknown.")?;
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let write = || -> std::io::Result<()> {
            if let Some(directory) = path.parent() {
                std::fs::create_dir_all(directory)?;
            }
            std::fs::write(&temporary, selected_state(&existing, distro))?;
            std::fs::rename(&temporary, &path)
        };
        write().map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            format!("The choice could not be saved: {error}")
        })
    }

    fn list() -> Result<Vec<Listed>, String> {
        let listed = wsl(&["--list", "--verbose"], None, QUERY_TIMEOUT)?;
        if !listed.success {
            return Err(format!(
                "WSL distributions could not be listed: {}\n\nInstall a distribution from an elevated PowerShell with\n    wsl --install -d Ubuntu",
                reason(&listed)
            ));
        }
        Ok(parse_list(&listed.stdout))
    }

    /// The remembered distribution; otherwise the only installed one, or with
    /// several the one chosen in a dialog, which is then remembered. A
    /// distribution is started to be inspected only once it is the candidate.
    fn choose(expected: &str) -> Result<(String, Inspection), String> {
        if let Some(distro) = remembered_distro() {
            let inspection = inspect(&distro, expected)?;
            return match unsupported(&distro, &inspection) {
                None => Ok((distro, inspection)),
                Some(why) => Err(format!(
                    "The WSL distribution '{distro}' cannot be used: {why}"
                )),
            };
        }
        let listed = list()?;
        let candidates = candidates(&listed);
        let ask = candidates.len() > 1;
        let mut refused = Vec::new();
        for (index, distro) in candidates.iter().enumerate() {
            if ask {
                let rest = candidates.len() - index - 1;
                let otherwise = match rest {
                    0 => "No leaves Splitty without a distribution.".to_owned(),
                    1 => format!("No uses '{}'.", candidates[index + 1]),
                    _ => "No asks about the next one.".to_owned(),
                };
                // The last of several is the answer when the others were declined.
                if rest > 0 || !refused.is_empty() {
                    let question = format!(
                        "Several WSL distributions are installed.\n\nRun local terminals in '{distro}'?\n\n{otherwise} The choice can be changed in the settings."
                    );
                    if !confirm(&question) {
                        continue;
                    }
                }
            }
            let problem = match inspect(distro, expected) {
                Ok(inspection) => match unsupported(distro, &inspection) {
                    None => {
                        if ask {
                            // Starting works without the file; only the next
                            // start would ask again.
                            let _ = remember(distro);
                        }
                        return Ok(((*distro).to_owned(), inspection));
                    }
                    Some(why) => format!("{distro}: {why}"),
                },
                Err(error) => error,
            };
            if ask && index + 1 < candidates.len() {
                report_failure(&format!("This distribution cannot be used.\n\n{problem}"));
            }
            refused.push(problem);
        }
        Err(if candidates.is_empty() {
            "No WSL 2 distribution was found. Install one from an elevated PowerShell with\n    wsl --install -d Ubuntu\nand start Splitty again.".to_owned()
        } else if refused.is_empty() {
            "No WSL distribution was chosen.".to_owned()
        } else {
            format!(
                "No supported WSL distribution was found (WSL 2, x86_64, Ubuntu or Debian, a normal default user).\n\n{}",
                refused.join("\n")
            )
        })
    }

    /// The distributions the settings offer, the default one first.
    pub fn distributions() -> Vec<String> {
        list()
            .map(|listed| candidates(&listed).into_iter().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// Remembers `distro` for the next start, once it is known to be supported.
    pub fn select(distro: &str) -> Result<(), String> {
        let inspection = inspect(distro, "")?;
        if let Some(why) = unsupported(distro, &inspection) {
            return Err(format!("'{distro}' cannot be used: {why}"));
        }
        remember(distro)
    }

    fn install(distro: &str, expected: &str) -> Result<String, String> {
        let archive_path = resource(ARCHIVE_NAME)?;
        let archive = std::fs::read(&archive_path)
            .map_err(|error| format!("Could not read {}: {error}", archive_path.display()))?;
        if format!("{:x}", Sha256::digest(&archive)) != expected {
            return Err("The WSL backend that ships with Splitty does not match its checksum.\nInstall Splitty again.".to_owned());
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let suffix = format!("{}-{}-{nonce}", &expected[..12], std::process::id());
        let output = wsl(
            &[
                "-d",
                distro,
                "--exec",
                "sh",
                "-c",
                INSTALL_SCRIPT,
                "splitty-install",
                VERSION,
                expected,
                &suffix,
            ],
            Some(archive),
            INSTALL_TIMEOUT,
        )?;
        let installed = backend_path(&decode(&output.stdout));
        match (output.success, installed) {
            (true, Some(backend)) => Ok(backend),
            _ => Err(format!(
                "The backend could not be installed in the WSL distribution '{distro}': {}",
                reason(&output)
            )),
        }
    }

    /// The launch target of a start without options; installs the backend
    /// first when the distribution lacks the one that ships with this app.
    pub fn prepare() -> Result<WslLaunch, String> {
        let checksum_path = resource(&format!("{ARCHIVE_NAME}.sha256"))?;
        let expected = std::fs::read_to_string(&checksum_path)
            .ok()
            .as_deref()
            .and_then(checksum)
            .ok_or("The checksum of the WSL backend that ships with Splitty is invalid.\nInstall Splitty again.")?;
        let (distro, inspection) = choose(&expected)?;
        let backend = match inspection.backend {
            Some(backend) => backend,
            None => install(&distro, &expected)?,
        };
        WslLaunch::new(distro, backend, None).map_err(|error| error.to_string())
    }

    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(
            window: *mut std::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            flags: u32,
        ) -> i32;
    }

    fn message_box(message: &str, flags: u32) -> i32 {
        let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain([0]).collect() };
        let (text, caption) = (wide(message), wide("Splitty"));
        // SAFETY: both buffers are NUL-terminated UTF-16 and outlive the call;
        // a null owner window is allowed.
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), flags) }
    }

    /// A release build has no console, so a startup failure is shown in a dialog.
    pub fn report_failure(message: &str) {
        const MB_ICONERROR: u32 = 0x10;
        message_box(message, MB_ICONERROR);
    }

    fn confirm(question: &str) -> bool {
        const MB_YESNO: u32 = 0x4;
        const MB_ICONQUESTION: u32 = 0x20;
        const IDYES: i32 = 6;
        message_box(question, MB_YESNO | MB_ICONQUESTION) == IDYES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_read_from_utf16_and_utf8_and_the_default_is_tried_first() {
        let text = "  NAME                   STATE           VERSION\r\n  Old Linux              Stopped         1\r\n  Debian                 Stopped         2\r\n* Ubuntu 24.04           Running         2\r\n";
        let utf16: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        for bytes in [utf16.as_slice(), text.as_bytes()] {
            let listed = parse_list(bytes);
            assert_eq!(
                listed
                    .iter()
                    .map(|item| (item.name.as_str(), item.version, item.is_default))
                    .collect::<Vec<_>>(),
                [
                    ("Old Linux", 1, false),
                    ("Debian", 2, false),
                    ("Ubuntu 24.04", 2, true)
                ]
            );
            assert_eq!(candidates(&listed), ["Ubuntu 24.04", "Debian"]);
        }
        assert!(parse_list(b"").is_empty());
    }

    #[test]
    fn distributions_of_container_tools_are_not_offered() {
        let listed = parse_list(
            b"  NAME                STATE     VERSION\n* docker-desktop      Stopped   2\n  Ubuntu              Running   2\n  rancher-desktop-data Stopped  2\n",
        );
        assert_eq!(candidates(&listed), ["Ubuntu"]);
    }

    #[test]
    fn a_saved_choice_replaces_a_pending_switch_and_keeps_other_fields() {
        let state = selected_state(
            r#"{"selectedDistro":"Ubuntu","pendingDistro":"Arch","other":1}"#,
            "Debian",
        );
        assert_eq!(remembered(&state).as_deref(), Some("Debian"));
        let state: serde_json::Value = serde_json::from_str(&state).unwrap();
        assert_eq!(state["other"], 1);
        assert!(state.get("pendingDistro").is_none());
        assert_eq!(
            remembered(&selected_state("not json", "Ubuntu 24.04")).as_deref(),
            Some("Ubuntu 24.04")
        );
    }

    #[test]
    fn a_pending_switch_is_remembered_before_the_selected_distribution() {
        assert_eq!(
            remembered(r#"{"selectedDistro":"Ubuntu"}"#).as_deref(),
            Some("Ubuntu")
        );
        assert_eq!(
            remembered(r#"{"selectedDistro":"Ubuntu","pendingDistro":"Debian"}"#).as_deref(),
            Some("Debian")
        );
        assert_eq!(remembered(r#"{"selectedDistro":null}"#), None);
        assert_eq!(remembered("not json"), None);
    }

    #[test]
    fn only_a_sha256_is_accepted_as_the_shipped_checksum() {
        let sum = "A".repeat(64);
        assert_eq!(
            checksum(&format!("{sum}  splitty-wsl-x86_64.tar.gz\n")),
            Some("a".repeat(64))
        );
        assert_eq!(checksum("abc  file"), None);
        assert_eq!(checksum(&"g".repeat(64)), None);
        assert_eq!(checksum(""), None);
    }

    #[test]
    fn inspection_reads_its_fields_and_only_an_absolute_backend_path() {
        let inspection = parse_inspection("uid=1000\r\narch=x86_64\nid=Ubuntu\nbackend=/home/a b/.local/share/splitty/desktop/1/splitty\n");
        assert_eq!(
            inspection,
            Inspection {
                uid: "1000".into(),
                arch: "x86_64".into(),
                os_id: "ubuntu".into(),
                backend: Some("/home/a b/.local/share/splitty/desktop/1/splitty".into()),
            }
        );
        assert_eq!(unsupported("Ubuntu", &inspection), None);
        assert_eq!(
            parse_inspection("uid=1000\nbackend=relative/splitty\n").backend,
            None
        );
        assert_eq!(parse_inspection("uid=1000\n").backend, None);
    }

    #[test]
    fn root_other_architectures_and_other_distributions_are_refused() {
        let ok = Inspection {
            uid: "1000".into(),
            arch: "x86_64".into(),
            os_id: "debian".into(),
            backend: None,
        };
        assert_eq!(unsupported("Debian", &ok), None);
        for refused in [
            Inspection {
                uid: "0".into(),
                ..parse_inspection("arch=x86_64\nid=debian")
            },
            Inspection {
                uid: String::new(),
                ..parse_inspection("arch=x86_64\nid=debian")
            },
            Inspection {
                arch: "aarch64".into(),
                ..parse_inspection("uid=1000\nid=debian")
            },
            Inspection {
                os_id: "arch".into(),
                ..parse_inspection("uid=1000\narch=x86_64")
            },
        ] {
            assert!(unsupported("Debian", &refused).is_some(), "{refused:?}");
        }
    }

    /// The scripts run under the system `sh` with a temporary HOME: an
    /// installation is found by its version and checksum only, and a broken
    /// archive leaves nothing behind.
    #[cfg(unix)]
    #[test]
    fn the_scripts_install_once_and_find_the_installation_by_version_and_checksum() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let home = std::env::temp_dir().join(format!("splitty-wsl-setup-{}", uuid::Uuid::new_v4()));
        let payload = home.join("payload");
        std::fs::create_dir_all(&payload).unwrap();
        std::fs::write(payload.join("splitty"), "#!/bin/sh\n").unwrap();
        let archive = Command::new("tar")
            .args(["-czf", "-", "-C"])
            .arg(&payload)
            .arg("splitty")
            .output()
            .unwrap()
            .stdout;
        let run = |script: &str, arguments: &[&str], input: &[u8]| {
            let mut child = Command::new("sh")
                .args(["-c", script, "test"])
                .args(arguments)
                .env("HOME", &home)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(input).unwrap();
            let output = child.wait_with_output().unwrap();
            (
                output.status.success(),
                String::from_utf8(output.stdout).unwrap(),
            )
        };
        let base = home.join(".local/share/splitty/desktop");

        assert_eq!(
            parse_inspection(&run(INSPECT_SCRIPT, &["1.2.3", "sum-a"], b"").1).backend,
            None
        );
        let (failed, _) = run(
            INSTALL_SCRIPT,
            &["1.2.3", "sum-a", "broken"],
            b"not an archive",
        );
        assert!(!failed);
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0);

        let (installed, output) = run(INSTALL_SCRIPT, &["1.2.3", "sum-a", "one"], &archive);
        assert!(installed);
        let backend = backend_path(&output).unwrap();
        assert_eq!(backend, base.join("1.2.3/splitty").to_str().unwrap());
        let found = parse_inspection(&run(INSPECT_SCRIPT, &["1.2.3", "sum-a"], b"").1);
        assert_eq!(found.backend.as_deref(), Some(backend.as_str()));
        assert!(!found.uid.is_empty() && !found.arch.is_empty());
        assert_eq!(
            parse_inspection(&run(INSPECT_SCRIPT, &["1.2.3", "sum-b"], b"").1).backend,
            None
        );
        assert_eq!(
            parse_inspection(&run(INSPECT_SCRIPT, &["1.2.4", "sum-a"], b"").1).backend,
            None
        );

        // A second installation of the same version replaces the first in place.
        assert!(run(INSTALL_SCRIPT, &["1.2.3", "sum-b", "two"], &archive).0);
        assert_eq!(
            parse_inspection(&run(INSPECT_SCRIPT, &["1.2.3", "sum-b"], b"").1)
                .backend
                .as_deref(),
            Some(backend.as_str())
        );
        std::fs::remove_dir_all(&home).unwrap();
    }
}
