//! Bounded editor values projected from the service's existing settings contract.
use serde::Deserialize;
use std::{collections::BTreeMap, io};

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
    CloseOnExit,
    DrawerPinned,
    DrawerWidth,
}
impl SettingsField {
    pub const ALL: [Self; 12] = [
        Self::ThemeMode,
        Self::Theme,
        Self::ThemeDark,
        Self::ThemeLight,
        Self::FontSize,
        Self::FontFamily,
        Self::Zoom,
        Self::ZoomScope,
        Self::TerminalTheme,
        Self::CloseOnExit,
        Self::DrawerPinned,
        Self::DrawerWidth,
    ];
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
            Self::CloseOnExit => "terminal.closeOnExit",
            Self::DrawerPinned => "drawer.pinned",
            Self::DrawerWidth => "drawer.sidebarWidth",
        }
    }
    pub(crate) fn is_theme(self) -> bool {
        matches!(
            self,
            Self::Theme | Self::ThemeDark | Self::ThemeLight | Self::TerminalTheme
        )
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, serde::Serialize)]
#[serde(untagged)]
pub enum SettingsValue {
    Text(String),
    Number(f64),
    Bool(bool),
}

/// One explicit field intent. Construction admits only bounded native values;
/// published metadata/catalog and the service remain authoritative.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsEdit {
    field: SettingsField,
    value: SettingsValue,
}
impl SettingsEdit {
    pub fn new(field: SettingsField, value: SettingsValue) -> io::Result<Self> {
        use SettingsField as F;
        let valid = match (&value, field) {
            (SettingsValue::Bool(_), F::DrawerPinned) => true,
            (SettingsValue::Number(n), F::FontSize) => {
                *n == 0.0 || (n.fract() == 0.0 && (8.0..=24.0).contains(n))
            }
            (SettingsValue::Number(n), F::Zoom) => n.is_finite() && (0.5..=3.0).contains(n),
            (SettingsValue::Number(n), F::DrawerWidth) => {
                n.fract() == 0.0 && (150.0..=400.0).contains(n)
            }
            (SettingsValue::Text(s), F::ThemeMode) => matches!(s.as_str(), "auto" | "fixed"),
            (SettingsValue::Text(s), F::ZoomScope) => matches!(s.as_str(), "window" | "terminals"),
            (SettingsValue::Text(s), F::CloseOnExit) => {
                matches!(s.as_str(), "always" | "never" | "graceful")
            }
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
    pub(crate) fn accepts(&self, value: &SettingsValue) -> bool {
        match value {
            SettingsValue::Text(s) => !self.choices.is_empty() && self.choices.contains(s),
            SettingsValue::Number(n) => {
                self.min.is_some_and(|min| *n >= min)
                    && self.max.is_some_and(|max| *n <= max)
                    && (self.values.is_empty() || self.values.contains(n))
            }
            SettingsValue::Bool(_) => false,
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
                SettingsValue::Bool(_) => {}
            }
        }
        Ok(result)
    }
}
