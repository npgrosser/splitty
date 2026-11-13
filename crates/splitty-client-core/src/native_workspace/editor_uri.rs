//! The URI that hands a file or a folder to an editor, or a remote folder to
//! an SFTP client. The same rules as the web client's (`editor_target.ts`),
//! checked against the shared vectors in `testdata/`: the path is untrusted
//! data and is encoded as such, the SSH target is the window's own (its SSH
//! alias), and the WSL distribution is the local backend's.
use super::PathKind;

/// The program a URI is built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandoffEditor {
    VsCode,
    Zed,
    /// The system's handler for `sftp:` links, for remote folders only.
    Sftp,
}

const ALIAS_HINT: &str = "Use a local SSH alias for this target, then reconnect through that alias in Splitty. Configure the same alias in your editor's SSH config (Windows and WSL configs are separate).";
/// The longest path a URI carries, in bytes.
const MAX_PATH_BYTES: usize = 4096;

/// The URI for `path` on the host the window reaches as `target`, or why
/// there is none.
pub fn remote_editor_uri(
    target: &str,
    path: &str,
    editor: HandoffEditor,
    kind: PathKind,
) -> Result<String, String> {
    let unsupported = || format!("Unsupported SSH target. {ALIAS_HINT}");
    if target.len() > 255 || target.starts_with('-') {
        return Err(unsupported());
    }
    let parts: Vec<&str> = target.split('@').collect();
    let host = parts[parts.len() - 1];
    let user_valid = parts.len() == 1
        || (parts.len() == 2
            && !parts[0].is_empty()
            && parts[0]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)));
    if editor == HandoffEditor::Sftp {
        // An sftp:// authority carries a port and a bracketed IPv6 literal as they are.
        if !user_valid || !host_with_port_valid(host) {
            return Err("Unsupported SSH target.".into());
        }
    } else {
        if target.contains([':', '[', ']']) {
            return Err(format!(
                "Explicit SSH ports and IPv6 addresses need an SSH alias. {ALIAS_HINT}"
            ));
        }
        // Remote-SSH also uses hex-encoded authorities internally. Do not
        // mistake an all-hex alias for an encoded host description and open
        // another host.
        if editor == HandoffEditor::VsCode
            && !target.is_empty()
            && target.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("This target is ambiguous with VS Code's encoded SSH authorities. Use an SSH alias containing a non-hex character, then reconnect through it in Splitty.".into());
        }
        if !user_valid || !hostname_valid(host) {
            return Err(unsupported());
        }
    }
    if editor == HandoffEditor::Sftp && kind != PathKind::Folder {
        return Err("SFTP handoff supports folders only.".into());
    }
    let path = uri_path(path, "remote", kind)?;
    Ok(match editor {
        HandoffEditor::Sftp => format!("sftp://{target}{path}"),
        HandoffEditor::Zed => format!("zed://ssh/{target}{path}"),
        HandoffEditor::VsCode => format!(
            "vscode://vscode-remote/ssh-remote+{}{path}",
            encode_part(target)
        ),
    })
}

/// The URI for `path` on this computer, or on the WSL distribution the local
/// backend runs in, or why there is none.
pub fn local_editor_uri(
    path: &str,
    wsl_distro: Option<&str>,
    editor: HandoffEditor,
    kind: PathKind,
) -> Result<String, String> {
    let scheme = match editor {
        HandoffEditor::Sftp => return Err("An SFTP client opens remote folders only.".into()),
        HandoffEditor::VsCode => "vscode",
        HandoffEditor::Zed => "zed",
    };
    let path = uri_path(path, "local", kind)?;
    let Some(distro) = wsl_distro else {
        return Ok(format!("{scheme}://file{path}"));
    };
    if editor == HandoffEditor::Zed {
        return Err("Zed cannot open WSL folders through a URL yet. Open the distribution from Zed’s Remote Projects instead.".into());
    }
    if distro.is_empty()
        || distro.len() > 64
        || !distro
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err("The WSL distribution's name is not one VS Code can open.".into());
    }
    Ok(format!(
        "vscode://vscode-remote/wsl+{}{path}",
        encode_part(distro)
    ))
}

