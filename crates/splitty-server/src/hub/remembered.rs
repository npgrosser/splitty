//! The remote screens a client shows, remembered across restarts:
//! `remote-screens.json` in the server's configuration directory, which
//! `GET` and `PUT /api/remote/screens` read and replace, and
//! `PUT /api/remote/screens/host` changes for one host.
//!
//! The list belongs to the server, not to a browser profile, so every window
//! on the machine sees the same one, and the desktop apps keep it with an
//! embedded server too. The server only keeps it: when a screen connects and
//! where it is shown is the client's business.
//!
//! A window writes the hosts it shows one by one ([`replace_host`]), so two
//! windows that show different hosts never undo each other's entries.
//!
//! A list a client sends is checked as a whole and refused when one entry is
//! wrong. A file read back is checked entry by entry, like the recent hosts:
//! an entry that is wrong or repeated is left out, and a file that is missing,
//! unreadable or not in the format is an empty list.

use std::fs;
use std::io;
use std::path::Path;

use serde::Deserialize;
use splitty_remote::hosts;
use splitty_remote::ssh::SshTarget;

/// The file in the configuration directory.
pub const FILE: &str = "remote-screens.json";

pub use splitty_core::native_api::remembered::{
    Remembered, RememberedScreen, FORMAT, MAX_ID, MAX_NAME, MAX_SCREENS,
};

/// The list as the file holds it and a client sends it, with unknown fields
/// refused; replies are [`Remembered`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Strict {
    version: u32,
    screens: Vec<StrictScreen>,
}

/// One entry of [`Strict`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StrictScreen {
    target: String,
    screen_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    position: Option<u32>,
    #[serde(default)]
    closed: bool,
}

impl From<StrictScreen> for RememberedScreen {
    fn from(screen: StrictScreen) -> Self {
        RememberedScreen {
            target: screen.target,
            screen_id: screen.screen_id,
            name: screen.name,
            position: screen.position,
            closed: screen.closed,
        }
    }
}

fn same_screen(one: &RememberedScreen, other: &RememberedScreen) -> bool {
    one.target == other.target && one.screen_id == other.screen_id
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
        return Remembered::default();
    };
    let mut screens: Vec<RememberedScreen> = Vec::new();
    for entry in file.screens {
        let Ok(screen) = serde_json::from_value::<StrictScreen>(entry).map(RememberedScreen::from)
        else {
            continue;
        };
        if check_screen(&screen).is_ok() && !screens.iter().any(|seen| same_screen(seen, &screen)) {
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
    let remembered: Strict = serde_json::from_value(document)
        .map_err(|error| invalid(format!("not a list of remote screens: {error}")))?;
    let remembered = Remembered {
        version: remembered.version,
        screens: remembered.screens.into_iter().map(Into::into).collect(),
    };
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
            .any(|seen| same_screen(seen, screen))
        {
            return Err(invalid(format!(
                "screens[{index}]: screen {:?} of {} is listed twice",
                screen.screen_id, screen.target
            )));
        }
    }
    Ok(remembered)
}

/// The one host's Screens a client sent, if they have the shape: a valid
/// target, ids and names within their caps, each id once, at most
/// [`MAX_SCREENS`].
pub fn check_host(
    document: serde_json::Value,
) -> Result<splitty_core::native_api::remembered::HostScreens, RememberError> {
    let invalid = RememberError::Invalid;
    let host: splitty_core::native_api::remembered::HostScreens = serde_json::from_value(document)
        .map_err(|error| invalid(format!("not the remote screens of a host: {error}")))?;
    if host.screens.len() > MAX_SCREENS {
        return Err(invalid(format!(
            "{} screens; at most {MAX_SCREENS} are remembered",
            host.screens.len()
        )));
    }
    for (index, screen) in host.screens.iter().enumerate() {
        check_screen(&entry(&host.target, screen, host.closed))
            .map_err(|reason| invalid(format!("screens[{index}]: {reason}")))?;
        if host.screens[..index]
            .iter()
            .any(|seen| seen.screen_id == screen.screen_id)
        {
            return Err(invalid(format!(
                "screens[{index}]: screen {:?} is listed twice",
                screen.screen_id
            )));
        }
    }
    // An empty list forgets a host; it, too, names only a valid one.
    check_target(&host.target).map_err(invalid)?;
    Ok(host)
}

/// `remembered` with the entries of `host.target` replaced by `host`'s, in
/// the place of the host's first entry (a new host goes last). An empty list
/// forgets the host. At most [`MAX_SCREENS`] entries are kept.
pub fn replace_host(
    remembered: &mut Remembered,
    host: &splitty_core::native_api::remembered::HostScreens,
) {
    let screens = &mut remembered.screens;
    let at = screens
        .iter()
        .position(|screen| screen.target == host.target)
        .unwrap_or(screens.len());
    screens.retain(|screen| screen.target != host.target);
    let at = at.min(screens.len());
    screens.splice(
        at..at,
        host.screens
            .iter()
            .map(|screen| entry(&host.target, screen, host.closed)),
    );
    screens.truncate(MAX_SCREENS);
}

