//! Which session `splitty attach` shows: one named by a pane or session id, or
//! by a unique prefix of either, or one picked from a numbered list of the
//! server's panes.

use serde_json::Value;

use crate::cli::CliError;

/// A target shorter than this must match an id exactly. Ids are UUIDs, so four
/// characters are short to type and rarely ambiguous within one workspace.
const MIN_PREFIX: usize = 4;

/// A pane bound to a session, as the workspace lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    pub id: String,
    pub session_id: String,
    pub screen: String,
    pub title: String,
    pub cwd: String,
}

/// The panes of `state` (`GET /api/mux/state`) that are bound to a session:
/// screen by screen in the workspace's order, each screen's panes in layout
/// order, its floating windows last.
pub fn panes(state: &Value) -> Vec<Pane> {
    let Some(screens) = state["screens"].as_object() else {
        return Vec::new();
    };
    let mut order: Vec<&str> = state["screenOrder"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|id| screens.contains_key(*id))
        .collect();
    let mut rest: Vec<&str> = screens
        .keys()
        .map(String::as_str)
        .filter(|id| !order.contains(id))
        .collect();
    rest.sort_unstable();
    order.extend(rest);

    let mut result = Vec::new();
    for screen in order.into_iter().filter_map(|id| screens.get(id)) {
        let name = text(&screen["name"]);
        let mut ids = Vec::new();
        layout_order(&screen["root"], &mut ids);
        for floating in screen["floating"].as_array().into_iter().flatten() {
            layout_order(&floating["root"], &mut ids);
        }
        let Some(panes) = screen["panes"].as_object() else {
            continue;
        };
        let mut others: Vec<&String> = panes.keys().filter(|id| !ids.contains(id)).collect();
        others.sort_unstable();
        ids.extend(others.into_iter().cloned());
        for id in ids {
            let pane = &panes[&id];
            let Some(session_id) = pane["sessionId"].as_str() else {
                continue;
            };
            result.push(Pane {
                session_id: session_id.to_owned(),
                screen: name.clone(),
                title: text(&pane["title"]),
                cwd: text(&pane["cwd"]),
                id,
            });
        }
    }
    result
}

fn layout_order(node: &Value, ids: &mut Vec<String>) {
    for id in node["paneIds"].as_array().into_iter().flatten() {
        if let Some(id) = id.as_str() {
            ids.push(id.to_owned());
        }
    }
    for child in node["children"].as_array().into_iter().flatten() {
        layout_order(child, ids);
    }
}

fn text(value: &Value) -> String {
    match value.as_str() {
        Some(text) if !text.is_empty() => text.to_owned(),
        _ => "-".to_owned(),
    }
}

/// The session `target` names: a pane or session id, a prefix of at least
/// four characters that matches one pane, or a full id the workspace does not
/// list (a session without a pane), which the server then checks.
pub fn resolve(target: &str, panes: &[Pane]) -> Result<String, CliError> {
    if let Some(pane) = panes
        .iter()
        .find(|p| p.id == target || p.session_id == target)
    {
        return Ok(pane.session_id.clone());
    }
    if target.chars().count() < MIN_PREFIX {
        return Err(CliError::Failed(format!(
            "no pane or session is `{target}`; a prefix needs at least {MIN_PREFIX} characters"
        )));
    }
    let matches: Vec<&Pane> = panes
        .iter()
        .filter(|p| p.id.starts_with(target) || p.session_id.starts_with(target))
        .collect();
    match matches.as_slice() {
        [pane] => Ok(pane.session_id.clone()),
        [] if looks_like_an_id(target) => Ok(target.to_owned()),
        [] => Err(CliError::Failed(format!(
            "no pane or session matches `{target}`"
        ))),
        several => Err(CliError::Failed(format!(
            "`{target}` matches several panes:\n{}",
            table(several.iter().copied(), false)
        ))),
    }
}

/// A full UUID, as the server generates ids.
fn looks_like_an_id(target: &str) -> bool {
    target.len() == 36 && target.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// The numbered list `splitty attach` shows without a target.
pub fn list(panes: &[Pane]) -> String {
    table(panes.iter(), true)
}

/// The pane an answer to the list picks.
pub fn pick<'a>(answer: &str, panes: &'a [Pane]) -> Option<&'a Pane> {
    let number: usize = answer.trim().parse().ok()?;
    panes.get(number.checked_sub(1)?)
}

