//! Saved layout templates: YAML-based definitions of multi-pane screen layouts.
//!
//! A template names a set of panes (each with a command and working directory)
//! and a recursive layout tree that arranges them into splits and slots.
//! Templates are persisted as YAML files in `~/.config/splitty/templates/`
//! and managed through the `TemplateStore` trait (with `FileStore` as the
//! production implementation).

pub mod file_store;
pub mod model;
pub mod store;

pub use file_store::FileStore;
pub use model::{
    validate, FloatBounds, FloatingEntry, LayoutNode, Pane, SlotPanes, Summary, Template,
};
pub use store::TemplateStore;

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("validation error: {0}")]
    Validation(String),
    #[error("template not found: {0}")]
    NotFound(String),
    #[error("invalid slug: {0}")]
    InvalidSlug(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
}
