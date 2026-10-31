//! Request an external editor on the attached client, never on this server.
use super::{api_post, CliError, ServerInfo};
use serde_json::json;
use std::path::Path;

pub fn absolute_path(path: &Path) -> Result<String, CliError> {
    let path = path
        .canonicalize()
        .map_err(|e| CliError::Usage(format!("Cannot resolve {path:?}: {e}")))?;
    let metadata = path
        .metadata()
        .map_err(|e| CliError::Usage(e.to_string()))?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(CliError::Usage(
            "Only regular files and folders can be opened".into(),
        ));
    }
    path.into_os_string()
        .into_string()
        .map_err(|_| CliError::Usage("The path must be valid UTF-8".into()))
}

pub async fn open(
    server: &ServerInfo,
    path: &Path,
    session: &str,
    editor: Option<&str>,
) -> Result<(), CliError> {
    if session.is_empty() || session.len() > 256 {
        return Err(CliError::Usage(
            "Run this command in a Splitty terminal, or supply --session <id>".into(),
        ));
    }
    let path = absolute_path(path)?;
    eprintln!("Confirm the editor request in Splitty (expires in 30 seconds)…");
    api_post(server, "/api/editor/open", json!({"sessionId":session,"path":path,"editor":editor})).await.map_err(|error| {
        if matches!(error, CliError::Api(404, _)) {
            CliError::Usage("The server does not support editor requests, or this terminal session is gone. Update the host and reopen its terminal.".into())
        } else { error }
    })?;
    println!("Editor handoff dispatched. Splitty cannot confirm whether the editor connected.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_files_folders_and_symlinks_but_rejects_missing_and_special_paths() {
        let dir = tempfile::tempdir().expect("fixture");
        let file = dir.path().join("file #? 界.rs");
        std::fs::write(&file, "test").expect("file");
        assert_eq!(
            absolute_path(&dir.path().join("./file #? 界.rs")).expect("file"),
            file.canonicalize()
                .expect("canonical file")
                .to_str()
                .expect("UTF-8")
        );
        assert!(absolute_path(dir.path()).is_ok());
        assert!(absolute_path(&dir.path().join("missing")).is_err());
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&file, &link).expect("link");
            assert_eq!(
                absolute_path(&link).expect("link"),
                file.canonicalize()
                    .expect("canonical file")
                    .to_str()
                    .expect("UTF-8")
            );
            assert!(absolute_path(Path::new("/dev/null")).is_err());
        }
    }
}
