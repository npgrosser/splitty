//! Template data model: `Template`, `Pane`, `LayoutNode`, `FloatingEntry`, and validation.
//!
//! A `Template` contains named panes and a recursive `LayoutNode` tree (split/slot).
//! `SlotPanes` supports both scalar and array deserialization from YAML.
//! `validate` ensures referential integrity between panes and layout nodes.
//! Reading a template drops the panes of the removed type `webview`.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::TemplateError;

/// A named layout definition with pane commands.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "StoredTemplate")]
pub struct Template {
    pub name: String,
    pub panes: HashMap<String, Pane>,
    pub layout: LayoutNode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub floating: Vec<FloatingEntry>,
}

impl Template {
    /// Retain commands for selected pane names. A discarded command must not
    /// retain its restore behavior when this template is saved or applied.
    pub fn retain_commands(&mut self, mut keep: impl FnMut(&str) -> bool) {
        for (name, pane) in &mut self.panes {
            if !keep(name) {
                pane.command.clear();
                pane.restore_mode.clear();
            }
        }
    }
}

/// A template as it is read, from its file, a request or a host's answer.
#[derive(Deserialize)]
struct StoredTemplate {
    name: String,
    panes: HashMap<String, Pane>,
    layout: LayoutNode,
    #[serde(default)]
    floating: Vec<FloatingEntry>,
}

/// The type of the web panes templates held before they were removed.
const LEGACY_WEB_PANE: &str = "webview";

impl From<StoredTemplate> for Template {
    /// Drop legacy web panes with their layout references (`templates`,
    /// Template Validation): an emptied slot goes, and a split left with one
    /// child is that child in the split's place.
    fn from(stored: StoredTemplate) -> Self {
        let StoredTemplate {
            name,
            mut panes,
            layout,
            floating,
        } = stored;
        let web: HashSet<String> = panes
            .iter()
            .filter(|(_, pane)| pane.pane_type == LEGACY_WEB_PANE)
            .map(|(name, _)| name.clone())
            .collect();
        if web.is_empty() {
            return Self {
                name,
                panes,
                layout,
                floating,
            };
        }
        panes.retain(|name, _| !web.contains(name));
        Self {
            name,
            panes,
            // Nothing left: an empty node, which validation refuses.
            layout: without_panes(layout, &web).unwrap_or_default(),
            floating: floating
                .into_iter()
                .filter_map(|entry| {
                    Some(FloatingEntry {
                        layout: without_panes(entry.layout, &web)?,
                        bounds: entry.bounds,
                    })
                })
                .collect(),
        }
    }
}

/// `node` without references to `names`; `None` when nothing is left.
fn without_panes(node: LayoutNode, names: &HashSet<String>) -> Option<LayoutNode> {
    if let Some(SlotPanes(slot)) = node.slot {
        let kept: Vec<_> = slot
            .into_iter()
            .filter(|name| !names.contains(name))
            .collect();
        if kept.is_empty() {
            return None;
        }
        return Some(LayoutNode {
            slot: Some(SlotPanes(kept)),
            ..node
        });
    }
    let before = node.children.len();
    let mut children: Vec<_> = node
        .children
        .into_iter()
        .filter_map(|child| without_panes(child, names))
        .collect();
    if children.len() == before {
        return Some(LayoutNode { children, ..node });
    }
    match children.len() {
        0 => None,
        1 => children.pop().map(|child| LayoutNode {
            ratio: node.ratio,
            ..child
        }),
        _ => Some(LayoutNode { children, ..node }),
    }
}

/// Defines a terminal pane's command and working directory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pane {
    /// Empty or `terminal`; reading a template drops `webview` panes.
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "type")]
    pub pane_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dir: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub restore_mode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shell_profile: String,
}

