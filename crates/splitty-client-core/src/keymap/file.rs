//! The user's key bindings in `keybindings.json`, in the configuration
//! directory of the computer the application runs on, beside the remembered
//! appearance. Only the application reads it; no service sees it.
use super::{Overrides, Platform};
use std::io::Read as _;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "keybindings.json";
/// More than a file of every command with several chords holds.
const MAX_BYTES: u64 = 64 * 1024;

/// The file the overrides are kept in, and what it holds.
#[derive(Debug)]
pub struct KeybindingsFile {
    path: PathBuf,
    overrides: Overrides,
}

impl KeybindingsFile {
    pub fn default_path() -> PathBuf {
        splitty_core::config::config_dir().join(FILE_NAME)
    }

    /// Reads `path`. A missing or unreadable file holds no overrides; an
    /// oversized or malformed one holds none either and says so in the
    /// overrides' problems.
    pub fn open(path: PathBuf) -> Self {
        let overrides = read(&path);
        Self { path, overrides }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn overrides(&self) -> &Overrides {
        &self.overrides
    }

    /// Writes `overrides` atomically into a private file and keeps them.
    /// A failed write leaves the file and the kept overrides as they were.
    pub fn save(&mut self, overrides: Overrides) -> std::io::Result<()> {
        let contents = overrides.to_json(Platform::current());
        splitty_core::config::write_private_file(&self.path, contents.as_bytes())?;
        self.overrides = overrides;
        Ok(())
    }
}

fn read(path: &Path) -> Overrides {
    let Ok(file) = std::fs::File::open(path) else {
        return Overrides::default();
    };
    let mut contents = Vec::new();
    if file.take(MAX_BYTES + 1).read_to_end(&mut contents).is_err() {
        return Overrides::default();
    }
    if contents.len() as u64 > MAX_BYTES {
        let mut overrides = Overrides::default();
        overrides
            .problems
            .push("The keybindings file is too large; the default keys apply.".into());
        return overrides;
    }
    match String::from_utf8(contents) {
        Ok(text) => Overrides::from_json(&text),
        Err(_) => {
            let mut overrides = Overrides::default();
            overrides
                .problems
                .push("The keybindings file is not text; the default keys apply.".into());
            overrides
        }
    }
}
