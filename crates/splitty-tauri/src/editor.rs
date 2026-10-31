//! Deliberate editor (or, for remote folders, SFTP client) handoff from the
//! exact-origin main UI. Never a URL opener for terminal links. The trusted UI supplies its owning connection's target;
//! the remote session contributes only cwd, which is encoded as path data.

use splitty_remote::ssh::SshTarget;
use tauri_plugin_opener::OpenerExt;

#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Editor {
    #[default]
    Vscode,
    Zed,
    /// The OS's handler for `sftp:` links, for remote folders only.
    Sftp,
}

#[derive(Clone, Copy, Default, serde::Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PathKind {
    #[default]
    Folder,
    File,
}

#[tauri::command]
pub fn open_remote_editor(
    app: tauri::AppHandle,
    target: String,
    cwd: String,
    editor: Option<Editor>,
    kind: Option<PathKind>,
) -> Result<(), String> {
    dispatch(
        &app,
        remote_editor_path_uri(
            &target,
            &cwd,
            editor.unwrap_or_default(),
            kind.unwrap_or_default(),
        )?,
    )
}

#[tauri::command]
pub fn open_local_editor(
    app: tauri::AppHandle,
    cwd: String,
    editor: Editor,
    wsl_distro: Option<String>,
    kind: Option<PathKind>,
) -> Result<(), String> {
    dispatch(
        &app,
        local_editor_path_uri(
            &cwd,
            wsl_distro.as_deref(),
            editor,
            kind.unwrap_or_default(),
        )?,
    )
}

fn dispatch(app: &tauri::AppHandle, uri: String) -> Result<(), String> {
    app.opener().open_url(uri, None::<&str>).map_err(|_| {
        "Could not hand off to the local editor. Check its installation and OS protocol handler.".into()
    })
}

fn local_editor_path_uri(
    cwd: &str,
    distro: Option<&str>,
    editor: Editor,
    kind: PathKind,
) -> Result<String, String> {
    validate_folder(cwd)?;
    if kind == PathKind::File
        && (cwd.ends_with('/') || cwd.contains(':') || matches!(editor, Editor::Sftp))
    {
        return Err("File path or editor cannot be handed off unambiguously.".into());
    }
    let mut uri = match (editor, distro) {
        (Editor::Sftp, _) => return Err("An SFTP client opens remote folders only.".into()),
        (Editor::Zed, Some(_)) => return Err("Zed cannot open WSL folders through a URL yet. Open the distribution from Zed’s Remote Projects instead.".into()),
        (Editor::Zed, None) => String::from("zed://file"),
        (Editor::Vscode, None) => String::from("vscode://file"),
        (Editor::Vscode, Some(distro)) => {
            if distro.is_empty() || distro.len() > 64 || !distro.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) {
                return Err("Unsupported WSL distribution name.".into());
            }
            format!("vscode://vscode-remote/wsl+{distro}")
        }
    };
    append_path(cwd, &mut uri, kind);
    Ok(uri)
}

fn remote_editor_path_uri(
    target: &str,
    cwd: &str,
    editor: Editor,
    kind: PathKind,
) -> Result<String, String> {
    let parsed = SshTarget::parse(target).map_err(|_| "Unsupported SSH target")?;
    if matches!(editor, Editor::Sftp) {
        // An sftp:// authority carries a port and a bracketed IPv6 literal as
        // they are; the byte check also refuses the `+` a u16 port parse allows.
        if parsed.as_str() != target
            || target.len() > 255
            || !target
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-@:[]".contains(&b))
        {
            return Err("Unsupported SSH target.".into());
        }
    } else if parsed.as_str() != target || target.len() > 255 || target.contains([':', '[', ']']) {
        return Err(
            "Unsupported SSH target; configure a local SSH alias and reconnect through it.".into(),
        );
    }
    if matches!(editor, Editor::Vscode) && target.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Use an SSH alias containing a non-hex character to avoid VS Code encoded-authority ambiguity.".into());
    }
    validate_folder(cwd)?;
    if kind == PathKind::File
        && (cwd.ends_with('/') || cwd.contains(':') || matches!(editor, Editor::Sftp))
    {
        return Err("File path or editor cannot be handed off unambiguously.".into());
    }
    let mut uri = match editor {
        Editor::Vscode => {
            let mut uri = String::from("vscode://vscode-remote/ssh-remote+");
            encode(target, false, &mut uri);
            uri
        }
        // Preserve the SSH user/host separator; input is restricted by SshTarget.
        Editor::Zed => format!("zed://ssh/{target}"),
        Editor::Sftp => format!("sftp://{target}"),
    };
    append_path(cwd, &mut uri, kind);
    Ok(uri)
}

