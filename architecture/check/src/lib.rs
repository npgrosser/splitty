//! The architecture check: compares the model in `architecture/*.c4` with the module
//! dependencies of the workspace's crates. `tests/model.rs` runs it; the conventions are in
//! `architecture/README.md`.

pub mod check;
pub mod code;
pub mod model;
pub mod workspace;
