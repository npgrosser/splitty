# Known Issues

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

**Status:** Not implemented in the app.

Linux middle-click paste reads a separate buffer, `PRIMARY`. The app copies a
terminal selection only with **Copy** (Ctrl+Shift+C, the context menu) and
writes it to the clipboard (`CLIPBOARD`), not to `PRIMARY`. A middle click in
a terminal goes to a program that asked for mouse reports and pastes nothing
otherwise.

**Workaround:** Paste with Ctrl+Shift+V.
