//! Settings structs, defaults, validation, and TOML persistence.
//!
//! Defines `HomeConfig` (the main user-facing configuration) along with all
//! its settings groups. Includes TOML <-> JSON key transformation
//! (snake_case on disk, camelCase on the wire) and deep-merge for partial updates.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::config_path;
use crate::shell_profiles::{
    detect_shell_profiles, reconcile_shell_settings, DetectionEnvironment, DetectionResult,
    ShellSettings,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("TOML serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid setting {field}: {message}")]
    InvalidField { field: String, message: String },
}

// ---------------------------------------------------------------------------
// Key transformation helpers
// ---------------------------------------------------------------------------

fn snake_to_camel(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = false;
    for ch in s.chars() {
        if ch == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(ch.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }
    result
}

fn camel_to_snake(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(ch.to_ascii_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}

fn transform_keys(value: Value, f: &dyn Fn(&str) -> String) -> Value {
    match value {
        Value::Object(map) => {
            let new_map: Map<String, Value> = map
                .into_iter()
                .map(|(k, v)| (f(&k), transform_keys(v, f)))
                .collect();
            Value::Object(new_map)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(|v| transform_keys(v, f)).collect()),
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Deep merge
// ---------------------------------------------------------------------------

pub fn deep_merge(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (k, v) in patch_map {
                let entry = base_map.entry(k).or_insert(Value::Null);
                deep_merge(entry, v);
            }
        }
        (base, patch) => {
            *base = patch;
        }
    }
}

// ---------------------------------------------------------------------------
// Struct definitions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HomeConfig {
    #[serde(default)]
    pub editors: EditorSettings,
    pub terminal: TerminalSettings,
    pub appearance: AppearanceSettings,
    pub drawer: DrawerSettings,
    pub notifications: NotificationSettings,
    #[serde(default)]
    pub remote: RemoteSettings,
    #[serde(default)]
    pub native_tabs: NativeTabSettings,
    #[serde(default)]
    pub shells: ShellSettings,
}

impl Default for HomeConfig {
    fn default() -> Self {
        Self::with_detection(&detect_shell_profiles(&DetectionEnvironment::current()))
    }
}

impl HomeConfig {
    /// The defaults, with the shells `detection` found. Detection searches
    /// `PATH` and is slow where it holds network or WSL drive directories, so
    /// one operation detects once and passes the result on.
    pub fn with_detection(detection: &DetectionResult) -> Self {
        let mut shells = ShellSettings::default();
        reconcile_shell_settings(&mut shells, detection);
        Self {
            editors: EditorSettings::default(),
            terminal: TerminalSettings::default(),
            appearance: AppearanceSettings::default(),
            drawer: DrawerSettings::default(),
            notifications: NotificationSettings::default(),
            remote: RemoteSettings::default(),
            native_tabs: NativeTabSettings::default(),
            shells,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSettings {
    /// The terminal palette: a name, or `default` to use `theme_dark` and
    /// `theme_light` by the UI theme's mode.
    pub theme: String,
    /// The palette while the UI theme is dark and `theme` is `default`: a
    /// name, or `default` to follow the UI theme.
    #[serde(default = "default_terminal_theme")]
    pub theme_dark: String,
    /// The same for a light UI theme.
    #[serde(default = "default_terminal_theme")]
    pub theme_light: String,
    pub close_on_exit: String,
    pub scroll_to_bottom: Option<bool>,
    pub allow_osc52_write: bool,
    /// Which clients decide the PTY size of a session several clients show:
    /// `active`, `latest` or `smallest`. Applies to this server's sessions.
    pub size_policy: String,
    /// How long after its user's last action a client still counts as active.
    pub size_idle_seconds: u32,
    /// The family terminals draw text in; empty for the default (the UI
    /// theme's proposal, else the bundled font). Absent in older settings.
    #[serde(default)]
    pub font_family: String,
    /// The terminal text size in pixels before zoom, within
    /// [`TERMINAL_FONT_SIZES`]. Absent in older settings, which means the
    /// size terminals had before the setting existed.
    #[serde(default = "default_terminal_font_size")]
    pub font_size: u32,
}

/// The terminal text size of settings that name none.
pub const DEFAULT_TERMINAL_FONT_SIZE: u32 = 14;
/// The terminal text sizes an update may set, listed in the metadata.
pub const TERMINAL_FONT_SIZES: std::ops::RangeInclusive<u32> = 8..=32;
/// The longest font family a setting holds.
const MAX_FONT_FAMILY_BYTES: usize = 4096;

fn default_terminal_font_size() -> u32 {
    DEFAULT_TERMINAL_FONT_SIZE
}

/// A family name a setting can hold: bounded, without control characters.
fn valid_font_family(family: &str) -> bool {
    family.len() <= MAX_FONT_FAMILY_BYTES && !family.chars().any(char::is_control)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceSettings {
    /// Native client title-bar colors, independent of decoration geometry.
    #[serde(default = "default_interface_style")]
    pub interface_style: String,
    #[serde(default = "default_window_decoration")]
    pub window_decoration: String,
    #[serde(default = "default_window_buttons")]
    pub window_buttons: String,
    #[serde(default = "default_title_bar_style")]
    pub title_bar_style: String,
    pub theme: String,
    pub theme_mode: String,
    pub theme_dark: String,
    pub theme_light: String,
    pub border_radius: i32,
    pub font_size: i32,
    pub font_family: String,
    /// The zoom level of this computer's windows, within [`ZOOM_LEVELS`];
    /// an update sets one of them.
    pub zoom: f64,
    /// What zooms: `window` (the whole window) or `terminals` (their font size).
    pub zoom_scope: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DrawerSettings {
    #[serde(default = "default_drawer_pinned")]
    pub pinned: bool,
    pub edge_reveal: bool,
    pub show_tips: bool,
    pub sidebar_width: i32,
}

fn default_drawer_pinned() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSettings {
    pub enabled: bool,
    pub toasts: bool,
    pub desktop: bool,
    pub process_finished: bool,
    pub process_exited: bool,
    pub process_crashed: bool,
    /// Notifications for program status reports (OSC 7501). On when stored
    /// settings or an older server's reply do not have it.
    #[serde(default = "default_program_status")]
    pub program_status: bool,
}

fn default_program_status() -> bool {
    true
}

/// Local editor selection only; these preferences never authorize a launch.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EditorSettings {
    pub file: String,
    pub folder: String,
    pub extensions: Vec<EditorExtension>,
    pub projects: Vec<EditorProject>,
}
impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            file: "vscode".into(),
            folder: "vscode".into(),
            extensions: vec![],
            projects: vec![],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EditorExtension {
    pub extension: String,
    pub editor: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EditorProject {
    pub connection: String,
    pub path: String,
    pub editor: String,
}

/// The desktop app's tab bars show every group action and a close button
/// on every tab by default.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct NativeTabSettings {
    pub show_split: bool,
    pub show_float: bool,
    pub show_maximize: bool,
    /// `always`, or `active` for the selected tab of a group only.
    pub close_button: String,
    /// Tabs stay where they are: no drag and drop.
    pub locked: bool,
}
impl Default for NativeTabSettings {
    fn default() -> Self {
        Self {
            show_split: true,
            show_float: true,
            show_maximize: true,
            close_button: "always".into(),
            locked: false,
        }
    }
}

/// Screens of other hosts (`remote-screens`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSettings {
    /// Forward a port that starts listening on a connected host without
    /// asking. Off by default: a forward opens a port on this machine that
    /// any local process can reach.
    #[serde(default)]
    pub auto_forward_ports: bool,
    /// The hosts, by SSH target, that the native app connects at start when
    /// their screens were open as it was closed. Empty by default: connecting
    /// can ask for a password before anything else was done.
    #[serde(default)]
    pub reconnect_hosts: Vec<String>,
}

// ---------------------------------------------------------------------------
// Default implementations
// ---------------------------------------------------------------------------

fn default_terminal_theme() -> String {
    "default".into()
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            theme: "default".into(),
            theme_dark: default_terminal_theme(),
            theme_light: default_terminal_theme(),
            close_on_exit: "always".into(),
            scroll_to_bottom: Some(true),
            allow_osc52_write: true,
            size_policy: "active".into(),
            size_idle_seconds: 120,
            font_family: String::new(),
            font_size: DEFAULT_TERMINAL_FONT_SIZE,
        }
    }
}

fn default_interface_style() -> String {
    "theme".into()
}
fn default_window_decoration() -> String {
    "system".into()
}
fn default_window_buttons() -> String {
    "all".into()
}

fn default_title_bar_style() -> String {
    "theme".into()
}

/// The UI themes of settings that name none: the dark one also for a fixed
/// theme, the light one for a light system preference.
pub const DEFAULT_THEME_DARK: &str = "splitty-dark";
pub const DEFAULT_THEME_LIGHT: &str = "splitty-light";

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            interface_style: default_interface_style(),
            window_decoration: default_window_decoration(),
            window_buttons: default_window_buttons(),
            title_bar_style: default_title_bar_style(),
            theme: DEFAULT_THEME_DARK.into(),
            theme_mode: "fixed".into(),
            theme_dark: DEFAULT_THEME_DARK.into(),
            theme_light: DEFAULT_THEME_LIGHT.into(),
            border_radius: 1,
            font_size: 0,
            font_family: String::new(),
            zoom: 1.0,
            zoom_scope: "window".into(),
        }
    }
}

