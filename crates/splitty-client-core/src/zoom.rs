//! Window-local zoom intents using the existing display-zoom settings contract.
use std::collections::HashSet;
use uuid::Uuid;

const STEPS: [f64; 13] = [
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

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
impl Zoom {
    pub fn from_values(level: f64, scope: &str) -> Self {
        Self {
            level: if level.is_finite() && (0.5..=3.0).contains(&level) {
                level
            } else {
                1.0
            },
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
            self.terminal_scale()
        }
    }
    pub fn terminal_scale(self) -> f32 {
        self.level as f32
    }
    /// The level one action leads to from this one, in the display steps.
    pub fn apply(self, action: ZoomAction) -> Self {
        let level = match action {
            ZoomAction::In => STEPS
                .into_iter()
                .find(|step| *step > self.level + 0.001)
                .unwrap_or(3.0),
            ZoomAction::Out => STEPS
                .into_iter()
                .rev()
                .find(|step| *step < self.level - 0.001)
                .unwrap_or(0.5),
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
    /// An action that was sent: its step from the zoom shown, shown now.
    pub fn step(&mut self, request: Uuid, action: ZoomAction) -> Zoom {
        self.outstanding.insert(request);
        self.presented = self.presented.apply(action);
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

    #[test]
    fn a_step_shows_at_once_and_the_service_value_stands_when_none_is_outstanding() {
        let at = |level| Zoom::from_values(level, "window");
        let confirmed = |level| ZoomOutcome::Confirmed(at(level));
        let mut zoom = PresentedZoom::new(at(1.0));
        let ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        // A burst: every step is shown from the one before it.
        assert_eq!(zoom.step(ids[0], ZoomAction::In), at(1.1));
        assert_eq!(zoom.step(ids[1], ZoomAction::In), at(1.25));
        assert_eq!(zoom.step(ids[2], ZoomAction::In), at(1.5));
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
            assert_eq!(zoom.step(id, ZoomAction::Out), at(1.1));
            assert_eq!(zoom.settle(id, &outcome), Some(at(1.25)));
        }
        assert_eq!(zoom.observe(at(1.1)), at(1.1));

        // A failure among steps that are still outstanding waits for them.
        let (failed, last) = (Uuid::new_v4(), Uuid::new_v4());
        zoom.step(failed, ZoomAction::In);
        assert_eq!(zoom.step(last, ZoomAction::In), at(1.5));
        let rejected = ZoomOutcome::Rejected {
            reason: "full".into(),
        };
        assert_eq!(zoom.settle(failed, &rejected), Some(at(1.5)));
        assert_eq!(zoom.settle(last, &confirmed(1.25)), Some(at(1.25)));
        // Reset is a step like the others; the scope shown is kept.
        let mut zoom = PresentedZoom::new(Zoom::from_values(2.0, "terminals"));
        assert_eq!(
            zoom.step(Uuid::new_v4(), ZoomAction::Reset),
            Zoom::from_values(1.0, "terminals")
        );
    }
    #[test]
    fn steps_scopes_defaults_and_saturation_match_display_contract() {
        for level in [f64::NAN, f64::INFINITY, 0.49, 3.01] {
            assert_eq!(
                Zoom::from_values(level, "unknown"),
                Zoom::from_values(1.0, "window")
            );
        }
        for (index, level) in STEPS.into_iter().enumerate() {
            let zoom = Zoom::from_values(level, "terminals");
            assert_eq!(zoom.window_scale(), 1.0);
            assert_eq!(zoom.terminal_scale(), level as f32);
            assert_eq!(
                zoom.apply(ZoomAction::In).level(),
                STEPS[(index + 1).min(12)]
            );
            assert_eq!(
                zoom.apply(ZoomAction::Out).level(),
                STEPS[index.saturating_sub(1)]
            );
            assert_eq!(zoom.apply(ZoomAction::Reset).level(), 1.0);
        }
        assert_eq!(
            Zoom::from_values(1.17, "window")
                .apply(ZoomAction::In)
                .level(),
            1.25
        );
        assert_eq!(
            Zoom::from_values(1.17, "window")
                .apply(ZoomAction::Out)
                .level(),
            1.1
        );
        assert_eq!(
            Zoom::from_values(1.10001, "window")
                .apply(ZoomAction::Out)
                .level(),
            1.0
        );
    }
}
