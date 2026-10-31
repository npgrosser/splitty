# Splitty shell integration for Zsh
# Emits OSC 133 semantic prompt sequences so the terminal can detect
# prompt/input/output/command-finished boundaries.
#
# Loaded automatically by Splitty's ZDOTDIR injection.
# Can also be sourced manually:
#   [[ -n "$SPLITTY_SHELL_INTEGRATION" ]] && source "$SPLITTY_SHELL_INTEGRATION/zsh/splitty.zsh"

# Guard: only run inside Splitty, prevent double-sourcing
[[ -z "$SPLITTY_SESSION_ID" ]] && return
(( ${+__splitty_zsh_integration} )) && return
__splitty_zsh_integration=1

# The first prompt runs after .zprofile/.zshrc; they may reset the inherited PATH.
__splitty_restore_cli_path() {
    [[ -n "${SPLITTY_CLI_DIR-}" && "$SPLITTY_CLI_DIR" != *:* && -f "$SPLITTY_CLI_DIR/splitty" && -x "$SPLITTY_CLI_DIR/splitty" ]] || return 0
    local -a kept
    local entry
    for entry in "${path[@]}"; do
        [[ "$entry" == "$SPLITTY_CLI_DIR" ]] || kept+=("$entry")
    done
    path=("$SPLITTY_CLI_DIR" "${kept[@]}")
    export PATH
}

# precmd: runs after each command finishes, before the prompt is drawn.
# Emits 133;D (command finished) and 133;A (prompt start).
__splitty_precmd() {
    local ec="$?"
    __splitty_restore_cli_path
    printf '\e]133;D;%s\a' "$ec"
    printf '\e]133;A\a'
}

# preexec: runs after the user enters a command, before it executes.
# Emits 133;C (output start / command executing).
__splitty_preexec() {
    printf '\e]133;C\a'
}

# Install hooks via zsh hook arrays
autoload -Uz add-zsh-hook
add-zsh-hook precmd __splitty_precmd
add-zsh-hook preexec __splitty_preexec

# Append 133;B (input start) after the prompt
PS1="${PS1}%{$(printf '\e]133;B\a')%}"
