# Known Issues

## xterm.js: Umlaut / IME input corruption (macOS)

**Status:** Upstream xterm.js bug, not Splitty-specific (also reproduces in ttyd).
**Not verified on Linux** — but not ruled out either.

Typing characters that involve IME composition or dead-key input (e.g. `u`
on a German keyboard layout) can corrupt the displayed line — characters get
duplicated or the cursor jumps. The shell executes correctly; it is purely a
rendering issue in xterm.js's input handling.

**Workaround:** None known. Switching to the ghostty-web backend avoids it if
ghostty-web handles IME differently, but this has not been verified.

**Reproduction:** Open any xterm.js-based web terminal (Splitty, ttyd, etc.) on
macOS, type `asdf` followed by a key — the display may show `asdff` or
similar artifacts.

## ghostty-web: no mouse reporting (TUI mouse scroll/clicks)

**Status:** Upstream ghostty-web limitation. The xterm.js backend (default) is
unaffected.

ghostty-web never encodes mouse reports (SGR 1006 and friends) back to the app,
even though it tracks the mode and exposes `hasMouseTracking()`. Fullscreen TUIs
that request mouse tracking — opencode, Claude Code in TUI mode, `htop`, `lazygit` —
therefore get no wheel, click, or drag events on this backend. Splitty falls back
to synthesizing cursor keys so the wheel scrolls *something*, but apps that bind
Up/Down to other actions (history, list selection) will behave oddly.

**Workaround:** Use the xterm.js renderer (Settings → Terminal → Renderer) for
mouse-driven TUIs.

## Image paste from GNOME image viewers doesn't work (Linux/Wayland)

**Status:** Not a Splitty bug — GNOME/Wayland platform behavior.

Copying an image with Ctrl+C in GNOME image viewers (Loupe, Eye of GNOME) and
pasting into a CLI tool (Claude Code, Codex, etc.) inside Splitty does not
work. Taking a screenshot and pasting directly works fine.

**Cause:** GNOME image viewers copy a *file reference* (`text/uri-list`,
`application/vnd.portal.files`) to the clipboard, not the actual image data
(`image/png`). CLI tools like Claude Code call `wl-paste -t image/png`, which
finds no matching MIME type and fails. This is not specific to Splitty — the
same behavior occurs in any terminal.

**Workaround:** Use the screenshot tool directly (GNOME Screenshot, PrtSc) —
this places `image/png` on the clipboard and paste works as expected.

## Middle-click paste of a terminal selection (Linux)

**Status:** Browser limitation.

Selecting text in a terminal copies it to the clipboard (`CLIPBOARD`), so
Ctrl+V and Ctrl+Shift+V paste it. Linux middle-click paste reads a separate
buffer, `PRIMARY`, which a web page cannot write, and the terminal's selection
is drawn by Splitty rather than being a native text selection. In the browser,
middle-click therefore does not paste what you selected in a terminal. The
desktop apps on Linux have not been checked for this since the move to Tauri
and Electron.

**Workaround:** Paste with Ctrl+Shift+V.