impl Default for DrawerSettings {
    fn default() -> Self {
        Self {
            pinned: default_drawer_pinned(),
            edge_reveal: true,
            show_tips: true,
            sidebar_width: 200,
        }
    }
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            toasts: false,
            desktop: false,
            process_finished: true,
            process_exited: false,
            process_crashed: true,
            program_status: default_program_status(),
        }
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

impl HomeConfig {
    pub fn validate(&mut self) {
        self.validate_with(&detect_shell_profiles(&DetectionEnvironment::current()));
    }

    /// [`Self::validate`] with the shells `detection` found.
    pub fn validate_with(&mut self, detection: &DetectionResult) {
        // Terminal settings
        for theme in [
            &mut self.terminal.theme,
            &mut self.terminal.theme_dark,
            &mut self.terminal.theme_light,
        ] {
            if theme.is_empty() {
                *theme = "default".into();
            }
        }
        if !matches!(
            self.terminal.close_on_exit.as_str(),
            "always" | "never" | "graceful"
        ) {
            self.terminal.close_on_exit = "always".into();
        }
        if !matches!(
            self.terminal.size_policy.as_str(),
            "active" | "latest" | "smallest"
        ) {
            self.terminal.size_policy = "active".into();
        }
        if !(10..=3600).contains(&self.terminal.size_idle_seconds) {
            self.terminal.size_idle_seconds = 120;
        }
        if !TERMINAL_FONT_SIZES.contains(&self.terminal.font_size) {
            self.terminal.font_size = DEFAULT_TERMINAL_FONT_SIZE;
        }
        if !valid_font_family(&self.terminal.font_family) {
            self.terminal.font_family.clear();
        }

        if !matches!(self.native_tabs.close_button.as_str(), "always" | "active") {
            self.native_tabs.close_button = "always".into();
        }

        // Appearance settings
        if !matches!(self.appearance.theme_mode.as_str(), "fixed" | "auto") {
            self.appearance.theme_mode = "fixed".into();
        }
        if self.appearance.theme_dark.is_empty() {
            self.appearance.theme_dark = DEFAULT_THEME_DARK.into();
        }
        if self.appearance.theme_light.is_empty() {
            self.appearance.theme_light = DEFAULT_THEME_LIGHT.into();
        }
        if !(0..=20).contains(&self.appearance.border_radius) {
            self.appearance.border_radius = 1;
        }
        if self.appearance.font_size != 0 && !(8..=24).contains(&self.appearance.font_size) {
            self.appearance.font_size = 0;
        }
        if !(ZOOM_LEVELS[0]..=ZOOM_LEVELS[ZOOM_LEVELS.len() - 1]).contains(&self.appearance.zoom) {
            self.appearance.zoom = 1.0;
        }
        if !matches!(self.appearance.zoom_scope.as_str(), "window" | "terminals") {
            self.appearance.zoom_scope = "window".into();
        }

        // Drawer settings
        if !(150..=400).contains(&self.drawer.sidebar_width) {
            self.drawer.sidebar_width = 200;
        }

        // Remote settings: a bounded list of distinct plain targets.
        let mut hosts = Vec::new();
        for host in std::mem::take(&mut self.remote.reconnect_hosts) {
            if valid_reconnect_host(&host)
                && !hosts.contains(&host)
                && hosts.len() < MAX_RECONNECT_HOSTS
            {
                hosts.push(host);
            }
        }
        self.remote.reconnect_hosts = hosts;

        reconcile_shell_settings(&mut self.shells, detection);
    }
}

