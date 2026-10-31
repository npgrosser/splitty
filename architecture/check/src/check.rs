//! The checks. Each returns every violation it finds, so one run lists all of them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::code::{display, Code, File, Reference};
use crate::model::{Component, Model, PLANNED};

/// The component that owns `file`: for a crate root, the component naming that crate; for any
/// other file, the component whose module is the longest prefix of the file's module. A
/// component naming a crate root owns only the root file, so a new module of that crate still
/// needs a component of its own.
pub fn owner<'m>(file: &File, model: &'m Model) -> Option<&'m Component> {
    let mut components = model.components.values();
    if file.crate_root {
        return components.find(|c| c.module == file.module);
    }
    components
        .filter(|c| c.module.len() > 1 && file.module.starts_with(&c.module))
        .max_by_key(|c| c.module.len())
}

/// References from one component to another, by (source, target) component name.
pub type Dependencies = BTreeMap<(String, String), Vec<Reference>>;

pub fn dependencies(code: &Code, model: &Model) -> Dependencies {
    let mut found = Dependencies::new();
    for reference in &code.references {
        let Some(source) = code
            .files
            .get(&reference.file)
            .and_then(|f| owner(f, model))
        else {
            continue;
        };
        let Some(target) = code
            .module_of(&reference.path)
            .and_then(|module| code.files.get(&code.modules[module]))
            .and_then(|file| owner(file, model))
        else {
            continue;
        };
        if source != target {
            let key = (source.name.clone(), target.name.clone());
            found.entry(key).or_default().push(reference.clone());
        }
    }
    found
}

fn relative<'p>(path: &'p Path, repository: &Path) -> &'p Path {
    path.strip_prefix(repository).unwrap_or(path)
}

pub fn model_errors(model: &Model, repository: &Path) -> Vec<String> {
    let mut violations: Vec<String> = model
        .validation
        .errors
        .iter()
        .map(|e| {
            let file = relative(Path::new(&e.file), repository).display();
            format!("{file}:{}: {}", e.line + 1, e.message)
        })
        .collect();
    if !model.validation.valid && violations.is_empty() {
        violations.push("LikeC4 reports the model as invalid".to_owned());
    }
    violations
}

pub fn scan_problems(code: &Code) -> Vec<String> {
    code.problems.clone()
}

pub fn unowned_files(code: &Code, model: &Model, repository: &Path) -> Vec<String> {
    code.files
        .iter()
        .filter(|(_, file)| file.has_code && owner(file, model).is_none())
        .map(|(path, file)| {
            let path = relative(path, repository).display();
            let module = display(&file.module);
            if file.crate_root {
                format!("{path}: no component names the crate root {module}")
            } else {
                format!("{path}: no component's module is a prefix of {module}")
            }
        })
        .collect()
}

pub fn component_modules(code: &Code, model: &Model) -> Vec<String> {
    let modules: BTreeSet<&Vec<String>> = code.files.values().map(|file| &file.module).collect();
    let mut violations = Vec::new();
    let mut named: BTreeMap<&Vec<String>, &str> = BTreeMap::new();
    for c in model.components.values() {
        let module = display(&c.module);
        if let Some(other) = named.insert(&c.module, &c.name) {
            violations.push(format!(
                "{}: {module} is also the module of {other}",
                c.name
            ));
        }
        if c.planned && modules.contains(&c.module) {
            violations.push(format!(
                "{}: {module} exists; remove the tag #{PLANNED}",
                c.name
            ));
        } else if !c.planned && !modules.contains(&c.module) {
            violations.push(format!(
                "{}: {module} is not a module file of the workspace",
                c.name
            ));
        }
    }
    violations
}

pub fn missing_relationships(
    dependencies: &Dependencies,
    model: &Model,
    repository: &Path,
) -> Vec<String> {
    dependencies
        .iter()
        .filter(|(key, _)| !model.relationships.contains_key(*key))
        .flat_map(|((source, target), references)| {
            references.iter().map(move |r| {
                format!(
                    "{}:{} names {}: no relationship {source} -> {target}",
                    relative(&r.file, repository).display(),
                    r.line,
                    display(&r.path)
                )
            })
        })
        .collect()
}

