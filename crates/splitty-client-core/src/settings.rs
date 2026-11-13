//! Bounded editor values projected from the service's existing settings contract.
use serde::Deserialize;
use std::{collections::BTreeMap, io};

/// Independent authoritative read lifecycles; these are not editor sections.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsDomain {
    Appearance,
    Notifications,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SettingsField {
    ThemeMode,
    Theme,
    ThemeDark,
    ThemeLight,
    FontSize,
    FontFamily,
    Zoom,
    ZoomScope,
    TerminalTheme,
    TerminalThemeDark,
    TerminalThemeLight,
    CloseOnExit,
    ScrollToBottom,
    DrawerPinned,
    DrawerWidth,
    ReconnectHosts,
    AutoForwardPorts,
    /// The native app's own tab bars (`nativeTabs`).
    TabsShowSplit,
    TabsShowFloat,
    TabsShowMaximize,
    TabsCloseButton,
    TabsLocked,
    /// The editor a pane's request opens a file or a folder in.
    EditorFile,
    EditorFolder,
    NotificationEnabled,
    NotificationToasts,
    NotificationDesktop,
    NotificationProcessFinished,
    NotificationProcessExited,
    NotificationProcessCrashed,
    /// Program status reports (OSC 7501) of panes the user is not watching.
    NotificationProgramStatus,
}
impl SettingsField {
    pub const ALL: [Self; 31] = [
        Self::ThemeMode,
        Self::Theme,
        Self::ThemeDark,
        Self::ThemeLight,
        Self::FontSize,
        Self::FontFamily,
        Self::Zoom,
        Self::ZoomScope,
        Self::TerminalTheme,
        Self::TerminalThemeDark,
        Self::TerminalThemeLight,
        Self::CloseOnExit,
        Self::ScrollToBottom,
        Self::DrawerPinned,
        Self::DrawerWidth,
        Self::ReconnectHosts,
        Self::AutoForwardPorts,
        Self::TabsShowSplit,
        Self::TabsShowFloat,
        Self::TabsShowMaximize,
        Self::TabsCloseButton,
        Self::TabsLocked,
        Self::EditorFile,
        Self::EditorFolder,
        Self::NotificationEnabled,
        Self::NotificationToasts,
        Self::NotificationDesktop,
        Self::NotificationProcessFinished,
        Self::NotificationProcessExited,
        Self::NotificationProcessCrashed,
        Self::NotificationProgramStatus,
    ];
    pub fn domain(self) -> SettingsDomain {
        match self {
            Self::NotificationEnabled
            | Self::NotificationToasts
            | Self::NotificationDesktop
            | Self::NotificationProcessFinished
            | Self::NotificationProcessExited
            | Self::NotificationProcessCrashed
            | Self::NotificationProgramStatus => SettingsDomain::Notifications,
            _ => SettingsDomain::Appearance,
        }
    }
    pub fn path(self) -> &'static str {
        match self {
            Self::ThemeMode => "appearance.themeMode",
            Self::Theme => "appearance.theme",
            Self::ThemeDark => "appearance.themeDark",
            Self::ThemeLight => "appearance.themeLight",
            Self::FontSize => "appearance.fontSize",
            Self::FontFamily => "appearance.fontFamily",
            Self::Zoom => "appearance.zoom",
            Self::ZoomScope => "appearance.zoomScope",
            Self::TerminalTheme => "terminal.theme",
            Self::TerminalThemeDark => "terminal.themeDark",
            Self::TerminalThemeLight => "terminal.themeLight",
            Self::CloseOnExit => "terminal.closeOnExit",
            Self::ScrollToBottom => "terminal.scrollToBottom",
            Self::DrawerPinned => "drawer.pinned",
            Self::DrawerWidth => "drawer.sidebarWidth",
            Self::ReconnectHosts => "remote.reconnectHosts",
            Self::AutoForwardPorts => "remote.autoForwardPorts",
            Self::TabsShowSplit => "nativeTabs.showSplit",
            Self::TabsShowFloat => "nativeTabs.showFloat",
            Self::TabsShowMaximize => "nativeTabs.showMaximize",
            Self::TabsCloseButton => "nativeTabs.closeButton",
            Self::TabsLocked => "nativeTabs.locked",
            Self::EditorFile => "editors.file",
            Self::EditorFolder => "editors.folder",
            Self::NotificationEnabled => "notifications.enabled",
            Self::NotificationToasts => "notifications.toasts",
            Self::NotificationDesktop => "notifications.desktop",
            Self::NotificationProcessFinished => "notifications.processFinished",
            Self::NotificationProcessExited => "notifications.processExited",
            Self::NotificationProcessCrashed => "notifications.processCrashed",
            Self::NotificationProgramStatus => "notifications.programStatus",
        }
    }
    pub fn is_theme(self) -> bool {
        matches!(
            self,
            Self::Theme
                | Self::ThemeDark
                | Self::ThemeLight
                | Self::TerminalTheme
                | Self::TerminalThemeDark
                | Self::TerminalThemeLight
        )
    }
    /// A terminal palette field, which also accepts `default`.
    pub fn is_terminal_theme(self) -> bool {
        matches!(
            self,
            Self::TerminalTheme | Self::TerminalThemeDark | Self::TerminalThemeLight
        )
    }
    /// A field that is edited only where the server's metadata names its
    /// default, which shows that the server knows it: without metadata the
    /// client cannot tell, and edits nothing of it.
    pub fn needs_metadata(self) -> bool {
        matches!(
            self,
            Self::FontFamily
                | Self::DrawerPinned
                | Self::ScrollToBottom
                | Self::ReconnectHosts
                | Self::AutoForwardPorts
                | Self::TabsShowSplit
                | Self::TabsShowFloat
                | Self::TabsShowMaximize
                | Self::TabsCloseButton
                | Self::TabsLocked
                | Self::TerminalThemeDark
                | Self::TerminalThemeLight
        )
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, serde::Serialize)]
#[serde(untagged)]
pub enum SettingsValue {
    Text(String),
    Number(f64),
    Bool(bool),
    /// A list of plain names, as the hosts to reconnect at start.
    List(Vec<String>),
}

