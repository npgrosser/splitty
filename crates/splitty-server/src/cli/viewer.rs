//! `splitty open <file>`: show a file of this machine in a viewer pane next
//! to the caller's pane, without a confirmation (`cli`, Open In A Viewer Pane).
use super::{send_mux_op, CliError, ServerInfo};
use crate::client_session::viewer::MAX_FILE_BYTES;
use serde_json::{json, Value};
use std::path::Path;

/// The absolute path of the regular file at `path`, links followed, that a
/// viewer pane can show.
pub fn resolve(path: &Path) -> Result<String, CliError> {
    let resolved = path
        .canonicalize()
        .map_err(|e| CliError::Usage(format!("Cannot resolve {path:?}: {e}")))?;
    let metadata = resolved
        .metadata()
        .map_err(|e| CliError::Usage(e.to_string()))?;
    if !metadata.is_file() {
        return Err(CliError::Usage(
            "Only regular files can be shown in a viewer pane".into(),
        ));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(CliError::Usage(format!(
            "{} is larger than 8 MiB, the most a viewer pane shows; open it with --editor",
            resolved.display()
        )));
    }
    resolved
        .into_os_string()
        .into_string()
        .map_err(|_| CliError::Usage("The path must be valid UTF-8".into()))
}

/// A checked `splitty open` of a file: the op and the screen it goes to.
#[derive(Debug)]
pub struct Request {
    pub(super) path: String,
    pub(super) screen: String,
    pub(super) op: Value,
}

/// Check the file at `path` and build the `open_viewer` op that shows it next
/// to `pane` of `screen`, or around the screen's layout without a pane.
pub fn prepare(path: &Path, screen: &str, pane: &str, tab: bool) -> Result<Request, CliError> {
    let path = resolve(path)?;
    if screen.is_empty() && pane.is_empty() {
        return Err(CliError::Usage(
            "Run this command in a Splitty terminal, or supply --screen <id> and --pane <id>"
                .into(),
        ));
    }
    let mut op = json!({
        "type": "open_viewer",
        "placement": if tab { "tab" } else { "right" },
        "source": {"kind": "file", "path": path},
    });
    if !pane.is_empty() {
        op["paneId"] = json!(pane);
    }
    Ok(Request {
        path,
        screen: screen.to_owned(),
        op,
    })
}

/// Submit `request`: the server shows the file in a new viewer pane, or again
/// in the viewer pane that already shows it.
pub async fn submit(server: &ServerInfo, request: &Request) -> Result<(), CliError> {
    let answer = send_mux_op(server, &request.screen, request.op.clone())
        .await
        .map_err(|error| match error {
            CliError::Api(400, message)
                if message.starts_with("invalid op:")
                    && message.contains("unknown variant `open_viewer`") =>
            {
                CliError::Failed(
                    "This Splitty server cannot show files in viewer panes. Update it, or use --editor."
                        .into(),
                )
            }
            other => other,
        })?;
    println!("{}", outcome(&request.path, &answer));
    Ok(())
}

/// The line `splitty open` prints for the server's answer.
fn outcome(path: &str, answer: &Value) -> String {
    let verb = if answer["reused"] == true {
        "Showing"
    } else {
        "Opened"
    };
    match answer["paneId"].as_str() {
        Some(pane) => format!("{verb} {path} in pane {pane}"),
        None => format!("{verb} {path}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cli, Open In A Viewer Pane (Too large) and the files it refuses.
    #[test]
    fn only_regular_files_up_to_the_limit_resolve() {
        let dir = tempfile::tempdir().expect("fixture");
        let file = dir.path().join("PLAN 界.md");
        std::fs::write(&file, "# Plan").expect("file");
        let canonical = file.canonicalize().expect("canonical");
        let expected = canonical.to_str().expect("UTF-8");
        assert_eq!(
            resolve(&dir.path().join("./PLAN 界.md")).expect("file"),
            expected
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("link.md");
            std::os::unix::fs::symlink(&file, &link).expect("link");
            assert_eq!(resolve(&link).expect("link"), expected);
            assert!(matches!(
                resolve(Path::new("/dev/null")),
                Err(CliError::Usage(_))
            ));
        }
        assert!(matches!(resolve(dir.path()), Err(CliError::Usage(_))));
        assert!(matches!(
            resolve(&dir.path().join("missing.md")),
            Err(CliError::Usage(_))
        ));

        let large = dir.path().join("app.log");
        std::fs::File::create(&large)
            .and_then(|file| file.set_len(20 * 1024 * 1024))
            .expect("sparse file");
        match resolve(&large) {
            Err(CliError::Usage(message)) => assert!(message.contains("8 MiB"), "{message}"),
            other => panic!("not refused: {other:?}"),
        }
        let at_limit = dir.path().join("limit.log");
        std::fs::File::create(&at_limit)
            .and_then(|file| file.set_len(MAX_FILE_BYTES))
            .expect("sparse file");
        assert!(resolve(&at_limit).is_ok());
    }

    /// cli, Open In A Viewer Pane: the op for the implicit or explicit pane
    /// and screen, `--tab`, and both output lines.
    #[test]
    fn the_op_names_the_pane_and_the_placement_and_the_outcome_the_pane() {
        let dir = tempfile::tempdir().expect("fixture");
        let file = dir.path().join("PLAN.md");
        std::fs::write(&file, "# Plan").expect("file");
        let path = resolve(&file).expect("file");
        let request = prepare(&file, "s1", "p1", false).expect("request");
        assert_eq!(request.screen, "s1");
        assert_eq!(
            request.op,
            json!({
                "type": "open_viewer", "paneId": "p1", "placement": "right",
                "source": {"kind": "file", "path": path},
            })
        );
        let request = prepare(&file, "s1", "", true).expect("request");
        assert_eq!(request.op["placement"], "tab");
        assert!(request.op.get("paneId").is_none());
        assert!(
            prepare(&file, "", "p1", false).is_ok(),
            "the server finds the pane's screen"
        );
        assert!(matches!(
            prepare(&file, "", "", false),
            Err(CliError::Usage(message)) if message.contains("--screen")
        ));
        assert_eq!(
            outcome(
                "/home/me/app/PLAN.md",
                &json!({"ok": true, "paneId": "pane-1", "reused": false})
            ),
            "Opened /home/me/app/PLAN.md in pane pane-1"
        );
        assert_eq!(
            outcome(
                "/home/me/app/PLAN.md",
                &json!({"ok": true, "paneId": "pane-1", "reused": true})
            ),
            "Showing /home/me/app/PLAN.md in pane pane-1"
        );
    }
}
