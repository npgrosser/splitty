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
pub use store::{Saved, TemplateStore};

/// The longest file name derived from a template's name, in bytes.
pub const MAX_FILE_NAME: usize = 128;

/// The file name a template named `name` is saved under: its ASCII letters
/// and digits, lowercased, every run of other characters as one `-`, without
/// a leading or trailing `-`, at most [`MAX_FILE_NAME`] bytes. `None` when
/// nothing is left.
pub fn file_name(name: &str) -> Option<String> {
    let mut slug = String::new();
    let mut gap = false;
    for character in name.trim().chars() {
        if character.is_ascii_alphanumeric() {
            if gap && !slug.is_empty() {
                slug.push('-');
            }
            gap = false;
            slug.push(character.to_ascii_lowercase());
        } else {
            gap = true;
        }
        if slug.len() >= MAX_FILE_NAME {
            break;
        }
    }
    slug.truncate(MAX_FILE_NAME);
    let slug = slug.trim_end_matches('-');
    (!slug.is_empty()).then(|| slug.to_owned())
}

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("validation error: {0}")]
    Validation(String),
    #[error("template not found: {0}")]
    NotFound(String),
    #[error("invalid slug: {0}")]
    InvalidSlug(String),
    #[error("template name has no letters or digits for a file name")]
    InvalidName,
    #[error("template exists: {0}")]
    Exists(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// templates, Saved Template Names: one rule for every window.
    #[test]
    fn file_names_keep_letters_and_digits_and_join_the_rest_with_one_dash() {
        assert_eq!(file_name("v1.2").as_deref(), Some("v1-2"));
        assert_eq!(
            file_name("  My Session  2 ").as_deref(),
            Some("my-session-2")
        );
        assert_eq!(
            file_name("--Build/Test__Run--").as_deref(),
            Some("build-test-run")
        );
        assert_eq!(
            file_name("caf\u{e9} au lait").as_deref(),
            Some("caf-au-lait")
        );
        assert_eq!(file_name("..."), None);
        assert_eq!(file_name("  "), None);
        assert_eq!(file_name("\u{65e5}\u{672c}"), None);
        let long = file_name(&"ab-".repeat(100)).expect("name");
        assert!(long.len() <= MAX_FILE_NAME, "{}", long.len());
        assert!(!long.ends_with('-'));
        assert_eq!(
            file_name(&"x".repeat(300)).map(|s| s.len()),
            Some(MAX_FILE_NAME)
        );
    }
}
