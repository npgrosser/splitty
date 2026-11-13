//! Pure template projection and extraction. Launch I/O belongs to the effect runner.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::*;
use crate::ids::*;
use crate::template::{self, Template};

pub const TEMPLATE_CAPABILITY: &str = "server-templates-v1";

/// One window intent, interpreted identically before and after transport.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyTemplate {
    pub template: Template,
    #[serde(default)]
    pub pane_ids: HashMap<String, PaneId>,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub create_screen: bool,
    /// The user agreed to replacing an existing screen's panes and sessions
    /// (`templates`, Confirmed Screen Replacement).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace_confirmed: bool,
}

fn invalid(message: impl Into<String>) -> MuxError {
    MuxError::InvalidTemplate(message.into())
}

/// Resolves against the target host's root without filesystem access.
pub fn template_directory(dir: &str, root: &str) -> String {
    if dir.is_empty() || root.is_empty() || Path::new(dir).is_absolute() {
        dir.into()
    } else {
        Path::new(root).join(dir).to_string_lossy().into_owned()
    }
}

/// A validated layout intent, without sessions, generated identities or effects.
#[derive(Clone, Debug)]
pub struct TemplateProjection {
    pub root: LayoutNode,
    pub floating: Vec<FloatingEntry>,
    /// Retained terminal names in the authoritative launch order.
    pub pane_names: Vec<String>,
    pub skipped_web_count: usize,
}

impl TemplateProjection {
    /// Tiled panes followed by floating panes in their layout order.
    pub fn pane_ids(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.root.collect_pane_ids().into_iter().chain(
            self.floating
                .iter()
                .flat_map(|entry| entry.root.collect_pane_ids()),
        )
    }
}

/// Project a complete client intent using the same rules as authoritative application.
/// Every retained terminal must have a supplied nonempty, unique pane ID. This
/// function does not allocate IDs or resolve paths on the client's host.
pub fn project_template(request: &ApplyTemplate) -> Result<TemplateProjection, MuxError> {
    template::validate(&request.template).map_err(|e| invalid(e.to_string()))?;
    if request.template.panes.len() > 256 {
        return Err(invalid("at most 256 template panes"));
    }
    let mut names = HashSet::new();
    let mut next_node = 0;
    let root = project_node(
        &request.template.layout,
        request,
        &mut names,
        0,
        &mut next_node,
    )?
    .ok_or_else(|| invalid("template has no tiled terminals"))?;
    let mut floating = Vec::new();
    for (index, entry) in request.template.floating.iter().enumerate() {
        if entry.layout.slot.is_none() {
            return Err(invalid("floating layouts must be slots"));
        }
        let bounds = FloatBounds {
            x: entry.bounds.x,
            y: entry.bounds.y,
            w: entry.bounds.w,
            h: entry.bounds.h,
        };
        if !bounds.is_valid() {
            return Err(invalid("invalid floating bounds"));
        }
        if let Some(root) = project_node(&entry.layout, request, &mut names, 0, &mut next_node)? {
            floating.push(FloatingEntry {
                id: FloatingId::from(format!("template-floating-{index}")),
                root,
                bounds,
            });
        }
    }
    for (name, pane) in &request.template.panes {
        if pane.pane_type != "webview" && !names.contains(name) {
            return Err(invalid(format!("unused terminal {name}")));
        }
    }
    let mut pane_names: Vec<_> = names
        .into_iter()
        .filter(|name| request.template.panes[name].pane_type != "webview")
        .collect();
    pane_names.sort();
    let projection = TemplateProjection {
        root,
        floating,
        pane_names,
        skipped_web_count: request
            .template
            .panes
            .values()
            .filter(|pane| pane.pane_type == "webview")
            .count(),
    };
    let mut unique = HashSet::new();
    for pane_id in projection.pane_ids() {
        if pane_id.as_str().is_empty() || !unique.insert(pane_id) {
            return Err(invalid("template pane IDs must be nonempty and unique"));
        }
    }
    Ok(projection)
}

