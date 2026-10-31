//! The model side: `architecture/*.c4` read through the pinned LikeC4 CLI.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};

use serde::Deserialize;

use crate::code::ModulePath;

pub const PLANNED: &str = "planned";

pub const INSTALL: &str = "The architecture check needs the LikeC4 CLI: run \
    `pnpm install --dir architecture` in the repository root (Node 22.22.3 or later).";

// The parts of LikeC4's JSON that the check reads; other fields are ignored.

#[derive(Debug, Deserialize)]
pub struct Validation {
    pub valid: bool,
    pub errors: Vec<ModelError>,
}

#[derive(Debug, Deserialize)]
pub struct ModelError {
    pub file: String,
    /// 0-based.
    pub line: usize,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct Element {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct Endpoint {
    model: String,
}

#[derive(Debug, Deserialize)]
struct Relation {
    source: Endpoint,
    target: Endpoint,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Export {
    elements: BTreeMap<String, Element>,
    relations: BTreeMap<String, Relation>,
}

/// An element with `metadata { module '…' }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    /// The last part of its id, unique across the model.
    pub name: String,
    pub module: ModulePath,
    pub planned: bool,
}

#[derive(Debug)]
pub struct Model {
    pub validation: Validation,
    pub elements: BTreeMap<String, Element>,
    /// Components by element id.
    pub components: BTreeMap<String, Component>,
    /// Relationships between components, (source, target) by name, with their titles.
    pub relationships: BTreeMap<(String, String), String>,
}

impl Model {
    fn from_export(validation: Validation, export: Export) -> Self {
        let components: BTreeMap<String, Component> = export
            .elements
            .iter()
            .filter_map(|(id, element)| {
                let module = element.metadata.get("module")?;
                let component = Component {
                    name: id.rsplit('.').next().unwrap_or(id).to_owned(),
                    module: module.split("::").map(str::to_owned).collect(),
                    planned: element.tags.iter().any(|tag| tag == PLANNED),
                };
                Some((id.clone(), component))
            })
            .collect();
        let relationships = export
            .relations
            .values()
            .filter_map(|relation| {
                let source = components.get(&relation.source.model)?;
                let target = components.get(&relation.target.model)?;
                let title = relation.title.clone().unwrap_or_default();
                Some(((source.name.clone(), target.name.clone()), title))
            })
            .collect();
        Model {
            validation,
            elements: export.elements,
            components,
            relationships,
        }
    }
}

fn likec4(likec4: &Path, args: &[&str]) -> Result<Output, String> {
    if !likec4.exists() {
        return Err(format!("{} is missing. {INSTALL}", likec4.display()));
    }
    Command::new(likec4)
        .args(args)
        .output()
        .map_err(|error| format!("LikeC4 could not run: {error}. {INSTALL}"))
}

/// The model in `architecture`, validated and exported by LikeC4 at once; the export goes
/// through `scratch`.
pub fn read(cli: &Path, architecture: &Path, scratch: &Path) -> Result<Model, String> {
    let export_file = scratch.join("model.json");
    let directory = architecture.to_string_lossy();
    let file = export_file.to_string_lossy();
    let (validated, exported) = std::thread::scope(|scope| {
        let validating =
            scope.spawn(|| likec4(cli, &["validate", "--json", "--no-layout", &directory]));
        let exported = likec4(
            cli,
            &["export", "json", "--skip-layout", "-o", &file, &directory],
        );
        let validated = validating
            .join()
            .unwrap_or_else(|_| Err("the validation thread panicked".to_owned()));
        (validated, exported)
    });
    let (validated, exported) = (validated?, exported?);
    if !exported.status.success() || !export_file.exists() {
        return Err(format!(
            "likec4 export failed:\n{}\n{INSTALL}",
            String::from_utf8_lossy(&exported.stderr).trim()
        ));
    }
    let validation: Validation = serde_json::from_slice(&validated.stdout).map_err(|error| {
        format!(
            "Unexpected LikeC4 output: {error}\n{}\n{INSTALL}",
            String::from_utf8_lossy(&validated.stderr).trim()
        )
    })?;
    let export: Export = std::fs::read(&export_file)
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        .map_err(|error| format!("Unexpected LikeC4 export: {error}\n{INSTALL}"))?;
    Ok(Model::from_export(validation, export))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_are_elements_with_a_module() {
        let export: Export = serde_json::from_str(
            r#"{
              "elements": {
                "s": {"kind": "system"},
                "s.c.a": {"kind": "component", "tags": ["planned"], "metadata": {"module": "x::a", "spec": "cli"}},
                "s.c.b": {"kind": "component", "metadata": {"module": "x"}},
                "s.c.doc": {"kind": "component"}
              },
              "relations": {
                "r1": {"source": {"model": "s.c.a"}, "target": {"model": "s.c.b"}, "title": "uses"},
                "r2": {"source": {"model": "s.c.a"}, "target": {"model": "s"}}
              }
            }"#,
        )
        .expect("export");
        let validation = Validation {
            valid: true,
            errors: Vec::new(),
        };
        let model = Model::from_export(validation, export);
        assert_eq!(
            model.components["s.c.a"],
            Component {
                name: "a".into(),
                module: vec!["x".into(), "a".into()],
                planned: true
            }
        );
        assert!(!model.components["s.c.b"].planned);
        assert_eq!(model.components.len(), 2);
        let relationships: Vec<_> = model.relationships.into_iter().collect();
        assert_eq!(
            relationships,
            [(("a".to_owned(), "b".to_owned()), "uses".to_owned())]
        );
    }
}
