//! Window-local zoom intents using the existing display-zoom settings contract.
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
    pub(crate) fn apply(self, action: ZoomAction) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;
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
