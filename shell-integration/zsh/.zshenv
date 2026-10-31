# Splitty shell integration bootstrap for zsh.
# This file is loaded automatically when ZDOTDIR is set to this directory
# by the Splitty PTY spawner. Do not source manually.

# Restore the original ZDOTDIR so the user's config files are found.
if [[ -n "${SPLITTY_ZSH_ZDOTDIR+X}" ]]; then
    'builtin' 'export' ZDOTDIR="$SPLITTY_ZSH_ZDOTDIR"
    'builtin' 'unset' 'SPLITTY_ZSH_ZDOTDIR'
else
    'builtin' 'unset' 'ZDOTDIR'
fi

# Source the user's .zshenv first (it may set fpath, etc.).
{
    'builtin' 'typeset' _splitty_file=${ZDOTDIR-$HOME}"/.zshenv"
    [[ ! -r "$_splitty_file" ]] || 'builtin' 'source' '--' "$_splitty_file"
} always {
    # Source our integration for interactive shells.
    if [[ -o 'interactive' ]]; then
        _splitty_file="${${(%):-%x}:A:h}"/splitty.zsh
        if [[ -r "$_splitty_file" ]]; then
            'builtin' 'source' '--' "$_splitty_file"
        fi
    fi
    'builtin' 'unset' '_splitty_file'
}