pub(super) fn apply_template(
    mut state: WorkspaceState,
    id: ScreenId,
    request: ApplyTemplate,
) -> Result<(WorkspaceState, Vec<Effect>), MuxError> {
    let projection = project_template(&request)?;
    let old = state.screens.get(&id);
    if request.create_screen && old.is_some() {
        return Err(MuxError::ScreenExists(id));
    }
    if !request.create_screen && old.is_none() {
        return Err(MuxError::ScreenNotFound(id));
    }
    if old.is_some_and(|screen| !screen.panes.is_empty() || !screen.sessions.is_empty())
        && !request.replace_confirmed
    {
        return Err(MuxError::ReplaceNotConfirmed(id));
    }
    for pane_id in projection.pane_ids() {
        if state
            .screens
            .values()
            .any(|screen| screen.panes.contains_key(&pane_id))
        {
            return Err(MuxError::PaneExists(pane_id));
        }
    }
    let mut screen = ScreenState::new(
        id.clone(),
        old.map_or_else(|| Some(request.template.name.clone()), |s| s.name.clone()),
    );
    screen.revision = old.map_or(1, |s| s.revision + 1);
    screen.root = projection.root;
    screen.floating = projection.floating;
    let mut effects: Vec<_> = old
        .into_iter()
        .flat_map(|s| s.sessions.keys())
        .map(|session_id| Effect::KillSession {
            session_id: session_id.clone(),
        })
        .collect();
    for name in projection.pane_names {
        let pane = &request.template.panes[&name];
        let pane_id = request.pane_ids[&name].clone();
        let session_id = SessionId::with_prefix("sess");
        let present = |value: &str| (!value.is_empty()).then(|| value.to_owned());
        let command = present(&pane.command);
        let cwd = present(&template_directory(&pane.dir, &request.root));
        let shell_profile_id = present(&pane.shell_profile);
        let restore_mode = command.as_ref().map(|_| {
            if pane.restore_mode.is_empty() {
                "replay".into()
            } else {
                pane.restore_mode.clone()
            }
        });
        screen.panes.insert(
            pane_id.clone(),
            PaneState {
                id: pane_id.clone(),
                job_id: None,
                session_id: Some(session_id.clone()),
                title: None,
                cwd: cwd.clone(),
                role: Some(PaneRole::Terminal),
                viewer: None,
            },
        );
        screen.sessions.insert(
            session_id.clone(),
            SessionEntry {
                display_title: None,
                busy: false,
                id: session_id.clone(),
                job_id: None,
                restart_policy: Default::default(),
                shell_profile_id: shell_profile_id.clone(),
                command: command.clone(),
                restore_mode: restore_mode.clone(),
                cwd: cwd.clone(),
                shell: None,
                status: SessionStatus::Starting,
                exit_code: None,
                scratchpad: String::new(),
            },
        );
        effects.push(Effect::SpawnSession {
            session_id,
            pane_id,
            screen_id: id.clone(),
            command,
            restore_mode,
            cwd,
            shell_profile_id,
            shell: None,
        });
    }
    if request.create_screen {
        state.screen_order.push(id.clone());
    }
    if state.active_screen_id.is_none() {
        state.active_screen_id = Some(id.clone());
    }
    state.screens.insert(id, screen);
    super::job_reducer::mark_closing_jobs(&mut state, &effects);
    Ok((state, effects))
}

/// Fill omitted IDs once, so the complete operation has stable identities throughout conversion.
pub(super) fn assign_template_ids(mut request: ApplyTemplate) -> ApplyTemplate {
    for (name, pane) in &request.template.panes {
        if pane.pane_type != "webview" {
            request
                .pane_ids
                .entry(name.clone())
                .or_insert_with(|| PaneId::with_prefix("pane"));
        }
    }
    request
}

