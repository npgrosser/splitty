//! What closing a part of the workspace would end (`mux-protocol`, Close
//! Assessment). The windows ask before a close from this answer only, so the
//! rule of what counts as lost lives here once. Pure: it reads the state and
//! changes nothing.
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::*;
use crate::ids::*;

/// The most panes or screens one scope may name.
pub const CLOSE_SCOPE_LIMIT: usize = 512;

/// The part of the workspace a close would end.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CloseScope {
    /// Panes of one screen: a tab, several tabs, a viewer.
    #[serde(rename_all = "camelCase")]
    Panes {
        screen_id: ScreenId,
        pane_ids: Vec<PaneId>,
    },
    /// Whole screens, also a screen a template replaces.
    #[serde(rename_all = "camelCase")]
    Screens { screen_ids: Vec<ScreenId> },
    /// Every screen, as the end of an embedded server ends them.
    Workspace,
}

/// The answer: what would be lost, entry by entry, with totals.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseAssessment {
    /// Whether the window asks before closing: any entry loses something.
    pub confirm: bool,
    /// Terminals whose program runs.
    pub running: u32,
    /// Terminals whose session has not started yet.
    pub starting: u32,
    /// Sessions whose scratchpad holds text.
    pub scratchpads: u32,
    /// Annotations of the viewer panes.
    pub annotations: u32,
    /// Only the panes and sessions that lose something, in layout order.
    #[serde(default)]
    pub entries: Vec<CloseLoss>,
}

/// One pane, or one session of a screen without a pane, and what it loses.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseLoss {
    pub screen_id: ScreenId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<PaneId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// The session's display title, else its command, else the pane's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub starting: bool,
    #[serde(default)]
    pub scratchpad: bool,
    #[serde(default)]
    pub annotations: u32,
}

impl CloseLoss {
    pub fn loses_something(&self) -> bool {
        self.running || self.starting || self.scratchpad || self.annotations != 0
    }
}

impl CloseAssessment {
    /// Totals and `confirm` follow from the entries.
    fn push(&mut self, loss: CloseLoss) {
        if !loss.loses_something() {
            return;
        }
        self.running += u32::from(loss.running);
        self.starting += u32::from(loss.starting);
        self.scratchpads += u32::from(loss.scratchpad);
        self.annotations = self.annotations.saturating_add(loss.annotations);
        self.confirm = true;
        self.entries.push(loss);
    }
}

/// Assess `scope` against `state`. Screens and panes the state does not have
/// add nothing; only a scope larger than the limit is refused.
pub fn assess_close(
    state: &WorkspaceState,
    scope: &CloseScope,
) -> Result<CloseAssessment, MuxError> {
    let mut out = CloseAssessment::default();
    match scope {
        CloseScope::Panes {
            screen_id,
            pane_ids,
        } => {
            limit(pane_ids.len())?;
            let Some(screen) = state.screens.get(screen_id) else {
                return Ok(out);
            };
            let wanted: HashSet<&PaneId> = pane_ids.iter().collect();
            for pane_id in pane_order(screen) {
                if wanted.contains(&pane_id) {
                    if let Some(pane) = screen.panes.get(&pane_id) {
                        out.push(pane_loss(screen, pane));
                    }
                }
            }
        }
        CloseScope::Screens { screen_ids } => {
            limit(screen_ids.len())?;
            let mut seen = HashSet::new();
            for id in screen_ids.iter().filter(|id| seen.insert(*id)) {
                if let Some(screen) = state.screens.get(id) {
                    screen_losses(screen, &mut out);
                }
            }
        }
        CloseScope::Workspace => {
            let mut ids: Vec<&ScreenId> = state
                .screen_order
                .iter()
                .filter(|id| state.screens.contains_key(*id))
                .collect();
            let mut rest: Vec<&ScreenId> = state
                .screens
                .keys()
                .filter(|id| !state.screen_order.contains(id))
                .collect();
            rest.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            ids.extend(rest);
            let mut seen = HashSet::new();
            for id in ids.into_iter().filter(|id| seen.insert(*id)) {
                screen_losses(&state.screens[id], &mut out);
            }
        }
    }
    Ok(out)
}

fn limit(len: usize) -> Result<(), MuxError> {
    if len > CLOSE_SCOPE_LIMIT {
        return Err(MuxError::Internal(format!(
            "a close scope names at most {CLOSE_SCOPE_LIMIT} items"
        )));
    }
    Ok(())
}

/// Tiled panes, then floating ones, then any the layout does not hold.
fn pane_order(screen: &ScreenState) -> Vec<PaneId> {
    let mut order = screen.root.collect_pane_ids();
    for entry in &screen.floating {
        order.extend(entry.root.collect_pane_ids());
    }
    let mut rest: Vec<PaneId> = screen.panes.keys().cloned().collect();
    rest.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    order.extend(rest);
    let mut seen = HashSet::new();
    order.retain(|id| screen.panes.contains_key(id) && seen.insert(id.clone()));
    order
}