/// A recursive tree node describing splits and slots.
///
/// A leaf has a `slot` (list of pane references).
/// A branch has `split` ("horizontal" or "vertical") and `children`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LayoutNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<SlotPanes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<LayoutNode>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ratio: f64,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// Holds pane references for a slot.
///
/// In YAML it accepts both a scalar (`"editor"`) and a sequence (`["a", "b"]`).
/// When serializing, a single element is written as a scalar.
#[derive(Clone, Debug)]
pub struct SlotPanes(pub Vec<String>);

impl<'de> Deserialize<'de> for SlotPanes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Helper {
            Single(String),
            Multiple(Vec<String>),
        }
        match Helper::deserialize(deserializer)? {
            Helper::Single(s) => Ok(SlotPanes(vec![s])),
            Helper::Multiple(v) => Ok(SlotPanes(v)),
        }
    }
}

impl Serialize for SlotPanes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.0.len() == 1 {
            self.0[0].serialize(serializer)
        } else {
            self.0.serialize(serializer)
        }
    }
}

/// Describes a floating window that overlays the grid layout.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FloatingEntry {
    pub layout: LayoutNode,
    pub bounds: FloatBounds,
}

/// Normalised viewport coordinates (0-1) for a floating window.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FloatBounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Lightweight representation of a template for listing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub slug: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
}

/// Validate that a template is well-formed.
pub fn validate(template: &Template) -> Result<(), TemplateError> {
    if template.name.is_empty() {
        return Err(TemplateError::Validation(
            "template name is required".into(),
        ));
    }
    if template.panes.is_empty() {
        return Err(TemplateError::Validation(
            "template must define at least one pane".into(),
        ));
    }
    for (name, pane) in &template.panes {
        if !pane.pane_type.is_empty() && pane.pane_type != "terminal" {
            return Err(TemplateError::Validation(format!(
                "pane {name:?} has invalid type {:?}",
                pane.pane_type
            )));
        }
        if !pane.restore_mode.is_empty()
            && pane.restore_mode != "replay"
            && pane.restore_mode != "shell"
            && pane.restore_mode != "close"
        {
            return Err(TemplateError::Validation(format!(
                "pane {name:?} has invalid restore_mode {:?}",
                pane.restore_mode
            )));
        }
    }
    validate_node(&template.layout, &template.panes)?;
    for (i, entry) in template.floating.iter().enumerate() {
        validate_node(&entry.layout, &template.panes)
            .map_err(|e| TemplateError::Validation(format!("floating[{i}]: {e}")))?;
    }
    Ok(())
}