fn project_node(
    node: &template::LayoutNode,
    request: &ApplyTemplate,
    seen: &mut HashSet<String>,
    depth: usize,
    next_node: &mut usize,
) -> Result<Option<LayoutNode>, MuxError> {
    let node_id = NodeId::from(format!("template-node-{next_node}"));
    *next_node += 1;
    if depth > 32 {
        return Err(invalid("template layout is too deep"));
    }
    if !node.ratio.is_finite() || node.ratio < 0.0 {
        return Err(invalid("invalid template ratio"));
    }
    if let Some(slot) = &node.slot {
        if node.split.is_some() {
            return Err(invalid("slot cannot also be a split"));
        }
        let mut panes = Vec::new();
        for name in &slot.0 {
            if !seen.insert(name.clone()) {
                return Err(invalid(format!("duplicate pane {name}")));
            }
            if request.template.panes[name].pane_type != "webview" {
                panes.push(
                    request
                        .pane_ids
                        .get(name)
                        .ok_or_else(|| invalid(format!("missing pane ID for {name}")))?
                        .clone(),
                );
            }
        }
        return Ok((!panes.is_empty()).then(|| LayoutNode::Slot {
            id: node_id,
            active_pane_id: panes.first().cloned(),
            pane_ids: panes,
        }));
    }
    let mut children = Vec::new();
    let mut sizes = Vec::new();
    for child in &node.children {
        if let Some(projected) = project_node(child, request, seen, depth + 1, next_node)? {
            children.push(projected);
            sizes.push(if child.ratio > 0.0 { child.ratio } else { 1.0 });
        }
    }
    if children.is_empty() {
        return Ok(None);
    }
    if children.len() == 1 {
        return Ok(children.pop());
    }
    let total: f64 = sizes.iter().sum();
    if !total.is_finite() {
        return Err(invalid("invalid template ratios"));
    }
    for size in &mut sizes {
        *size /= total;
    }
    Ok(Some(LayoutNode::Split {
        id: node_id,
        axis: if node.split.as_deref() == Some("horizontal") {
            Axis::Horizontal
        } else {
            Axis::Vertical
        },
        children,
        sizes,
    }))
}

/// Extract terminal-only layout from an already enriched snapshot.
pub fn extract_template(
    screen: &ScreenState,
    name: String,
    root: &str,
) -> Result<Template, MuxError> {
    let mut panes = HashMap::new();
    for (id, pane) in &screen.panes {
        if pane.role == Some(PaneRole::Viewer) || pane.role == Some(PaneRole::Webview) {
            continue;
        }
        let session = pane
            .session_id
            .as_ref()
            .and_then(|id| screen.sessions.get(id));
        let command = session.and_then(|s| s.command.clone()).unwrap_or_default();
        let cwd = pane
            .cwd
            .as_ref()
            .or_else(|| session.and_then(|s| s.cwd.as_ref()))
            .cloned()
            .unwrap_or_default();
        let dir = if !root.is_empty() {
            Path::new(&cwd)
                .strip_prefix(root)
                .ok()
                .map(|relative| {
                    if relative.as_os_str().is_empty() {
                        ".".into()
                    } else {
                        format!("./{}", relative.to_string_lossy())
                    }
                })
                .unwrap_or(cwd)
        } else {
            cwd
        };
        panes.insert(
            id.to_string(),
            template::Pane {
                pane_type: String::new(),
                url: String::new(),
                restore_mode: if command.is_empty() {
                    String::new()
                } else {
                    session
                        .and_then(|s| s.restore_mode.clone())
                        .unwrap_or_else(|| "replay".into())
                },
                command,
                dir,
                shell_profile: session
                    .and_then(|s| s.shell_profile_id.clone())
                    .unwrap_or_default(),
            },
        );
    }
    let layout = extract_node(&screen.root, &panes)
        .ok_or_else(|| invalid("screen has no tiled terminals"))?;
    let floating = screen
        .floating
        .iter()
        .filter_map(|entry| {
            extract_node(&entry.root, &panes).map(|layout| template::FloatingEntry {
                layout,
                bounds: template::FloatBounds {
                    x: entry.bounds.x,
                    y: entry.bounds.y,
                    w: entry.bounds.w,
                    h: entry.bounds.h,
                },
            })
        })
        .collect();
    Ok(Template {
        name,
        panes,
        layout,
        floating,
    })
}

