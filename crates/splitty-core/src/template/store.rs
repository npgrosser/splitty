//! The `TemplateStore` trait: CRUD interface for layout templates.

use super::model::{Summary, Template};
use super::TemplateError;

/// Where a save by name went (`templates`, Saved Template Names).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Saved {
    pub slug: String,
    /// A saved template of that file name was replaced.
    pub replaced: bool,
}

/// Provides CRUD operations for templates.
pub trait TemplateStore: Send + Sync {
    fn list(&self) -> Result<Vec<Summary>, TemplateError>;
    fn get(&self, slug: &str) -> Result<Template, TemplateError>;
    fn save(&self, slug: &str, template: &Template) -> Result<(), TemplateError>;
    /// Save under the file name derived from the template's name: only when
    /// none of that name exists, or replacing it with `overwrite`. The check
    /// and the save are one step.
    fn save_named(&self, template: &Template, overwrite: bool) -> Result<Saved, TemplateError>;
    fn delete(&self, slug: &str) -> Result<(), TemplateError>;
}
