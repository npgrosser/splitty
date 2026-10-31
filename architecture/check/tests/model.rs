//! The architecture model (`architecture/*.c4`) and the module dependencies of the workspace
//! agree, in both directions. Conventions: `architecture/README.md`.
//!
//! The model is read once per run through the pinned LikeC4 CLI. Without it the checks fail,
//! never skip.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use splitty_architecture::check::{self, Dependencies};
use splitty_architecture::code::{self, Code};
use splitty_architecture::model::{self, Model};
use splitty_architecture::workspace;

struct Checked {
    model: Model,
    code: Code,
    dependencies: Dependencies,
}

fn repository() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .expect("architecture/check lies two levels below the repository root")
        .to_path_buf()
}

fn checked() -> &'static Checked {
    static CHECKED: OnceLock<Result<Checked, String>> = OnceLock::new();
    let result = CHECKED.get_or_init(|| {
        let repository = repository();
        let architecture = repository.join("architecture");
        let cli = architecture.join("node_modules/.bin/likec4");
        let model = model::read(&cli, &architecture, Path::new(env!("CARGO_TARGET_TMPDIR")))?;
        let targets = workspace::targets(&repository, env!("CARGO_PKG_NAME"))?;
        let code = code::scan(&targets);
        let dependencies = check::dependencies(&code, &model);
        Ok(Checked {
            model,
            code,
            dependencies,
        })
    });
    match result {
        Ok(checked) => checked,
        Err(error) => panic!("{error}"),
    }
}

fn assert_none(violations: Vec<String>) {
    assert!(violations.is_empty(), "\n{}", violations.join("\n"));
}

#[test]
fn model_validates() {
    assert_none(check::model_errors(&checked().model, &repository()));
}

#[test]
fn every_module_file_is_found() {
    assert_none(check::scan_problems(&checked().code));
}

#[test]
fn every_file_with_code_has_a_component() {
    let c = checked();
    assert_none(check::unowned_files(&c.code, &c.model, &repository()));
}

#[test]
fn every_component_names_its_module() {
    let c = checked();
    assert_none(check::component_modules(&c.code, &c.model));
}

#[test]
fn every_dependency_has_a_relationship() {
    let c = checked();
    assert_none(check::missing_relationships(
        &c.dependencies,
        &c.model,
        &repository(),
    ));
}

#[test]
fn every_relationship_is_backed_by_a_dependency() {
    let c = checked();
    assert_none(check::unbacked_relationships(&c.dependencies, &c.model));
}

#[test]
fn every_spec_exists() {
    assert_none(check::missing_specs(
        &checked().model,
        &repository().join("openspec/specs"),
    ));
}

/// Within the mux, only the effect runner touches the PTY layer, and the reducer, ops,
/// validation, state and grid never run effects (`openspec/config.yaml`, architecture
/// invariants). A relationship added to the model to let such an import pass fails here.
#[test]
fn the_mux_stays_pure() {
    let violations = checked()
        .model
        .relationships
        .keys()
        .filter(|(source, target)| source == "mux" && (target == "pty" || target == "effect_runner"))
        .map(|(source, target)| {
            format!("relationship {source} -> {target}: the mux is pure; only effect_runner runs effects and touches pty")
        })
        .collect();
    assert_none(violations);
}
