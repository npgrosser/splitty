//! Opt-in charged snapshots for UI integration tests only.

use super::*;
use splitty_core::{config::settings_metadata, theme::resolve_theme};
use splitty_transport::budget::ConnectionBudget;

#[derive(Clone, Copy)]
pub enum Availability {
    Full,
    MissingMetadata,
    MissingThemes,
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
    };
    let theme = resolve_theme("splitty-dark", std::path::Path::new("."));
    let metadata = serde_json::from_value::<SettingsMetadata>(settings_metadata())
        .expect("test settings metadata");
    let themes = vec!["splitty-dark".to_owned(), "splitty-light".to_owned()];
    Arc::new(AppearanceSnapshot {
        connection_id,
        generation,
        settings: budget.charge(0, false, || settings).expect("test charge"),
        theme: Arc::new(budget.charge(0, false, || theme).expect("test charge")),
        terminal_theme: None,
        terminal_generation: generation,
        terminal_palette_error: None,
        metadata: (!matches!(availability, Availability::MissingMetadata))
            .then(|| budget.charge(0, false, || metadata).expect("test charge")),
        themes: (!matches!(availability, Availability::MissingThemes))
            .then(|| budget.charge(0, false, || themes).expect("test charge")),
        metadata_error: matches!(availability, Availability::MissingMetadata)
            .then(|| "Metadata unavailable".to_owned()),
        themes_error: matches!(availability, Availability::MissingThemes)
            .then(|| "Theme catalog unavailable".to_owned()),
    })
}