fn validate_folder(cwd: &str) -> Result<(), String> {
    if !cwd.starts_with('/')
        || cwd.starts_with("//")
        || cwd.len() > 4096
        || cwd.contains('\\')
        || cwd.chars().any(|c| c.is_control() || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}'))
        || cwd.split('/').any(|part| part == "." || part == "..")
    {
        return Err("A supported absolute working directory is required.".into());
    }
    Ok(())
}

fn append_path(cwd: &str, uri: &mut String, kind: PathKind) {
    encode(cwd, true, uri);
    // Force folder handling for file-like suffixes such as :123.
    if kind == PathKind::Folder && !cwd.ends_with('/') {
        uri.push('/');
    }
}

fn encode(value: &str, slashes: bool, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) || (slashes && byte == b'/') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    fn remote_editor_uri(target: &str, cwd: &str, editor: Editor) -> Result<String, String> {
        remote_editor_path_uri(target, cwd, editor, PathKind::Folder)
    }
    fn local_editor_uri(cwd: &str, distro: Option<&str>, editor: Editor) -> Result<String, String> {
        local_editor_path_uri(cwd, distro, editor, PathKind::Folder)
    }

    #[derive(Deserialize)]
    struct Case {
        target: String,
        cwd: String,
        uri: Option<String>,
    }

    #[derive(Deserialize)]
    struct Cases {
        valid: Vec<Case>,
        invalid: Vec<Case>,
    }

    #[test]
    fn shared_browser_and_native_uri_vectors() {
        let cases: Cases =
            serde_json::from_str(include_str!("../../../testdata/remote-editor.json"))
                .expect("editor fixtures");
        for case in cases.valid {
            assert_eq!(
                remote_editor_uri(&case.target, &case.cwd, Editor::Vscode).ok(),
                case.uri
            );
        }
        for case in cases.invalid {
            assert!(
                remote_editor_uri(&case.target, &case.cwd, Editor::Vscode).is_err(),
                "{} {:?}",
                case.target,
                case.cwd
            );
        }
    }

    #[test]
    fn bounds_untrusted_input_before_dispatch() {
        assert!(remote_editor_uri(&"x".repeat(256), "/srv", Editor::Vscode).is_err());
        assert!(
            remote_editor_uri("vm1", &format!("/{}", "界".repeat(1366)), Editor::Vscode).is_err()
        );
    }
    #[test]
    fn shared_local_and_zed_vectors() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../../../testdata/editor-folders.json"))
                .expect("editor fixtures");
        for group in ["valid", "invalid"] {
            for input in cases[group].as_array().expect("case list") {
                let result = serde_json::from_value::<Editor>(input["editor"].clone())
                    .map_err(|e| e.to_string())
                    .and_then(|editor| {
                        let cwd = input["cwd"].as_str().expect("cwd");
                        if let Some(target) = input["target"].as_str() {
                            remote_editor_uri(target, cwd, editor)
                        } else {
                            local_editor_uri(cwd, input["wslDistro"].as_str(), editor)
                        }
                    });
                if group == "valid" {
                    assert_eq!(
                        result.as_deref(),
                        Ok(input["uri"].as_str().expect("uri")),
                        "{input}"
                    );
                } else {
                    assert!(result.is_err(), "{input}");
                }
            }
        }
    }
    #[test]
    fn file_handoff_vectors() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../../../testdata/editor-files.json"))
                .expect("fixtures");
        for group in ["valid", "invalid"] {
            for input in cases[group].as_array().expect("list") {
                let result = serde_json::from_value::<Editor>(input["editor"].clone())
                    .map_err(|e| e.to_string())
                    .and_then(|editor| {
                        let kind = serde_json::from_value::<PathKind>(input["kind"].clone())
                            .map_err(|e| e.to_string())?;
                        let path = input["cwd"].as_str().expect("path");
                        if let Some(target) = input["target"].as_str() {
                            remote_editor_path_uri(target, path, editor, kind)
                        } else {
                            local_editor_path_uri(path, input["wslDistro"].as_str(), editor, kind)
                        }
                    });
                if group == "valid" {
                    assert_eq!(
                        result.as_deref(),
                        Ok(input["uri"].as_str().expect("URI")),
                        "{input}"
                    );
                } else {
                    assert!(result.is_err(), "{input}");
                }
            }
        }
    }
}