fn hostname_valid(host: &str) -> bool {
    fn label_valid(label: &str) -> bool {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 63
            && bytes
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
            && bytes[0] != b'-'
            && bytes[bytes.len() - 1] != b'-'
    }
    !host.is_empty() && host.len() <= 253 && host.split('.').all(label_valid)
}

/// A hostname, alias or IPv4 address, or a bracketed IPv6 literal, each
/// with an optional port.
fn host_with_port_valid(host: &str) -> bool {
    fn port_valid(port: Option<&str>) -> bool {
        port.is_none_or(|port| {
            (1..=5).contains(&port.len())
                && port.bytes().all(|b| b.is_ascii_digit())
                && port.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n))
        })
    }
    if let Some(rest) = host.strip_prefix('[') {
        if let Some((literal, after)) = rest.split_once(']') {
            if !literal.is_empty()
                && literal
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
            {
                let port = match after {
                    "" => None,
                    after => match after.strip_prefix(':') {
                        Some(port) if !port.contains(':') => Some(port),
                        _ => return false,
                    },
                };
                return literal.parse::<std::net::Ipv6Addr>().is_ok() && port_valid(port);
            }
        }
    }
    let mut parts = host.split(':');
    let name = parts.next().unwrap_or_default();
    let port = parts.next();
    parts.next().is_none() && hostname_valid(name) && port_valid(port)
}

/// The URI path of a file or a folder; a folder's trailing slash forces
/// folder handling for names such as `:123` or `.code-workspace`.
fn uri_path(path: &str, side: &str, kind: PathKind) -> Result<String, String> {
    if kind == PathKind::File && (path.ends_with('/') || path.contains(':')) {
        return Err(
            "File paths containing colons or ending with a slash cannot be handed off unambiguously."
                .into(),
        );
    }
    if path.is_empty() {
        return Err(format!(
            "The {side} terminal has not reported its working directory yet."
        ));
    }
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.len() > MAX_PATH_BYTES
        || path.chars().any(|c| {
            c == '\\'
                || c.is_control()
                || matches!(
                    c,
                    '\u{200b}'..='\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2060}'..='\u{206f}'
                        | '\u{feff}'
                )
        })
        || path.split('/').any(|part| part == "." || part == "..")
    {
        return Err(format!("The {side} working directory must be a supported absolute Linux/macOS path without control characters or dot segments."));
    }
    // Encode data, not a URL; slashes stay only as directory separators.
    let mut encoded = path
        .split('/')
        .map(encode_part)
        .collect::<Vec<_>>()
        .join("/");
    if kind == PathKind::Folder && !path.ends_with('/') {
        encoded.push('/');
    }
    Ok(encoded)
}

