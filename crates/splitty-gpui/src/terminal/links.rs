//! Terminal-local link gesture state. Only the engine resolves cells; only the
//! client policy decides whether an owned hit may reach the platform opener.
use crate::terminal_engine::{LinkHit, LinkRun};
use splitty_client_core::external_links::WebUrl;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkOrigin {
    Local,
    Remote,
}

impl LinkOrigin {
    pub(super) fn activate(
        self,
        url: &WebUrl,
        open: impl FnOnce(&str),
    ) -> Result<(), &'static str> {
        if self == Self::Remote && url.is_loopback_or_unspecified() {
            return Err("Remote loopback link requires workspace forwarding");
        }
        open(url.as_str());
        Ok(())
    }
}

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
    runs: Vec<LinkRun>,
}

struct Press {
    cell: Cell,
    target: Target,
    position: (f32, f32),
    generation: u64,
    dragged: bool,
}

/// A completed plain click that opens its target once no second click
/// arrives in time. Its target was validated at release; a new press or any
/// cancellation revokes it.
pub(super) struct Pending {
    url: WebUrl,
    generation: u64,
}

#[derive(Default)]
pub(super) struct State {
    revision: u64,
    geometry: Option<Geometry>,
    hover: Option<Hover>,
    press: Option<Press>,
    pending: Option<Pending>,
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
        self.pending = None;
        changed
    }

    pub fn cached_hover(&self, cell: Cell) -> Option<Option<&Target>> {
        self.hover
            .as_ref()
            .filter(|hover| hover.cell == cell && hover.revision == self.revision)
            .map(|hover| hover.target.as_ref())
    }

    pub fn set_hover(&mut self, cell: Cell, target: Option<Target>, runs: Vec<LinkRun>) -> bool {
        let runs = if target.is_some() { runs } else { Vec::new() };
        let changed = self
            .hover
            .as_ref()
            .is_none_or(|old| old.target != target || old.runs != runs);
        self.hover = Some(Hover {
            cell,
            revision: self.revision,
            target,
            runs,
        });
        changed
    }

    pub fn preview(&self) -> Option<&Target> {
        self.hover.as_ref().and_then(|hover| hover.target.as_ref())
    }

    /// The viewport cells of the hovered link, for its underline.
    pub fn underline(&self) -> &[LinkRun] {
        self.hover
            .as_ref()
            .map(|hover| hover.runs.as_slice())
            .unwrap_or_default()
    }

    pub fn pressed(&self) -> bool {
        self.press.is_some()
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

    /// Like `finish`, but a completed click is held until `take_pending`
    /// instead of opening at once. Reports whether one is held.
    pub fn finish_deferred(
        &mut self,
        cell: Option<Cell>,
        position: (f32, f32),
        generation: u64,
        current: Option<Target>,
    ) -> bool {
        self.pending = self
            .finish(cell, position, generation, current)
            .map(|url| Pending { url, generation });
        self.pending.is_some()
    }

    /// Revoke a held click, as a new press does.
    pub fn cancel_pending(&mut self) {
        self.pending = None;
    }

    pub fn take_pending(&mut self) -> Option<Pending> {
        self.pending.take()
    }

    pub fn restore_pending(&mut self, pending: Option<Pending>) {
        self.pending = pending;
    }

    /// The held click's target while the terminal shows the same session.
    pub fn complete_pending(pending: Pending, generation: u64) -> Option<WebUrl> {
        (pending.generation == generation).then_some(pending.url)
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
    fn completed_remote_loopback_gesture_cannot_reach_opener() {
        let cell = Cell { col: 4, row: 2 };
        let mut state = State::default();
        let mut opened = Vec::new();
        for (origin, raw, expected_open) in [
            (LinkOrigin::Local, "http://127.1/", true),
            (LinkOrigin::Remote, "http://127.1/", false),
            (LinkOrigin::Remote, "http://localhost@public.example/", true),
            (LinkOrigin::Remote, "https://example.com/path", true),
        ] {
            let target = target(raw, LinkKind::Osc8);
            state.press(cell, target.clone(), (10., 10.), 7);
            let url = state
                .finish(Some(cell), (10., 10.), 7, Some(target))
                .unwrap();
            let before = opened.len();
            let result = origin.activate(&url, |url| opened.push(url.to_owned()));
            assert_eq!(opened.len() == before + 1, expected_open, "{raw}");
            assert_eq!(result.is_ok(), expected_open, "{raw}");
        }
        state.press(
            cell,
            target("http://localhost/", LinkKind::Osc8),
            (10., 10.),
            7,
        );
        state.moving((20., 10.));
        assert!(state
            .finish(
                Some(cell),
                (10., 10.),
                7,
                Some(target("http://localhost/", LinkKind::Osc8))
            )
            .is_none());
        assert_eq!(
            opened.len(),
            3,
            "dragged link does not reach policy or opener"
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
        state.set_hover(cell, Some(target.clone()), Vec::new());
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

    #[test]
    fn a_held_plain_click_opens_only_its_current_target_in_its_session() {
        let cell = Cell { col: 4, row: 2 };
        let original = target("https://example.com/a", LinkKind::Detected);
        let mut state = State::default();
        state.press(cell, original.clone(), (10., 10.), 7);
        assert!(state.pressed());
        assert!(state.finish_deferred(Some(cell), (11., 10.), 7, Some(original.clone())));
        assert!(!state.pressed());
        let pending = state.take_pending().expect("held click");
        assert_eq!(
            State::complete_pending(pending, 7),
            Some(original.url.clone())
        );
        // Another session opens nothing.
        state.press(cell, original.clone(), (10., 10.), 7);
        assert!(state.finish_deferred(Some(cell), (10., 10.), 7, Some(original.clone())));
        let pending = state.take_pending().unwrap();
        assert_eq!(State::complete_pending(pending, 8), None);
        // A changed target at release, a drag, a release elsewhere, a new
        // press or a cancellation holds nothing.
        state.press(cell, original.clone(), (10., 10.), 7);
        assert!(!state.finish_deferred(
            Some(cell),
            (10., 10.),
            7,
            Some(target("https://example.com/b", LinkKind::Detected))
        ));
        state.press(cell, original.clone(), (10., 10.), 7);
        state.moving((20., 10.));
        assert!(!state.finish_deferred(Some(cell), (10., 10.), 7, Some(original.clone())));
        state.press(cell, original.clone(), (10., 10.), 7);
        assert!(!state.finish_deferred(None, (10., 10.), 7, Some(original.clone())));
        assert!(state.take_pending().is_none());
        state.press(cell, original.clone(), (10., 10.), 7);
        assert!(state.finish_deferred(Some(cell), (10., 10.), 7, Some(original.clone())));
        state.cancel_pending();
        assert!(state.take_pending().is_none());
        state.press(cell, original.clone(), (10., 10.), 7);
        assert!(state.finish_deferred(Some(cell), (10., 10.), 7, Some(original)));
        state.cancel();
        assert!(state.take_pending().is_none());
    }

    #[test]
    fn hover_keeps_the_runs_of_an_allowed_target_only() {
        let cell = Cell { col: 0, row: 0 };
        let runs = vec![LinkRun {
            row: 0,
            cols: 0..20,
        }];
        let mut state = State::default();
        assert!(state.set_hover(
            cell,
            Some(target("https://example.com/", LinkKind::Detected)),
            runs.clone()
        ));
        assert_eq!(state.underline(), runs.as_slice());
        assert!(!state.set_hover(
            cell,
            Some(target("https://example.com/", LinkKind::Detected)),
            runs.clone()
        ));
        assert!(state.set_hover(cell, None, runs));
        assert!(state.underline().is_empty());
        state.cancel_hover();
        assert!(state.underline().is_empty());
    }
}