fn table<'a>(panes: impl Iterator<Item = &'a Pane>, numbered: bool) -> String {
    let rows: Vec<[String; 5]> = panes
        .enumerate()
        .map(|(index, pane)| {
            [
                if numbered {
                    (index + 1).to_string()
                } else {
                    String::new()
                },
                pane.screen.clone(),
                pane.title.clone(),
                pane.cwd.clone(),
                pane.id.chars().take(8).collect(),
            ]
        })
        .collect();
    let mut widths = [0; 5];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in rows {
        let mut line = String::from(" ");
        for (index, (cell, width)) in row.iter().zip(widths).enumerate() {
            if index == 0 && !numbered {
                continue;
            }
            let padding = width - cell.chars().count();
            if index == 0 {
                line.push_str(&" ".repeat(padding));
                line.push_str(cell);
            } else if index == row.len() - 1 {
                line.push_str(cell);
            } else {
                line.push_str(cell);
                line.push_str(&" ".repeat(padding));
            }
            line.push_str("  ");
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const A: &str = "3f9a1111-0000-4000-8000-000000000001";
    const B: &str = "3f9a2222-0000-4000-8000-000000000002";
    const C: &str = "c0de3333-0000-4000-8000-000000000003";

    fn state() -> Value {
        json!({
            "screenOrder": ["s2", "s1"],
            "screens": {
                "s1": {
                    "id": "s1", "name": "dev",
                    "root": {"type": "split", "children": [
                        {"type": "slot", "paneIds": [B]},
                        {"type": "slot", "paneIds": [A]},
                    ]},
                    "panes": {
                        A: {"id": A, "sessionId": "sess-a", "title": "claude", "cwd": "/code"},
                        B: {"id": B, "sessionId": "sess-b"},
                    },
                },
                "s2": {
                    "id": "s2",
                    "root": {"type": "slot", "paneIds": ["no-session"]},
                    "floating": [{"root": {"type": "slot", "paneIds": [C]}}],
                    "panes": {
                        "no-session": {"id": "no-session"},
                        C: {"id": C, "sessionId": "sess-c", "title": "build"},
                    },
                },
            },
        })
    }

    #[test]
    fn panes_follow_the_screen_order_and_the_layout() {
        let panes = panes(&state());
        let ids: Vec<&str> = panes.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, [C, B, A]);
        assert_eq!(panes[0].screen, "-");
        assert_eq!(
            panes[2],
            Pane {
                id: A.into(),
                session_id: "sess-a".into(),
                screen: "dev".into(),
                title: "claude".into(),
                cwd: "/code".into(),
            }
        );
    }

    #[test]
    fn a_target_names_a_pane_or_session_by_id_or_unique_prefix() {
        let panes = panes(&state());
        assert_eq!(resolve(A, &panes).expect("pane id"), "sess-a");
        assert_eq!(resolve("sess-c", &panes).expect("session id"), "sess-c");
        assert_eq!(resolve("c0de", &panes).expect("pane prefix"), "sess-c");
        assert_eq!(resolve("sess-b", &panes).expect("exact"), "sess-b");
        let full = "0a0b0c0d-0000-4000-8000-00000000000f";
        assert_eq!(
            resolve(full, &panes).expect("a session without a pane"),
            full
        );
    }

    #[test]
    fn ambiguous_short_and_unknown_targets_fail() {
        let panes = panes(&state());
        let Err(CliError::Failed(message)) = resolve("3f9a", &panes) else {
            panic!("an ambiguous prefix must fail");
        };
        assert!(
            message.contains("3f9a1111") && message.contains("3f9a2222"),
            "{message}"
        );
        assert!(matches!(resolve("c0d", &panes), Err(CliError::Failed(_))));
        assert!(matches!(resolve("ffff", &panes), Err(CliError::Failed(_))));
    }

    #[test]
    fn the_list_is_numbered_and_an_answer_picks_a_pane() {
        let panes = panes(&state());
        let list = list(&panes);
        let lines: Vec<&str> = list.lines().collect();
        assert_eq!(lines.len(), 3, "{list}");
        assert!(
            lines[0].starts_with(" 1  -  ") && lines[0].ends_with("c0de3333"),
            "{list}"
        );
        assert!(lines[2].starts_with(" 3  dev  claude  /code"), "{list}");
        assert_eq!(pick(" 2\n", &panes).map(|p| p.id.as_str()), Some(B));
        assert!(pick("0", &panes).is_none());
        assert!(pick("4", &panes).is_none());
        assert!(pick("", &panes).is_none());
    }
}