/// Percent-encode everything but the unreserved characters of RFC 3986.
fn encode_part(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn editor(value: &Value) -> Option<HandoffEditor> {
        match value.as_str()? {
            "vscode" => Some(HandoffEditor::VsCode),
            "zed" => Some(HandoffEditor::Zed),
            "sftp" => Some(HandoffEditor::Sftp),
            _ => None,
        }
    }

    fn kind(value: &Value) -> Option<PathKind> {
        match value.as_str() {
            None | Some("folder") => Some(PathKind::Folder),
            Some("file") => Some(PathKind::File),
            Some(_) => None,
        }
    }

    /// One vector's URI; an editor or a kind the client does not know is
    /// refused as the web client refuses it.
    fn build(input: &Value) -> Result<String, String> {
        let editor = editor(&input["editor"]).ok_or("unknown editor")?;
        let kind = kind(&input["kind"]).ok_or("unknown kind")?;
        let path = input["cwd"].as_str().expect("path");
        match input["target"].as_str() {
            Some(target) => remote_editor_uri(target, path, editor, kind),
            None => local_editor_uri(path, input["wslDistro"].as_str(), editor, kind),
        }
    }

    /// The vectors the web client and the Tauri shell are checked against.
    #[test]
    fn shared_vectors() {
        for (name, file, default_editor) in [
            (
                "remote-editor",
                include_str!("../../../../testdata/remote-editor.json"),
                Some("vscode"),
            ),
            (
                "editor-folders",
                include_str!("../../../../testdata/editor-folders.json"),
                None,
            ),
            (
                "editor-files",
                include_str!("../../../../testdata/editor-files.json"),
                None,
            ),
        ] {
            let cases: Value = serde_json::from_str(file).expect("vectors");
            for group in ["valid", "invalid"] {
                for case in cases[group].as_array().expect("cases") {
                    let mut input = case.clone();
                    if let Some(editor) = default_editor {
                        input["editor"] = Value::from(editor);
                    }
                    let result = build(&input);
                    if group == "valid" {
                        assert_eq!(
                            result.as_deref(),
                            Ok(case["uri"].as_str().expect("uri")),
                            "{name}: {case}"
                        );
                    } else {
                        assert!(result.is_err(), "{name}: {case} gave {result:?}");
                    }
                }
            }
        }
    }

    /// A file is handed over as a file, a folder as a folder.
    #[test]
    fn the_kind_decides_the_trailing_slash() {
        assert_eq!(
            local_editor_uri("/srv/main.rs", None, HandoffEditor::VsCode, PathKind::File)
                .as_deref(),
            Ok("vscode://file/srv/main.rs")
        );
        assert_eq!(
            local_editor_uri("/srv/app", None, HandoffEditor::VsCode, PathKind::Folder).as_deref(),
            Ok("vscode://file/srv/app/")
        );
        assert_eq!(
            remote_editor_uri("vm1", "/srv/main.rs", HandoffEditor::Zed, PathKind::File).as_deref(),
            Ok("zed://ssh/vm1/srv/main.rs")
        );
        assert_eq!(
            local_editor_uri(
                "/home/me/main.rs",
                Some("Ubuntu-24.04"),
                HandoffEditor::VsCode,
                PathKind::File
            )
            .as_deref(),
            Ok("vscode://vscode-remote/wsl+Ubuntu-24.04/home/me/main.rs")
        );
    }

    /// Ports and IPv6 literals reach an SFTP client as they are; the editors
    /// need an alias, and VS Code refuses an all-hex one.
    #[test]
    fn targets_follow_the_editor() {
        let folder = PathKind::Folder;
        assert_eq!(
            remote_editor_uri("me@vm1:2222", "/srv", HandoffEditor::Sftp, folder).as_deref(),
            Ok("sftp://me@vm1:2222/srv/")
        );
        assert_eq!(
            remote_editor_uri("[2001:db8::1]:22", "/srv", HandoffEditor::Sftp, folder).as_deref(),
            Ok("sftp://[2001:db8::1]:22/srv/")
        );
        let refused =
            remote_editor_uri("vm1:2222", "/srv", HandoffEditor::VsCode, folder).expect_err("port");
        assert!(refused.contains("SSH alias"), "{refused}");
        assert!(remote_editor_uri("cafe", "/srv", HandoffEditor::VsCode, folder).is_err());
        assert!(remote_editor_uri("cafe", "/srv", HandoffEditor::Zed, folder).is_ok());
        assert!(remote_editor_uri(
            &format!("-{}", "x".repeat(1024 * 1024)),
            "/srv",
            HandoffEditor::Sftp,
            folder
        )
        .is_err());
        assert!(remote_editor_uri(
            "vm1",
            &format!("/{}", "界".repeat(1366)),
            HandoffEditor::VsCode,
            folder
        )
        .is_err());
    }

    /// Characters that reorder or hide text never reach a URI.
    #[test]
    fn invisible_and_direction_characters_are_refused() {
        for c in ['\u{200b}', '\u{200e}', '\u{202e}', '\u{2066}', '\u{feff}'] {
            assert!(
                local_editor_uri(
                    &format!("/srv/a{c}b"),
                    None,
                    HandoffEditor::VsCode,
                    PathKind::Folder
                )
                .is_err(),
                "{c:?}"
            );
        }
    }
}