/// The zoom levels a window steps through and offers, in order; the
/// metadata lists them as the values of `appearance.zoom`, and an update
/// takes no other level.
pub const ZOOM_LEVELS: [f64; 13] = [
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

/// Server-owned control metadata; paths use the JSON wire spelling.
pub fn settings_metadata() -> Value {
    serde_json::json!({ "defaults": HomeConfig::default(), "limits": settings_limits() })
}

/// The `limits` of [`settings_metadata`].
fn settings_limits() -> Value {
    serde_json::json!({
            "terminal.closeOnExit": {"values": ["always", "never", "graceful"]},
            "terminal.sizePolicy": {"values": ["active", "latest", "smallest"]},
            "terminal.sizeIdleSeconds": {"min": 10, "max": 3600, "step": 10},
            "terminal.fontSize": {
                "min": TERMINAL_FONT_SIZES.start(),
                "max": TERMINAL_FONT_SIZES.end(),
                "step": 1,
                "values": TERMINAL_FONT_SIZES.collect::<Vec<_>>()
            },
            "appearance.themeMode": {"values": ["fixed", "auto"]},
            "appearance.interfaceStyle": {"values": ["system", "theme"]},
            "appearance.windowDecoration": {"values": ["system", "splitty"]},
            "appearance.windowButtons": {"values": ["system", "all"]},
            "appearance.titleBarStyle": {"values": ["system", "theme"]},
            "appearance.borderRadius": {"min": 0, "max": 20, "step": 1},
            "appearance.fontSize": {"min": 0, "max": 24, "step": 1, "values": [0, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24]},
            "appearance.zoom": {"min": ZOOM_LEVELS[0], "max": ZOOM_LEVELS[ZOOM_LEVELS.len() - 1], "values": ZOOM_LEVELS},
            "appearance.zoomScope": {"values": ["window", "terminals"]},
            "nativeTabs.closeButton": {"values": ["always", "active"]},
            "drawer.sidebarWidth": {"min": 150, "max": 400, "step": 10},
            "editors.file": {"values": ["vscode", "zed"]},
            "editors.folder": {"values": ["vscode", "zed"]}
    })
}

fn invalid(field: &str, message: &str) -> ConfigError {
    ConfigError::InvalidField {
        field: field.to_owned(),
        message: message.to_owned(),
    }
}

/// Checks `patch` against the fields of `defaults` and the metadata limits.
fn validate_patch(patch: &Value, defaults: &HomeConfig) -> Result<(), ConfigError> {
    let defaults = serde_json::to_value(defaults)?;
    let limits = settings_limits();
    fn walk(
        value: &Value,
        template: &Value,
        limits: &Value,
        path: &str,
    ) -> Result<(), ConfigError> {
        if let Some(object) = value.as_object() {
            let Some(expected) = template.as_object() else {
                return Err(invalid(path, "expected a value"));
            };
            for (key, child) in object {
                let field = if path.is_empty() {
                    key.to_owned()
                } else {
                    format!("{path}.{key}")
                };
                let Some(expected_child) = expected.get(key) else {
                    return Err(invalid(&field, "unknown setting"));
                };
                if field == "editors.extensions" {
                    validate_editor_extensions(child, &field)?;
                } else if field == "editors.projects" {
                    validate_editor_projects(child, &field)?;
                } else if field == "remote.reconnectHosts" {
                    validate_reconnect_hosts(child, &field)?;
                } else if field == "terminal.fontFamily" {
                    if !child.as_str().is_some_and(valid_font_family) {
                        return Err(invalid(&field, "expected a font family"));
                    }
                } else if field == "shells" { /* Shell profiles have their own update validation. */
                } else {
                    walk(child, expected_child, limits, &field)?;
                }
            }
            return Ok(());
        }
        if let Some(limit) = limits.get(path) {
            if let Some(values) = limit.get("values").and_then(Value::as_array) {
                // Numbers compare by value: `1` is the listed level `1.0`.
                let listed = values.iter().any(|allowed| {
                    allowed == value
                        || matches!(
                            (allowed.as_f64(), value.as_f64()),
                            (Some(allowed), Some(value)) if allowed == value
                        )
                });
                if !listed {
                    return Err(invalid(path, "unsupported value"));
                }
            }
            if let (Some(min), Some(max)) = (
                limit.get("min").and_then(Value::as_f64),
                limit.get("max").and_then(Value::as_f64),
            ) {
                let Some(n) = value.as_f64() else {
                    return Err(invalid(path, "expected a number"));
                };
                if !n.is_finite() || n < min || n > max {
                    return Err(invalid(path, "outside allowed range"));
                }
            }
        }
        let same_type = match template {
            Value::Bool(_) => value.is_boolean(),
            Value::String(_) => value.is_string(),
            Value::Number(n) if n.is_i64() || n.is_u64() => {
                value.as_i64().is_some() || value.as_u64().is_some()
            }
            Value::Number(_) => value.is_number(),
            Value::Null => value.is_null() || value.is_boolean(),
            Value::Array(_) => value.is_array(),
            _ => false,
        };
        if !same_type {
            return Err(invalid(path, "wrong value type"));
        }
        Ok(())
    }
    walk(patch, &defaults, &limits, "")
}

fn valid_editor(value: &Value) -> bool {
    matches!(value.as_str(), Some("vscode" | "zed"))
}

/// How many hosts can be reconnected at start.
const MAX_RECONNECT_HOSTS: usize = 64;

/// A target as the host list spells it; whether it is an SSH target is
/// decided where it is connected, not here.
fn valid_reconnect_host(host: &str) -> bool {
    !host.is_empty()
        && host.chars().count() <= 256
        && !host.starts_with('-')
        && !host.chars().any(|c| c.is_control() || c.is_whitespace())
}

fn validate_reconnect_hosts(value: &Value, path: &str) -> Result<(), ConfigError> {
    let Some(hosts) = value.as_array() else {
        return Err(invalid(path, "expected an array"));
    };
    if hosts.len() > MAX_RECONNECT_HOSTS {
        return Err(invalid(path, "too many hosts"));
    }
    for (index, host) in hosts.iter().enumerate() {
        if !host.as_str().is_some_and(valid_reconnect_host) || hosts[..index].contains(host) {
            return Err(invalid(
                &format!("{path}.{index}"),
                "expected a distinct host",
            ));
        }
    }
    Ok(())
}

fn validate_editor_extensions(value: &Value, path: &str) -> Result<(), ConfigError> {
    let Some(rules) = value.as_array() else {
        return Err(invalid(path, "expected an array"));
    };
    if rules.len() > 100 {
        return Err(invalid(path, "too many rules"));
    }
    for (i, rule) in rules.iter().enumerate() {
        let field = format!("{path}.{i}");
        let Some(ext) = rule.get("extension").and_then(Value::as_str) else {
            return Err(invalid(
                &format!("{field}.extension"),
                "expected an extension",
            ));
        };
        let normalized = ext.strip_prefix('.').unwrap_or(ext);
        if normalized.is_empty()
            || normalized.len() > 32
            || !normalized
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(invalid(&format!("{field}.extension"), "invalid extension"));
        }
        if !valid_editor(&rule["editor"]) {
            return Err(invalid(&format!("{field}.editor"), "unsupported editor"));
        }
    }
    Ok(())
}

