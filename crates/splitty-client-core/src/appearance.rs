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
use uuid::Uuid;

pub(crate) mod local;

#[derive(Debug)]
pub(crate) struct HomePresentation {
    pub appearance: AppearanceSettings,
    pub drawer: DrawerSettings,
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
    theme: Charged<ResolvedTheme>,
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
