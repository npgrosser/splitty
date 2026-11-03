# Splitty shell integration for Bash
# Emits OSC 133 semantic prompt sequences so the terminal can detect
# prompt/input/output/command-finished boundaries.
#
# Loaded automatically by Splitty before the first prompt or command.
# Can also be sourced manually:
#   [[ -n "$SPLITTY_SHELL_INTEGRATION" ]] && source "$SPLITTY_SHELL_INTEGRATION/bash/splitty.bash"

# We need to be in interactive mode.
if [[ "$-" != *i* ]]; then builtin return; fi

# Auto-injected shells defer startup files until this bootstrap, keeping
# their real login_shell state on Bash 3.2 as well as newer versions.
if [[ ${SPLITTY_BASH_INJECT+x} ]]; then
    builtin declare __splitty_bash_flags="$SPLITTY_BASH_INJECT"
    builtin unset SPLITTY_BASH_INJECT

    if [[ ${SPLITTY_BASH_PROMPT_COMMAND+x} ]]; then
        PROMPT_COMMAND="$SPLITTY_BASH_PROMPT_COMMAND"
    else
        builtin unset PROMPT_COMMAND
    fi
    builtin unset SPLITTY_BASH_PROMPT_COMMAND

    # The first prompt follows Bash's automatic history load. Its temporary
    # empty HISTFILE prevents premature reads or truncation; restore the same
    # variables startup files normally observe before loading those files.
    for __splitty_history_var in HISTSIZE HISTFILESIZE; do
        __splitty_saved_var="SPLITTY_BASH_$__splitty_history_var"
        if builtin declare -p "$__splitty_saved_var" &>/dev/null; then
            builtin printf -v "$__splitty_history_var" '%s' "${!__splitty_saved_var}"
            builtin export "$__splitty_history_var"
        else
            builtin unset "$__splitty_history_var"
        fi
    done
    if [[ ${SPLITTY_BASH_HISTFILE+x} ]]; then
        builtin export HISTFILE="$SPLITTY_BASH_HISTFILE"
    else
        HISTFILE="$HOME/.bash_history"
        builtin export -n HISTFILE
    fi
    builtin unset SPLITTY_BASH_HISTFILE SPLITTY_BASH_HISTSIZE SPLITTY_BASH_HISTFILESIZE
    builtin unset __splitty_history_var __splitty_saved_var

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

    # Load ordinary startup history after profiles. Leave history explicitly
    # populated by startup files alone, including a profile's `history -s`.
    if [[ $__splitty_bash_flags != *"--splitty-command"* && -z "$(builtin history 1)" ]]; then
        HISTSIZE="${HISTSIZE-500}"
        HISTFILESIZE="${HISTFILESIZE-$HISTSIZE}"
        if [[ -n "$HISTFILE" && -f "$HISTFILE" && -r "$HISTFILE" ]]; then
            builtin history -r "$HISTFILE"
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