fn validate_node(node: &LayoutNode, panes: &HashMap<String, Pane>) -> Result<(), TemplateError> {
    if let Some(ref slot) = node.slot {
        if slot.0.is_empty() {
            return Err(TemplateError::Validation(
                "slot must contain at least one pane reference".into(),
            ));
        }
        for name in &slot.0 {
            if !panes.contains_key(name) {
                return Err(TemplateError::Validation(format!(
                    "layout references undefined pane {name:?}"
                )));
            }
        }
        if !node.children.is_empty() {
            return Err(TemplateError::Validation(
                "slot node must not have children".into(),
            ));
        }
        return Ok(());
    }
    if let Some(ref split) = node.split {
        if split != "horizontal" && split != "vertical" {
            return Err(TemplateError::Validation(format!(
                "split must be \"horizontal\" or \"vertical\", got {split:?}"
            )));
        }
        if node.children.len() < 2 {
            return Err(TemplateError::Validation(
                "split node must have at least 2 children".into(),
            ));
        }
        for child in &node.children {
            validate_node(child, panes)?;
        }
        return Ok(());
    }
    Err(TemplateError::Validation(
        "layout node must have either \"slot\" or \"split\"".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slot_panes_deserialize_scalar() {
        let sp: SlotPanes = serde_yaml::from_str("editor").unwrap();
        assert_eq!(sp.0, vec!["editor"]);
    }

    #[test]
    fn test_slot_panes_deserialize_array() {
        let sp: SlotPanes = serde_yaml::from_str("[\"a\", \"b\"]").unwrap();
        assert_eq!(sp.0, vec!["a", "b"]);
    }

    #[test]
    fn test_slot_panes_serialize_single() {
        let sp = SlotPanes(vec!["x".into()]);
        let yaml = serde_yaml::to_string(&sp).unwrap();
        assert_eq!(yaml.trim(), "x");
    }

    #[test]
    fn test_slot_panes_serialize_multiple() {
        let sp = SlotPanes(vec!["a".into(), "b".into()]);
        let yaml = serde_yaml::to_string(&sp).unwrap();
        assert!(yaml.contains("- a"));
        assert!(yaml.contains("- b"));
    }

    #[test]
    fn test_template_yaml_roundtrip() {
        let yaml_str = r#"
name: test
panes:
  editor:
    command: vim
    dir: /tmp
  term:
    command: bash
layout:
  split: horizontal
  children:
    - slot: editor
      ratio: 0.6
    - slot: term
      ratio: 0.4
"#;
        let t: Template = serde_yaml::from_str(yaml_str).unwrap();
        assert_eq!(t.name, "test");
        assert_eq!(t.panes.len(), 2);

        let serialized = serde_yaml::to_string(&t).unwrap();
        let t2: Template = serde_yaml::from_str(&serialized).unwrap();
        assert_eq!(t2.name, "test");
        assert_eq!(t2.panes.len(), 2);
    }

    #[test]
    fn test_shell_profile_yaml_roundtrip_and_old_template_default() {
        let old: Template =
            serde_yaml::from_str("name: old\npanes:\n  shell: {}\nlayout:\n  slot: shell\n")
                .unwrap();
        assert!(old.panes["shell"].shell_profile.is_empty());

        let with_profile: Template = serde_yaml::from_str(
            "name: profiled\npanes:\n  shell:\n    shell_profile: shell-fish\nlayout:\n  slot: shell\n",
        )
        .unwrap();
        let yaml = serde_yaml::to_string(&with_profile).unwrap();
        assert!(yaml.contains("shell_profile: shell-fish"));
        let restored: Template = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(restored.panes["shell"].shell_profile, "shell-fish");
    }

    fn make_valid_template() -> Template {
        Template {
            name: "test".into(),
            panes: HashMap::from([
                (
                    "editor".into(),
                    Pane {
                        pane_type: "terminal".into(),
                        command: "vim".into(),
                        dir: String::new(),
                        restore_mode: String::new(),
                        shell_profile: String::new(),
                    },
                ),
                (
                    "term".into(),
                    Pane {
                        pane_type: String::new(),
                        command: "bash".into(),
                        dir: String::new(),
                        restore_mode: "replay".into(),
                        shell_profile: String::new(),
                    },
                ),
            ]),
            layout: LayoutNode {
                slot: None,
                split: Some("horizontal".into()),
                children: vec![
                    LayoutNode {
                        slot: Some(SlotPanes(vec!["editor".into()])),
                        split: None,
                        children: vec![],
                        ratio: 0.6,
                    },
                    LayoutNode {
                        slot: Some(SlotPanes(vec!["term".into()])),
                        split: None,
                        children: vec![],
                        ratio: 0.4,
                    },
                ],
                ratio: 0.0,
            },
            floating: vec![],
        }
    }

    #[test]
    fn test_validate_empty_name() {
        let mut t = make_valid_template();
        t.name = String::new();
        assert!(validate(&t).is_err());
    }

    #[test]
    fn retained_commands_keep_launch_metadata_and_clear_discarded_restore_modes() {
        let mut t = make_valid_template();
        for pane in t.panes.values_mut() {
            pane.command = "printf hello".into();
            pane.restore_mode = "close".into();
            pane.dir = "./work".into();
            pane.shell_profile = "test-shell".into();
        }
        let before = serde_json::to_value(&t).unwrap();
        t.retain_commands(|name| name == "editor");
        assert_eq!(
            serde_json::to_value(&t.panes["editor"]).unwrap(),
            before["panes"]["editor"]
        );
        assert!(t.panes["term"].command.is_empty());
        assert!(t.panes["term"].restore_mode.is_empty());
        assert_eq!(t.panes["term"].dir, "./work");
        assert_eq!(t.panes["term"].shell_profile, "test-shell");
        assert_eq!(serde_json::to_value(&t.layout).unwrap(), before["layout"]);
        t.retain_commands(|_| false);
        assert!(t
            .panes
            .values()
            .all(|pane| pane.command.is_empty() && pane.restore_mode.is_empty()));
    }

    #[test]
    fn test_validate_no_panes() {
        let mut t = make_valid_template();
        t.panes.clear();
        assert!(validate(&t).is_err());
    }

    /// templates, Template Validation (Saved template with a web pane): a
    /// file of an earlier release reads without its web pane, whose slot
    /// goes and whose split becomes the remaining terminal's slot.
    #[test]
    fn reading_drops_legacy_web_panes_and_collapses_their_layout() {
        let yaml = "\
name: Mixed
panes:
  a:
    command: vim
  b:
    type: webview
    url: https://example.invalid
  c: {}
  d:
    type: webview
    url: https://example.invalid
layout:
  split: horizontal
  ratio: 2
  children:
    - slot: a
      ratio: 3
    - split: vertical
      ratio: 1
      children:
        - slot: [b, c]
        - slot: d
floating:
  - layout:
      slot: d
    bounds: {x: 0.1, y: 0.1, w: 0.5, h: 0.5}
";
        let template: Template = serde_yaml::from_str(yaml).unwrap();
        let mut names: Vec<_> = template.panes.keys().cloned().collect();
        names.sort();
        assert_eq!(names, ["a", "c"]);
        validate(&template).unwrap();
        let layout = serde_json::to_value(&template.layout).unwrap();
        assert_eq!(
            layout,
            serde_json::json!({"split": "horizontal", "ratio": 2.0, "children": [
                {"slot": "a", "ratio": 3.0}, {"slot": "c", "ratio": 1.0}]})
        );
        assert!(template.floating.is_empty());
        let saved = serde_yaml::to_string(&template).unwrap();
        assert!(
            !saved.contains("webview") && !saved.contains("url"),
            "{saved}"
        );
        // Only web panes: nothing to apply.
        let web_only: Template = serde_json::from_value(serde_json::json!({
            "name": "Web", "panes": {"w": {"type": "webview", "url": "https://example.invalid"}},
            "layout": {"slot": "w"}
        }))
        .unwrap();
        assert!(web_only.panes.is_empty());
        assert!(validate(&web_only).is_err());
    }

    #[test]
    fn test_validate_bad_pane_type() {
        let mut t = make_valid_template();
        t.panes.get_mut("editor").unwrap().pane_type = "invalid".into();
        assert!(validate(&t).is_err());
    }

    #[test]
    fn test_validate_bad_restore_mode() {
        let mut t = make_valid_template();
        t.panes.get_mut("term").unwrap().restore_mode = "bad".into();
        assert!(validate(&t).is_err());
    }

    #[test]
    fn test_validate_split_one_child() {
        let mut t = make_valid_template();
        t.layout.children = vec![LayoutNode {
            slot: Some(SlotPanes(vec!["editor".into()])),
            split: None,
            children: vec![],
            ratio: 1.0,
        }];
        assert!(validate(&t).is_err());
    }

    #[test]
    fn test_validate_slot_refs_missing_pane() {
        let mut t = make_valid_template();
        t.layout.children[0].slot = Some(SlotPanes(vec!["nonexistent".into()]));
        assert!(validate(&t).is_err());
    }

    #[test]
    fn test_validate_valid_template() {
        let t = make_valid_template();
        assert!(validate(&t).is_ok());
    }
}
