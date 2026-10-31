//! Shell profile detection, persistence reconciliation, validation, and runtime lookup.
//!
//! Profiles deliberately keep the executable and argv separate.  Nothing in this
//! module accepts or produces a shell command string.

use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where a shell process is executed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellTarget {
    #[default]
    Local,
    WindowsInterop,
}

/// Whether a profile was discovered by Splitty or created by the user.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellProfileSource {
    #[default]
    Detected,
    Custom,
}

/// Persisted representation stored below `[shells]` in `config.toml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredShellProfile {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub target: ShellTarget,
    #[serde(default)]
    pub source: ShellProfileSource,
}

fn enabled_by_default() -> bool {
    true
}

/// Persistent shell settings in the home configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellSettings {
    #[serde(default)]
    pub default_profile_id: String,
    #[serde(default)]
    pub profiles: Vec<StoredShellProfile>,
}

/// Original values for a detected profile.  The settings UI uses these for
/// "Reset" without guessing server-side executable paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedProfileDefaults {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub target: ShellTarget,
}

/// Public profile returned by `/api/shell-profiles`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellProfile {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub enabled: bool,
    pub target: ShellTarget,
    pub source: ShellProfileSource,
    #[serde(default)]
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub experimental: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_defaults: Option<DetectedProfileDefaults>,
}

impl ShellProfile {
    fn stored(&self) -> StoredShellProfile {
        StoredShellProfile {
            id: self.id.clone(),
            name: self.name.clone(),
            command: self.command.clone(),
            args: self.args.clone(),
            enabled: self.enabled,
            target: self.target.clone(),
            source: self.source.clone(),
        }
    }
}

/// Atomic GET/PUT payload for shell profiles.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellProfilesState {
    pub default_profile_id: String,
    #[serde(default)]
    pub profiles: Vec<ShellProfile>,
}

impl ShellProfilesState {
    /// Resolve according to the compatibility order: profile ID, legacy shell,
    /// then configured default profile.
    pub fn resolve(
        &self,
        profile_id: Option<&str>,
        legacy_shell: Option<&str>,
    ) -> Result<ResolvedShellProfile, ShellProfilesError> {
        if let Some(id) = profile_id.filter(|id| !id.is_empty()) {
            if let Some(profile) = self.profiles.iter().find(|p| p.id == id) {
                if profile.enabled && profile.available {
                    return Ok(ResolvedShellProfile::from_profile(profile));
                }
                return Err(ShellProfilesError::Unavailable(id.to_string()));
            }
            // Stale snapshots and old templates must remain startable.
            if let Some(profile) = self.default_profile() {
                return Ok(ResolvedShellProfile::from_profile(profile));
            }
            return Err(ShellProfilesError::Unavailable(id.to_string()));
        }

        if let Some(shell) = legacy_shell.filter(|shell| !shell.is_empty()) {
            return Ok(ResolvedShellProfile {
                id: None,
                command: shell.to_string(),
                args: login_args(shell),
                target: ShellTarget::Local,
            });
        }

        self.default_profile()
            .map(ResolvedShellProfile::from_profile)
            .ok_or(ShellProfilesError::NoAvailableProfile)
    }

