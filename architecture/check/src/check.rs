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
    let mut ids: BTreeMap<&str, &str> = BTreeMap::new();
    for (id, c) in &model.components {
        if let Some(other) = ids.insert(&c.name, id) {
            violations.push(format!(
                "{}: {other} and {id} have the same name; relationships could not tell them apart",
                c.name
            ));
        }
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

/// Production code of the crate `source` names the crate `target` only from the components in
/// `through`. Test code (see [`crate::code::is_test`]) may name it anywhere.
pub fn production_crossings(
    code: &Code,
    model: &Model,
    repository: &Path,
    (source, target): (&str, &str),
    through: &[&str],
) -> Vec<String> {
    let mut violations = scanned(code, source);
    for reference in code.references.iter().filter(|r| !r.test) {
        let Some(file) = code.files.get(&reference.file) else {
            continue;
        };
        if file.module[0] != source || reference.path[0] != target {
            continue;
        }
        let owner = owner(file, model).map(|c| c.name.as_str());
        if !owner.is_some_and(|name| through.contains(&name)) {
            violations.push(format!(
                "{}:{} names {}: production code of {source} names {target} only from {}",
                relative(&reference.file, repository).display(),
                reference.line,
                display(&reference.path),
                through.join(", ")
            ));
        }
    }
    violations
}

/// The code of the crate `source`, tests included, names no crate of the workspace but itself
/// and `allowed`.
pub fn foreign_crates(
    code: &Code,
    repository: &Path,
    source: &str,
    allowed: &[&str],
) -> Vec<String> {
    let mut violations = scanned(code, source);
    let others = match allowed {
        [] => "no other crate of the workspace".to_owned(),
        _ => format!("no crate of the workspace but {}", allowed.join(", ")),
    };
    for reference in &code.references {
        let Some(file) = code.files.get(&reference.file) else {
            continue;
        };
        let target = reference.path[0].as_str();
        if file.module[0] != source || target == source || allowed.contains(&target) {
            continue;
        }
        violations.push(format!(
            "{}:{} names {}: {source} names {others}",
            relative(&reference.file, repository).display(),
            reference.line,
            display(&reference.path),
        ));
    }
    violations
}

/// A rule about a crate that was not scanned would hold for nothing.
fn scanned(code: &Code, name: &str) -> Vec<String> {
    if code.files.values().any(|file| file.module[0] == name) {
        Vec::new()
    } else {
        vec![format!("{name}: no target of this crate was scanned")]
    }
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

    /// A crate `client` with a component allowed to name `server`, one that is not, and a test
    /// file, next to the library `server` and the binary `app`.
    fn client_and_server() -> (tempfile::TempDir, Code, Model) {
        let dir = tempfile::tempdir().expect("temporary directory");
        let files = [
            ("client/lib.rs", "pub mod link;\npub mod other;"),
            ("client/link.rs", "pub fn f() { server::run() }"),
            (
                "client/other.rs",
                "pub fn f() { crate::link::f() }\n\
                 #[cfg(test)]\nmod tests {\n    fn g() { server::run() }\n}\n\
                 #[cfg(test)]\nmod more;",
            ),
            ("client/other/more.rs", "fn h() { server::run() }"),
            ("server/lib.rs", "pub fn run() {}"),
            ("app/main.rs", "fn main() { client::link::f() }"),
        ];
        for (name, text) in files {
            let path = dir.path().join(name);
            fs::create_dir_all(path.parent().expect("parent")).expect("create directory");
            fs::write(path, text).expect("write file");
        }
        let target = |name: &str, root: &str, library| Target {
            crate_name: name.into(),
            root: dir.path().join(root),
            library,
        };
        let code = scan(&[
            target("client", "client/lib.rs", true),
            target("server", "server/lib.rs", true),
            target("app", "app/main.rs", false),
        ]);
        let model = model(
            &[
                ("client_root", "client", false),
                ("link", "client::link", false),
                ("other", "client::other", false),
                ("server_root", "server", false),
                ("app", "app", false),
            ],
            &[],
        );
        (dir, code, model)
    }

    #[test]
    fn production_code_crosses_into_a_crate_only_through_named_components() {
        let (dir, code, model) = client_and_server();
        let rule = ("client", "server");
        assert!(production_crossings(&code, &model, dir.path(), rule, &["link"]).is_empty());
        assert_eq!(
            production_crossings(&code, &model, dir.path(), rule, &["other"]),
            ["client/link.rs:1 names server::run: production code of client names server only from other"]
        );
        assert_eq!(
            production_crossings(&code, &model, dir.path(), ("gone", "server"), &[]),
            ["gone: no target of this crate was scanned"]
        );
    }

    #[test]
    fn a_crate_names_only_the_allowed_crates_of_the_workspace() {
        let (dir, code, _model) = client_and_server();
        assert!(foreign_crates(&code, dir.path(), "app", &["client"]).is_empty());
        assert_eq!(
            foreign_crates(&code, dir.path(), "client", &[]),
            [
                "client/link.rs:1 names server::run: client names no other crate of the workspace",
                "client/other/more.rs:1 names server::run: client names no other crate of the workspace",
                "client/other.rs:4 names server::run: client names no other crate of the workspace",
            ]
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

    #[test]
    fn component_names_are_unique_across_containers() {
        let (_dir, code) = demo();
        let mut twice = model(&[("root", "demo", false), ("a", "demo::a", false)], &[]);
        let mut other = twice.components["s.c.a"].clone();
        other.module = vec!["demo".into(), "b".into()];
        other.planned = true;
        twice.components.insert("s.d.a".into(), other);
        assert_eq!(
            component_modules(&code, &twice),
            ["a: s.c.a and s.d.a have the same name; relationships could not tell them apart"]
        );
    }
}
