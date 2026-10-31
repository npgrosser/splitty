//! File-system backed template store using YAML files.
//!
//! Templates are stored as `<slug>.yaml` in the templates directory.
//! Slugs are validated to prevent path traversal attacks.

use std::path::PathBuf;

use super::model::{validate, Summary, Template};
use super::store::TemplateStore;
use super::TemplateError;

/// File-system backed template store using YAML files.
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// Create a new `FileStore` with an explicit directory.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Create a new `FileStore` using the default config directory
    /// (`~/.config/splitty/templates/`).
    pub fn new_default() -> Result<Self, TemplateError> {
        let dir = crate::config::config_dir().join("templates");
        Ok(Self { dir })
    }

    /// Validate that a slug does not contain path traversal characters.
    fn validate_slug(slug: &str) -> Result<(), TemplateError> {
        if slug.contains('/') || slug.contains('\\') || slug.contains("..") || slug.is_empty() {
            return Err(TemplateError::InvalidSlug(slug.into()));
        }
        Ok(())
    }
}

impl TemplateStore for FileStore {
    fn list(&self) -> Result<Vec<Summary>, TemplateError> {
        let pattern = self.dir.join("*.yaml");
        let pattern_str = pattern.to_string_lossy();
        let matches = glob::glob(&pattern_str)
            .map_err(|e| TemplateError::Io(std::io::Error::other(e.to_string())))?;

        let mut summaries = Vec::new();
        for entry in matches {
            let path = match entry {
                Ok(p) => p,
                Err(_) => continue,
            };
            let data = match std::fs::read_to_string(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let t: Template = match serde_yaml::from_str(&data) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let slug = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let display_name = if t.name.is_empty() {
                slug.clone()
            } else {
                t.name
            };
            summaries.push(Summary { slug, display_name });
        }
        Ok(summaries)
    }

    fn get(&self, slug: &str) -> Result<Template, TemplateError> {
        Self::validate_slug(slug)?;
        let path = self.dir.join(format!("{slug}.yaml"));
        let data = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TemplateError::NotFound(slug.into())
            } else {
                TemplateError::Io(e)
            }
        })?;
        let t: Template = serde_yaml::from_str(&data)?;
        validate(&t)?;
        Ok(t)
    }

    fn save(&self, slug: &str, template: &Template) -> Result<(), TemplateError> {
        Self::validate_slug(slug)?;
        validate(template)?;

        crate::config::ensure_private_dir(&self.dir)?;

        let data = serde_yaml::to_string(template)?;
        let path = self.dir.join(format!("{slug}.yaml"));
        let tmp = self.dir.join(format!("{slug}.yaml.tmp"));

        std::fs::write(&tmp, data.as_bytes())?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn delete(&self, slug: &str) -> Result<(), TemplateError> {
        Self::validate_slug(slug)?;
        let path = self.dir.join(format!("{slug}.yaml"));
        std::fs::remove_file(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TemplateError::NotFound(slug.into())
            } else {
                TemplateError::Io(e)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::model::{LayoutNode, Pane, SlotPanes};
    use std::collections::HashMap;

    #[test]
    fn test_slug_validation() {
        assert!(FileStore::validate_slug("my-template").is_ok());
        assert!(FileStore::validate_slug("foo/bar").is_err());
        assert!(FileStore::validate_slug("foo\\bar").is_err());
        assert!(FileStore::validate_slug("..hidden").is_err());
        assert!(FileStore::validate_slug("").is_err());
    }

    fn make_test_template() -> Template {
        Template {
            name: "Test Template".into(),
            panes: HashMap::from([(
                "main".into(),
                Pane {
                    pane_type: String::new(),
                    url: String::new(),
                    command: "bash".into(),
                    dir: String::new(),
                    restore_mode: String::new(),
                    shell_profile: String::new(),
                },
            )]),
            layout: LayoutNode {
                slot: Some(SlotPanes(vec!["main".into()])),
                split: None,
                children: vec![],
                ratio: 0.0,
            },
            floating: vec![],
        }
    }

    #[test]
    fn test_file_store_crud() {
        let dir = std::env::temp_dir().join(format!("splitty-test-{}", uuid::Uuid::new_v4()));
        let store = FileStore::new(dir.clone());
        let template = make_test_template();

        // Save
        store.save("demo", &template).unwrap();
        assert!(dir.join("demo.yaml").exists());

        // Get
        let loaded = store.get("demo").unwrap();
        assert_eq!(loaded.name, "Test Template");
        assert!(loaded.panes.contains_key("main"));

        // List
        let summaries = store.list().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].slug, "demo");
        assert_eq!(summaries[0].display_name, "Test Template");

        // Delete
        store.delete("demo").unwrap();
        assert!(!dir.join("demo.yaml").exists());
        assert!(store.get("demo").is_err());

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }
}