    pub fn default_profile(&self) -> Option<&ShellProfile> {
        self.profiles
            .iter()
            .find(|p| p.id == self.default_profile_id && p.enabled && p.available)
            .or_else(|| self.profiles.iter().find(|p| p.enabled && p.available))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedShellProfile {
    pub id: Option<String>,
    pub command: String,
    pub args: Vec<String>,
    pub target: ShellTarget,
}

impl ResolvedShellProfile {
    fn from_profile(profile: &ShellProfile) -> Self {
        Self {
            id: Some(profile.id.clone()),
            command: profile.command.clone(),
            args: profile.args.clone(),
            target: profile.target.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ShellProfilesError {
    #[error("shell profile id is empty")]
    EmptyId,
    #[error("duplicate shell profile id: {0}")]
    DuplicateId(String),
    #[error("shell profile {0} has an empty name")]
    EmptyName(String),
    #[error("shell profile {0} has an empty executable")]
    EmptyCommand(String),
    #[error("shell profile is unavailable: {0}")]
    Unavailable(String),
    #[error("no enabled, available shell profile exists")]
    NoAvailableProfile,
}

/// Environment inputs are explicit so detection can be tested without changing
/// process-global environment variables.
#[derive(Clone, Debug)]
pub struct DetectionEnvironment {
    pub path: Vec<PathBuf>,
    pub shell: Option<String>,
    pub is_wsl: bool,
}

impl Default for DetectionEnvironment {
    fn default() -> Self {
        Self::current()
    }
}

impl DetectionEnvironment {
    pub fn current() -> Self {
        Self {
            path: env::var_os("PATH")
                .map(|value| env::split_paths(&value).collect())
                .unwrap_or_default(),
            shell: env::var("SHELL").ok().filter(|s| !s.trim().is_empty()),
            is_wsl: is_wsl(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DetectionResult {
    pub profiles: Vec<ShellProfile>,
    pub system_default_id: Option<String>,
    pub environment: DetectionEnvironment,
}

/// Detect supported local shells and Windows executables exposed by WSL.
pub fn detect_shell_profiles(env: &DetectionEnvironment) -> DetectionResult {
    let mut result = DetectionResult {
        environment: env.clone(),
        ..DetectionResult::default()
    };
    let mut commands = HashSet::new();
    let mut ids = HashSet::new();

    if let Some(shell) = env.shell.as_deref() {
        if let Some(command) = find_executable(shell, &env.path) {
            let id = known_id_for_command(&command)
                .map(str::to_string)
                .unwrap_or_else(|| format!("shell-system-{:016x}", stable_hash(&command)));
            let profile = detected_profile(
                id.clone(),
                display_name(&command),
                command.clone(),
                ShellTarget::Local,
            );
            commands.insert(command);
            ids.insert(id.clone());
            result.system_default_id = Some(id);
            result.profiles.push(profile);
        }
    }

    for executable in ["bash", "zsh", "fish", "pwsh"] {
        if let Some(command) = find_executable(executable, &env.path) {
            let id = known_id_for_name(executable).to_string();
            if commands.insert(command.clone()) && ids.insert(id.clone()) {
                result.profiles.push(detected_profile(
                    id,
                    display_name(executable),
                    command,
                    ShellTarget::Local,
                ));
            }
        }
    }

    if env.is_wsl {
        for executable in ["powershell.exe", "pwsh.exe"] {
            if let Some(command) = find_executable(executable, &env.path) {
                let id = known_id_for_name(executable).to_string();
                if !ids.insert(id.clone()) {
                    continue;
                }
                // A Linux pwsh and Windows pwsh.exe are intentionally distinct.
                result.profiles.push(detected_profile(
                    id,
                    display_name(executable),
                    command,
                    ShellTarget::WindowsInterop,
                ));
            }
        }
    }

    if result.system_default_id.is_none() {
        result.system_default_id = result.profiles.first().map(|p| p.id.clone());
    }
    result
}

/// Merge saved edits with current detection. Saved ordering wins; newly found
/// shells are appended. Detected profiles which disappeared remain visible but
/// unavailable, and custom profiles remain until explicitly deleted.
pub fn reconcile_shell_settings(
    settings: &mut ShellSettings,
    detection: &DetectionResult,
) -> ShellProfilesState {
    let detected: HashMap<&str, &ShellProfile> = detection
        .profiles
        .iter()
        .map(|profile| (profile.id.as_str(), profile))
        .collect();
    let mut seen = HashSet::new();
    let mut profiles = Vec::new();

    for saved in &settings.profiles {
        if saved.id.is_empty() || !seen.insert(saved.id.clone()) {
            continue;
        }
        let defaults = detected.get(saved.id.as_str()).copied();
        profiles.push(runtime_profile(saved, defaults, &detection.environment));
    }

    for profile in &detection.profiles {
        if seen.insert(profile.id.clone()) {
            profiles.push(profile.clone());
        }
    }

    // Extremely minimal environments may expose neither SHELL nor a supported
    // executable on PATH. Keep the application usable through /bin/sh.
    if profiles.is_empty() {
        let command = find_executable("sh", &detection.environment.path)
            .unwrap_or_else(|| "/bin/sh".to_string());
        profiles.push(detected_profile(
            "shell-system-sh".to_string(),
            "Shell".to_string(),
            command,
            ShellTarget::Local,
        ));
    }

    if !profiles
        .iter()
        .any(|profile| profile.enabled && profile.available)
    {
        if let Some(profile) = profiles.iter_mut().find(|profile| profile.available) {
            profile.enabled = true;
        }
    }

    let requested = settings.default_profile_id.clone();
    let default_profile_id = choose_default(&requested, &profiles, detection);
    settings.default_profile_id = default_profile_id.clone();
    settings.profiles = profiles.iter().map(ShellProfile::stored).collect();

    ShellProfilesState {
        default_profile_id,
        profiles,
    }
}

/// Build the runtime state for already-loaded settings using current detection.
pub fn state_from_settings(settings: &ShellSettings) -> ShellProfilesState {
    let mut settings = settings.clone();
    let detection = detect_shell_profiles(&DetectionEnvironment::current());
    reconcile_shell_settings(&mut settings, &detection)
}

/// Validate and atomically replace editable profile data, then reconcile it
/// with current detection. Detected profiles omitted by a client are retained
/// as disabled; custom profiles omitted by a client are deleted.
pub fn apply_shell_profiles_update(
    settings: &mut ShellSettings,
    update: ShellProfilesState,
    detection: &DetectionResult,
) -> Result<ShellProfilesState, ShellProfilesError> {
    let mut seen = HashSet::new();
    let detected_ids: HashSet<&str> = detection.profiles.iter().map(|p| p.id.as_str()).collect();
    let mut stored = Vec::with_capacity(update.profiles.len());

    for mut profile in update.profiles {
        profile.id = profile.id.trim().to_string();
        profile.name = profile.name.trim().to_string();
        profile.command = profile.command.trim().to_string();
        if profile.id.is_empty() {
            return Err(ShellProfilesError::EmptyId);
        }
        if !seen.insert(profile.id.clone()) {
            return Err(ShellProfilesError::DuplicateId(profile.id));
        }
        if profile.name.is_empty() {
            return Err(ShellProfilesError::EmptyName(profile.id));
        }
        if profile.command.is_empty() {
            return Err(ShellProfilesError::EmptyCommand(profile.id));
        }
        profile.source = if detected_ids.contains(profile.id.as_str())
            || settings
                .profiles
                .iter()
                .any(|p| p.id == profile.id && p.source == ShellProfileSource::Detected)
        {
            ShellProfileSource::Detected
        } else {
            ShellProfileSource::Custom
        };
        stored.push(profile.stored());
    }

    for old in &settings.profiles {
        if old.source == ShellProfileSource::Detected && seen.insert(old.id.clone()) {
            let mut retained = old.clone();
            retained.enabled = false;
            stored.push(retained);
        }
    }

    let requested_default = update.default_profile_id;
    let mut candidate = ShellSettings {
        default_profile_id: requested_default.clone(),
        profiles: stored,
    };
    let mut state = reconcile_shell_settings(&mut candidate, detection);
    let requested_is_valid = state
        .profiles
        .iter()
        .any(|profile| profile.id == requested_default && profile.enabled && profile.available);
    if !requested_is_valid {
        if let Some(first) = state
            .profiles
            .iter()
            .find(|profile| profile.enabled && profile.available)
        {
            candidate.default_profile_id = first.id.clone();
            state.default_profile_id = first.id.clone();
        }
    }
    // The invariant is deliberately stronger than mere ID validity: a default
    // can always be started.
    if state.default_profile().is_none() {
        return Err(ShellProfilesError::NoAvailableProfile);
    }
    *settings = candidate;
    Ok(state)
}

fn runtime_profile(
    saved: &StoredShellProfile,
    detected: Option<&ShellProfile>,
    env: &DetectionEnvironment,
) -> ShellProfile {
    let (available, reason) = availability(&saved.command, &saved.target, env);
    ShellProfile {
        id: saved.id.clone(),
        name: saved.name.clone(),
        command: saved.command.clone(),
        args: saved.args.clone(),
        enabled: saved.enabled,
        target: saved.target.clone(),
        source: saved.source.clone(),
        available,
        reason,
        experimental: saved.target == ShellTarget::WindowsInterop,
        detected_defaults: detected.map(|profile| DetectedProfileDefaults {
            name: profile.name.clone(),
            command: profile.command.clone(),
            args: profile.args.clone(),
            target: profile.target.clone(),
        }),
    }
}

fn detected_profile(
    id: String,
    name: String,
    command: String,
    target: ShellTarget,
) -> ShellProfile {
    let args = login_args(&command);
    let defaults = DetectedProfileDefaults {
        name: name.clone(),
        command: command.clone(),
        args: args.clone(),
        target: target.clone(),
    };
    ShellProfile {
        id,
        name,
        command,
        args,
        enabled: true,
        experimental: target == ShellTarget::WindowsInterop,
        target,
        source: ShellProfileSource::Detected,
        available: true,
        reason: None,
        detected_defaults: Some(defaults),
    }
}

fn choose_default(
    requested: &str,
    profiles: &[ShellProfile],
    detection: &DetectionResult,
) -> String {
    if profiles
        .iter()
        .any(|p| p.id == requested && p.enabled && p.available)
    {
        return requested.to_string();
    }
    if let Some(system) = detection.system_default_id.as_deref() {
        if profiles
            .iter()
            .any(|p| p.id == system && p.enabled && p.available)
        {
            return system.to_string();
        }
    }
    profiles
        .iter()
        .find(|p| p.enabled && p.available)
        .or_else(|| profiles.iter().find(|p| p.available))
        .map(|p| p.id.clone())
        .unwrap_or_default()
}

fn availability(
    command: &str,
    target: &ShellTarget,
    env: &DetectionEnvironment,
) -> (bool, Option<String>) {
    if *target == ShellTarget::WindowsInterop && !env.is_wsl {
        return (
            false,
            Some("Windows interoperability is only available inside WSL".to_string()),
        );
    }
    if find_executable(command, &env.path).is_some() {
        (true, None)
    } else {
        (false, Some(format!("Executable not found: {command}")))
    }
}

fn find_executable(command: &str, path: &[PathBuf]) -> Option<String> {
    let candidate = Path::new(command);
    if candidate.components().count() > 1 || candidate.is_absolute() {
        return is_executable(candidate).then(|| command.to_string());
    }
    path.iter()
        .map(|dir| dir.join(command))
        .find(|candidate| is_executable(candidate))
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn login_args(command: &str) -> Vec<String> {
    match executable_name(command).to_ascii_lowercase().as_str() {
        "bash" | "zsh" => vec!["-l".to_string()],
        "fish" => vec!["--login".to_string()],
        "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe" => {
            vec!["-NoLogo".to_string()]
        }
        _ => Vec::new(),
    }
}

fn known_id_for_command(command: &str) -> Option<&'static str> {
    let name = executable_name(command).to_ascii_lowercase();
    match name.as_str() {
        "bash" => Some("shell-bash"),
        "zsh" => Some("shell-zsh"),
        "fish" => Some("shell-fish"),
        "pwsh" => Some("shell-pwsh"),
        "powershell.exe" => Some("shell-windows-powershell"),
        "pwsh.exe" => Some("shell-windows-pwsh"),
        _ => None,
    }
}

fn known_id_for_name(name: &str) -> &'static str {
    known_id_for_command(name).unwrap_or("shell-system")
}

fn executable_name(command: &str) -> &str {
    Path::new(command)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(command)
}

fn display_name(command: &str) -> String {
    match executable_name(command).to_ascii_lowercase().as_str() {
        "bash" => "Bash".to_string(),
        "zsh" => "Zsh".to_string(),
        "fish" => "Fish".to_string(),
        "pwsh" => "PowerShell".to_string(),
        "powershell.exe" => "Windows PowerShell".to_string(),
        "pwsh.exe" => "PowerShell (Windows)".to_string(),
        other => other.to_string(),
    }
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn is_wsl() -> bool {
    if env::var_os("WSL_DISTRO_NAME").is_some() || env::var_os("WSL_INTEROP").is_some() {
        return true;
    }
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .or_else(|_| std::fs::read_to_string("/proc/version"))
        .map(|value| value.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn executable(dir: &Path, name: &str) -> String {
        let path = dir.join(name);
        fs::write(&path, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn detects_supported_shells_and_wsl_interop_in_order() {
        let temp = TempDir::new().unwrap();
        let bash = executable(temp.path(), "bash");
        executable(temp.path(), "zsh");
        executable(temp.path(), "fish");
        executable(temp.path(), "pwsh");
        executable(temp.path(), "powershell.exe");
        executable(temp.path(), "pwsh.exe");
        let result = detect_shell_profiles(&DetectionEnvironment {
            path: vec![temp.path().to_path_buf()],
            shell: Some(bash),
            is_wsl: true,
        });
        assert_eq!(result.system_default_id.as_deref(), Some("shell-bash"));
        assert_eq!(
            result
                .profiles
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "shell-bash",
                "shell-zsh",
                "shell-fish",
                "shell-pwsh",
                "shell-windows-powershell",
                "shell-windows-pwsh",
            ]
        );
        assert_eq!(result.profiles[0].args, vec!["-l"]);
        assert_eq!(result.profiles[2].args, vec!["--login"]);
        assert_eq!(result.profiles[4].args, vec!["-NoLogo"]);
        assert!(result.profiles[4].experimental);
    }

    #[test]
    fn reconciliation_keeps_disabled_detected_and_custom_profiles() {
        let temp = TempDir::new().unwrap();
        let bash = executable(temp.path(), "bash");
        executable(temp.path(), "zsh");
        let detection = detect_shell_profiles(&DetectionEnvironment {
            path: vec![temp.path().to_path_buf()],
            shell: Some(bash.clone()),
            is_wsl: false,
        });
        let mut settings = ShellSettings {
            default_profile_id: "missing".into(),
            profiles: vec![
                StoredShellProfile {
                    id: "shell-bash".into(),
                    name: "My Bash".into(),
                    command: bash,
                    args: vec!["--noprofile".into()],
                    enabled: false,
                    target: ShellTarget::Local,
                    source: ShellProfileSource::Detected,
                },
                StoredShellProfile {
                    id: "custom-test".into(),
                    name: "Custom".into(),
                    command: "/missing".into(),
                    args: vec!["literal $HOME".into()],
                    enabled: true,
                    target: ShellTarget::Local,
                    source: ShellProfileSource::Custom,
                },
            ],
        };
        let state = reconcile_shell_settings(&mut settings, &detection);
        assert!(!state.profiles[0].enabled);
        assert_eq!(state.profiles[0].name, "My Bash");
        assert_eq!(state.profiles[1].args, vec!["literal $HOME"]);
        assert!(!state.profiles[1].available);
    }

    #[test]
    fn update_rejects_duplicate_ids() {
        let profile = ShellProfile {
            id: "custom-a".into(),
            name: "A".into(),
            command: "/bin/sh".into(),
            args: vec![],
            enabled: true,
            target: ShellTarget::Local,
            source: ShellProfileSource::Custom,
            available: true,
            reason: None,
            experimental: false,
            detected_defaults: None,
        };
        let mut settings = ShellSettings::default();
        let err = apply_shell_profiles_update(
            &mut settings,
            ShellProfilesState {
                default_profile_id: "custom-a".into(),
                profiles: vec![profile.clone(), profile],
            },
            &DetectionResult::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ShellProfilesError::DuplicateId(_)));
    }

    #[test]
    fn runtime_resolution_prioritizes_profile_then_legacy_then_default() {
        let default = ShellProfile {
            id: "default".into(),
            name: "Default".into(),
            command: "/bin/sh".into(),
            args: vec!["literal $HOME".into(), "two words".into()],
            enabled: true,
            target: ShellTarget::Local,
            source: ShellProfileSource::Custom,
            available: true,
            reason: None,
            experimental: false,
            detected_defaults: None,
        };
        let alternate = ShellProfile {
            id: "alternate".into(),
            name: "Alternate".into(),
            command: "/bin/echo".into(),
            args: vec!["$(not-interpolated)".into()],
            enabled: true,
            target: ShellTarget::Local,
            source: ShellProfileSource::Custom,
            available: true,
            reason: None,
            experimental: false,
            detected_defaults: None,
        };
        let state = ShellProfilesState {
            default_profile_id: "default".into(),
            profiles: vec![default, alternate],
        };

        let selected = state
            .resolve(Some("alternate"), Some("/legacy/bash"))
            .unwrap();
        assert_eq!(selected.command, "/bin/echo");
        assert_eq!(selected.args, vec!["$(not-interpolated)"]);

        let legacy = state.resolve(None, Some("/bin/bash")).unwrap();
        assert_eq!(legacy.command, "/bin/bash");
        assert_eq!(legacy.args, vec!["-l"]);
        assert_eq!(legacy.id, None);

        let fallback = state.resolve(None, None).unwrap();
        assert_eq!(fallback.id.as_deref(), Some("default"));
        assert_eq!(fallback.args, vec!["literal $HOME", "two words"]);
    }

    #[test]
    fn disabling_default_selects_first_active_profile_in_saved_order() {
        let temp = TempDir::new().unwrap();
        let bash = executable(temp.path(), "bash");
        executable(temp.path(), "zsh");
        let detection = detect_shell_profiles(&DetectionEnvironment {
            path: vec![temp.path().to_path_buf()],
            shell: Some(bash),
            is_wsl: false,
        });
        let mut settings = ShellSettings::default();
        let state = apply_shell_profiles_update(
            &mut settings,
            ShellProfilesState {
                default_profile_id: "shell-bash".into(),
                profiles: vec![
                    detection.profiles[1].clone(),
                    ShellProfile {
                        enabled: false,
                        ..detection.profiles[0].clone()
                    },
                ],
            },
            &detection,
        )
        .unwrap();
        assert_eq!(state.default_profile_id, "shell-zsh");
        assert_eq!(state.profiles[0].id, "shell-zsh");
    }
}