fn validate_editor_projects(value: &Value, path: &str) -> Result<(), ConfigError> {
    let Some(rules) = value.as_array() else {
        return Err(invalid(path, "expected an array"));
    };
    if rules.len() > 100 {
        return Err(invalid(path, "too many rules"));
    }
    for (i, rule) in rules.iter().enumerate() {
        let field = format!("{path}.{i}");
        let Some(connection) = rule.get("connection").and_then(Value::as_str) else {
            return Err(invalid(
                &format!("{field}.connection"),
                "expected a connection",
            ));
        };
        if connection != "local"
            && !(connection.starts_with("host:") && connection.len() > 5 && connection.len() <= 260)
        {
            return Err(invalid(
                &format!("{field}.connection"),
                "invalid connection",
            ));
        }
        let Some(project_path) = rule.get("path").and_then(Value::as_str) else {
            return Err(invalid(&format!("{field}.path"), "expected a path"));
        };
        if !project_path.starts_with('/')
            || project_path.len() > 4096
            || project_path
                .split('/')
                .any(|part| matches!(part, "." | ".."))
        {
            return Err(invalid(&format!("{field}.path"), "invalid path"));
        }
        if !valid_editor(&rule["editor"]) {
            return Err(invalid(&format!("{field}.editor"), "unsupported editor"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Load / Save / Update
// ---------------------------------------------------------------------------

/// Loads config from the default path. Returns defaults if file is missing.
pub fn load() -> HomeConfig {
    let path = config_path();
    let detection = detect_shell_profiles(&DetectionEnvironment::current());
    let should_persist = match fs::read_to_string(&path) {
        Ok(data) => parse_toml_to_config(&data, &detection).is_ok(),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    };
    let cfg = load_with(&path, &detection);
    // Persist migrations (including the first detected shell-profile section)
    // without making startup depend on a writable config directory. A file
    // that failed to parse is deliberately left untouched for recovery.
    if should_persist {
        let _ = save_to(&cfg, &path);
    }
    cfg
}

/// Loads config from a specific path. Returns defaults if file is missing.
pub fn load_from(path: &Path) -> HomeConfig {
    load_with(
        path,
        &detect_shell_profiles(&DetectionEnvironment::current()),
    )
}

/// [`load_from`] with the shells `detection` found.
fn load_with(path: &Path, detection: &DetectionResult) -> HomeConfig {
    let mut cfg = fs::read_to_string(path)
        .ok()
        .and_then(|data| parse_toml_to_config(&data, detection).ok())
        .unwrap_or_else(|| HomeConfig::with_detection(detection));
    cfg.validate_with(detection);
    cfg
}

/// Parses a TOML string into `HomeConfig` using key transformation.
///
/// TOML uses snake_case keys, but our structs expect camelCase (via `rename_all`).
/// We parse TOML into a generic `toml::Value`, convert to `serde_json::Value`,
/// transform keys from snake_case to camelCase, then deserialize into our struct.
fn parse_toml_to_config(
    toml_str: &str,
    detection: &DetectionResult,
) -> Result<HomeConfig, ConfigError> {
    let toml_val: toml::Value = toml::from_str(toml_str)?;
    let json_val = toml_value_to_json(toml_val);
    let camel_val = transform_keys(json_val, &snake_to_camel);

    // Merge with defaults so missing fields get filled in.
    let mut base = serde_json::to_value(HomeConfig::with_detection(detection))?;
    deep_merge(&mut base, camel_val);

    let cfg: HomeConfig = serde_json::from_value(base)?;
    Ok(cfg)
}

/// Converts a `toml::Value` to `serde_json::Value`.
fn toml_value_to_json(val: toml::Value) -> Value {
    match val {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::Number(serde_json::Number::from(i)),
        toml::Value::Float(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
        toml::Value::Array(arr) => Value::Array(arr.into_iter().map(toml_value_to_json).collect()),
        toml::Value::Table(tbl) => {
            let map: Map<String, Value> = tbl
                .into_iter()
                .map(|(k, v)| (k, toml_value_to_json(v)))
                .collect();
            Value::Object(map)
        }
    }
}

/// Converts `HomeConfig` to a TOML string with snake_case keys.
fn config_to_toml_string(config: &HomeConfig) -> Result<String, ConfigError> {
    let json_val = serde_json::to_value(config)?;
    let snake_val = transform_keys(json_val, &camel_to_snake);
    let toml_val: toml::Value = serde_json::from_value(snake_val)?;
    let s = toml::to_string_pretty(&toml_val)?;
    Ok(s)
}

/// Saves config to the default path atomically.
pub fn save(config: &HomeConfig) -> Result<(), ConfigError> {
    save_to(config, &config_path())
}

/// Saves config to a specific path atomically.
pub fn save_to(config: &HomeConfig, path: &Path) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        super::ensure_private_dir(parent)?;
    }

    let toml_str = config_to_toml_string(config)?;

    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, &toml_str)?;
    fs::rename(&tmp, path)?;

    Ok(())
}

/// Loads the config, applies a partial JSON update (deep merge), validates, saves, and returns it.
pub fn update(partial: Value) -> Result<HomeConfig, ConfigError> {
    update_at(&config_path(), partial)
}

/// Same as `update` but uses a specific file path (for testing).
pub fn update_at(path: &Path, partial: Value) -> Result<HomeConfig, ConfigError> {
    update_at_with(
        path,
        partial,
        &detect_shell_profiles(&DetectionEnvironment::current()),
    )
}

/// [`update_at`] with the shells `detection` found: the stored settings, the
/// defaults the patch is checked against and the result all use it.
pub fn update_at_with(
    path: &Path,
    partial: Value,
    detection: &DetectionResult,
) -> Result<HomeConfig, ConfigError> {
    let cfg = load_with(path, detection);
    validate_patch(&partial, &HomeConfig::with_detection(detection))?;
    let mut base = serde_json::to_value(&cfg)?;
    deep_merge(&mut base, partial);
    let mut cfg: HomeConfig = serde_json::from_value(base)?;
    cfg.validate_with(detection);
    save_to(&cfg, path)?;
    Ok(cfg)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn the_terminal_palette_pair_defaults_persists_and_is_absent_in_older_files() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        // A file written before the pair existed.
        fs::write(&path, "[terminal]\ntheme = 'default'\n").unwrap();
        let cfg = load_from(&path);
        assert_eq!(cfg.terminal.theme_dark, "default");
        assert_eq!(cfg.terminal.theme_light, "default");
        let defaults = &settings_metadata()["defaults"]["terminal"];
        assert_eq!(defaults["themeDark"], "default");
        assert_eq!(defaults["themeLight"], "default");

        let updated = update_at(
            &path,
            serde_json::json!({"terminal": {"themeDark": "catppuccin-mocha", "themeLight": ""}}),
        )
        .unwrap();
        assert_eq!(updated.terminal.theme_dark, "catppuccin-mocha");
        // An empty name means the default.
        assert_eq!(updated.terminal.theme_light, "default");
        assert_eq!(load_from(&path).terminal.theme_dark, "catppuccin-mocha");
        assert_eq!(updated.terminal.theme, "default");
    }

    /// An update takes the stored settings, the defaults its patch is checked
    /// against and its result from the one detection it was given: shells
    /// of the process's own `PATH` appear nowhere.
    #[test]
    fn an_update_uses_the_shell_detection_it_was_given() {
        let dir = TempDir::new().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let zsh = bin.join("zsh");
        fs::write(&zsh, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&zsh, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let detection = detect_shell_profiles(&DetectionEnvironment {
            path: vec![bin],
            shell: None,
            is_wsl: false,
        });
        let zsh = zsh.to_string_lossy().into_owned();
        let profiles = |cfg: &HomeConfig| {
            cfg.shells
                .profiles
                .iter()
                .map(|profile| (profile.id.clone(), profile.command.clone()))
                .collect::<Vec<_>>()
        };
        let expected = vec![("shell-zsh".to_owned(), zsh)];
        let path = dir.path().join("config.toml");
        // Without a stored file, then over the file the first update wrote.
        for size in [16, 18] {
            let updated = update_at_with(
                &path,
                serde_json::json!({"terminal": {"fontSize": size}}),
                &detection,
            )
            .unwrap();
            assert_eq!(updated.terminal.font_size, size);
            assert_eq!(profiles(&updated), expected);
        }
        assert_eq!(profiles(&load_with(&path, &detection)), expected);
        let refused = update_at_with(
            &path,
            serde_json::json!({"terminal": {"fontSize": 99}}),
            &detection,
        );
        assert!(matches!(refused, Err(ConfigError::InvalidField { .. })));
    }

    /// settings, Terminal Font Settings.
    #[test]
    fn terminal_font_settings_default_persist_and_are_validated() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        // Settings stored before the font settings existed keep today's look.
        fs::write(&path, "[terminal]\ntheme = 'default'\n").unwrap();
        let older = load_from(&path);
        assert_eq!(older.terminal.font_family, "");
        assert_eq!(older.terminal.font_size, 14);
        let json = serde_json::to_value(&older).unwrap();
        assert_eq!(json["terminal"]["fontFamily"], "");
        assert_eq!(json["terminal"]["fontSize"], 14);
        let metadata = settings_metadata();
        assert_eq!(metadata["defaults"]["terminal"]["fontFamily"], "");
        assert_eq!(metadata["defaults"]["terminal"]["fontSize"], 14);
        let limit = &metadata["limits"]["terminal.fontSize"];
        assert_eq!(
            (limit["min"].clone(), limit["max"].clone()),
            (8.into(), 32.into())
        );
        assert_eq!(
            limit["values"],
            serde_json::json!((8..=32).collect::<Vec<u32>>())
        );

        // Both persist, snake_case on disk, and reach the effective settings.
        let updated = update_at(
            &path,
            serde_json::json!({"terminal": {"fontFamily": "Fira Code", "fontSize": 16}}),
        )
        .unwrap();
        assert_eq!(
            (
                updated.terminal.font_family.as_str(),
                updated.terminal.font_size
            ),
            ("Fira Code", 16)
        );
        let saved: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["terminal"]["font_family"].as_str(), Some("Fira Code"));
        assert_eq!(saved["terminal"]["font_size"].as_integer(), Some(16));
        let reloaded = load_from(&path);
        assert_eq!(reloaded.terminal.font_family, "Fira Code");
        assert_eq!(reloaded.terminal.font_size, 16);
        // The interface font is a setting of its own.
        assert_eq!(reloaded.appearance.font_family, "");
        assert_eq!(reloaded.appearance.font_size, 0);

        // A size the metadata does not list, or an unusable family, is refused
        // and nothing changes.
        let saved = fs::read(&path).unwrap();
        for (patch, field) in [
            (serde_json::json!({"fontSize": 7}), "terminal.fontSize"),
            (serde_json::json!({"fontSize": 33}), "terminal.fontSize"),
            (serde_json::json!({"fontSize": 14.5}), "terminal.fontSize"),
            (serde_json::json!({"fontSize": "16"}), "terminal.fontSize"),
            (
                serde_json::json!({"fontFamily": "Fira\nCode"}),
                "terminal.fontFamily",
            ),
            (
                serde_json::json!({"fontFamily": "x".repeat(4097)}),
                "terminal.fontFamily",
            ),
            (serde_json::json!({"fontFamily": 3}), "terminal.fontFamily"),
        ] {
            let err = update_at(&path, serde_json::json!({ "terminal": patch })).unwrap_err();
            assert!(
                matches!(err, ConfigError::InvalidField { field: ref actual, .. } if actual == field),
                "{err}"
            );
            assert_eq!(fs::read(&path).unwrap(), saved);
        }
        // The default family is empty again.
        let cleared = update_at(&path, serde_json::json!({"terminal": {"fontFamily": ""}}));
        assert_eq!(cleared.unwrap().terminal.font_family, "");

        // A stored value outside the limits falls back to the default.
        fs::write(
            &path,
            "[terminal]\nfont_size = 99\nfont_family = \"Bad\\u0007Name\"\n",
        )
        .unwrap();
        let repaired = load_from(&path);
        assert_eq!(repaired.terminal.font_size, 14);
        assert_eq!(repaired.terminal.font_family, "");
    }

    #[test]
    fn test_defaults() {
        let cfg = HomeConfig::default();
        assert_eq!(cfg.terminal.theme, "default");
        assert_eq!(cfg.terminal.close_on_exit, "always");
        assert_eq!(cfg.terminal.scroll_to_bottom, Some(true));
        assert_eq!(cfg.appearance.theme, "splitty-dark");
        assert_eq!(cfg.appearance.border_radius, 1);
        assert!(cfg.drawer.pinned);
        assert!(cfg.drawer.edge_reveal);
        assert!(cfg.drawer.show_tips);
        assert!(cfg.notifications.enabled);
        assert!(!cfg.notifications.toasts);
        assert!(!cfg.notifications.desktop);
        assert!(cfg.notifications.process_finished);
        assert!(!cfg.notifications.process_exited);
        assert!(cfg.notifications.process_crashed);
        assert!(cfg.notifications.program_status);
        assert!(!cfg.remote.auto_forward_ports);
    }

    /// settings, Native Theme Catalog, First start and Settings from before
    /// the default changed: new settings hold Splitty Modern in a fixed theme
    /// mode; a stored theme stays.
    #[test]
    fn a_first_start_has_a_fixed_modern_theme_and_a_stored_theme_stays() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let first = load_from(&path).appearance;
        assert_eq!(
            (
                first.theme_mode.as_str(),
                first.theme.as_str(),
                first.theme_dark.as_str(),
                first.theme_light.as_str()
            ),
            ("fixed", "splitty-dark", "splitty-dark", "splitty-light")
        );
        assert_eq!(first, AppearanceSettings::default());

        fs::write(&path, "[appearance]\ntheme = \"splitty-classic\"\n").unwrap();
        let stored = load_from(&path).appearance;
        assert_eq!(stored.theme, "splitty-classic");
        assert_eq!(stored.theme_mode, "fixed");
    }

    #[test]
    fn test_zoom_settings() {
        let cfg = HomeConfig::default();
        assert!((cfg.appearance.zoom - 1.0).abs() < f64::EPSILON);
        assert_eq!(cfg.appearance.zoom_scope, "window");

        let mut cfg = HomeConfig::default();
        cfg.appearance.zoom = 1.25;
        cfg.appearance.zoom_scope = "terminals".into();
        cfg.validate();
        assert!((cfg.appearance.zoom - 1.25).abs() < f64::EPSILON);
        assert_eq!(cfg.appearance.zoom_scope, "terminals");

        cfg.appearance.zoom = 3.5;
        cfg.appearance.zoom_scope = "page".into();
        cfg.validate();
        assert!((cfg.appearance.zoom - 1.0).abs() < f64::EPSILON);
        assert_eq!(cfg.appearance.zoom_scope, "window");

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[appearance]\nzoom = 1.5\nzoom_scope = \"terminals\"\n",
        )
        .unwrap();
        let loaded = load_from(&path);
        let json = serde_json::to_value(&loaded).unwrap();
        assert_eq!(json["appearance"]["zoom"], 1.5);
        assert_eq!(json["appearance"]["zoomScope"], "terminals");

        // display-zoom, Levels from the server: the metadata lists the
        // levels with their bounds and no grid step.
        let limit = &settings_metadata()["limits"]["appearance.zoom"];
        assert_eq!(limit["values"], serde_json::json!(ZOOM_LEVELS));
        assert_eq!(limit["min"], 0.5);
        assert_eq!(limit["max"], 3.0);
        assert!(limit.get("step").is_none());

        // display-zoom, Level outside the list: refused, nothing stored. A
        // listed level is taken also as an integer.
        let refused = update_at(&path, serde_json::json!({"appearance": {"zoom": 1.3}}));
        assert!(matches!(
            refused,
            Err(ConfigError::InvalidField { ref field, .. }) if field == "appearance.zoom"
        ));
        assert_eq!(load_from(&path).appearance.zoom, 1.5);
        for (sent, stored) in [(serde_json::json!(0.67), 0.67), (serde_json::json!(1), 1.0)] {
            let updated = update_at(&path, serde_json::json!({"appearance": {"zoom": sent}}));
            assert_eq!(updated.unwrap().appearance.zoom, stored);
        }

        // display-zoom, Level between the listed ones: a stored level the
        // list does not hold still loads.
        fs::write(&path, "[appearance]\nzoom = 1.17\n").unwrap();
        assert_eq!(load_from(&path).appearance.zoom, 1.17);
    }

    #[test]
    fn test_size_policy_settings() {
        let cfg = HomeConfig::default();
        assert_eq!(cfg.terminal.size_policy, "active");
        assert_eq!(cfg.terminal.size_idle_seconds, 120);

        let mut cfg = HomeConfig::default();
        cfg.terminal.size_policy = "smallest".into();
        cfg.terminal.size_idle_seconds = 10;
        cfg.validate();
        assert_eq!(cfg.terminal.size_policy, "smallest");
        assert_eq!(cfg.terminal.size_idle_seconds, 10);

        cfg.terminal.size_policy = "largest".into();
        cfg.terminal.size_idle_seconds = 9;
        cfg.validate();
        assert_eq!(cfg.terminal.size_policy, "active");
        assert_eq!(cfg.terminal.size_idle_seconds, 120);
        cfg.terminal.size_idle_seconds = 3601;
        cfg.validate();
        assert_eq!(cfg.terminal.size_idle_seconds, 120);

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[terminal]\nsize_policy = \"latest\"\nsize_idle_seconds = 30\n",
        )
        .unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.terminal.size_policy, "latest");
        assert_eq!(loaded.terminal.size_idle_seconds, 30);
        let json = serde_json::to_value(&loaded).unwrap();
        assert_eq!(json["terminal"]["sizePolicy"], "latest");
        assert_eq!(json["terminal"]["sizeIdleSeconds"], 30);
    }

    #[test]
    fn test_deep_merge_scalar() {
        let mut base = serde_json::json!({"a": 1, "b": 2});
        let patch = serde_json::json!({"b": 99});
        deep_merge(&mut base, patch);
        assert_eq!(base["a"], 1);
        assert_eq!(base["b"], 99);
    }

    #[test]
    fn test_deep_merge_nested() {
        let mut base = serde_json::json!({"x": {"a": 1, "b": 2}, "y": 10});
        let patch = serde_json::json!({"x": {"b": 99, "c": 3}});
        deep_merge(&mut base, patch);
        assert_eq!(base["x"]["a"], 1);
        assert_eq!(base["x"]["b"], 99);
        assert_eq!(base["x"]["c"], 3);
        assert_eq!(base["y"], 10);
    }

    #[test]
    fn test_snake_to_camel() {
        assert_eq!(snake_to_camel("gpu_acceleration"), "gpuAcceleration");
        assert_eq!(snake_to_camel("hello"), "hello");
        assert_eq!(
            snake_to_camel("draw_bold_text_in_bright_colors"),
            "drawBoldTextInBrightColors"
        );
    }

    #[test]
    fn test_camel_to_snake() {
        assert_eq!(camel_to_snake("gpuAcceleration"), "gpu_acceleration");
        assert_eq!(camel_to_snake("hello"), "hello");
        assert_eq!(
            camel_to_snake("drawBoldTextInBrightColors"),
            "draw_bold_text_in_bright_colors"
        );
    }

    #[test]
    fn test_load_save_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        let original = HomeConfig::default();
        save_to(&original, &path).unwrap();

        let loaded = load_from(&path);
        assert_eq!(original, loaded);
    }

    #[test]
    fn test_config_without_shell_section_migrates_to_system_profile() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[terminal]\ntheme = \"default\"\n").unwrap();

        let loaded = load_from(&path);
        assert!(!loaded.shells.profiles.is_empty());
        // An empty stored default follows the system default.
        assert_eq!(loaded.shells.default_profile_id, "");
        let state = crate::shell_profiles::state_from_settings(&loaded.shells);
        assert!(state.follow_system_default);
        assert!(state.default_profile().is_some());
        let ids: std::collections::HashSet<_> = loaded
            .shells
            .profiles
            .iter()
            .map(|profile| &profile.id)
            .collect();
        assert_eq!(ids.len(), loaded.shells.profiles.len());
    }

    #[test]
    fn test_custom_shell_profile_order_roundtrip() {
        use crate::shell_profiles::{ShellProfileSource, ShellTarget, StoredShellProfile};

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let mut original = HomeConfig::default();
        original.shells.profiles.insert(
            0,
            StoredShellProfile {
                id: "custom-literal".into(),
                name: "Literal".into(),
                command: "/bin/sh".into(),
                args: vec!["literal $HOME".into(), "two words".into()],
                enabled: true,
                target: ShellTarget::Local,
                source: ShellProfileSource::Custom,
            },
        );
        original.shells.default_profile_id = "custom-literal".into();
        save_to(&original, &path).unwrap();

        let loaded = load_from(&path);
        assert_eq!(loaded.shells.profiles[0].id, "custom-literal");
        assert_eq!(
            loaded.shells.profiles[0].args,
            vec!["literal $HOME", "two words"]
        );
        assert_eq!(loaded.shells.default_profile_id, "custom-literal");
    }

    #[test]
    fn clipboard_write_default_and_opt_out_survive_reload() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        // Older configurations have no clipboard field.
        fs::write(&path, "[terminal]\ntheme = 'default'\n").unwrap();
        assert!(load_from(&path).terminal.allow_osc52_write);
        for enabled in [false, true] {
            let patch = serde_json::json!({"terminal": {"allowOsc52Write": enabled}});
            let updated = update_at(&path, patch).unwrap();
            assert_eq!(updated.terminal.allow_osc52_write, enabled);
            let reloaded = load_from(&path);
            assert_eq!(reloaded.terminal.allow_osc52_write, enabled);
            assert_eq!(
                serde_json::to_value(reloaded).unwrap()["terminal"]["allowOsc52Write"],
                enabled
            );
        }
    }

    #[test]
    fn drawer_pin_and_unpin_survive_reload() {
        let dir = TempDir::new().expect("temporary directory");
        let path = dir.path().join("config.toml");
        assert!(load_from(&path).drawer.pinned);
        // Existing configurations have no pinned field.
        fs::write(&path, "[drawer]\nsidebar_width = 280\n").expect("old settings");
        assert!(load_from(&path).drawer.pinned);
        let direct: DrawerSettings = serde_json::from_value(serde_json::json!({
            "edgeReveal": true,
            "showTips": true,
            "sidebarWidth": 280
        }))
        .expect("drawer without pinned field");
        assert!(direct.pinned);
        for pinned in [false, true] {
            let updated = update_at(&path, serde_json::json!({"drawer": {"pinned": pinned}}))
                .expect("persist pin");
            assert_eq!(updated.drawer.pinned, pinned);
            let unrelated = update_at(&path, serde_json::json!({"terminal": {"theme": "monokai"}}))
                .expect("persist unrelated change");
            assert_eq!(unrelated.drawer.pinned, pinned);
            let reloaded = load_from(&path);
            assert_eq!(reloaded.drawer.pinned, pinned);
            assert_eq!(reloaded.drawer.sidebar_width, 280);
            assert_eq!(
                serde_json::to_value(reloaded).expect("settings JSON")["drawer"]["pinned"],
                pinned
            );
        }
    }

    #[test]
    fn test_update_partial() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        let original = HomeConfig::default();
        save_to(&original, &path).unwrap();

        let partial = serde_json::json!({"terminal": {"theme": "monokai"}});
        let updated = update_at(&path, partial).unwrap();

        assert_eq!(updated.terminal.theme, "monokai");
        // Everything else stays at defaults.
        assert_eq!(updated.appearance.theme, "splitty-dark");
    }

    #[test]
    fn native_chrome_is_independent_and_validated_atomically() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[appearance]\ntheme = \"adwaita-dark\"\n").unwrap();
        let before = load_from(&path);
        assert_eq!(before.appearance.title_bar_style, "theme");
        let changed = update_at(
            &path,
            serde_json::json!({"appearance": {"titleBarStyle": "system", "interfaceStyle": "system", "windowDecoration": "splitty", "windowButtons": "system"}}),
        )
        .unwrap();
        assert_eq!(load_from(&path).appearance.title_bar_style, "system");
        assert_eq!(changed.appearance.interface_style, "system");
        assert_eq!(changed.appearance.window_decoration, "splitty");
        assert_eq!(changed.appearance.window_buttons, "system");
        assert_eq!(changed.appearance.theme, before.appearance.theme);
        assert_eq!(changed.terminal, before.terminal);
        let saved = fs::read_to_string(&path).unwrap();
        assert!(update_at(
            &path,
            serde_json::json!({"appearance": {"titleBarStyle": "invalid", "theme": "other"}})
        )
        .is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), saved);
        for key in ["interfaceStyle", "windowDecoration", "windowButtons"] {
            assert!(update_at(
                &path,
                serde_json::json!({"appearance": {key: "invalid", "theme": "other"}})
            )
            .is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), saved);
        }
        assert_eq!(
            settings_metadata()["limits"]["appearance.titleBarStyle"]["values"],
            serde_json::json!(["system", "theme"])
        );
        let mut old = serde_json::to_value(before.appearance).unwrap();
        old.as_object_mut().unwrap().remove("titleBarStyle");
        for key in ["interfaceStyle", "windowDecoration", "windowButtons"] {
            old.as_object_mut().unwrap().remove(key);
        }
        let restored: AppearanceSettings = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(restored.interface_style, "theme");
        assert_eq!(restored.window_decoration, "system");
        assert_eq!(restored.window_buttons, "all");
        assert_eq!(
            serde_json::from_value::<AppearanceSettings>(old)
                .unwrap()
                .title_bar_style,
            "theme"
        );
    }

    #[test]
    fn test_notification_settings_roundtrip() {
        // Update the settings with camelCase JSON and reload from disk.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        // Start with defaults
        let original = HomeConfig::default();
        save_to(&original, &path).unwrap();
        assert!(!original.notifications.toasts);

        // Frontend sends camelCase partial update
        let partial = serde_json::json!({"notifications": {"toasts": true}});
        let updated = update_at(&path, partial).unwrap();
        assert!(updated.notifications.toasts);

        // Simulate page refresh: reload from disk
        let reloaded = load_from(&path);
        assert!(
            reloaded.notifications.toasts,
            "toasts should survive reload"
        );
        // Other notification defaults should be preserved
        assert!(reloaded.notifications.enabled);
        assert!(!reloaded.notifications.desktop);
        assert!(reloaded.notifications.process_finished);
        assert!(!reloaded.notifications.process_exited);
        assert!(reloaded.notifications.process_crashed);

        // Also verify the wire format.
        let json_response = serde_json::to_value(&reloaded).unwrap();
        let notifs = json_response.get("notifications").unwrap();
        assert_eq!(notifs.get("toasts").unwrap(), true);
    }

    #[test]
    fn program_status_switch_defaults_on_for_older_settings_and_persists_off() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        // Settings stored before the switch existed.
        fs::write(
            &path,
            "[notifications]\nenabled = true\ntoasts = false\ndesktop = false\n\
             process_finished = true\nprocess_exited = false\nprocess_crashed = true\n",
        )
        .unwrap();
        let cfg = load_from(&path);
        assert!(cfg.notifications.program_status);
        assert_eq!(
            serde_json::to_value(&cfg).unwrap()["notifications"]["programStatus"],
            true
        );
        // A reply of a server that does not know the switch reads it as on.
        let reply: NotificationSettings = serde_json::from_value(serde_json::json!({
            "enabled": true, "toasts": false, "desktop": false,
            "processFinished": true, "processExited": false, "processCrashed": true
        }))
        .unwrap();
        assert!(reply.program_status);
        assert_eq!(
            settings_metadata()["defaults"]["notifications"]["programStatus"],
            true
        );

        let updated = update_at(
            &path,
            serde_json::json!({"notifications": {"programStatus": false}}),
        )
        .unwrap();
        assert!(!updated.notifications.program_status);
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("program_status = false"));
        let reloaded = load_from(&path);
        assert!(!reloaded.notifications.program_status);
        assert!(reloaded.notifications.enabled && reloaded.notifications.process_crashed);
        let err = update_at(
            &path,
            serde_json::json!({"notifications": {"programStatus": "off"}}),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidField { field, .. } if field == "notifications.programStatus"
        ));
    }

    #[test]
    fn test_remote_settings_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        // A configuration from before the section has it off.
        fs::write(&path, "[terminal]\ntheme = \"dark\"\n").unwrap();
        assert!(!load_from(&path).remote.auto_forward_ports);

        // A camelCase update survives a reload, with snake_case on disk.
        let partial = serde_json::json!({"remote": {"autoForwardPorts": true}});
        assert!(update_at(&path, partial).unwrap().remote.auto_forward_ports);
        assert!(load_from(&path).remote.reconnect_hosts.is_empty());
        let partial = serde_json::json!({"remote": {"reconnectHosts": ["demo@vm1", "vm2:2222"]}});
        let updated = update_at(&path, partial).unwrap();
        assert_eq!(updated.remote.reconnect_hosts, ["demo@vm1", "vm2:2222"]);
        assert!(updated.remote.auto_forward_ports);
        for refused in [
            serde_json::json!(["vm1", "vm1"]),
            serde_json::json!(["-oProxyCommand=x"]),
            serde_json::json!(["two words"]),
            serde_json::json!([7]),
            serde_json::json!("vm1"),
        ] {
            let partial = serde_json::json!({"remote": {"reconnectHosts": refused}});
            assert!(update_at(&path, partial).is_err());
        }
        assert_eq!(load_from(&path).remote.reconnect_hosts.len(), 2);
        let toml = fs::read_to_string(&path).unwrap();
        assert!(
            toml.contains("[remote]\nauto_forward_ports = true"),
            "{toml}"
        );
        let reloaded = load_from(&path);
        assert!(reloaded.remote.auto_forward_ports);

        let json = serde_json::to_value(&reloaded).unwrap();
        assert_eq!(json["remote"]["autoForwardPorts"], true);

        // A value of the wrong type is not taken; the file falls back to the defaults.
        fs::write(&path, "[remote]\nauto_forward_ports = \"yes\"\n").unwrap();
        assert!(!load_from(&path).remote.auto_forward_ports);
    }

    #[test]
    fn test_toml_load_partial() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        fs::write(&path, "[terminal]\ntheme = \"dark\"\n").unwrap();

        let cfg = load_from(&path);
        assert_eq!(cfg.terminal.theme, "dark");
        // Other terminal fields should be defaults.
        assert_eq!(cfg.terminal.close_on_exit, "always");
        // Other sections should be defaults.
        assert_eq!(cfg.appearance.theme, "splitty-dark");
    }

    #[test]
    fn a_section_of_an_older_version_loads_and_is_dropped_on_save() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        // `[screens]` held the settings of a screen switcher the app no longer has.
        fs::write(
            &path,
            "[screens]\nopacity = 80\n\n[terminal]\ntheme = \"dark\"\n",
        )
        .unwrap();

        let cfg = load_from(&path);
        assert_eq!(cfg.terminal.theme, "dark");

        save_to(&cfg, &path).unwrap();
        let toml = fs::read_to_string(&path).unwrap();
        assert!(!toml.contains("[screens]"), "{toml}");
    }
    #[test]
    fn editor_preferences_roundtrip_preserves_rule_values() {
        let dir = tempfile::tempdir().expect("config fixture");
        let path = dir.path().join("home.toml");
        let input = serde_json::json!({"editors":{"file":"zed","folder":"vscode","extensions":[{"extension":"my_ext","editor":"zed"}],"projects":[{"connection":"host:Me@vm1","path":"/srv/My_Project","editor":"vscode"}]}});
        let config = update_at(&path, input).expect("update");
        let restored = load_from(&path);
        assert_eq!(config.editors, restored.editors);
        assert_eq!(restored.editors.extensions[0].extension, "my_ext");
        assert_eq!(restored.editors.projects[0].path, "/srv/My_Project");
        assert_eq!(restored.editors.projects[0].connection, "host:Me@vm1");
    }
    #[test]
    fn metadata_and_fractional_zoom_are_server_owned() {
        let metadata = settings_metadata();
        assert_eq!(metadata["defaults"]["drawer"]["pinned"], true);
        assert_eq!(metadata["defaults"]["drawer"]["sidebarWidth"], 200);
        assert!(metadata["limits"]["appearance.zoom"]["values"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(1.25)));
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let config = update_at(&path, serde_json::json!({"appearance":{"zoom":1.25}})).unwrap();
        assert_eq!(config.appearance.zoom, 1.25);
        assert_eq!(load_from(&path).appearance.zoom, 1.25);
    }

    #[test]
    fn rejected_patch_preserves_file_and_identifies_field() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let before = update_at(&path, serde_json::json!({"drawer":{"sidebarWidth":280}})).unwrap();
        let saved = fs::read(&path).unwrap();
        for (patch, field) in [
            (
                serde_json::json!({"terminal":{"xterm":{"cursorBlink":true}}}),
                "terminal.xterm",
            ),
            (
                serde_json::json!({"terminal":{"linkTarget":"tab"}}),
                "terminal.linkTarget",
            ),
            (
                serde_json::json!({"appearance":{"fontSize":7}}),
                "appearance.fontSize",
            ),
            (
                serde_json::json!({"editors":{"extensions":[{"extension":"../rs","editor":"zed"}]}}),
                "editors.extensions.0.extension",
            ),
            (
                serde_json::json!({"editors":{"projects":[{"connection":"local","path":"/a/../b","editor":"zed"}]}}),
                "editors.projects.0.path",
            ),
        ] {
            let err = update_at(&path, patch).unwrap_err();
            assert!(
                matches!(err, ConfigError::InvalidField { field: actual, .. } if actual == field)
            );
            assert_eq!(load_from(&path), before);
            assert_eq!(fs::read(&path).unwrap(), saved);
        }
    }

    #[test]
    fn removed_settings_are_ignored_in_old_files_and_dropped_on_save() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[appearance]
