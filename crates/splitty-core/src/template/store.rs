//! The `TemplateStore` trait: CRUD interface for layout templates.

use super::model::{Summary, Template};
use super::TemplateError;

/// Provides CRUD operations for templates.
pub trait TemplateStore: Send + Sync {
    fn list(&self) -> Result<Vec<Summary>, TemplateError>;
    fn get(&self, slug: &str) -> Result<Template, TemplateError>;
    fn save(&self, slug: &str, template: &Template) -> Result<(), TemplateError>;
    fn delete(&self, slug: &str) -> Result<(), TemplateError>;
}
