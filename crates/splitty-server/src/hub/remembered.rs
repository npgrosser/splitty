//! The remote screens a client shows, remembered across restarts:
//! `remote-screens.json` in the server's configuration directory, which
//! `GET` and `PUT /api/remote/screens` read and replace.
//!
//! The list belongs to the server, not to a browser profile, so every window
//! on the machine sees the same one, and the desktop apps keep it with an
//! embedded server too. The server only keeps it: when a screen connects and
//! where it is shown is the client's business.
//!
//! A list a client sends is checked as a whole and refused when one entry is
//! wrong. A file read back is checked entry by entry, like the recent hosts:
//! an entry that is wrong or repeated is left out, and a file that is missing,
//! unreadable or not in the format is an empty list.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use splitty_remote::hosts;
use splitty_remote::ssh::SshTarget;

/// The file in the configuration directory.
pub const FILE: &str = "remote-screens.json";

/// The format version of the file and of the API's documents.
pub const FORMAT: u32 = 1;

/// How many screens the list holds at most.
pub const MAX_SCREENS: usize = 200;

/// The longest screen id, in characters.
pub const MAX_ID: usize = 256;

/// The longest name, in characters: the client's cap on names.
pub const MAX_NAME: usize = 200;

/// The remembered list, as the file holds it and the API sends and takes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remembered {
    pub version: u32,
    pub screens: Vec<RememberedScreen>,
}

impl Remembered {
    fn empty() -> Self {
        Self {
            version: FORMAT,
            screens: Vec::new(),
        }
    }
}

/// A remote screen the client shows: its host, its id on the host, and what
/// the client needs to show it before the host is connected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RememberedScreen {
    pub target: String,
    pub screen_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its place among the window's screens, local ones included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
}

impl RememberedScreen {
    fn same_screen(&self, other: &Self) -> bool {
        self.target == other.target && self.screen_id == other.screen_id
    }
}

/// Why a list cannot be remembered.
#[derive(Debug, thiserror::Error)]
pub enum RememberError {
    /// The client's list does not have the shape; nothing was written.
    #[error("{0}")]
    Invalid(String),