pub fn unbacked_relationships(dependencies: &Dependencies, model: &Model) -> Vec<String> {
    let planned: BTreeSet<&str> = model
        .components
        .values()
        .filter(|c| c.planned)
        .map(|c| c.name.as_str())
        .collect();
    model
        .relationships
        .iter()
        .filter(|((source, target), _)| {
            !planned.contains(source.as_str())
                && !planned.contains(target.as_str())
                && !dependencies.contains_key(&(source.clone(), target.clone()))
        })
        .map(|((source, target), title)| {
            format!(
                "relationship {source} -> {target} '{title}': no module of {source} names {target}"
            )
        })
        .collect()
}

pub fn missing_specs(model: &Model, specs: &Path) -> Vec<String> {
    model
        .elements
        .iter()
        .filter(|(_, element)| !element.tags.iter().any(|tag| tag == PLANNED))
        .filter_map(|(id, element)| {
            let spec = element.metadata.get("spec")?;
            (!specs.join(spec).is_dir())
                .then(|| format!("{id}: spec '{spec}' is not a directory under openspec/specs/"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::{scan, Target};
    use crate::model::Validation;
    use std::fs;

    fn component(name: &str, module: &str, planned: bool) -> (String, Component) {
        let component = Component {
            name: name.to_owned(),
            module: module.split("::").map(str::to_owned).collect(),
            planned,
        };
        (format!("s.c.{name}"), component)
    }

    fn model(components: &[(&str, &str, bool)], relationships: &[(&str, &str)]) -> Model {
        Model {
            validation: Validation {
                valid: true,
                errors: Vec::new(),
            },
            elements: BTreeMap::new(),
            components: components
                .iter()
                .map(|(name, module, planned)| component(name, module, *planned))
                .collect(),
            relationships: relationships
                .iter()
                .map(|(s, t)| (((*s).to_owned(), (*t).to_owned()), String::new()))
                .collect(),
        }
    }

    /// A crate `demo` whose root holds code and calls into `a`, which calls back into the root.
    fn demo() -> (tempfile::TempDir, Code) {
        let dir = tempfile::tempdir().expect("temporary directory");
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("create directory");
        fs::write(src.join("lib.rs"), "pub mod a;\npub fn run() { a::f() }").expect("write");
        fs::write(src.join("a.rs"), "pub fn f() {}\nfn g() { crate::run() }").expect("write");
        let target = Target {
            crate_name: "demo".into(),
            root: src.join("lib.rs"),
            library: true,
        };
        let code = scan(&[target]);
        (dir, code)
    }

    #[test]
    fn a_crate_root_component_owns_only_the_root_file() {
        let (dir, code) = demo();
        let only_root = model(&[("root", "demo", false)], &[]);
        assert_eq!(
            unowned_files(&code, &only_root, dir.path()),
            ["src/a.rs: no component's module is a prefix of demo::a"]
        );
        let both = model(&[("root", "demo", false), ("a", "demo::a", false)], &[]);
        assert!(unowned_files(&code, &both, dir.path()).is_empty());
    }

    #[test]
    fn references_are_compared_with_relationships_both_ways() {
        let (dir, code) = demo();
        let components = [
            ("root", "demo", false),
            ("a", "demo::a", false),
            ("b", "demo::b", true),
        ];
        let stale = model(
            &components,
            &[("root", "a"), ("a", "b"), ("b", "a"), ("root", "b")],
        );
        let found = dependencies(&code, &stale);
        assert_eq!(
            missing_relationships(&found, &stale, dir.path()),
            ["src/a.rs:2 names demo::run: no relationship a -> root"]
        );
        // Relationships of the planned `b` are not compared with the code.
        assert!(unbacked_relationships(&found, &stale).is_empty());
        let reversed = model(&components, &[("a", "root"), ("a", "b")]);
        let found = dependencies(&code, &reversed);
        assert_eq!(
            missing_relationships(&found, &reversed, dir.path()),
            ["src/lib.rs:2 names demo::a::f: no relationship root -> a"]
        );
    }

    #[test]
    fn planned_components_name_modules_that_do_not_exist() {
        let (_dir, code) = demo();
        let wrong = model(
            &[
                ("a", "demo::a", true),
                ("b", "demo::b", false),
                ("c", "demo::a", false),
            ],
            &[],
        );
        assert_eq!(
            component_modules(&code, &wrong),
            [
                "a: demo::a exists; remove the tag #planned",
                "b: demo::b is not a module file of the workspace",
                "c: demo::a is also the module of a",
            ]
        );
    }
}