fn screen_losses(screen: &ScreenState, out: &mut CloseAssessment) {
    let mut linked = HashSet::new();
    for pane_id in pane_order(screen) {
        let Some(pane) = screen.panes.get(&pane_id) else {
            continue;
        };
        if let Some(session) = &pane.session_id {
            linked.insert(session.clone());
        }
        out.push(pane_loss(screen, pane));
    }
    let mut orphans: Vec<&SessionEntry> = screen
        .sessions
        .values()
        .filter(|session| !linked.contains(&session.id))
        .collect();
    orphans.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    for session in orphans {
        let terminal = session.job_id.is_none();
        out.push(CloseLoss {
            screen_id: screen.id.clone(),
            pane_id: None,
            session_id: Some(session.id.clone()),
            title: session_title(session),
            running: terminal && is_running(session),
            starting: terminal && session.status == SessionStatus::Starting,
            scratchpad: !session.scratchpad.is_empty(),
            annotations: 0,
        });
    }
}

fn pane_loss(screen: &ScreenState, pane: &PaneState) -> CloseLoss {
    let session = pane
        .session_id
        .as_ref()
        .and_then(|id| screen.sessions.get(id));
    // A managed job's pane keeps its own lifecycle; only terminals count as
    // running programs.
    let terminal = pane
        .role
        .as_ref()
        .is_none_or(|role| *role == PaneRole::Terminal)
        && pane.job_id.is_none()
        && session.is_none_or(|session| session.job_id.is_none());
    let annotations = pane.viewer.as_ref().map_or(0, |viewer| {
        u32::try_from(viewer.annotations.len()).unwrap_or(u32::MAX)
    });
    CloseLoss {
        screen_id: screen.id.clone(),
        pane_id: Some(pane.id.clone()),
        session_id: pane.session_id.clone(),
        title: session
            .and_then(session_title)
            .or_else(|| pane.title.clone().filter(|title| !title.is_empty())),
        running: terminal && session.is_some_and(is_running),
        starting: terminal && session.is_some_and(|s| s.status == SessionStatus::Starting),
        scratchpad: session.is_some_and(|s| !s.scratchpad.is_empty()),
        annotations,
    }
}

fn is_running(session: &SessionEntry) -> bool {
    session.busy && session.status == SessionStatus::Running
}

