//! Read-only local appearance. Settings remain service-owned; they name the UI
//! theme and terminal palette, which the application resolves in its own catalog.
//!
//! A snapshot owns its decoded allocation leases through presentation. The local
//! reader uses a separate API stream: its request helper must never consume the
//! session stream, where unrelated frames carry terminal output and credits.
use splitty_core::config::{AppearanceSettings, DrawerSettings, RemoteSettings};
use splitty_transport::budget::Charged;
use uuid::Uuid;

use crate::settings::{
    SettingsDomain, SettingsEdit, SettingsField, SettingsLimits, SettingsMetadata, SettingsValue,
};

mod terminal_preferences;
pub use terminal_preferences::TerminalPreferences;

/// The family of the terminal font the web client bundles.
pub const TERMINAL_FONT_FAMILY: &str = "JetBrainsMono Nerd Font Mono";

/// That font's data, compiled in from the web client's source, so the native
/// client needs neither the server's embedded web build nor a font on disk.
pub fn terminal_font_data() -> Option<std::borrow::Cow<'static, [u8]>> {
    Some(std::borrow::Cow::Borrowed(include_bytes!(
        "../../../frontend/public/JetBrainsMonoNerdFontMono-Regular.ttf"
    )))
}

/// The directory of this computer whose files add UI themes and terminal
/// palettes to the bundled ones.
pub fn themes_dir() -> std::path::PathBuf {
    splitty_core::config::themes_dir()
}

pub(crate) mod local;
#[cfg(feature = "ui-test-support")]
#[doc(hidden)]
pub mod test_support;

