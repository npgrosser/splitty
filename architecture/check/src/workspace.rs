//! The workspace's library and binary targets, from `cargo metadata`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::code::Target;

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    targets: Vec<CargoTarget>,
}

#[derive(Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
}

const LIBRARY_KINDS: &[&str] = &["lib", "rlib", "dylib", "staticlib", "cdylib", "proc-macro"];

/// The library and binary targets of every workspace member except `exclude` (the check's own
/// crate). Tests, examples, benchmarks and build scripts are not part of the model.
pub fn targets(repository: &Path, exclude: &str) -> Result<Vec<Target>, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(repository.join("Cargo.toml"))
        .output()
        .map_err(|error| format!("cargo metadata could not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: Metadata = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("unexpected cargo metadata output: {error}"))?;
    let mut targets = Vec::new();
    for package in metadata.packages.into_iter().filter(|p| p.name != exclude) {
        for target in package.targets {
            let library = target
                .kind
                .iter()
                .any(|k| LIBRARY_KINDS.contains(&k.as_str()));
            if library || target.kind.iter().any(|k| k == "bin") {
                targets.push(Target {
                    crate_name: target.name.replace('-', "_"),
                    root: target.src_path,
                    library,
                });
            }
        }
    }
    targets.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
    Ok(targets)
}
