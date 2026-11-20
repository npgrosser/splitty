# Native key bindings

## Why

The window's keyboard shortcuts are matched by hand in one function. Nothing can be rebound, the key
labels in menus, the palette, the help text and the README are written separately and disagree, and
frequent actions have no key at all: the next or previous Screen, closing or renaming a Screen, moving a
tab, moving a pane to another group or Screen, resizing a split, floating a group, clearing the buffer,
scrolling the history, full screen. Two defaults also take keys from programs in the terminal: Ctrl+B
(the drawer, consumed even when it does nothing) and Ctrl+F (find).

No roadmap milestone; it follows `native-workspace-shortcuts`, which left customization, keyboard
resizing and a complete set of keys out of scope.

## What Changes

- Every command of the window has a stable identifier, a title, a category and default keys per platform
  in one registry. The dispatch, the command palette, menus, tooltips, hints, the `--help` text and the
  README table take keys from it.
- Default keys for the commands that had none, among them Ctrl+Tab and Ctrl+Shift+Tab for the next and
  previous Screen, Mod+1 to Mod+9 for a Screen by number, Mod+Shift+W and Mod+Shift+R to close and rename
  a Screen, Mod+Shift+PageUp/PageDown to move a tab, Mod+Shift+M to move a tab to another Screen,
  Mod+Shift+F to float or dock, Mod+K to clear the buffer, Shift+PageUp/PageDown/Home/End to scroll the
  history, F11 for full screen and Cmd+Q on macOS.
- **BREAKING** defaults: the drawer moves from Ctrl+B to Mod+Shift+B and find from Ctrl+F to Ctrl+Shift+F
  (Cmd+F on macOS stays), so Ctrl+B and Ctrl+F reach the terminal again; Settings is Cmd+, on macOS.
  Mod+Shift+1 to 9 stays as a second key for Screens on Linux and Windows.
- An arrange mode (Mod+R): arrow keys resize the focused pane's splits, Shift with an arrow moves the pane
  into the neighboring group, Escape or Enter ends it.
- The user's own bindings live in `keybindings.json` in the application's configuration directory and
  replace or remove defaults per command.
- Settings gain a Keyboard page: every command by category with its keys, a search, recording a new key,
  resetting to the default, and a note where a key is taken by another command or commonly used by
  terminal programs. It is also the overview of all keys.

## Out of scope

- Key sequences (a prefix key followed by another), per-host or per-Screen bindings, and bindings sent to
  or stored by the server.
- Keys inside menus, lists and dialogs (arrows, Enter, Escape), which stay fixed.
- Passing a bound key through to the terminal for one press.
- New windows; the app has one.

## Capabilities

### New Capabilities

- `key-bindings`: command identifiers, default-key rules, the bindings file, conflicts, the arrange mode
  and where keys are shown.

### Modified Capabilities

- `display-zoom`: Zoom Shortcuts names the keys as defaults of rebindable commands.

## Impact

- `crates/splitty-client-core`: a new `keymap` module (registry, chords, defaults, overrides file).
- `crates/splitty-gpui`: key dispatch, the new commands, the arrange mode, the Keyboard settings page,
  every key label.
- `README.md`, `docs/`: the shortcut table comes from the registry; a keyboard reference.
- `architecture/`: the new module and its dependents.