#[derive(Debug)]
pub(crate) struct HomePresentation {
    pub appearance: AppearanceSettings,
    pub drawer: DrawerSettings,
    pub terminal: TerminalPreferences,
    pub remote: RemoteSettings,
    pub native_tabs: splitty_core::config::NativeTabSettings,
    pub editors: splitty_core::config::EditorSettings,
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
    scheme: ColorScheme,
    terminal_generation: u64,
    metadata: Option<Charged<SettingsMetadata>>,
    metadata_error: Option<String>,
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
    /// The limits of `field`: the server's metadata, else, for a server
    /// without metadata, the client's fallback (`settings::fallback_limits`).
    pub fn limits(&self, field: SettingsField) -> Option<&SettingsLimits> {
        match self.metadata() {
            Some(metadata) => metadata.limits(field),
            None => crate::settings::fallback_limits(field),
        }
    }
    /// The allowed texts of a field of choices, from [`Self::limits`].
    pub fn choices(&self, field: SettingsField) -> Option<&[String]> {
        self.limits(field)
            .map(SettingsLimits::choices)
            .filter(|choices| !choices.is_empty())
    }
    pub fn metadata_error(&self) -> Option<&str> {
        self.metadata_error.as_deref()
    }
    /// Which editor opens what; a preference, never a permission.
    pub fn editors(&self) -> &splitty_core::config::EditorSettings {
        &self.settings.editors
    }
    pub fn terminal_generation(&self) -> u64 {
        self.terminal_generation
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
            F::TerminalThemeDark => V::Text(self.terminal().theme_dark().into()),
            F::TerminalThemeLight => V::Text(self.terminal().theme_light().into()),
            F::CloseOnExit => V::Text(self.terminal().close_on_exit().into()),
            F::ScrollToBottom => V::Bool(self.terminal().scroll_to_bottom()),
            F::DrawerPinned => V::Bool(self.drawer().pinned),
            F::DrawerWidth => V::Number(f64::from(self.drawer().sidebar_width)),
            F::ReconnectHosts => V::List(self.settings.remote.reconnect_hosts.clone()),
            F::AutoForwardPorts => V::Bool(self.settings.remote.auto_forward_ports),
            F::TabsShowSplit => V::Bool(self.settings.native_tabs.show_split),
            F::TabsShowFloat => V::Bool(self.settings.native_tabs.show_float),
            F::TabsShowMaximize => V::Bool(self.settings.native_tabs.show_maximize),
            F::TabsCloseButton => V::Text(self.settings.native_tabs.close_button.clone()),
            F::TabsLocked => V::Bool(self.settings.native_tabs.locked),
            F::EditorFile => V::Text(self.settings.editors.file.clone()),
            F::EditorFolder => V::Text(self.settings.editors.folder.clone()),
            F::NotificationEnabled
            | F::NotificationToasts
            | F::NotificationDesktop
            | F::NotificationProcessFinished
            | F::NotificationProcessExited
            | F::NotificationProcessCrashed
            | F::NotificationProgramStatus => return None,
        })
    }
    pub(crate) fn validate_edit(&self, edit: &SettingsEdit) -> std::io::Result<()> {
        if edit.field().domain() != SettingsDomain::Appearance {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Not an appearance setting",
            ));
        }
        let field = edit.field();
        // A theme name is checked against the application's catalog, which
        // offers the choices; the service stores the name as it is. A field
        // the server may not know is edited only where its metadata names
        // it; ranges and choices are the metadata's, or for a server without
        // metadata the client's fallback.
        let metadata = self.metadata();
        // Without metadata the client cannot tell whether the server knows
        // a newer field, and edits none of them.
        let known = metadata.is_some() || !field.needs_metadata();
        let valid = if field.is_theme() {
            matches!(edit.value(), SettingsValue::Text(_))
                && known
                && metadata.is_none_or(|metadata| metadata.default_value(field).is_some())
        } else if crate::settings::fallback_limits(field).is_some() {
            known
                && self
                    .limits(field)
                    .is_some_and(|limits| limits.accepts(edit.value()))
        } else {
            metadata.is_some_and(|metadata| metadata.default_value(field).is_some())
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
    /// The operating system's preference this snapshot was read for.
    pub fn os_scheme(&self) -> ColorScheme {
        self.scheme
    }
    /// The name of the UI theme the settings select for that preference.
    pub fn ui_theme(&self) -> &str {
        theme_name(self.settings(), self.scheme)
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

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_transport::budget::ConnectionBudget;

    /// A snapshot of default settings with `metadata`, or none.
    fn snapshot(metadata: Option<serde_json::Value>) -> AppearanceSnapshot {
        let budget = ConnectionBudget::default();
        let settings = HomePresentation {
            appearance: AppearanceSettings::default(),
            drawer: DrawerSettings::default(),
            terminal: serde_json::from_value(
                serde_json::json!({"theme": "default", "closeOnExit": "graceful"}),
            )
            .unwrap(),
            remote: Default::default(),
            native_tabs: Default::default(),
            editors: Default::default(),
        };
        let metadata = metadata.map(|value| {
            let metadata = serde_json::from_value::<SettingsMetadata>(value).unwrap();
            budget.charge(0, false, || metadata).unwrap()
        });
        AppearanceSnapshot {
            connection_id: Uuid::new_v4(),
            generation: 1,
            settings: budget.charge(0, false, || settings).unwrap(),
            scheme: ColorScheme::Dark,
            terminal_generation: 1,
            metadata_error: metadata.is_none().then(|| "unavailable".to_owned()),
            metadata,
        }
    }

    fn accepts(snapshot: &AppearanceSnapshot, field: SettingsField, value: SettingsValue) -> bool {
        SettingsEdit::new(field, value)
            .and_then(|edit| snapshot.validate_edit(&edit))
            .is_ok()
    }

    /// settings, Server-specific defaults and limits: a server that widens
    /// a limit or adds an editor is taken at its word.
    #[test]
    fn edits_follow_the_servers_limits() {
        use SettingsField as F;
        use SettingsValue as V;
        let mut metadata = splitty_core::config::settings_metadata();
        metadata["limits"]["appearance.fontSize"] =
            serde_json::json!({"min": 0, "max": 32, "step": 1});
        metadata["limits"]["drawer.sidebarWidth"] =
            serde_json::json!({"min": 100, "max": 600, "step": 10});
        metadata["limits"]["appearance.zoom"] =
            serde_json::json!({"min": 0.25, "max": 4, "step": 0.25});
        metadata["limits"]["editors.file"] =
            serde_json::json!({"values": ["vscode", "zed", "helix"]});
        let wide = snapshot(Some(metadata));
        assert!(accepts(&wide, F::FontSize, V::Number(30.0)));
        assert!(accepts(&wide, F::DrawerWidth, V::Number(500.0)));
        assert!(accepts(&wide, F::Zoom, V::Number(4.0)));
        assert!(accepts(&wide, F::EditorFile, V::Text("helix".into())));
        assert!(!accepts(&wide, F::FontSize, V::Number(33.0)));
        assert!(!accepts(&wide, F::EditorFile, V::Text("emacs".into())));
        assert!(!accepts(&wide, F::FontSize, V::Number(8.5)));

        let current = snapshot(Some(splitty_core::config::settings_metadata()));
        for value in [7.0, 25.0] {
            assert!(!accepts(&current, F::FontSize, V::Number(value)));
        }
        assert!(accepts(&current, F::FontSize, V::Number(0.0)));
        assert!(!accepts(&current, F::EditorFile, V::Text("helix".into())));
    }

    /// settings, Older server without metadata: compatible updates go on
    /// within the fallback limits; fields such a server may not know stay
    /// unedited.
    #[test]
    fn without_metadata_the_fallback_limits_apply() {
        use SettingsField as F;
        use SettingsValue as V;
        let old = snapshot(None);
        assert!(accepts(&old, F::FontSize, V::Number(18.0)));
        assert!(accepts(&old, F::Zoom, V::Number(1.5)));
        assert!(accepts(&old, F::EditorFolder, V::Text("zed".into())));
        assert!(accepts(&old, F::Theme, V::Text("splitty-light".into())));
        assert!(!accepts(&old, F::FontSize, V::Number(30.0)));
        assert!(!accepts(&old, F::EditorFile, V::Text("helix".into())));
        assert!(!accepts(
            &old,
            F::TerminalThemeDark,
            V::Text("splitty-dark".into())
        ));
        assert!(!accepts(&old, F::DrawerPinned, V::Bool(false)));
        assert!(!accepts(&old, F::TabsCloseButton, V::Text("always".into())));
    }
}
