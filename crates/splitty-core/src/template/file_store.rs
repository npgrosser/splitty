//! File-system backed template store using YAML files.
//!
//! Templates are stored as `<slug>.yaml` in the templates directory.
//! Slugs are validated to prevent path traversal attacks.

use std::path::PathBuf;

use super::model::{validate, Summary, Template};
use super::store::{Saved, TemplateStore};
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

    /// Write `template` completely to a temporary file of its own beside
    /// `<slug>.yaml`, which no listing reads.
    fn write_temporary(&self, slug: &str, template: &Template) -> Result<PathBuf, TemplateError> {
        crate::config::ensure_private_dir(&self.dir)?;
        let data = serde_yaml::to_string(template)?;
        let tmp = self
            .dir
            .join(format!(".{slug}.{}.tmp", uuid::Uuid::new_v4().simple()));
        std::fs::write(&tmp, data.as_bytes())?;
        Ok(tmp)
    }
}

/// Make the complete file `tmp` appear as `path` only when `path` does not
/// exist; `Ok(false)` when it does. A file system without hard links gets an
/// exclusive create of `path` instead.
fn publish_if_absent(tmp: &std::path::Path, path: &std::path::Path) -> std::io::Result<bool> {
    match std::fs::hard_link(tmp, path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(_) => {
            use std::io::Write;
            let mut file = match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Ok(false)
                }
                Err(error) => return Err(error),
            };
            file.write_all(&std::fs::read(tmp)?)?;
            Ok(true)
        }
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

        let tmp = self.write_temporary(slug, template)?;
        let path = self.dir.join(format!("{slug}.yaml"));
        std::fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        Ok(())
    }

    fn save_named(&self, template: &Template, overwrite: bool) -> Result<Saved, TemplateError> {
        let slug = super::file_name(&template.name).ok_or(TemplateError::InvalidName)?;
        Self::validate_slug(&slug)?;
        validate(template)?;
        let tmp = self.write_temporary(&slug, template)?;
        let path = self.dir.join(format!("{slug}.yaml"));
        let result = match publish_if_absent(&tmp, &path) {
            Ok(true) => Ok(Saved {
                slug,
                replaced: false,
            }),
            Ok(false) if overwrite => std::fs::rename(&tmp, &path)
                .map(|()| Saved {
                    slug,
                    replaced: true,
                })
                .map_err(TemplateError::from),
            Ok(false) => Err(TemplateError::Exists(slug)),
            Err(error) => Err(error.into()),
        };
        // Gone after a rename; the hard link or exclusive copy leaves it.
        let _ = std::fs::remove_file(&tmp);
        result
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

    /// templates, Saved Template Names: saved if absent, refused when the
    /// file exists, replaced with `overwrite`; no temporary file stays.
    #[test]
    fn a_save_by_name_refuses_an_existing_file_unless_it_overwrites() {
        let dir = tempfile::tempdir().expect("dir");
        let store = FileStore::new(dir.path().join("templates"));
        let mut template = make_test_template();
        template.name = "v1.2".into();
        assert_eq!(
            store.save_named(&template, false).expect("saved"),
            Saved {
                slug: "v1-2".into(),
                replaced: false
            }
        );
        let mut other = make_test_template();
        other.name = "V1 2".into();
        assert!(matches!(
            store.save_named(&other, false),
            Err(TemplateError::Exists(slug)) if slug == "v1-2"
        ));
        assert_eq!(store.get("v1-2").expect("kept").name, "v1.2");
        assert_eq!(
            store.save_named(&other, true).expect("replaced"),
            Saved {
                slug: "v1-2".into(),
                replaced: true
            }
        );
        assert_eq!(store.get("v1-2").expect("replaced").name, "V1 2");
        other.name = "...".into();
        assert!(matches!(
            store.save_named(&other, true),
            Err(TemplateError::InvalidName)
        ));
        let files: Vec<_> = std::fs::read_dir(dir.path().join("templates"))
            .expect("dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(files, ["v1-2.yaml"]);
    }

    /// templates, Concurrent saves of a new name: exactly one wins.
    #[test]
    fn concurrent_saves_of_a_new_name_save_once() {
        let dir = tempfile::tempdir().expect("dir");
        let store = std::sync::Arc::new(FileStore::new(dir.path().to_path_buf()));
        let threads: Vec<_> = (0..8)
            .map(|index| {
                let store = store.clone();
                std::thread::spawn(move || {
                    let mut template = make_test_template();
                    template.name = "Race".into();
                    template.panes.get_mut("main").expect("pane").command = format!("echo {index}");
                    store.save_named(&template, false)
                })
            })
            .collect();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().expect("thread"))
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| matches!(error, TemplateError::Exists(slug) if slug == "race")));
        assert_eq!(store.list().expect("list").len(), 1);
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