/// The largest number a settings value may have before the server's limits
/// or the fallback ones are asked.
const MAX_SETTING_NUMBER: f64 = 1_000_000.0;
/// The longest choice a value or a metadata entry may have.
const MAX_CHOICE_BYTES: usize = 128;

/// One explicit field intent. Construction admits only values of the field's
/// shape, bounded; the server's metadata, or the fallback limits for a
/// server without it, decide the range (`AppearanceSnapshot::validate_edit`),
/// and the service remains authoritative.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsEdit {
    field: SettingsField,
    value: SettingsValue,
}
impl SettingsEdit {
    pub fn new(field: SettingsField, value: SettingsValue) -> io::Result<Self> {
        use SettingsField as F;
        let valid = match (&value, field) {
            (
                SettingsValue::Bool(_),
                F::DrawerPinned
                | F::ScrollToBottom
                | F::AutoForwardPorts
                | F::TabsShowSplit
                | F::TabsShowFloat
                | F::TabsShowMaximize
                | F::TabsLocked
                | F::NotificationEnabled
                | F::NotificationToasts
                | F::NotificationDesktop
                | F::NotificationProcessFinished
                | F::NotificationProcessExited
                | F::NotificationProcessCrashed
                | F::NotificationProgramStatus,
            ) => true,
            (SettingsValue::List(hosts), F::ReconnectHosts) => {
                hosts.len() <= 64
                    && hosts.iter().enumerate().all(|(index, host)| {
                        !host.is_empty()
                            && host.chars().count() <= 256
                            && !host.starts_with('-')
                            && !host.chars().any(|c| c.is_control() || c.is_whitespace())
                            && !hosts[..index].contains(host)
                    })
            }
            // Ranges and choices are the server's (`SettingsMetadata`), or
            // the fallback for a server without metadata (`fallback_limits`);
            // a value only has the field's shape here.
            (SettingsValue::Number(n), F::FontSize | F::DrawerWidth) => {
                n.fract() == 0.0 && (0.0..=MAX_SETTING_NUMBER).contains(n)
            }
            (SettingsValue::Number(n), F::Zoom) => {
                n.is_finite() && (0.0..=MAX_SETTING_NUMBER).contains(n)
            }
            (
                SettingsValue::Text(s),
                F::ThemeMode
                | F::ZoomScope
                | F::TabsCloseButton
                | F::EditorFile
                | F::EditorFolder
                | F::CloseOnExit,
            ) => !s.is_empty() && s.len() <= MAX_CHOICE_BYTES && !s.chars().any(char::is_control),
            (SettingsValue::Text(s), F::FontFamily) => {
                s.len() <= 4096 && !s.chars().any(char::is_control)
            }
            (SettingsValue::Text(s), f) if f.is_theme() => {
                !s.is_empty() && s.len() <= 64 && !s.chars().any(char::is_control)
            }
            _ => false,
        };
        if !valid {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported native settings value",
            ));
        }
        Ok(Self { field, value })
    }
    pub fn field(&self) -> SettingsField {
        self.field
    }
    pub fn value(&self) -> &SettingsValue {
        &self.value
    }
    pub(crate) fn args(&self) -> serde_json::Map<String, serde_json::Value> {
        let (group, key) = self
            .field
            .path()
            .split_once('.')
            .expect("known setting path");
        let value = match (&self.value, self.field) {
            // These fields are wire integers, not integral JSON floats.
            (SettingsValue::Number(n), SettingsField::FontSize | SettingsField::DrawerWidth) => {
                serde_json::from_str(&format!("{n:.0}")).expect("bounded integer")
            }
            _ => serde_json::to_value(&self.value).expect("bounded setting value"),
        };
        serde_json::json!({"body": {group: {key: value}}})
            .as_object()
            .expect("object literal")
            .clone()
    }
}

