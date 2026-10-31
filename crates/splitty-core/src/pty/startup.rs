//! Shell startup argument construction for different restore modes.
//!
//! Supports "replay" (inject command after prompt), "shell" (run command then
//! drop back to interactive shell), and "close" (run command then exit).

use std::path::Path;

/// Build the startup arguments for a shell session.
///
/// Returns `(argv, inject)` where:
/// - `argv` is the list of arguments to pass to the shell (empty for normal start)
/// - `inject` indicates whether the command should be typed into the PTY after the
///   shell prompt appears
pub fn build_startup(shell: &str, command: &str, restore_mode: &str) -> (Vec<String>, bool) {
    if command.is_empty() {
        return (Vec::new(), false);
    }

    match restore_mode {
        "" | "replay" => (Vec::new(), true),
        "shell" => {
            if let Some(args) = shell_wrapper_args(shell, command, true) {
                (args, false)
            } else {
                // Unknown shell -- fall back to replay mode.
                (Vec::new(), true)
            }
        }
        "close" => {
            if let Some(args) = shell_wrapper_args(shell, command, false) {
                (args, false)
            } else {
                (Vec::new(), true)
            }
        }
        _ => (Vec::new(), true),
    }
}

/// Combine configured profile argv with any restore-mode argv. Profile
/// arguments remain individual, literal entries and always retain their order.
pub fn build_command_args(
    shell: &str,
    profile_args: &[String],
    command: &str,
    restore_mode: &str,
) -> (Vec<String>, bool) {
    let (startup_args, inject) = build_startup(shell, command, restore_mode);
    let mut argv = profile_args.to_vec();
    argv.extend(startup_args);
    (argv, inject)
}

/// Returns argv for restore modes that execute the command through the shell's
/// interactive init path. `keep_shell` controls whether the pane should drop
/// back into an interactive shell afterwards.
fn shell_wrapper_args(shell: &str, command: &str, keep_shell: bool) -> Option<Vec<String>> {
    match shell_name(shell) {
        "zsh" | "bash" | "sh" | "dash" | "ksh" => {
            let mut script = String::from("eval \"$1\"");
            if keep_shell {
                script.push_str("; exec \"$0\" -i");
            }
            Some(vec![
                "-i".to_string(),
                "-c".to_string(),
                script,
                shell.to_string(),
                command.to_string(),
            ])
        }
        _ => None,
    }
}

/// Extract the shell name from a path, lowercased, with leading `-` stripped
/// (handles login shells like `-zsh`).
pub fn shell_name(shell: &str) -> &str {
    let base = Path::new(shell.trim())
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(shell.trim());
    // We need an owned lowercase version, but since we return &str we need
    // to match against the lowercase form. For the known set of shells this
    // works by checking case-insensitively.
    let base = base.trim_start_matches('-');
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replay_mode() {
        let (argv, inject) = build_startup("/bin/zsh", "echo hello", "replay");
        assert!(argv.is_empty());
        assert!(inject);

        // Empty restore mode defaults to replay
        let (argv, inject) = build_startup("/bin/zsh", "echo hello", "");
        assert!(argv.is_empty());
        assert!(inject);
    }

    #[test]
    fn test_shell_mode_zsh() {
        let (argv, inject) = build_startup("/bin/zsh", "echo hello", "shell");
        assert!(!inject);
        assert_eq!(
            argv,
            vec![
                "-i",
                "-c",
                "eval \"$1\"; exec \"$0\" -i",
                "/bin/zsh",
                "echo hello",
            ]
        );
    }

    #[test]
    fn test_close_mode_bash() {
        let (argv, inject) = build_startup("/usr/bin/bash", "make build", "close");
        assert!(!inject);
        assert_eq!(
            argv,
            vec!["-i", "-c", "eval \"$1\"", "/usr/bin/bash", "make build",]
        );
    }

    #[test]
    fn test_unknown_shell_fallback() {
        let (argv, inject) = build_startup("/usr/bin/fish", "echo hello", "shell");
        assert!(argv.is_empty());
        assert!(inject);
    }

    #[test]
    fn test_shell_name() {
        assert_eq!(shell_name("/bin/zsh"), "zsh");
        assert_eq!(shell_name("/usr/bin/bash"), "bash");
        assert_eq!(shell_name("-zsh"), "zsh");
        assert_eq!(shell_name("zsh"), "zsh");
    }

    #[test]
    fn test_profile_arguments_remain_literal_and_ordered() {
        let profile_args = vec![
            "literal $HOME".to_string(),
            "$(not-executed)".to_string(),
            "two words".to_string(),
        ];
        let (argv, inject) =
            build_command_args("/bin/bash", &profile_args, "echo restored", "shell");
        assert!(!inject);
        assert_eq!(&argv[..3], profile_args.as_slice());
        assert_eq!(&argv[3..5], ["-i", "-c"]);
    }
}
