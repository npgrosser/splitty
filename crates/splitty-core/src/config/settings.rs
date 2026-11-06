//! Settings structs, defaults, validation, and TOML persistence.
//!
//! Defines `HomeConfig` (the main user-facing configuration) and `ServerConfig`,
//! along with all nested settings groups (terminal, appearance, tabs, drawer,
//! window, notifications, remote). Includes TOML <-> JSON key transformation
//! (snake_case on disk, camelCase on the wire) and deep-merge for partial updates.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::config_path;
use crate::shell_profiles::{
    detect_shell_profiles, reconcile_shell_settings, DetectionEnvironment, ShellSettings,
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
    pub tabs: TabSettings,
    pub drawer: DrawerSettings,
    pub window: WindowSettings,
    pub notifications: NotificationSettings,
    pub experimental: ExperimentalSettings,
    #[serde(default)]
    pub remote: RemoteSettings,
    #[serde(default)]
    pub shells: ShellSettings,
}

impl Default for HomeConfig {
    fn default() -> Self {
        let detection = detect_shell_profiles(&DetectionEnvironment::current());
        let mut shells = ShellSettings::default();
        reconcile_shell_settings(&mut shells, &detection);
        Self {
            editors: EditorSettings::default(),
            terminal: TerminalSettings::default(),
            appearance: AppearanceSettings::default(),
            tabs: TabSettings::default(),
            drawer: DrawerSettings::default(),
            window: WindowSettings::default(),
            notifications: NotificationSettings::default(),
            experimental: ExperimentalSettings::default(),
            remote: RemoteSettings::default(),
            shells,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSettings {
    pub renderer: String,
    pub theme: String,
    pub close_on_exit: String,
    pub ligatures: bool,
    pub window_padding_balance: bool,
    pub scroll_to_bottom: Option<bool>,
    pub allow_osc52_write: bool,
    /// Which clients decide the PTY size of a session several clients show:
    /// `active`, `latest` or `smallest`. Applies to this server's sessions.
    pub size_policy: String,
    /// How long after its user's last action a client still counts as active.
    pub size_idle_seconds: u32,
    pub xterm: XtermSettings,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct XtermSettings {
    pub gpu_acceleration: String,
    pub scrollbar_width: i32,
    pub scrollbar_overlay: bool,
    pub scrollbar_fade: bool,
    pub scroll_sensitivity: f64,
    pub fast_scroll_sensitivity: f64,
    pub line_height: f64,
    pub letter_spacing: f64,
    pub font_weight: String,
    pub font_weight_bold: String,
    pub draw_bold_text_in_bright_colors: bool,
    pub minimum_contrast_ratio: f64,
    pub custom_glyphs: bool,
    pub rescale_overlapping_glyphs: bool,
    pub allow_transparency: bool,
    pub cursor_blink: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceSettings {
    pub theme: String,
    pub theme_mode: String,
    pub theme_dark: String,
    pub theme_light: String,
    pub border_radius: i32,
    pub font_size: i32,
    pub font_family: String,
    /// The zoom level of this computer's windows, from 0.5 to 3.0.
    pub zoom: f64,
    /// What zooms: `window` (the whole page) or `terminals` (their font size).
    pub zoom_scope: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    // Placeholder — real server settings come with remote connections milestone
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TabSettings {
    pub auto_hide: bool,
    pub auto_hide_multi: bool,
    pub sync_row: bool,
    pub locked: bool,
    pub close_button: String,
    pub min_width: i32,
    pub show_split: bool,
    pub show_float: bool,
    pub show_maximize: bool,
    pub show_popout: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WindowSettings {
    pub native_title_bar: bool,
    #[serde(default = "default_title_bar_color_mode")]
    pub title_bar_color_mode: String,
    pub title_bar_height: i32,
    pub title_bar_scroll: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DrawerSettings {
    #[serde(default = "default_drawer_pinned")]
    pub pinned: bool,
    pub edge_reveal: bool,
    pub show_tips: bool,
    #[serde(default)]
    pub resizable: bool,
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
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentalSettings {
    pub linked_sashes: bool,
    pub resize_debounce: u32,
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

/// Screens of other hosts (`remote-screens`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSettings {
    /// Forward a port that starts listening on a connected host without
    /// asking. Off by default: a forward opens a port on this machine that
    /// any local process can reach.
    #[serde(default)]
    pub auto_forward_ports: bool,
}

// ---------------------------------------------------------------------------
// Default implementations
// ---------------------------------------------------------------------------

fn default_title_bar_color_mode() -> String {
    "tabBar".into()
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            renderer: "xterm".into(),
            theme: "default".into(),
            close_on_exit: "always".into(),
            ligatures: false,
            window_padding_balance: true,
            scroll_to_bottom: Some(true),
            allow_osc52_write: true,
            size_policy: "active".into(),
            size_idle_seconds: 120,
            xterm: XtermSettings::default(),
        }
    }
}

impl Default for XtermSettings {
    fn default() -> Self {
        Self {
            gpu_acceleration: "on".into(),
            scrollbar_width: 14,
            scrollbar_overlay: true,
            scrollbar_fade: true,
            scroll_sensitivity: 1.0,
            fast_scroll_sensitivity: 5.0,
            line_height: 1.0,
            letter_spacing: 0.0,
            font_weight: "normal".into(),
            font_weight_bold: "bold".into(),
            draw_bold_text_in_bright_colors: true,
            minimum_contrast_ratio: 1.0,
            custom_glyphs: true,
            rescale_overlapping_glyphs: false,
            allow_transparency: false,
            cursor_blink: false,
        }
    }
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: "splitty-dark".into(),
            theme_mode: "fixed".into(),
            theme_dark: "splitty-dark".into(),
            theme_light: "splitty-light".into(),
            border_radius: 1,
            font_size: 0,
            font_family: String::new(),
            zoom: 1.0,
            zoom_scope: "window".into(),
        }
    }
}

impl Default for TabSettings {
    fn default() -> Self {
        Self {
            auto_hide: false,
            auto_hide_multi: false,
            sync_row: false,
            locked: false,
            close_button: "active".into(),
            min_width: 12,
            show_split: true,
            show_float: false,
            show_maximize: false,
            show_popout: false,
        }
    }
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            native_title_bar: !cfg!(target_os = "linux"),
            title_bar_color_mode: default_title_bar_color_mode(),
            title_bar_height: 0,
            title_bar_scroll: true,
        }
    }
}

impl Default for DrawerSettings {
    fn default() -> Self {
        Self {
            pinned: default_drawer_pinned(),
            edge_reveal: true,
            show_tips: true,
            resizable: false,
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
        }
    }
}

impl Default for ExperimentalSettings {
    fn default() -> Self {
        Self {
            linked_sashes: false,
            resize_debounce: 5,
        }
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

impl HomeConfig {
    pub fn validate(&mut self) {
        // Terminal settings
        if self.terminal.renderer != "xterm" && self.terminal.renderer != "ghostty" {
            self.terminal.renderer = "xterm".into();
        }
        if self.terminal.theme.is_empty() {
            self.terminal.theme = "default".into();
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

        // Xterm settings
        let xterm = &mut self.terminal.xterm;
        if xterm.gpu_acceleration != "on" && xterm.gpu_acceleration != "off" {
            xterm.gpu_acceleration = "on".into();
        }
        if !(1..=50).contains(&xterm.scrollbar_width) {
            xterm.scrollbar_width = 14;
        }
        if !(0.1..=10.0).contains(&xterm.scroll_sensitivity) {
            xterm.scroll_sensitivity = 1.0;
        }
        if !(1.0..=20.0).contains(&xterm.fast_scroll_sensitivity) {
            xterm.fast_scroll_sensitivity = 5.0;
        }
        if !(0.5..=3.0).contains(&xterm.line_height) {
            xterm.line_height = 1.0;
        }
        if !(-5.0..=10.0).contains(&xterm.letter_spacing) {
            xterm.letter_spacing = 0.0;
        }
        let valid_weights = [
            "normal", "bold", "100", "200", "300", "400", "500", "600", "700", "800", "900",
        ];
        if !valid_weights.contains(&xterm.font_weight.as_str()) {
            xterm.font_weight = "normal".into();
        }
        if !valid_weights.contains(&xterm.font_weight_bold.as_str()) {
            xterm.font_weight_bold = "bold".into();
        }
        if !(1.0..=21.0).contains(&xterm.minimum_contrast_ratio) {
            xterm.minimum_contrast_ratio = 1.0;
        }

        // Tab settings
        if !matches!(self.tabs.close_button.as_str(), "always" | "active") {
            self.tabs.close_button = "active".into();
        }
        if !(0..=40).contains(&self.tabs.min_width) {
            self.tabs.min_width = 12;
        }

        // Appearance settings
        if !matches!(self.appearance.theme_mode.as_str(), "fixed" | "auto") {
            self.appearance.theme_mode = "fixed".into();
        }
        if self.appearance.theme_dark.is_empty() {
            self.appearance.theme_dark = "splitty-dark".into();
        }
        if self.appearance.theme_light.is_empty() {
            self.appearance.theme_light = "splitty-light".into();
        }
        if !(0..=20).contains(&self.appearance.border_radius) {
            self.appearance.border_radius = 1;
        }
        if self.appearance.font_size != 0 && !(8..=24).contains(&self.appearance.font_size) {
            self.appearance.font_size = 0;
        }
        if !(0.5..=3.0).contains(&self.appearance.zoom) {
            self.appearance.zoom = 1.0;
        }
        if !matches!(self.appearance.zoom_scope.as_str(), "window" | "terminals") {
            self.appearance.zoom_scope = "window".into();
        }

        // Drawer settings
        if !(150..=400).contains(&self.drawer.sidebar_width) {
            self.drawer.sidebar_width = 200;
        }

        // Window settings
        if !matches!(
            self.window.title_bar_color_mode.as_str(),
            "tabBar" | "native" | "terminal" | "activeTab"
        ) {
            self.window.title_bar_color_mode = default_title_bar_color_mode();
        }
        if self.window.title_bar_height != 0 && !(28..=60).contains(&self.window.title_bar_height) {
            self.window.title_bar_height = 0;
        }

        // Experimental settings
        if self.experimental.resize_debounce > 200 {
            self.experimental.resize_debounce = 5;
        }

        let detection = detect_shell_profiles(&DetectionEnvironment::current());
        reconcile_shell_settings(&mut self.shells, &detection);
    }
}

/// Server-owned control metadata; paths use the JSON wire spelling.
pub fn settings_metadata() -> Value {
    let mut limits = serde_json::json!({
            "terminal.renderer": {"values": ["xterm", "ghostty"]},
            "terminal.closeOnExit": {"values": ["always", "never", "graceful"]},
            "terminal.sizePolicy": {"values": ["active", "latest", "smallest"]},
            "terminal.sizeIdleSeconds": {"min": 10, "max": 3600, "step": 10},
            "terminal.xterm.gpuAcceleration": {"values": ["on", "off"]},
            "terminal.xterm.scrollbarWidth": {"min": 1, "max": 50, "step": 1},
            "terminal.xterm.scrollSensitivity": {"min": 0.1, "max": 10, "step": 0.1},
            "terminal.xterm.fastScrollSensitivity": {"min": 1, "max": 20, "step": 0.5},
            "terminal.xterm.lineHeight": {"min": 0.5, "max": 3, "step": 0.05},
            "terminal.xterm.letterSpacing": {"min": -5, "max": 10, "step": 0.5},
            "terminal.xterm.fontWeight": {"values": ["normal", "bold", "100", "200", "300", "400", "500", "600", "700", "800", "900"]},
            "terminal.xterm.fontWeightBold": {"values": ["normal", "bold", "100", "200", "300", "400", "500", "600", "700", "800", "900"]},
            "terminal.xterm.minimumContrastRatio": {"min": 1, "max": 21, "step": 0.1},
    });
    limits.as_object_mut().expect("limit map").extend(serde_json::json!({
            "appearance.themeMode": {"values": ["fixed", "auto"]},
            "appearance.borderRadius": {"min": 0, "max": 20, "step": 1},
            "appearance.fontSize": {"min": 0, "max": 24, "step": 1, "values": [0, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24]},
            "appearance.zoom": {"min": 0.5, "max": 3, "step": 0.1},
            "appearance.zoomScope": {"values": ["window", "terminals"]},
            "tabs.closeButton": {"values": ["always", "active"]},
            "tabs.minWidth": {"min": 0, "max": 40, "step": 1},
            "drawer.sidebarWidth": {"min": 150, "max": 400, "step": 10},
            "window.titleBarColorMode": {"values": ["tabBar", "native", "terminal", "activeTab"]},
            "window.titleBarHeight": {"min": 0, "max": 60, "step": 1, "values": [0, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60]},
            "experimental.resizeDebounce": {"min": 0, "max": 200, "step": 5},
            "editors.file": {"values": ["vscode", "zed"]},
            "editors.folder": {"values": ["vscode", "zed"]}
    }).as_object().expect("limit map").clone());
    serde_json::json!({ "defaults": HomeConfig::default(), "limits": limits })
}

fn invalid(field: &str, message: &str) -> ConfigError {
    ConfigError::InvalidField {
        field: field.to_owned(),
        message: message.to_owned(),
    }
}

fn validate_patch(patch: &Value) -> Result<(), ConfigError> {
    let defaults = serde_json::to_value(HomeConfig::default())?;
    let metadata = settings_metadata();
    let limits = &metadata["limits"];
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
                if path == "terminal" && matches!(key.as_str(), "linkTarget" | "webviewMode") {
                    continue;
                }
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
                } else if field == "shells" { /* Shell profiles have their own update validation. */
                } else {
                    walk(child, expected_child, limits, &field)?;
                }
            }
            return Ok(());
        }
        if let Some(limit) = limits.get(path) {
            if let Some(values) = limit.get("values").and_then(Value::as_array) {
                if !values.contains(value) {
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
                if matches!(
                    path,
                    "terminal.xterm.scrollSensitivity" | "terminal.xterm.fastScrollSensitivity"
                ) {
                    let step = limit["step"].as_f64().unwrap_or(1.0);
                    if ((n - min) / step - ((n - min) / step).round()).abs() > 1e-7 {
                        return Err(invalid(path, "not on an allowed step"));
                    }
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
    walk(patch, &defaults, limits, "")
}

fn valid_editor(value: &Value) -> bool {
    matches!(value.as_str(), Some("vscode" | "zed"))
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
    let should_persist = match fs::read_to_string(&path) {
        Ok(data) => parse_toml_to_config(&data).is_ok(),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    };
    let cfg = load_from(&path);
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
    let data = match fs::read_to_string(path) {
        Ok(d) => d,
        Err(_) => {
            let mut cfg = HomeConfig::default();
            cfg.validate();
            return cfg;
        }
    };

    let mut cfg = match parse_toml_to_config(&data) {
        Ok(c) => c,
        Err(_) => {
            let mut cfg = HomeConfig::default();
            cfg.validate();
            return cfg;
        }
    };

    cfg.validate();
    cfg
}

/// Parses a TOML string into `HomeConfig` using key transformation.
///
/// TOML uses snake_case keys, but our structs expect camelCase (via `rename_all`).
/// We parse TOML into a generic `toml::Value`, convert to `serde_json::Value`,
/// transform keys from snake_case to camelCase, then deserialize into our struct.
fn parse_toml_to_config(toml_str: &str) -> Result<HomeConfig, ConfigError> {
    let toml_val: toml::Value = toml::from_str(toml_str)?;
    let json_val = toml_value_to_json(toml_val);
    let camel_val = transform_keys(json_val, &snake_to_camel);

    // Merge with defaults so missing fields get filled in.
    let mut base = serde_json::to_value(HomeConfig::default())?;
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

pub fn load_server() -> ServerConfig {
    load_server_from(&super::server_config_path())
}

pub fn load_server_from(path: &Path) -> ServerConfig {
    let data = match fs::read_to_string(path) {
        Ok(d) => d,
        Err(_) => return ServerConfig::default(),
    };
    toml::from_str(&data).unwrap_or_default()
}

pub fn save_server(config: &ServerConfig) -> Result<(), ConfigError> {
    save_server_to(config, &super::server_config_path())
}

pub fn save_server_to(config: &ServerConfig, path: &Path) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        super::ensure_private_dir(parent)?;
    }
    let toml_str = toml::to_string_pretty(config)?;
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
    let cfg = load_from(path);
    validate_patch(&partial)?;
    let mut base = serde_json::to_value(&cfg)?;
    deep_merge(&mut base, partial);
    let mut cfg: HomeConfig = serde_json::from_value(base)?;
    cfg.validate();
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
    fn test_defaults() {
        let cfg = HomeConfig::default();
        assert_eq!(cfg.terminal.renderer, "xterm");
        assert_eq!(cfg.terminal.theme, "default");
        assert_eq!(cfg.terminal.close_on_exit, "always");
        assert!(!cfg.terminal.ligatures);
        assert!(cfg.terminal.window_padding_balance);
        assert_eq!(cfg.terminal.scroll_to_bottom, Some(true));
        assert_eq!(cfg.terminal.xterm.gpu_acceleration, "on");
        assert_eq!(cfg.terminal.xterm.scrollbar_width, 14);
        assert!(cfg.terminal.xterm.scrollbar_overlay);
        assert!(cfg.terminal.xterm.scrollbar_fade);
        assert_eq!(cfg.terminal.xterm.scroll_sensitivity, 1.0);
        assert_eq!(cfg.terminal.xterm.fast_scroll_sensitivity, 5.0);
        assert!((cfg.terminal.xterm.line_height - 1.0).abs() < f64::EPSILON);
        assert!((cfg.terminal.xterm.letter_spacing - 0.0).abs() < f64::EPSILON);
        assert_eq!(cfg.terminal.xterm.font_weight, "normal");
        assert_eq!(cfg.terminal.xterm.font_weight_bold, "bold");
        assert!(cfg.terminal.xterm.draw_bold_text_in_bright_colors);
        assert!((cfg.terminal.xterm.minimum_contrast_ratio - 1.0).abs() < f64::EPSILON);
        assert!(cfg.terminal.xterm.custom_glyphs);
        assert!(!cfg.terminal.xterm.rescale_overlapping_glyphs);
        assert!(!cfg.terminal.xterm.allow_transparency);
        assert!(!cfg.terminal.xterm.cursor_blink);
        assert_eq!(cfg.appearance.theme, "splitty-dark");
        assert_eq!(cfg.appearance.border_radius, 1);
        assert!(!cfg.tabs.auto_hide);
        assert!(!cfg.tabs.auto_hide_multi);
        assert!(!cfg.tabs.sync_row);
        assert!(!cfg.tabs.locked);
        assert_eq!(cfg.tabs.close_button, "active");
        assert_eq!(cfg.tabs.min_width, 12);
        assert!(cfg.tabs.show_split);
        assert!(!cfg.tabs.show_float);
        assert!(!cfg.tabs.show_maximize);
        assert!(!cfg.tabs.show_popout);
        assert!(cfg.drawer.pinned);
        assert!(cfg.drawer.edge_reveal);
        assert!(cfg.drawer.show_tips);
        assert!(!cfg.drawer.resizable);
        assert_eq!(cfg.window.native_title_bar, !cfg!(target_os = "linux"));
        assert_eq!(cfg.window.title_bar_color_mode, "tabBar");
        assert_eq!(cfg.window.title_bar_height, 0);
        assert!(cfg.window.title_bar_scroll);
        assert!(cfg.notifications.enabled);
        assert!(!cfg.notifications.toasts);
        assert!(!cfg.notifications.desktop);
        assert!(cfg.notifications.process_finished);
        assert!(!cfg.notifications.process_exited);
        assert!(cfg.notifications.process_crashed);
        assert!(!cfg.remote.auto_forward_ports);
    }

    #[test]
    fn test_validate_renderer() {
        let mut cfg = HomeConfig::default();
        cfg.terminal.renderer = "invalid".into();
        cfg.validate();
        assert_eq!(cfg.terminal.renderer, "xterm");

        cfg.terminal.renderer = "ghostty".into();
        cfg.validate();
        assert_eq!(cfg.terminal.renderer, "ghostty");
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
    fn test_validate_scrollbar_width_range() {
        let mut cfg = HomeConfig::default();
        cfg.terminal.xterm.scrollbar_width = 0;
        cfg.validate();
        assert_eq!(cfg.terminal.xterm.scrollbar_width, 14);

        cfg.terminal.xterm.scrollbar_width = 51;
        cfg.validate();
        assert_eq!(cfg.terminal.xterm.scrollbar_width, 14);

        cfg.terminal.xterm.scrollbar_width = 25;
        cfg.validate();
        assert_eq!(cfg.terminal.xterm.scrollbar_width, 25);
    }

    #[test]
    fn test_validate_line_height_range() {
        let mut cfg = HomeConfig::default();
        cfg.terminal.xterm.line_height = 0.1;
        cfg.validate();
        assert!((cfg.terminal.xterm.line_height - 1.0).abs() < f64::EPSILON);

        cfg.terminal.xterm.line_height = 4.0;
        cfg.validate();
        assert!((cfg.terminal.xterm.line_height - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_validate_font_weight() {
        let mut cfg = HomeConfig::default();
        cfg.terminal.xterm.font_weight = "invalid".into();
        cfg.validate();
        assert_eq!(cfg.terminal.xterm.font_weight, "normal");

        cfg.terminal.xterm.font_weight = "400".into();
        cfg.validate();
        assert_eq!(cfg.terminal.xterm.font_weight, "400");
    }

    #[test]
    fn test_validate_title_bar_height() {
        let mut cfg = HomeConfig::default();
        cfg.window.title_bar_height = 20;
        cfg.validate();
        assert_eq!(cfg.window.title_bar_height, 0);

        cfg.window.title_bar_height = 35;
        cfg.validate();
        assert_eq!(cfg.window.title_bar_height, 35);
    }

    #[test]
    fn test_validate_title_bar_color_mode() {
        let mut cfg = HomeConfig::default();
        cfg.window.title_bar_color_mode = "invalid".into();
        cfg.validate();
        assert_eq!(cfg.window.title_bar_color_mode, "tabBar");

        cfg.window.title_bar_color_mode = "terminal".into();
        cfg.validate();
        assert_eq!(cfg.window.title_bar_color_mode, "terminal");
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
        fs::write(&path, "[terminal]\nrenderer = \"xterm\"\n").unwrap();

        let loaded = load_from(&path);
        assert!(!loaded.shells.profiles.is_empty());
        assert!(!loaded.shells.default_profile_id.is_empty());
        assert!(loaded
            .shells
            .profiles
            .iter()
            .any(|profile| profile.id == loaded.shells.default_profile_id));
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
        assert_eq!(updated.terminal.renderer, "xterm");
        assert_eq!(updated.appearance.theme, "splitty-dark");
        assert_eq!(updated.tabs.min_width, 12);
    }

    #[test]
    fn native_title_bar_default_and_saved_preferences() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        // An older partial config takes the current platform default.
        fs::write(&path, "[window]\ntitle_bar_height = 40\n").unwrap();
        assert_eq!(
            load_from(&path).window.native_title_bar,
            !cfg!(target_os = "linux")
        );

        for saved in [true, false] {
            fs::write(&path, format!("[window]\nnative_title_bar = {saved}\n")).unwrap();
            assert_eq!(load_from(&path).window.native_title_bar, saved);
            let updated = update_at(&path, serde_json::json!({"tabs": {"minWidth": 18}})).unwrap();
            assert_eq!(updated.window.native_title_bar, saved);
            assert_eq!(load_from(&path).window.native_title_bar, saved);
        }
    }

    #[test]
    fn test_notification_settings_roundtrip() {
        // Simulate exactly what the frontend does: PATCH with camelCase JSON,
        // then reload from disk (simulating page refresh / server restart).
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

        // Also verify the GET response format (what frontend receives)
        let json_response = serde_json::to_value(&reloaded).unwrap();
        let notifs = json_response.get("notifications").unwrap();
        assert_eq!(notifs.get("toasts").unwrap(), true);
    }

    #[test]
    fn test_remote_settings_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        // A configuration from before the section has it off.
        fs::write(&path, "[terminal]\ntheme = \"dark\"\n").unwrap();
        assert!(!load_from(&path).remote.auto_forward_ports);

        // The frontend's camelCase update survives a reload, snake_case on disk.
        let partial = serde_json::json!({"remote": {"autoForwardPorts": true}});
        assert!(update_at(&path, partial).unwrap().remote.auto_forward_ports);
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
        assert_eq!(cfg.terminal.renderer, "xterm");
        assert_eq!(cfg.terminal.close_on_exit, "always");
        // Other sections should be defaults.
        assert_eq!(cfg.appearance.theme, "splitty-dark");
        assert_eq!(cfg.tabs.min_width, 12);
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
    fn metadata_and_fractional_scroll_are_server_owned() {
        let metadata = settings_metadata();
        assert_eq!(metadata["defaults"]["drawer"]["pinned"], true);
        assert_eq!(metadata["defaults"]["drawer"]["sidebarWidth"], 200);
        assert_eq!(
            metadata["limits"]["terminal.xterm.scrollSensitivity"]["step"],
            0.1
        );
        let dir = TempDir::new().unwrap();
        let cfg = update_at(
            &dir.path().join("config.toml"),
            serde_json::json!({
                "terminal": { "xterm": { "scrollSensitivity": 1.3, "fastScrollSensitivity": 2.5 } }
            }),
        )
        .unwrap();
        assert_eq!(cfg.terminal.xterm.scroll_sensitivity, 1.3);
        assert_eq!(cfg.terminal.xterm.fast_scroll_sensitivity, 2.5);
        let restored = load_from(&dir.path().join("config.toml"));
        assert_eq!(restored.terminal.xterm.scroll_sensitivity, 1.3);
        assert_eq!(restored.terminal.xterm.fast_scroll_sensitivity, 2.5);
    }

    #[test]
    fn rejected_patch_preserves_file_and_identifies_field() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let before = update_at(&path, serde_json::json!({"drawer":{"sidebarWidth":280}})).unwrap();
        for (patch, field) in [
            (
                serde_json::json!({"terminal":{"xterm":{"scrollSensitivity":0.15}}}),
                "terminal.xterm.scrollSensitivity",
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
        }
    }

    #[test]
    fn removed_terminal_fields_do_not_survive_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[terminal]\nlink_target = \"webview\"\nwebview_mode = \"proxy\"\n",
        )
        .unwrap();
        let loaded = load_from(&path);
        let json = serde_json::to_value(loaded).unwrap();
        assert!(json["terminal"].get("linkTarget").is_none());
        assert!(json["terminal"].get("webviewMode").is_none());
        let updated = update_at(
            &path,
            serde_json::json!({"terminal":{"linkTarget":"webview","webviewMode":"direct"}}),
        )
        .unwrap();
        assert!(serde_json::to_value(updated).unwrap()["terminal"]
            .get("linkTarget")
            .is_none());
    }
}
