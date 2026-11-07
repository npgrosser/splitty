//! Terminal-local link gesture state. Only the engine resolves cells; only the
//! client policy decides whether an owned hit may reach the platform opener.
use crate::terminal_engine::LinkHit;
use splitty_client_core::external_links::WebUrl;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Cell {
    pub col: u16,
    pub row: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub hit: LinkHit,
    pub url: WebUrl,
}

impl Target {
    pub fn allowed(hit: LinkHit) -> Option<Self> {
        Some(Self {
            url: WebUrl::parse(&hit.uri)?,
            hit,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Geometry {
    pub origin: (f32, f32),
    pub extent: (f32, f32),
    pub cell_width: f32,
    pub scale: f32,
    pub size: (u16, u16),
}

struct Hover {
    cell: Cell,
    revision: u64,
    target: Option<Target>,
}

struct Press {
    cell: Cell,
    target: Target,
    position: (f32, f32),
    generation: u64,
    dragged: bool,
}

#[derive(Default)]
pub(super) struct State {
    revision: u64,
    geometry: Option<Geometry>,
    hover: Option<Hover>,
    press: Option<Press>,
}

impl State {
    pub fn geometry(&mut self, geometry: Geometry) -> bool {
        if self.geometry == Some(geometry) {
            return false;
        }
        self.geometry = Some(geometry);
        self.cancel();
        true
    }

    pub fn output_changed(&mut self) -> bool {
        self.revision = self.revision.wrapping_add(1);
        self.hover.take().is_some()
    }

    pub fn cancel_hover(&mut self) -> bool {
        self.hover.take().is_some()
    }

    pub fn cancel(&mut self) -> bool {
        self.revision = self.revision.wrapping_add(1);
        let changed = self.hover.take().is_some();
        self.press = None;
        changed
    }

    pub fn cached_hover(&self, cell: Cell) -> Option<Option<&Target>> {
        self.hover
            .as_ref()
            .filter(|hover| hover.cell == cell && hover.revision == self.revision)
            .map(|hover| hover.target.as_ref())
    }

    pub fn set_hover(&mut self, cell: Cell, target: Option<Target>) -> bool {
        let changed = self.hover.as_ref().is_none_or(|old| old.target != target);
        self.hover = Some(Hover {
            cell,
            revision: self.revision,
            target,
        });
        changed
    }

    pub fn preview(&self) -> Option<&Target> {
        self.hover.as_ref().and_then(|hover| hover.target.as_ref())
    }

    pub fn press(&mut self, cell: Cell, target: Target, position: (f32, f32), generation: u64) {
        self.press = Some(Press {
            cell,
            target,
            position,
            generation,
            dragged: false,
        });
    }

    pub fn moving(&mut self, position: (f32, f32)) {
        if let Some(press) = self.press.as_mut() {
            // A small same-cell pointer jitter is still a click. Crossing the
            // threshold rejects the gesture even if it returns to that cell.
            let dx = position.0 - press.position.0;
            let dy = position.1 - press.position.1;
            press.dragged |= dx * dx + dy * dy > 9.;
        }
    }

    pub fn finish(
        &mut self,
        cell: Option<Cell>,
        position: (f32, f32),
        generation: u64,
        current: Option<Target>,
    ) -> Option<WebUrl> {
        self.moving(position);
        let press = self.press.take()?;
        (cell == Some(press.cell)
            && generation == press.generation
            && !press.dragged
            && current
                .as_ref()
                .is_some_and(|target| target.hit == press.target.hit))
        .then_some(press.target.url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_engine::LinkKind;

    fn target(uri: &str, kind: LinkKind) -> Target {
        Target::allowed(LinkHit {
            uri: uri.into(),
            kind,
        })
        .unwrap()
    }

    #[test]
    fn fresh_hit_and_captured_identity_decide_activation() {
        let cell = Cell { col: 4, row: 2 };
        let original = target("https://example.com/a", LinkKind::Osc8);
        let mut state = State::default();
        state.press(cell, original.clone(), (10., 10.), 7);
        state.output_changed();
        assert_eq!(
            state.finish(Some(cell), (11., 10.), 7, Some(original.clone())),
            Some(original.url.clone())
        );
        for fresh in [
            target("https://example.com/b", LinkKind::Osc8),
            target("https://example.com/a", LinkKind::Detected),
        ] {
            state.press(cell, original.clone(), (10., 10.), 7);
            assert_eq!(state.finish(Some(cell), (10., 10.), 7, Some(fresh)), None);
        }
        state.press(cell, original.clone(), (10., 10.), 7);
        assert_eq!(
            state.finish(None, (10., 10.), 7, Some(original.clone())),
            None
        );
        state.press(cell, original.clone(), (10., 10.), 7);
        state.moving((16., 10.));
        assert_eq!(
            state.finish(Some(cell), (10., 10.), 7, Some(original.clone())),
            None
        );
        state.press(cell, original.clone(), (10., 10.), 7);
        assert_eq!(
            state.finish(Some(cell), (10., 10.), 8, Some(original)),
            None
        );
    }

    #[test]
    fn viewport_geometry_and_cancellation_revoke_press_but_output_only_rechecks() {
        let cell = Cell { col: 0, row: 0 };
        let target = target("https://example.com/", LinkKind::Detected);
        let geometry = Geometry {
            origin: (0., 0.),
            extent: (800., 600.),
            cell_width: 9.,
            scale: 1.,
            size: (80, 24),
        };
        let mut state = State::default();
        state.geometry(geometry);
        state.set_hover(cell, Some(target.clone()));
        assert!(state.cached_hover(cell).is_some());
        state.output_changed();
        assert!(state.cached_hover(cell).is_none());
        state.press(cell, target.clone(), (1., 1.), 1);
        state.cancel();
        assert_eq!(
            state.finish(Some(cell), (1., 1.), 1, Some(target.clone())),
            None
        );
        state.press(cell, target.clone(), (1., 1.), 1);
        assert!(!state.geometry(geometry));
        assert!(state.geometry(Geometry {
            scale: 2.,
            ..geometry
        }));
        assert_eq!(state.finish(Some(cell), (1., 1.), 1, Some(target)), None);
    }

    #[test]
    fn disallowed_targets_never_enter_gesture_state() {
        for uri in ["file:///tmp/a", "javascript:alert(1)", "https://"] {
            assert!(Target::allowed(LinkHit {
                uri: uri.into(),
                kind: LinkKind::Osc8
            })
            .is_none());
        }
    }
}