#[derive(Debug, Deserialize)]
pub struct SettingsMetadata {
    defaults: serde_json::Value,
    limits: BTreeMap<String, SettingsLimits>,
}
impl SettingsMetadata {
    pub fn default_value(&self, field: SettingsField) -> Option<SettingsValue> {
        let (group, key) = field.path().split_once('.')?;
        let value: SettingsValue =
            serde_json::from_value(self.defaults.get(group)?.get(key)?.clone()).ok()?;
        SettingsEdit::new(field, value.clone()).ok()?;
        Some(value)
    }
    /// The metadata names no default for `field` at all, as the metadata of
    /// a server that does not know the field.
    pub(crate) fn lacks_default(&self, field: SettingsField) -> bool {
        field.path().split_once('.').is_some_and(|(group, key)| {
            self.defaults
                .get(group)
                .is_some_and(|group| group.is_object() && group.get(key).is_none())
        })
    }
    pub fn choices(&self, field: SettingsField) -> Option<&[String]> {
        let choices = &self.limits.get(field.path())?.choices;
        (!choices.is_empty()).then_some(choices)
    }
    pub fn limits(&self, field: SettingsField) -> Option<&SettingsLimits> {
        self.limits.get(field.path())
    }
    pub(crate) fn validate(&self) -> io::Result<()> {
        if self.limits.len() > 128
            || self.limits.iter().any(|(key, limit)| {
                key.len() > 128
                    || limit.choices.len() + limit.values.len() > 128
                    || limit.choices.iter().any(|v| v.len() > 128)
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "settings metadata exceeds native limit",
            ));
        }
        Ok(())
    }
}

