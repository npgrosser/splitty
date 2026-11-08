//! Read-only local appearance. Settings and theme resolution remain service-owned.
//!
//! A snapshot owns its decoded allocation leases through presentation. The local
//! reader uses a separate API stream: its request helper must never consume the
//! session stream, where unrelated frames carry terminal output and credits.
use splitty_core::{
    config::{AppearanceSettings, DrawerSettings},
    theme::ResolvedTheme,
};
use splitty_transport::budget::Charged;
use std::sync::Arc;
use uuid::Uuid;

use crate::settings::{
    SettingsDomain, SettingsEdit, SettingsField, SettingsMetadata, SettingsValue,
};

mod terminal_preferences;
pub use terminal_preferences::TerminalPreferences;

pub(crate) mod local;
#[cfg(feature = "ui-test-support")]
#[doc(hidden)]
pub mod test_support;

#[derive(Debug)]
pub(crate) struct HomePresentation {
    pub appearance: AppearanceSettings,
    pub drawer: DrawerSettings,
    pub terminal: TerminalPreferences,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DrawerVisibilityResult {
    pub connection_id: Uuid,
    pub request_id: Uuid,
    pub outcome: DrawerVisibilityOutcome,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DrawerVisibilityOutcome {
    Confirmed(bool),
    Rejected { reason: String },
    Uncertain { reason: String },
}

impl DrawerVisibilityResult {
    pub(crate) fn rejected(
        connection_id: Uuid,
        request_id: Uuid,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            connection_id,
            request_id,
            outcome: DrawerVisibilityOutcome::Rejected {
                reason: reason.into(),
            },
        }
    }
}

/// The UI supplies the operating system's preference; no platform calls belong here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorScheme {
    Light,
    Dark,
}

/// Share this with `Arc` if several views need it; the decoded values stay charged.
#[derive(Debug)]
pub struct AppearanceSnapshot {
    connection_id: Uuid,
    generation: u64,
    settings: Charged<HomePresentation>,
    theme: Arc<Charged<ResolvedTheme>>,
    terminal_theme: Option<Arc<Charged<ResolvedTheme>>>,
    terminal_generation: u64,
    terminal_palette_error: Option<String>,
    metadata: Option<Charged<SettingsMetadata>>,
    themes: Option<Charged<Vec<String>>>,
    metadata_error: Option<String>,
    themes_error: Option<String>,
}

impl AppearanceSnapshot {
    pub fn connection_id(&self) -> Uuid {
        self.connection_id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn settings(&self) -> &AppearanceSettings {
        &self.settings.appearance
    }
    pub fn drawer(&self) -> &DrawerSettings {
        &self.settings.drawer
    }
    pub fn terminal(&self) -> &TerminalPreferences {
        &self.settings.terminal
    }
    pub fn metadata(&self) -> Option<&SettingsMetadata> {
        self.metadata.as_deref()
    }
    pub fn themes(&self) -> Option<&[String]> {
        self.themes.as_ref().map(|v| v.as_slice())
    }
    pub fn metadata_error(&self) -> Option<&str> {
        self.metadata_error.as_deref()
    }
    pub fn themes_error(&self) -> Option<&str> {
        self.themes_error.as_deref()
    }
    pub fn terminal_theme(&self) -> &ResolvedTheme {
        self.terminal_theme
            .as_deref()
            .map(|t| &**t)
            .unwrap_or(&self.theme)
    }
    pub fn terminal_generation(&self) -> u64 {
        self.terminal_generation
    }
    pub fn terminal_palette_error(&self) -> Option<&str> {
        self.terminal_palette_error.as_deref()
    }
    pub fn value(&self, field: SettingsField) -> Option<SettingsValue> {
        use SettingsField as F;
        use SettingsValue as V;
        let a = self.settings();
        Some(match field {
            F::ThemeMode => V::Text(a.theme_mode.clone()),
            F::Theme => V::Text(a.theme.clone()),
            F::ThemeDark => V::Text(a.theme_dark.clone()),
            F::ThemeLight => V::Text(a.theme_light.clone()),
            F::FontSize => V::Number(f64::from(a.font_size)),
            F::FontFamily => V::Text(a.font_family.clone()),
            F::Zoom => V::Number(a.zoom),
            F::ZoomScope => V::Text(a.zoom_scope.clone()),
            F::TerminalTheme => V::Text(self.terminal().theme().into()),
            F::CloseOnExit => V::Text(self.terminal().close_on_exit().into()),
            F::DrawerPinned => V::Bool(self.drawer().pinned),
            F::DrawerWidth => V::Number(f64::from(self.drawer().sidebar_width)),
            F::NotificationEnabled
            | F::NotificationProcessFinished
            | F::NotificationProcessExited
            | F::NotificationProcessCrashed => return None,
        })
    }
    pub(crate) fn validate_edit(&self, edit: &SettingsEdit) -> std::io::Result<()> {
        if edit.field().domain() != SettingsDomain::Appearance {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Not an appearance setting",
            ));
        }
        let metadata = self
            .metadata()
            .ok_or_else(|| std::io::Error::other("Settings metadata is unavailable"))?;
        let field = edit.field();
        let valid = if field.is_theme() {
            match edit.value() {
                SettingsValue::Text(name)
                    if field == SettingsField::TerminalTheme && name == "default" =>
                {
                    metadata.default_value(field).is_some()
                }
                SettingsValue::Text(name) => {
                    self.themes().is_some_and(|names| names.contains(name))
                }
                _ => false,
            }
        } else if matches!(
            field,
            SettingsField::FontFamily | SettingsField::DrawerPinned
        ) {
            metadata.default_value(field).is_some()
        } else {
            metadata
                .limits(field)
                .is_some_and(|limits| limits.accepts(edit.value()))
        };
        if valid {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Setting choices are unavailable or the value is unsupported",
            ))
        }
    }
    /// The resolved theme variant, which can differ from the OS preference.
    pub fn scheme(&self) -> ColorScheme {
        match self.theme.variant {
            splitty_core::theme::Variant::Light => ColorScheme::Light,
            splitty_core::theme::Variant::Dark => ColorScheme::Dark,
        }
    }
    pub fn theme(&self) -> &ResolvedTheme {
        &self.theme
    }
}

fn theme_name(settings: &AppearanceSettings, scheme: ColorScheme) -> &str {
    if settings.theme_mode == "auto" {
        match scheme {
            ColorScheme::Light => &settings.theme_light,
            ColorScheme::Dark => &settings.theme_dark,
        }
    } else {
        &settings.theme
    }
}