fn session_title(session: &SessionEntry) -> Option<String> {
    session
        .display_title
        .clone()
        .or_else(|| session.command.clone())
        .filter(|title| !title.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, status: SessionStatus, busy: bool) -> SessionEntry {
        SessionEntry {
            display_title: Some(format!("title-{id}")),
            busy,
            id: SessionId::from(id),
            job_id: None,
            restart_policy: Default::default(),
            shell_profile_id: None,
            command: None,
            restore_mode: None,
            cwd: None,
            shell: None,
            status,
            exit_code: None,
            scratchpad: String::new(),
            scratchpad_version: 0,
        }
    }

    fn pane(id: &str, session: Option<&str>, role: PaneRole) -> PaneState {
        PaneState {
            id: PaneId::from(id),
            job_id: None,
            session_id: session.map(SessionId::from),
            title: None,
            cwd: None,
            role: Some(role),
            viewer: None,
        }
    }

    /// Screen `s` with an idle shell `p1`, a busy `p2` (vim) and a starting `p3`.
    fn workspace() -> WorkspaceState {
        let mut screen = ScreenState::new(ScreenId::from("s"), None);
        screen.root = LayoutNode::Slot {
            id: NodeId::from("slot"),
            pane_ids: vec![PaneId::from("p1"), PaneId::from("p2"), PaneId::from("p3")],
            active_pane_id: None,
        };
        for (pane_id, session_id, status, busy) in [
            ("p1", "s1", SessionStatus::Running, false),
            ("p2", "s2", SessionStatus::Running, true),
            ("p3", "s3", SessionStatus::Starting, false),
        ] {
            screen.panes.insert(
                PaneId::from(pane_id),
                pane(pane_id, Some(session_id), PaneRole::Terminal),
            );
            screen.sessions.insert(
                SessionId::from(session_id),
                session(session_id, status, busy),
            );
        }
        let mut state = WorkspaceState::default();
        state.screen_order.push(screen.id.clone());
        state.screens.insert(screen.id.clone(), screen);
        state
    }

    fn panes(ids: &[&str]) -> CloseScope {
        CloseScope::Panes {
            screen_id: ScreenId::from("s"),
            pane_ids: ids.iter().map(|id| PaneId::from(*id)).collect(),
        }
    }

    #[test]
    fn idle_shell_loses_nothing() {
        let answer = assess_close(&workspace(), &panes(&["p1"])).unwrap();
        assert_eq!(answer, CloseAssessment::default());
    }

    #[test]
    fn busy_and_starting_panes_ask() {
        let answer = assess_close(&workspace(), &panes(&["p1", "p2", "p3"])).unwrap();
        assert!(answer.confirm);
        assert_eq!((answer.running, answer.starting), (1, 1));
        let entries: Vec<_> = answer
            .entries
            .iter()
            .map(|e| {
                (
                    e.pane_id.clone().unwrap().to_string(),
                    e.running,
                    e.starting,
                )
            })
            .collect();
        assert_eq!(
            entries,
            [
                ("p2".to_owned(), true, false),
                ("p3".to_owned(), false, true)
            ]
        );
        assert_eq!(answer.entries[0].title.as_deref(), Some("title-s2"));
        assert_eq!(answer.entries[0].session_id, Some(SessionId::from("s2")));
    }

    #[test]
    fn scratchpad_text_asks_for_an_idle_shell() {
        let mut state = workspace();
        let screen = state.screens.get_mut(&ScreenId::from("s")).unwrap();
        screen
            .sessions
            .get_mut(&SessionId::from("s1"))
            .unwrap()
            .scratchpad = "notes".into();
        let answer = assess_close(&state, &panes(&["p1"])).unwrap();
        assert!(answer.confirm);
        assert_eq!((answer.running, answer.scratchpads), (0, 1));
        assert!(answer.entries[0].scratchpad);
    }

    #[test]
    fn viewer_annotations_count() {
        let mut state = workspace();
        let screen = state.screens.get_mut(&ScreenId::from("s")).unwrap();
        let mut viewer = pane("v", None, PaneRole::Viewer);
        let mut record = ViewerState::new(
            ViewerSource::File {
                path: "/home/me/PLAN.md".into(),
            },
            None,
        );
        for id in ["a1", "a2", "a3"] {
            record.annotations.push(
                serde_json::from_value(serde_json::json!({
                    "id": id, "kind": "comment", "body": "why?"
                }))
                .unwrap(),
            );
        }
        viewer.viewer = Some(record);
        screen.panes.insert(PaneId::from("v"), viewer);
        let answer = assess_close(&state, &panes(&["v"])).unwrap();
        assert!(answer.confirm);
        assert_eq!(answer.annotations, 3);
        assert_eq!(answer.entries[0].annotations, 3);
        assert!(!answer.entries[0].running);
    }

    #[test]
    fn job_panes_do_not_count_as_running() {
        let mut state = workspace();
        let screen = state.screens.get_mut(&ScreenId::from("s")).unwrap();
        screen.panes.get_mut(&PaneId::from("p2")).unwrap().job_id = Some(JobId::from("job"));
        let answer = assess_close(&state, &panes(&["p2"])).unwrap();
        assert!(!answer.confirm);
    }

    #[test]
    fn screens_and_workspace_cover_hidden_and_unlinked_sessions() {
        let mut state = workspace();
        let mut other = ScreenState::new(ScreenId::from("t"), None);
        other.sessions.insert(
            SessionId::from("lonely"),
            session("lonely", SessionStatus::Running, true),
        );
        state.screens.insert(other.id.clone(), other);
        let screen = assess_close(
            &state,
            &CloseScope::Screens {
                screen_ids: vec![ScreenId::from("t"), ScreenId::from("t")],
            },
        )
        .unwrap();
        assert_eq!(screen.running, 1);
        assert_eq!(screen.entries[0].pane_id, None);
        assert_eq!(
            screen.entries[0].session_id,
            Some(SessionId::from("lonely"))
        );
        let all = assess_close(&state, &CloseScope::Workspace).unwrap();
        assert_eq!((all.running, all.starting), (2, 1));
        // The ordered screen comes first.
        assert_eq!(all.entries[0].screen_id, ScreenId::from("s"));
    }

    #[test]
    fn unknown_targets_add_nothing_and_large_scopes_are_refused() {
        let state = workspace();
        let unknown = CloseScope::Panes {
            screen_id: ScreenId::from("gone"),
            pane_ids: vec![PaneId::from("p2")],
        };
        assert!(!assess_close(&state, &unknown).unwrap().confirm);
        assert!(!assess_close(&state, &panes(&["nope"])).unwrap().confirm);
        let many = CloseScope::Screens {
            screen_ids: (0..=CLOSE_SCOPE_LIMIT)
                .map(|i| ScreenId::from(format!("s{i}")))
                .collect(),
        };
        assert!(assess_close(&state, &many).is_err());
    }

    #[test]
    fn wire_shape() {
        let scope: CloseScope = serde_json::from_value(serde_json::json!({
            "kind": "panes", "screenId": "s", "paneIds": ["p2"]
        }))
        .unwrap();
        let answer = assess_close(&workspace(), &scope).unwrap();
        let json = serde_json::to_value(&answer).unwrap();
        assert_eq!(json["confirm"], true);
        assert_eq!(json["running"], 1);
        assert_eq!(json["entries"][0]["paneId"], "p2");
        assert_eq!(json["entries"][0]["sessionId"], "s2");
        assert_eq!(json["entries"][0]["running"], true);
        let workspace: CloseScope =
            serde_json::from_value(serde_json::json!({"kind": "workspace"})).unwrap();
        assert_eq!(workspace, CloseScope::Workspace);
    }
}
