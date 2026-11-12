//! Opt-in charged snapshots for UI integration tests only.

use super::*;
use crate::settings::SettingsMetadata;
use splitty_core::config::settings_metadata;
use splitty_transport::budget::ConnectionBudget;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub enum Availability {
    Full,
    MissingMetadata,
}

/// Build the same presentation type the owned local reader publishes, with
/// a real charged lease and unchanged confirmed settings across failure cases.
pub fn settings_snapshot(
    connection_id: Uuid,
    generation: u64,
    availability: Availability,
) -> Arc<AppearanceSnapshot> {
    settings_snapshot_with_pin(connection_id, generation, availability, true)
}

/// Keep the charged settings shape while varying the authoritative Drawer pin.
pub fn settings_snapshot_with_pin(
    connection_id: Uuid,
    generation: u64,
    availability: Availability,
    pinned: bool,
) -> Arc<AppearanceSnapshot> {
    let budget = ConnectionBudget::default();
    let drawer = DrawerSettings {
        pinned,
        ..DrawerSettings::default()
    };
    let settings = HomePresentation {
        appearance: AppearanceSettings::default(),
        drawer,
        terminal: serde_json::from_value(serde_json::json!({
            "theme": "default", "closeOnExit": "graceful"
        }))
        .expect("test terminal preferences"),
        remote: Default::default(),
        native_tabs: Default::default(),
        editors: Default::default(),
    };
    let metadata = serde_json::from_value::<SettingsMetadata>(settings_metadata())
        .expect("test settings metadata");
    Arc::new(AppearanceSnapshot {
        connection_id,
        generation,
        settings: budget.charge(0, false, || settings).expect("test charge"),
        scheme: ColorScheme::Dark,
        terminal_generation: generation,
        metadata: (!matches!(availability, Availability::MissingMetadata))
            .then(|| budget.charge(0, false, || metadata).expect("test charge")),
        metadata_error: matches!(availability, Availability::MissingMetadata)
            .then(|| "Metadata unavailable".to_owned()),
    })
}