font_size = 15
[terminal]
renderer = "xterm"
ligatures = true
window_padding_balance = false
link_target = "webview"
webview_mode = "proxy"
[terminal.xterm]
cursor_blink = true
[tabs]
min_width = 24
[window]
title_bar_scroll = true
[experimental]
linked_sashes = true
resize_debounce = 10
[drawer]
resizable = true
"#,
        )
        .unwrap();

        let loaded = load_from(&path);
        assert_eq!(loaded.appearance.font_size, 15);
        let effective = serde_json::to_value(&loaded).unwrap();
        let defaults = settings_metadata();
        for settings in [&effective, &defaults["defaults"]] {
            for group in ["tabs", "window", "experimental"] {
                assert!(settings.get(group).is_none(), "{group}");
            }
            for key in [
                "renderer",
                "ligatures",
                "windowPaddingBalance",
                "xterm",
                "linkTarget",
                "webviewMode",
            ] {
                assert!(settings["terminal"].get(key).is_none(), "{key}");
            }
            assert!(settings["drawer"].get("resizable").is_none());
        }

        save_to(&loaded, &path).unwrap();
        let saved: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["appearance"]["font_size"].as_integer(), Some(15));
        for group in ["tabs", "window", "experimental"] {
            assert!(saved.get(group).is_none(), "{group}");
        }
        for key in [
            "renderer",
            "ligatures",
            "window_padding_balance",
            "xterm",
            "link_target",
            "webview_mode",
        ] {
            assert!(saved["terminal"].get(key).is_none(), "{key}");
        }
        assert!(saved["drawer"].get("resizable").is_none());
    }
}