fn entry(
    target: &str,
    screen: &splitty_core::native_api::remembered::HostScreen,
    closed: bool,
) -> RememberedScreen {
    RememberedScreen {
        target: target.into(),
        screen_id: screen.screen_id.clone(),
        name: screen.name.clone(),
        position: screen.position,
        closed,
    }
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
    check_target(&screen.target)?;
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

fn check_target(target: &str) -> Result<(), String> {
    match SshTarget::parse(target) {
        Ok(parsed) if parsed.as_str() == target => Ok(()),
        Ok(_) => Err(format!("the target {target:?} has blanks around it")),
        Err(error) => Err(error.to_string()),
    }
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
            closed: false,
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
            {"target": "vm2", "screenId": "screen-1", "closed": true},
        ]});
        let remembered = check(document.clone()).expect("the shape");
        assert_eq!(remembered.screens.len(), 4);
        assert!(remembered.screens[3].closed && !remembered.screens[0].closed);
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
    fn a_host_write_replaces_only_that_host_in_its_place() {
        let host = |document: serde_json::Value| check_host(document).expect("the shape");
        let mut remembered = Remembered {
            version: FORMAT,
            screens: vec![
                screen("vm1", "a"),
                RememberedScreen {
                    closed: true,
                    ..screen("vm2", "b")
                },
                screen("vm1", "c"),
                screen("vm3", "d"),
            ],
        };
        replace_host(
            &mut remembered,
            &host(json!({"target": "vm1", "screens": [
                {"screenId": "e", "name": "api", "position": 4},
            ]})),
        );
        assert_eq!(
            serde_json::to_value(&remembered.screens).unwrap(),
            json!([
                {"target": "vm1", "screenId": "e", "name": "api", "position": 4},
                {"target": "vm2", "screenId": "b", "closed": true},
                {"target": "vm3", "screenId": "d"},
            ]),
            "the other hosts and their closed marks stay"
        );
        replace_host(
            &mut remembered,
            &host(json!({"target": "vm3", "screens": [{"screenId": "d"}], "closed": true})),
        );
        replace_host(
            &mut remembered,
            &host(json!({"target": "vm4", "screens": [{"screenId": "f"}]})),
        );
        replace_host(
            &mut remembered,
            &host(json!({"target": "vm2", "screens": []})),
        );
        assert_eq!(
            serde_json::to_value(&remembered.screens).unwrap(),
            json!([
                {"target": "vm1", "screenId": "e", "name": "api", "position": 4},
                {"target": "vm3", "screenId": "d", "closed": true},
                {"target": "vm4", "screenId": "f"},
            ]),
            "closed marks a host's screens, a new host goes last, an empty list forgets"
        );

        let full: Vec<_> = (0..MAX_SCREENS)
            .map(|n| screen("vm1", &format!("s{n}")))
            .collect();
        remembered.screens = full;
        replace_host(
            &mut remembered,
            &host(json!({"target": "vm2", "screens": [{"screenId": "x"}]})),
        );
        assert_eq!(remembered.screens.len(), MAX_SCREENS);
    }

    #[test]
    fn host_writes_without_the_shape_are_refused() {
        let reason = |document: serde_json::Value| match check_host(document) {
            Err(RememberError::Invalid(reason)) => reason,
            other => panic!("not refused: {other:?}"),
        };
        for (document, expected) in [
            (json!({"screens": []}), "not the remote screens of a host"),
            (
                json!({"target": "vm1", "screens": [], "version": 1}),
                "not the remote screens",
            ),
            (
                json!({"target": "vm1", "screens": [{"target": "vm1", "screenId": "s"}]}),
                "not the remote screens",
            ),
            (
                json!({"target": "-oProxyCommand=x", "screens": []}),
                "starts with a dash",
            ),
            (
                json!({"target": " vm1", "screens": []}),
                "has blanks around it",
            ),
            (
                json!({"target": "vm1", "screens": [{"screenId": ""}]}),
                "screens[0]: the screen id must have 1 to 256 characters",
            ),
            (
                json!({"target": "vm1", "screens": [{"screenId": "s"}, {"screenId": "s"}]}),
                "screens[1]: screen \"s\" is listed twice",
            ),
        ] {
            let reason = reason(document.clone());
            assert!(reason.contains(expected), "{document}: {reason}");
        }
    }

    #[test]
    fn a_written_list_reads_back_owner_only() {
        let dir = tempfile::tempdir().expect("dir");
        let config = dir.path().join("splitty");
        assert_eq!(read(&config), Remembered::default(), "no file yet");

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
            assert_eq!(read(dir.path()), Remembered::default(), "{corrupt}");
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