fn extract_node(
    node: &LayoutNode,
    panes: &HashMap<String, template::Pane>,
) -> Option<template::LayoutNode> {
    match node {
        LayoutNode::Slot { pane_ids, .. } => {
            let names: Vec<_> = pane_ids
                .iter()
                .map(ToString::to_string)
                .filter(|id| panes.contains_key(id))
                .collect();
            (!names.is_empty()).then_some(template::LayoutNode {
                slot: Some(template::SlotPanes(names)),
                split: None,
                children: vec![],
                ratio: 0.0,
            })
        }
        LayoutNode::Split {
            axis,
            children,
            sizes,
            ..
        } => {
            let mut kept: Vec<_> = children
                .iter()
                .zip(sizes)
                .filter_map(|(child, size)| {
                    extract_node(child, panes).map(|mut node| {
                        node.ratio = *size;
                        node
                    })
                })
                .collect();
            if kept.len() == 1 {
                let mut node = kept.pop()?;
                node.ratio = 0.0;
                return Some(node);
            }
            if kept.is_empty() {
                return None;
            }
            let total: f64 = kept.iter().map(|node| node.ratio).sum();
            for node in &mut kept {
                node.ratio /= total;
            }
            Some(template::LayoutNode {
                slot: None,
                split: Some(
                    if *axis == Axis::Horizontal {
                        "horizontal"
                    } else {
                        "vertical"
                    }
                    .into(),
                ),
                children: kept,
                ratio: 0.0,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> ApplyTemplate {
        serde_json::from_value(json!({
            "createScreen": true, "root": "/project", "paneIds": {"a":"pane-a", "b":"pane-b", "c":"pane-c"},
            "template": {"name":"Workspace", "panes": {
                "a":{"command":"editor", "dir":"src", "restore_mode":"close", "shell_profile":"fish"},
                "b":{}, "c":{"dir":"/elsewhere"}},
                "layout":{"split":"horizontal", "children":[{"slot":"a", "ratio":3},{"slot":"b", "ratio":1}]},
                "floating":[{"layout":{"slot":"c"}, "bounds":{"x":0.1,"y":0.2,"w":0.4,"h":0.3}}]
            }
        })).unwrap()
    }

    #[test]
    fn pending_projection_matches_authoritative_layout_without_mutating_the_intent() {
        let mut r = request();
        r.template.panes.insert(
            "web".into(),
            serde_json::from_value(json!({
                "type": "webview", "url": "https://example.invalid"
            }))
            .unwrap(),
        );
        r.template.layout.children[0]
            .slot
            .as_mut()
            .unwrap()
            .0
            .push("web".into());
        // Legacy web panes are pruned and do not need client IDs.
        let before = serde_json::to_value(&r).unwrap();
        let preview = project_template(&r).unwrap();
        assert_eq!(preview.pane_names, ["a", "b", "c"]);
        assert_eq!(preview.skipped_web_count, 1);
        assert_eq!(
            preview
                .pane_ids()
                .map(|id| id.to_string())
                .collect::<Vec<_>>(),
            ["pane-a", "pane-b", "pane-c"]
        );
        assert_eq!(serde_json::to_value(&r).unwrap(), before);
        let again = project_template(&r).unwrap();
        assert_eq!(
            serde_json::to_value(&preview.root).unwrap(),
            serde_json::to_value(&again.root).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&preview.floating).unwrap(),
            serde_json::to_value(&again.floating).unwrap()
        );
        let (state, effects) =
            apply_template(WorkspaceState::default(), "screen".into(), r).unwrap();
        let screen = &state.screens[&ScreenId::from("screen")];
        assert_eq!(
            serde_json::to_value(&preview.root).unwrap(),
            serde_json::to_value(&screen.root).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&preview.floating).unwrap(),
            serde_json::to_value(&screen.floating).unwrap()
        );
        assert_eq!(effects.len(), 3);
        assert_eq!(screen.sessions.len(), 3);
    }

    #[test]
    fn projection_requires_complete_unique_nonempty_terminal_ids() {
        for case in 0..3 {
            let mut r = request();
            match case {
                0 => {
                    r.pane_ids.remove("c");
                }
                1 => {
                    r.pane_ids.insert("c".into(), "".into());
                }
                _ => {
                    r.pane_ids.insert("c".into(), "pane-a".into());
                }
            }
            assert!(matches!(
                project_template(&r),
                Err(MuxError::InvalidTemplate(_))
            ));
        }
        // Omitted IDs remain supported at the authoritative dispatch boundary.
        let mut r = request();
        r.pane_ids.clear();
        let store = Store::new();
        assert!(store
            .dispatch(Op::ApplyTemplate {
                screen_id: "screen".into(),
                request: Box::new(r),
            })
            .is_ok());
        assert_eq!(store.get_screen(&"screen".into()).unwrap().panes.len(), 3);
    }

    #[test]
    fn template_apply_replace_extract_and_persistence_preserve_layout_and_launch_data() {
        let store = Store::new();
        let effects = store
            .dispatch(Op::ApplyTemplate {
                screen_id: "screen".into(),
                request: Box::new(request()),
            })
            .unwrap();
        assert_eq!(effects.len(), 3);
        let Effect::SpawnSession {
            command,
            cwd,
            restore_mode,
            shell_profile_id,
            ..
        } = &effects[0]
        else {
            panic!("spawn")
        };
        assert_eq!(command.as_deref(), Some("editor"));
        assert_eq!(cwd.as_deref(), Some("/project/src"));
        assert_eq!(restore_mode.as_deref(), Some("close"));
        assert_eq!(shell_profile_id.as_deref(), Some("fish"));
        let screen = store.get_screen(&"screen".into()).unwrap();
        let LayoutNode::Split { sizes, .. } = &screen.root else {
            panic!("split")
        };
        assert_eq!(sizes, &[0.75, 0.25]);
        let template = extract_template(&screen, "Saved".into(), "/project").unwrap();
        assert_eq!(template.panes["pane-a"].dir, "./src");
        assert_eq!(template.panes["pane-c"].dir, "/elsewhere");
        assert_eq!(template.panes["pane-a"].restore_mode, "close");
        assert_eq!(template.floating[0].bounds.x, 0.1);
        let roundtrip: ScreenState =
            serde_json::from_value(serde_json::to_value(&screen).unwrap()).unwrap();
        assert_eq!(
            roundtrip
                .sessions
                .values()
                .find(|s| s.command.is_some())
                .unwrap()
                .restore_mode
                .as_deref(),
            Some("close")
        );
        let mut replacement = request();
        replacement.create_screen = false;
        replacement.replace_confirmed = true;
        replacement.pane_ids.clear();
        let effects = store
            .dispatch(Op::ApplyTemplate {
                screen_id: "screen".into(),
                request: Box::new(replacement),
            })
            .unwrap();
        assert_eq!(
            effects
                .iter()
                .filter(|e| matches!(e, Effect::KillSession { .. }))
                .count(),
            3
        );
        assert_eq!(store.get_screen(&"screen".into()).unwrap().revision, 2);
        assert_eq!(store.get_workspace_state().screen_order.len(), 1);
    }

    #[test]
    fn replacing_a_screen_needs_the_confirmation_flag() {
        let store = Store::new();
        store
            .dispatch(Op::ApplyTemplate {
                screen_id: "screen".into(),
                request: Box::new(request()),
            })
            .unwrap();
        let before = serde_json::to_value(store.get_workspace_state()).unwrap();
        let mut replacement = request();
        replacement.create_screen = false;
        replacement.pane_ids.clear();
        assert!(matches!(
            store.dispatch(Op::ApplyTemplate {
                screen_id: "screen".into(),
                request: Box::new(replacement.clone()),
            }),
            Err(MuxError::ReplaceNotConfirmed(_))
        ));
        assert_eq!(
            serde_json::to_value(store.get_workspace_state()).unwrap(),
            before
        );
        // The wire field is camelCase and absent when false.
        let wire = serde_json::to_value(&replacement).unwrap();
        assert!(wire.get("replaceConfirmed").is_none());
        let mut wire = wire;
        wire["replaceConfirmed"] = json!(true);
        let confirmed: ApplyTemplate = serde_json::from_value(wire).unwrap();
        assert!(store
            .dispatch(Op::ApplyTemplate {
                screen_id: "screen".into(),
                request: Box::new(confirmed),
            })
            .is_ok());
    }

    #[test]
    fn template_rejection_is_atomic_for_collisions_duplicates_missing_and_unused_panes() {
        let store = Store::new();
        store
            .dispatch(Op::ApplyTemplate {
                screen_id: "screen".into(),
                request: Box::new(request()),
            })
            .unwrap();
        let before = serde_json::to_value(store.get_workspace_state()).unwrap();
        for invalid in 0..7 {
            let mut r = request();
            r.create_screen = false;
            r.replace_confirmed = true;
            r.pane_ids.clear();
            match invalid {
                0 => {
                    r.pane_ids.insert("a".into(), "pane-a".into());
                }
                1 => {
                    r.template.layout.children[1].slot =
                        Some(template::SlotPanes(vec!["a".into()]));
                }
                2 => {
                    r.template.layout.children[1].slot =
                        Some(template::SlotPanes(vec!["missing".into()]));
                }
                3 => {
                    r.template.layout.children[1].ratio = -1.0;
                }
                4 => {
                    r.template.floating[0].layout = r.template.layout.clone();
                }
                5 => {
                    r.template.floating.clear();
                }
                _ => {
                    r.pane_ids.insert("a".into(), "duplicate".into());
                    r.pane_ids.insert("b".into(), "duplicate".into());
                }
            }
            assert!(
                store
                    .dispatch(Op::ApplyTemplate {
                        screen_id: "screen".into(),
                        request: Box::new(r)
                    })
                    .is_err(),
                "case {invalid}"
            );
            assert_eq!(
                serde_json::to_value(store.get_workspace_state()).unwrap(),
                before
            );
        }
    }

    #[test]
    fn template_floating_bounds_share_domain_validation_and_reject_atomically() {
        let store = Store::new();
        store
            .dispatch(Op::ApplyTemplate {
                screen_id: "screen".into(),
                request: Box::new(request()),
            })
            .unwrap();
        let before = serde_json::to_value(store.get_workspace_state()).unwrap();
        for component in 0..4 {
            let mut values = vec![f64::NAN, f64::INFINITY, f64::NEG_INFINITY];
            if component >= 2 {
                values.extend([0.0, -1.0]);
            }
            for value in values {
                let mut r = request();
                r.create_screen = false;
                r.replace_confirmed = true;
                r.pane_ids.clear();
                let bounds = &mut r.template.floating[0].bounds;
                *match component {
                    0 => &mut bounds.x,
                    1 => &mut bounds.y,
                    2 => &mut bounds.w,
                    _ => &mut bounds.h,
                } = value;
                assert!(matches!(
                    store.dispatch(Op::ApplyTemplate {
                        screen_id: "screen".into(),
                        request: Box::new(r),
                    }),
                    Err(MuxError::InvalidTemplate(_))
                ));
                assert_eq!(
                    serde_json::to_value(store.get_workspace_state()).unwrap(),
                    before
                );
            }
        }
        let mut r = request();
        r.create_screen = false;
        r.replace_confirmed = true;
        r.pane_ids.clear();
        let bounds = &mut r.template.floating[0].bounds;
        bounds.x = -2.0;
        bounds.y = 3.0;
        bounds.w = 4.0;
        bounds.h = 5.0;
        store
            .dispatch(Op::ApplyTemplate {
                screen_id: "screen".into(),
                request: Box::new(r),
            })
            .unwrap();
        assert_eq!(
            store.get_screen(&"screen".into()).unwrap().floating[0].bounds,
            FloatBounds {
                x: -2.0,
                y: 3.0,
                w: 4.0,
                h: 5.0
            }
        );
    }

    #[test]
    fn template_skips_web_panes_and_extraction_prunes_viewers_with_normalized_sizes() {
        let mut r = request();
        r.template.panes.get_mut("b").unwrap().pane_type = "webview".into();
        r.template.panes.get_mut("b").unwrap().url = "https://example.invalid".into();
        let (state, effects) = apply_template(
            WorkspaceState::default(),
            "screen".into(),
            assign_template_ids(r.clone()),
        )
        .unwrap();
        assert_eq!(effects.len(), 2);
        assert!(matches!(
            state.screens[&ScreenId::from("screen")].root,
            LayoutNode::Slot { .. }
        ));
        r.template.panes.get_mut("a").unwrap().pane_type = "webview".into();
        r.template.panes.get_mut("a").unwrap().url = "https://example.invalid".into();
        assert!(apply_template(
            WorkspaceState::default(),
            "screen".into(),
            assign_template_ids(r)
        )
        .is_err());
        let (mut state, _) =
            apply_template(WorkspaceState::default(), "screen".into(), request()).unwrap();
        let screen = state.screens.get_mut(&ScreenId::from("screen")).unwrap();
        screen.panes.get_mut(&PaneId::from("pane-b")).unwrap().role = Some(PaneRole::Viewer);
        let template = extract_template(screen, "Terminals".into(), "").unwrap();
        assert_eq!(template.layout.slot.unwrap().0, ["pane-a"]);
        assert!(!template.panes.contains_key("pane-b"));
        screen.panes.get_mut(&PaneId::from("pane-a")).unwrap().role = Some(PaneRole::Viewer);
        assert!(extract_template(screen, "No tiled terminals".into(), "").is_err());
    }
}