/// The limits of a server without settings metadata: the ranges and choices
/// such a server takes. A server with metadata is asked instead, so that a
/// limit it widens or an editor it adds is taken as it is.
pub fn fallback_limits(field: SettingsField) -> Option<&'static SettingsLimits> {
    use SettingsField as F;
    static LIMITS: std::sync::LazyLock<BTreeMap<SettingsField, SettingsLimits>> =
        std::sync::LazyLock::new(|| {
            let range = |min: f64, max: f64, step: f64, values: Vec<f64>| SettingsLimits {
                min: Some(min),
                max: Some(max),
                step: Some(step),
                values,
                choices: Vec::new(),
            };
            let choices = |values: &[&str]| SettingsLimits {
                min: None,
                max: None,
                step: None,
                values: Vec::new(),
                choices: values.iter().map(|value| (*value).to_owned()).collect(),
            };
            let font_sizes = std::iter::once(0.0)
                .chain((8..=24).map(f64::from))
                .collect();
            BTreeMap::from([
                (F::FontSize, range(0.0, 24.0, 1.0, font_sizes)),
                // The levels this build's server lists; such a server took
                // any level in their range, and its windows offered these.
                (
                    F::Zoom,
                    range(0.5, 3.0, 0.1, splitty_core::config::ZOOM_LEVELS.to_vec()),
                ),
                (F::DrawerWidth, range(150.0, 400.0, 10.0, Vec::new())),
                (F::ThemeMode, choices(&["fixed", "auto"])),
                (F::ZoomScope, choices(&["window", "terminals"])),
                (F::TabsCloseButton, choices(&["always", "active"])),
                (F::EditorFile, choices(&["vscode", "zed"])),
                (F::EditorFolder, choices(&["vscode", "zed"])),
                (F::CloseOnExit, choices(&["always", "never", "graceful"])),
            ])
        });
    LIMITS.get(&field)
}
#[derive(Debug)]
pub struct SettingsLimits {
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    values: Vec<f64>,
    choices: Vec<String>,
}
impl SettingsLimits {
    pub fn min(&self) -> Option<f64> {
        self.min
    }
    pub fn max(&self) -> Option<f64> {
        self.max
    }
    pub fn step(&self) -> Option<f64> {
        self.step
    }
    pub fn values(&self) -> &[f64] {
        &self.values
    }
    /// The allowed texts of a field of choices.
    pub fn choices(&self) -> &[String] {
        &self.choices
    }
    pub(crate) fn accepts(&self, value: &SettingsValue) -> bool {
        match value {
            SettingsValue::Text(s) => !self.choices.is_empty() && self.choices.contains(s),
            SettingsValue::Number(n) => {
                self.min.is_some_and(|min| *n >= min)
                    && self.max.is_some_and(|max| *n <= max)
                    && (self.values.is_empty() || self.values.contains(n))
            }
            SettingsValue::Bool(_) | SettingsValue::List(_) => false,
        }
    }
}
impl<'de> Deserialize<'de> for SettingsLimits {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            min: Option<f64>,
            max: Option<f64>,
            step: Option<f64>,
            #[serde(default)]
            values: Vec<SettingsValue>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let mut result = Self {
            min: raw.min,
            max: raw.max,
            step: raw.step,
            values: Vec::new(),
            choices: Vec::new(),
        };
        for value in raw.values {
            match value {
                SettingsValue::Text(s) => result.choices.push(s),
                SettingsValue::Number(n) => result.values.push(n),
                SettingsValue::Bool(_) | SettingsValue::List(_) => {}
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_edits_are_boolean_fields_in_their_own_domain() {
        use SettingsField as F;
        let fields = [
            (F::NotificationEnabled, "enabled"),
            (F::NotificationToasts, "toasts"),
            (F::NotificationDesktop, "desktop"),
            (F::NotificationProcessFinished, "processFinished"),
            (F::NotificationProcessExited, "processExited"),
            (F::NotificationProcessCrashed, "processCrashed"),
            (F::NotificationProgramStatus, "programStatus"),
        ];
        for (field, key) in fields {
            assert_eq!(field.domain(), SettingsDomain::Notifications);
            assert_eq!(field.path(), format!("notifications.{key}"));
            for enabled in [true, false] {
                let edit = SettingsEdit::new(field, SettingsValue::Bool(enabled)).unwrap();
                assert_eq!(
                    edit.args(),
                    serde_json::json!({"body":{"notifications":{key:enabled}}})
                        .as_object()
                        .unwrap()
                        .clone()
                );
            }
            for wrong in [
                SettingsValue::Text("true".into()),
                SettingsValue::Number(1.0),
            ] {
                assert!(SettingsEdit::new(field, wrong).is_err());
            }
        }
        assert_eq!(
            F::ALL
                .into_iter()
                .filter(|field| field.domain() == SettingsDomain::Appearance)
                .count(),
            24
        );
    }
}
