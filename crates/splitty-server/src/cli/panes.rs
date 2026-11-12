//! `splitty list-panes`: a screen's panes, from `GET /api/mux/screen/{id}`,
//! as lines or as JSON.

use serde_json::{Map, Value};

use super::display_cell;

/// One line per pane: its id, title, working directory, session and the
/// session's program status, `status=<state>[:<kind>]` or `status=-`. No
/// cell holds terminal controls or line breaks.
pub fn pane_lines(screen: &Value) -> Vec<String> {
    let Some(panes) = screen.get("panes").and_then(Value::as_object) else {
        return Vec::new();
    };
    panes
        .iter()
        .map(|(id, pane)| {
            let text = |key: &str| pane.get(key).and_then(Value::as_str).unwrap_or("-");
            format!(
                "{}\t{}\t{}\tsession={}\tstatus={}",
                display_cell(id),
                display_cell(text("title")),
                display_cell(text("cwd")),
                display_cell(text("sessionId")),
                display_cell(&status_cell(program_status(screen, pane))),
            )
        })
        .collect()
}

/// The panes for `--json`, each with its session's `programStatus`.
pub fn panes_json(screen: &Value) -> Value {
    let mut panes = screen
        .get("panes")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    if let Some(panes) = panes.as_object_mut() {
        for pane in panes.values_mut() {
            let Some(status) = program_status(screen, pane).cloned() else {
                continue;
            };
            if let Some(pane) = pane.as_object_mut() {
                pane.insert("programStatus".into(), status);
            }
        }
    }
    panes
}

/// The root program status record of the pane's session.
fn program_status<'a>(screen: &'a Value, pane: &Value) -> Option<&'a Value> {
    let session = pane.get("sessionId")?.as_str()?;
    screen.get("sessions")?.get(session)?.get("programStatus")
}

fn status_cell(status: Option<&Value>) -> String {
    let field = |key: &str| status?.get(key)?.as_str();
    match (field("state"), field("kind")) {
        (None, _) => "-".to_owned(),
        (Some(state), None) => state.to_owned(),
        (Some(state), Some(kind)) => format!("{state}:{kind}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A screen with one pane per entry of `statuses`, `p<i>` on the session
    /// `s<i>` with that root record.
    fn screen(statuses: &[Option<Value>]) -> Value {
        let mut panes = Map::new();
        let mut sessions = Map::new();
        for (i, status) in statuses.iter().enumerate() {
            panes.insert(
                format!("p{i}"),
                json!({"title": "agent", "cwd": "/home/me/project", "sessionId": format!("s{i}")}),
            );
            let mut session = json!({"id": format!("s{i}"), "status": "running"});
            if let Some(status) = status {
                session["programStatus"] = status.clone();
            }
            sessions.insert(format!("s{i}"), session);
        }
        json!({"id": "screen", "panes": panes, "sessions": sessions})
    }

    #[test]
    fn each_line_ends_with_the_sessions_program_status() {
        let blocked =
            json!({"state": "blocked", "kind": "permission", "msg": "Allow edit?", "updatedAt": 1});
        let working = json!({"state": "working", "progress": 40, "updatedAt": 2});
        let lines = pane_lines(&screen(&[None, Some(blocked), Some(working)]));
        assert_eq!(
            lines,
            [
                "p0\tagent\t/home/me/project\tsession=s0\tstatus=-",
                "p1\tagent\t/home/me/project\tsession=s1\tstatus=blocked:permission",
                "p2\tagent\t/home/me/project\tsession=s2\tstatus=working",
            ]
        );
    }

    #[test]
    fn a_pane_without_a_session_has_no_status() {
        let screen = json!({"panes": {"p1": {"title": "viewer"}}, "sessions": {}});
        assert_eq!(pane_lines(&screen), ["p1\tviewer\t-\tsession=-\tstatus=-"]);
    }

    #[test]
    fn control_characters_in_cells_are_replaced() {
        let screen = json!({
            "panes": {"p\u{1b}1": {"title": "a\u{1b}[31mb\nc", "cwd": "/tmp/\u{202e}x", "sessionId": "s1"}},
            "sessions": {"s1": {"programStatus": {"state": "blocked", "kind": "q\u{7}", "updatedAt": 1}}},
        });
        assert_eq!(
            pane_lines(&screen),
            ["p\u{fffd}1\ta\u{fffd}[31mb\u{fffd}c\t/tmp/\u{fffd}x\tsession=s1\tstatus=blocked:q\u{fffd}"]
        );
    }

    #[test]
    fn json_panes_carry_their_sessions_program_status() {
        let working = json!({"state": "working", "updatedAt": 2});
        let panes = panes_json(&screen(&[Some(working.clone()), None]));
        assert_eq!(panes["p0"]["programStatus"], working);
        assert_eq!(panes["p0"]["sessionId"], "s0");
        assert!(panes["p1"].get("programStatus").is_none(), "{panes}");
        assert_eq!(panes_json(&json!({})), json!({}));
    }
}
