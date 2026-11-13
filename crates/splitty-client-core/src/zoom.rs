//! Window-local zoom intents using the existing display-zoom settings contract.
//! The levels are the service's (`ZoomSteps`), from its settings metadata.
use crate::settings::{SettingsField, SettingsLimits, SettingsMetadata};
use std::collections::HashSet;
use uuid::Uuid;

/// The zoom levels of a service, ascending: the `values` its settings
/// metadata lists for `appearance.zoom`. For a service that lists none, an
/// older one, the levels of this build's server core within that service's
/// bounds, which are the levels such a service's windows stepped through.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoomSteps(Vec<f64>);
impl ZoomSteps {
    /// The levels of `limits`, the limits of `appearance.zoom`.
    pub fn from_limits(limits: Option<&SettingsLimits>) -> Self {
        let listed = limits.map_or(&[][..], SettingsLimits::values);
        let mut levels: Vec<f64> = listed
            .iter()
            .copied()
            .filter(|level| level.is_finite() && *level > 0.)
            .collect();
        if levels.is_empty() {
            let min = limits.and_then(SettingsLimits::min).unwrap_or(f64::MIN);
            let max = limits.and_then(SettingsLimits::max).unwrap_or(f64::MAX);
            levels = splitty_core::config::ZOOM_LEVELS
                .into_iter()
                .filter(|level| (min..=max).contains(level))
                .collect();
        }
        levels.sort_by(f64::total_cmp);
        levels.dedup();
        if levels.is_empty() {
            // Bounds that hold none of the levels leave only the default.
            levels.push(1.);
        }
        Self(levels)
    }
    /// The levels of a service with `metadata`, or of one without metadata.
    pub fn from_metadata(metadata: Option<&SettingsMetadata>) -> Self {
        Self::from_limits(match metadata {
            Some(metadata) => metadata.limits(SettingsField::Zoom),
            None => crate::settings::fallback_limits(SettingsField::Zoom),
        })
    }
    pub fn levels(&self) -> &[f64] {
        &self.0
    }
    /// A level within the lowest and the highest step; one between two
    /// steps, stored by hand, is shown as it is.
    pub fn admits(&self, level: f64) -> bool {
        level.is_finite() && self.0[0] <= level && level <= self.0[self.0.len() - 1]
    }
}
impl Default for ZoomSteps {
    /// The levels of a service whose metadata names no zoom limits.
    fn default() -> Self {
        Self::from_limits(None)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZoomAction {
    In,
    Out,
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    level: f64,
    terminals_only: bool,
}
impl Default for Zoom {
    /// 100 % of the whole window, the settings' default.
    fn default() -> Self {
        Self {
            level: 1.,
            terminals_only: false,
        }
    }
}
impl Zoom {
    /// The stored level and scope; a level outside `steps` is 100 %.
    pub fn from_values(level: f64, scope: &str, steps: &ZoomSteps) -> Self {
        Self {
            level: if steps.admits(level) { level } else { 1.0 },
            terminals_only: scope == "terminals",
        }
    }
    pub fn level(self) -> f64 {
        self.level
    }
    pub fn window_scale(self) -> f32 {
        if self.terminals_only {
            1.0
        } else {
            self.level as f32
        }
    }
    /// The factor a terminal's font size `base_font_size` is scaled by: the
    /// level, and with the terminals scope the factor that rounds the size
    /// to whole pixels, as every client rounds it there. The window scope
    /// scales the whole window, unrounded.
    pub fn terminal_scale(self, base_font_size: f32) -> f32 {
        let level = self.level as f32;
        if self.terminals_only && base_font_size > 0. {
            (base_font_size * level).round().max(1.) / base_font_size
        } else {
            level
        }
    }
    /// The level one action leads to from this one, in the service's steps.
    pub fn apply(self, action: ZoomAction, steps: &ZoomSteps) -> Self {
        let levels = steps.levels();
        let level = match action {
            ZoomAction::In => levels
                .iter()
                .copied()
                .find(|step| *step > self.level + 0.001)
                .unwrap_or(levels[levels.len() - 1]),
            ZoomAction::Out => levels
                .iter()
                .rev()
                .copied()
                .find(|step| *step < self.level - 0.001)
                .unwrap_or(levels[0]),
            ZoomAction::Reset => 1.0,
        };
        Self { level, ..self }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct ZoomResult {
    pub connection_id: Uuid,
    pub request_id: Uuid,
    pub outcome: ZoomOutcome,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ZoomOutcome {
    Confirmed(Zoom),
    Rejected { reason: String },
    Uncertain { reason: String },
}
impl ZoomResult {
    pub(crate) fn rejected(
        connection_id: Uuid,
        request_id: Uuid,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            connection_id,
            request_id,
            outcome: ZoomOutcome::Rejected {
                reason: reason.into(),
            },
        }
    }
}

/// The zoom a window shows ahead of the service's answer. An admitted action
/// is shown at once; the service's value replaces what is shown only when no
/// action is outstanding any more, so that a burst of steps never moves the
/// display backwards, and it ends at what the service holds.
#[derive(Debug)]
pub struct PresentedZoom {
    presented: Zoom,
    confirmed: Zoom,
    outstanding: HashSet<Uuid>,
}
impl PresentedZoom {
    pub fn new(zoom: Zoom) -> Self {
        Self {
            presented: zoom,
            confirmed: zoom,
            outstanding: HashSet::new(),
        }
    }
    pub fn presented(&self) -> Zoom {
        self.presented
    }
    /// An action that was sent: its step from the zoom shown, in the
    /// service's `steps`, shown now.
    pub fn step(&mut self, request: Uuid, action: ZoomAction, steps: &ZoomSteps) -> Zoom {
        self.outstanding.insert(request);
        self.presented = self.presented.apply(action, steps);
        self.presented
    }
    /// The outcome of an action. A confirmed value is the service's; after a
    /// rejected or uncertain write the last confirmed one stands. `None` for
    /// a request that is not outstanding.
    pub fn settle(&mut self, request: Uuid, outcome: &ZoomOutcome) -> Option<Zoom> {
        if !self.outstanding.remove(&request) {
            return None;
        }
        if let ZoomOutcome::Confirmed(zoom) = outcome {
            self.confirmed = *zoom;
        }
        Some(self.reconcile())
    }
    /// What a settings read found the service to hold.
    pub fn observe(&mut self, zoom: Zoom) -> Zoom {
        self.confirmed = zoom;
        self.reconcile()
    }
    fn reconcile(&mut self) -> Zoom {
        if self.outstanding.is_empty() {
            self.presented = self.confirmed;
        }
        self.presented
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_core::config::ZOOM_LEVELS as STEPS;

    /// The levels of a service with this metadata's zoom limits.
    fn steps(limit: serde_json::Value) -> ZoomSteps {
        let mut metadata = splitty_core::config::settings_metadata();
        metadata["limits"]["appearance.zoom"] = limit;
        let metadata: SettingsMetadata = serde_json::from_value(metadata).unwrap();
        ZoomSteps::from_metadata(Some(&metadata))
    }

    #[test]
    fn a_step_shows_at_once_and_the_service_value_stands_when_none_is_outstanding() {
        let steps = ZoomSteps::default();
        let at = |level| Zoom::from_values(level, "window", &steps);
        let confirmed = |level| ZoomOutcome::Confirmed(at(level));
        let mut zoom = PresentedZoom::new(at(1.0));
        let ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        // A burst: every step is shown from the one before it.
        assert_eq!(zoom.step(ids[0], ZoomAction::In, &steps), at(1.1));
        assert_eq!(zoom.step(ids[1], ZoomAction::In, &steps), at(1.25));
        assert_eq!(zoom.step(ids[2], ZoomAction::In, &steps), at(1.5));
        // Earlier answers and reads do not move the display backwards.
        assert_eq!(zoom.settle(ids[0], &confirmed(1.1)), Some(at(1.5)));
        assert_eq!(zoom.observe(at(1.1)), at(1.5));
        assert_eq!(zoom.settle(ids[0], &confirmed(1.1)), None, "settled once");
        assert_eq!(zoom.settle(Uuid::new_v4(), &confirmed(3.0)), None);
        assert_eq!(zoom.settle(ids[1], &confirmed(1.5)), Some(at(1.5)));
        // The last answer is what the service holds, whatever was shown.
        assert_eq!(zoom.settle(ids[2], &confirmed(1.25)), Some(at(1.25)));
        assert_eq!(zoom.presented(), at(1.25));

        // A write that failed or whose outcome is unknown: the last confirmed
        // value is shown again, and a read corrects it.
        for outcome in [
            ZoomOutcome::Rejected {
                reason: "full".into(),
            },
            ZoomOutcome::Uncertain {
                reason: "lost".into(),
            },
        ] {
            let id = Uuid::new_v4();
            assert_eq!(zoom.step(id, ZoomAction::Out, &steps), at(1.1));
            assert_eq!(zoom.settle(id, &outcome), Some(at(1.25)));
        }
        assert_eq!(zoom.observe(at(1.1)), at(1.1));

        // A failure among steps that are still outstanding waits for them.
        let (failed, last) = (Uuid::new_v4(), Uuid::new_v4());
        zoom.step(failed, ZoomAction::In, &steps);
        assert_eq!(zoom.step(last, ZoomAction::In, &steps), at(1.5));
        let rejected = ZoomOutcome::Rejected {
            reason: "full".into(),
        };
        assert_eq!(zoom.settle(failed, &rejected), Some(at(1.5)));
        assert_eq!(zoom.settle(last, &confirmed(1.25)), Some(at(1.25)));
        // Reset is a step like the others; the scope shown is kept.
        let mut zoom = PresentedZoom::new(Zoom::from_values(2.0, "terminals", &steps));
        assert_eq!(
            zoom.step(Uuid::new_v4(), ZoomAction::Reset, &steps),
            Zoom::from_values(1.0, "terminals", &steps)
        );
    }
    #[test]
    fn steps_scopes_defaults_and_saturation_match_display_contract() {
        // display-zoom, Older local service: without listed levels, with or
        // without metadata, the steps are those of earlier windows.
        let older = steps(serde_json::json!({"min": 0.5, "max": 3, "step": 0.1}));
        assert_eq!(older.levels(), STEPS);
        assert_eq!(ZoomSteps::from_metadata(None).levels(), STEPS);
        assert_eq!(ZoomSteps::default().levels(), STEPS);
        let current = ZoomSteps::from_metadata(Some(
            &serde_json::from_value(splitty_core::config::settings_metadata()).unwrap(),
        ));
        assert_eq!(current.levels(), STEPS);
        for level in [f64::NAN, f64::INFINITY, 0.49, 3.01] {
            assert_eq!(
                Zoom::from_values(level, "unknown", &current),
                Zoom::from_values(1.0, "window", &current)
            );
        }
        assert_eq!(Zoom::default(), Zoom::from_values(1.0, "window", &current));
        for (index, level) in STEPS.into_iter().enumerate() {
            let zoom = Zoom::from_values(level, "window", &current);
            assert_eq!(zoom.window_scale(), level as f32);
            assert_eq!(zoom.terminal_scale(14.), level as f32);
            assert_eq!(
                zoom.apply(ZoomAction::In, &current).level(),
                STEPS[(index + 1).min(12)]
            );
            assert_eq!(
                zoom.apply(ZoomAction::Out, &current).level(),
                STEPS[index.saturating_sub(1)]
            );
            assert_eq!(zoom.apply(ZoomAction::Reset, &current).level(), 1.0);
        }
        // display-zoom, Level between the listed ones.
        let between = Zoom::from_values(1.17, "window", &current);
        assert_eq!(between.level(), 1.17);
        assert_eq!(between.apply(ZoomAction::In, &current).level(), 1.25);
        assert_eq!(between.apply(ZoomAction::Out, &current).level(), 1.1);
        assert_eq!(
            Zoom::from_values(1.10001, "window", &current)
                .apply(ZoomAction::Out, &current)
                .level(),
            1.0
        );
    }

    /// display-zoom, Levels from the server: a service that widens the
    /// levels is stepped through and shown as it lists them.
    #[test]
    fn the_services_levels_bound_steps_and_display() {
        let wide =
            steps(serde_json::json!({"min": 0.25, "max": 4, "values": [2, 0.25, 1, 4, 0.5, 1]}));
        assert_eq!(wide.levels(), [0.25, 0.5, 1., 2., 4.]);
        let at = |level| Zoom::from_values(level, "window", &wide);
        assert_eq!(at(4.).level(), 4.);
        assert_eq!(at(2.).apply(ZoomAction::In, &wide), at(4.));
        assert_eq!(at(4.).apply(ZoomAction::In, &wide), at(4.));
        assert_eq!(at(0.25).apply(ZoomAction::Out, &wide), at(0.25));
        assert_eq!(at(4.5).level(), 1.);
        // An older service's bounds narrow the earlier levels.
        let narrow = steps(serde_json::json!({"min": 0.75, "max": 2, "step": 0.1}));
        assert_eq!(
            narrow.levels(),
            [0.75, 0.8, 0.9, 1., 1.1, 1.25, 1.5, 1.75, 2.]
        );
        let none = steps(serde_json::json!({"min": 5, "max": 6, "values": [-1, 0]}));
        assert_eq!(none.levels(), [1.]);
    }

    /// display-zoom, Rounded terminal font size: with the terminals scope
    /// the size is whole pixels, with the window scope the level as it is.
    #[test]
    fn the_terminal_font_size_is_rounded_with_the_terminals_scope() {
        let steps = ZoomSteps::default();
        let size = |level, scope| 14. * Zoom::from_values(level, scope, &steps).terminal_scale(14.);
        for (level, expected) in [(1.1, 15.), (1.5, 21.), (0.67, 9.), (1.75, 25.)] {
            assert!(
                (size(level, "terminals") - expected).abs() < 1e-4,
                "{level}"
            );
        }
        assert_eq!(
            Zoom::from_values(1.1, "terminals", &steps).window_scale(),
            1.
        );
        let window = Zoom::from_values(1.1, "window", &steps);
        assert_eq!(window.terminal_scale(14.), 1.1);
    }
}
