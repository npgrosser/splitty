# Splitty shell integration for Bash
# Emits OSC 133 semantic prompt sequences so the terminal can detect
# prompt/input/output/command-finished boundaries.
#
# Loaded automatically by Splitty's POSIX/ENV injection.
# Can also be sourced manually:
#   [[ -n "$SPLITTY_SHELL_INTEGRATION" ]] && source "$SPLITTY_SHELL_INTEGRATION/bash/splitty.bash"

# We need to be in interactive mode.
if [[ "$-" != *i* ]]; then builtin return; fi

# When auto-injected, we were started in POSIX mode. Re-create the normal
# bash startup sequence and then disable POSIX mode.
if [ -n "$SPLITTY_BASH_INJECT" ]; then
    builtin declare __splitty_bash_flags="$SPLITTY_BASH_INJECT"
    builtin unset SPLITTY_BASH_INJECT

    # Restore an existing ENV value that was saved during injection.
    if [[ -n "$SPLITTY_BASH_ENV" ]]; then
        builtin export ENV="$SPLITTY_BASH_ENV"
        builtin unset SPLITTY_BASH_ENV
    else
        builtin unset ENV
    fi

    # Leave POSIX mode.
    builtin set +o posix
    builtin shopt -u inherit_errexit 2>/dev/null

    # Unexport HISTFILE if it was set by the injection code.
    if [[ -n "$SPLITTY_BASH_UNEXPORT_HISTFILE" ]]; then
        builtin export -n HISTFILE
        builtin unset SPLITTY_BASH_UNEXPORT_HISTFILE
    fi

    # Source the normal startup files that bash would have loaded.
    if builtin shopt -q login_shell; then
        if [[ $__splitty_bash_flags != *"--noprofile"* ]]; then
            [ -r /etc/profile ] && builtin source "/etc/profile"
            for __splitty_rcfile in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
                [ -r "$__splitty_rcfile" ] && {
                    builtin source "$__splitty_rcfile"
                    break
                }
            done
        fi
    else
        if [[ $__splitty_bash_flags != *"--norc"* ]]; then
            for __splitty_rcfile in /etc/bash.bashrc /etc/bash/bashrc /etc/bashrc; do
                [ -r "$__splitty_rcfile" ] && {
                    builtin source "$__splitty_rcfile"
                    break
                }
            done
            if [[ -z "$SPLITTY_BASH_RCFILE" ]]; then SPLITTY_BASH_RCFILE="$HOME/.bashrc"; fi
            [ -r "$SPLITTY_BASH_RCFILE" ] && builtin source "$SPLITTY_BASH_RCFILE"
        fi
    fi

    builtin unset __splitty_rcfile
    builtin unset __splitty_bash_flags
    builtin unset SPLITTY_BASH_RCFILE
fi

# Guard: only run inside Splitty, prevent double-sourcing
[[ -z "$SPLITTY_SESSION_ID" ]] && return
[[ -n "$__splitty_bash_integration" ]] && return
__splitty_bash_integration=1

# Login profiles may replace the PATH supplied by the server. Restore only the
# validated, non-secret CLI location; never edit startup files or install aliases.
__splitty_restore_cli_path() {
    [[ -n "${SPLITTY_CLI_DIR-}" && "$SPLITTY_CLI_DIR" != *:* && -f "$SPLITTY_CLI_DIR/splitty" && -x "$SPLITTY_CLI_DIR/splitty" ]] || return 0
    local entry remaining="${PATH-}" updated="$SPLITTY_CLI_DIR" last
    if [[ -n "${PATH+x}" ]]; then
        while :; do
            last=
            case "$remaining" in
                *:*) entry="${remaining%%:*}"; remaining="${remaining#*:}" ;;
                *) entry="$remaining"; last=1 ;;
            esac
            [[ "$entry" == "$SPLITTY_CLI_DIR" ]] || updated+=":$entry"
            [[ -n "$last" ]] && break
        done
    fi
    builtin export PATH="$updated"
}
__splitty_restore_cli_path

# Save the exit code of the previous command, then emit:
#   133;D;$? (command finished with exit code)
#   133;A    (prompt start)
__splitty_precmd() {
    local ec="$?"
    printf '\e]133;D;%s\a' "$ec"
    printf '\e]133;A\a'
}

# Emit 133;C (output start / command executing) before each command.
# Uses PS0 (bash 4.4+) which is expanded after a command is read but
# before it executes.
__splitty_ps0() {
    printf '\e]133;C\a'
}

# Install hooks
if [[ ! "$PROMPT_COMMAND" == *"__splitty_precmd"* ]]; then
    PROMPT_COMMAND="__splitty_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
fi

# PS0 emits 133;C before command execution (bash 4.4+)
PS0='$(__splitty_ps0)'

# Append 133;B (input start) after the prompt
PS1="${PS1}\[\e]133;B\a\]"
