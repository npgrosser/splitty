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
    let metadata = (!matches!(availability, Availability::MissingMetadata)).then(settings_metadata);
    snapshot(connection_id, generation, metadata, pinned)
}

/// The settings metadata this build's server publishes, to change in a test.
pub fn server_metadata() -> serde_json::Value {
    settings_metadata()
}

/// A snapshot with this metadata, as from a server whose limits differ.
pub fn settings_snapshot_with_metadata(
    connection_id: Uuid,
    generation: u64,
    metadata: serde_json::Value,
) -> Arc<AppearanceSnapshot> {
    snapshot(connection_id, generation, Some(metadata), true)
}

fn snapshot(
    connection_id: Uuid,
    generation: u64,
    metadata: Option<serde_json::Value>,
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
    let metadata_error = metadata
        .is_none()
        .then(|| "Metadata unavailable".to_owned());
    let metadata = metadata.map(|metadata| {
        let metadata =
            serde_json::from_value::<SettingsMetadata>(metadata).expect("test settings metadata");
        budget.charge(0, false, || metadata).expect("test charge")
    });
    Arc::new(AppearanceSnapshot {
        connection_id,
        generation,
        settings: budget.charge(0, false, || settings).expect("test charge"),
        scheme: ColorScheme::Dark,
        terminal_generation: generation,
        metadata,
        metadata_error,
    })
}
