//! Lists all available theme names: bundled themes plus custom `.toml` files
//! found in the user's themes directory.

use std::path::Path;

use super::bundled;

/// Lists all available theme names: bundled + custom from themes_dir.
pub fn list_themes(themes_dir: &Path) -> Vec<String> {
    let mut names = bundled::bundled_names();

    if let Ok(entries) = std::fs::read_dir(themes_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    let name = stem.to_string();
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
        }
    }

    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_includes_bundled() {
        let dir = tempfile::tempdir().unwrap();
        let themes = list_themes(dir.path());
        assert!(themes.contains(&"splitty-dark".to_string()));
        assert!(themes.contains(&"splitty-light".to_string()));
    }

    #[test]
    fn test_list_includes_custom() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("my-theme.toml"), "").unwrap();
        let themes = list_themes(dir.path());
        assert!(themes.contains(&"my-theme".to_string()));
    }
}