    #[error("cannot write {FILE}: {0}")]
    Write(#[from] io::Error),
}

/// The remembered list in `config_dir`.
pub fn read(config_dir: &Path) -> Remembered {
    /// The file before its entries are checked.
    #[derive(Deserialize)]
    struct Unchecked {
        version: u32,
        screens: Vec<serde_json::Value>,
    }

    let Some(file) = fs::read(config_dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Unchecked>(&bytes).ok())
        .filter(|file| file.version == FORMAT)
    else {
        return Remembered::empty();
    };
    let mut screens: Vec<RememberedScreen> = Vec::new();
    for entry in file.screens {
        let Ok(screen) = serde_json::from_value::<RememberedScreen>(entry) else {
            continue;
        };
        if check_screen(&screen).is_ok() && !screens.iter().any(|seen| seen.same_screen(&screen)) {
            screens.push(screen);
        }
    }
    screens.truncate(MAX_SCREENS);
    Remembered {
        version: FORMAT,
        screens,
    }
}

/// The list a client sent, if it has the shape: the format version, at most
/// [`MAX_SCREENS`] screens, each with a valid target, an id and a name within
/// their caps, and each (target, screen id) once.
pub fn check(document: serde_json::Value) -> Result<Remembered, RememberError> {
    let invalid = RememberError::Invalid;
    let remembered: Remembered = serde_json::from_value(document)
        .map_err(|error| invalid(format!("not a list of remote screens: {error}")))?;
    if remembered.version != FORMAT {
        return Err(invalid(format!(
            "version {} instead of {FORMAT}",
            remembered.version
        )));
    }
    if remembered.screens.len() > MAX_SCREENS {
        return Err(invalid(format!(
            "{} screens; at most {MAX_SCREENS} are remembered",
            remembered.screens.len()
        )));
    }
    for (index, screen) in remembered.screens.iter().enumerate() {
        check_screen(screen).map_err(|reason| invalid(format!("screens[{index}]: {reason}")))?;
        if remembered.screens[..index]
            .iter()
            .any(|seen| seen.same_screen(screen))
        {
            return Err(invalid(format!(
                "screens[{index}]: screen {:?} of {} is listed twice",
                screen.screen_id, screen.target
            )));
        }
    }
    Ok(remembered)
}

/// Replace the list in `config_dir`, owner-only and atomically.
pub fn write(config_dir: &Path, remembered: &Remembered) -> io::Result<()> {
    let mut text = serde_json::to_vec_pretty(remembered)?;
    text.push(b'\n');
    hosts::write_private(config_dir, FILE, &text)
}

/// Why `screen` cannot be remembered. The target must be valid as it is:
/// it is used as the client sent it.
fn check_screen(screen: &RememberedScreen) -> Result<(), String> {
    match SshTarget::parse(&screen.target) {
        Ok(target) if target.as_str() == screen.target => {}
        Ok(_) => {
            return Err(format!(
                "the target {:?} has blanks around it",
                screen.target
            ))
        }
        Err(error) => return Err(error.to_string()),
    }
    let id = screen.screen_id.chars().count();
    if id == 0 || id > MAX_ID {
        return Err(format!("the screen id must have 1 to {MAX_ID} characters"));
    }
    if screen
        .name
        .as_ref()
        .is_some_and(|name| name.chars().count() > MAX_NAME)
    {
        return Err(format!("the name has more than {MAX_NAME} characters"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn screen(target: &str, id: &str) -> RememberedScreen {
        RememberedScreen {
            target: target.into(),
            screen_id: id.into(),
            name: None,
            position: None,
        }
    }

    fn reason(document: serde_json::Value) -> String {
        match check(document) {
            Err(RememberError::Invalid(reason)) => reason,
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn a_list_of_the_shape_is_taken_as_it_is() {
        let document = json!({"version": 1, "screens": [
            {"target": "user@vm1", "screenId": "screen-1", "name": "api", "position": 2},
            {"target": "vm1", "screenId": "screen-1"},
            {"target": "user@vm1", "screenId": "screen-2", "name": ""},
        ]});
        let remembered = check(document.clone()).expect("the shape");
        assert_eq!(remembered.screens.len(), 3);
        assert_eq!(
            serde_json::to_value(&remembered).expect("JSON"),
            document,
            "absent fields stay absent"
        );
    }

    #[test]
    fn lists_without_the_shape_are_refused() {
        let long = "x".repeat(MAX_ID + 1);
        let long_name = "n".repeat(MAX_NAME + 1);
        for (document, expected) in [
            (json!([]), "not a list of remote screens"),
            (json!({"screens": []}), "not a list of remote screens"),
            (
                json!({"version": 2, "screens": []}),
                "version 2 instead of 1",
            ),
            (
                json!({"version": 1, "screens": [], "extra": 1}),
                "not a list",
            ),
            (
                json!({"version": 1, "screens": [{"target": "vm1"}]}),
                "not a list",
            ),
            (
                json!({"version": 1, "screens": [{"target": "vm1", "screenId": "s", "panes": 3}]}),
                "not a list",
            ),
            (
                json!({"version": 1, "screens": [{"target": "-oProxyCommand=x", "screenId": "s"}]}),
                "screens[0]: SSH target \"-oProxyCommand=x\" starts with a dash",
            ),
            (
                json!({"version": 1, "screens": [{"target": " vm1", "screenId": "s"}]}),
                "has blanks around it",
            ),
            (
                json!({"version": 1, "screens": [{"target": "vm1", "screenId": ""}]}),
                "the screen id must have 1 to 256 characters",
            ),
            (
                json!({"version": 1, "screens": [{"target": "vm1", "screenId": long}]}),
                "the screen id must have",
            ),
            (
                json!({"version": 1, "screens": [{"target": "vm1", "screenId": "s", "name": long_name}]}),
                "the name has more than 200 characters",
            ),
            (
                json!({"version": 1, "screens": [{"target": "vm1", "screenId": "s", "position": -1}]}),
                "not a list",
            ),
            (
                json!({"version": 1, "screens": [{"target": "vm1", "screenId": "s", "position": 1.5}]}),
                "not a list",
            ),
            (
                json!({"version": 1, "screens": [{"target": "vm1", "screenId": 7}]}),
                "not a list",
            ),
            (
                json!({"version": 1, "screens": [
                    {"target": "vm1", "screenId": "s", "name": "a"},
                    {"target": "vm2", "screenId": "s"},
                    {"target": "vm1", "screenId": "s", "name": "b"},
                ]}),
                "screens[2]: screen \"s\" of vm1 is listed twice",
            ),
        ] {
            let reason = reason(document.clone());
            assert!(reason.contains(expected), "{document}: {reason}");
        }

        let too_many: Vec<_> = (0..=MAX_SCREENS)
            .map(|n| json!({"target": "vm1", "screenId": format!("s{n}")}))
            .collect();
        assert_eq!(
            reason(json!({"version": 1, "screens": too_many})),
            "201 screens; at most 200 are remembered"
        );
    }

    #[test]
    fn a_written_list_reads_back_owner_only() {
        let dir = tempfile::tempdir().expect("dir");
        let config = dir.path().join("splitty");
        assert_eq!(read(&config), Remembered::empty(), "no file yet");

        let remembered = Remembered {
            version: FORMAT,
            screens: vec![screen("vm1", "s1"), screen("user@vm2:2222", "s1")],
        };
        write(&config, &remembered).expect("write");
        assert_eq!(read(&config), remembered);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| fs::metadata(path).expect("mode").permissions().mode() & 0o777;
            assert_eq!(mode(&config.join(FILE)), 0o600);
            assert_eq!(mode(&config), 0o700);
        }
    }

    #[test]
    fn a_corrupted_file_is_an_empty_list() {
        for corrupt in [
            "not JSON",
            r#"{"version":2,"screens":[{"target":"vm1","screenId":"s1"}]}"#,
            r#"{"screens":[]}"#,
            r#"[{"target":"vm1","screenId":"s1"}]"#,
        ] {
            let dir = tempfile::tempdir().expect("dir");
            fs::write(dir.path().join(FILE), corrupt).expect("write");
            assert_eq!(read(dir.path()), Remembered::empty(), "{corrupt}");
        }
    }

    /// A hand-edited file loses only its wrong and repeated entries.
    #[test]
    fn wrong_or_repeated_entries_of_a_file_are_left_out() {
        let dir = tempfile::tempdir().expect("dir");
        fs::write(
            dir.path().join(FILE),
            r#"{"version":1,"screens":[
                {"target":"-oProxyCommand=x","screenId":"s1"},
                {"target":"vm1","screenId":"s1","position":0},
                {"target":"vm1","screenId":""},
                {"target":"vm1","screenId":"s2","later":true},
                {"target":"vm1","screenId":"s1","position":4},
                {"target":"vm2","screenId":"s1"}]}"#,
        )
        .expect("write");
        let read = read(dir.path());
        assert_eq!(
            read.screens,
            [
                RememberedScreen {
                    position: Some(0),
                    ..screen("vm1", "s1")
                },
                screen("vm2", "s1")
            ]
        );
    }
}
