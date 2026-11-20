# Design: native key bindings

## Context

`Workspace::key` runs in the capture phase and decides, by the state of the window, which keys belong to
the window before the terminal sees them. The decision of *which* command a key means is a chain of
hand-written comparisons; labels are written again wherever a key is shown.

## Goals

- One registry is the source of commands, default keys and labels.
- A user can rebind or unbind any command; an unbound chord reaches the terminal.
- Defaults leave unmodified keys and Ctrl with a letter to programs in the terminal.
- Every action that had only a pointer path gets a command; frequent ones get a default key.

## Decisions

### Registry and keymap

`splitty_client_core::keymap` holds the commands (identifier such as `screen.next`, title, category), a
chord type (Ctrl, Alt, Shift, the platform key, and a key name), the defaults per platform and the
effective keymap: defaults with the user's overrides laid over them. It has no window types and is
unit-tested. The window turns a key press into a chord (for Shift with a digit, by the keyboard layout as
before) and asks the keymap for the command.

Which commands the window accepts in which state stays where it is, in the window: destinations of the
Drawer work from each other, the rest only on the Screens, and menus, prompts and dialogs keep the
keyboard while they are open.

### Defaults

`Mod` is Alt on Linux and Windows and Cmd on macOS. Chords with brackets or the backtick are avoided
because they need AltGr or a dead key on common layouts. The full table is in `docs/keyboard.md`, which a
test keeps equal to the registry.

- Screens are the unit a user moves between most, so they take the most common "next document" key:
  Ctrl+Tab and Ctrl+Shift+Tab. Tabs keep Mod+PageUp/PageDown; with Shift the same keys move the tab.
- A Screen by number is Mod+1 to Mod+9, which needs no layout knowledge. Mod+Shift+1 to 9 stays on Linux
  and Windows; on macOS it collides with the screenshot keys and goes.
- Ctrl+B and Ctrl+F return to the terminal. The drawer takes Mod+Shift+B, find Ctrl+Shift+F.
- Shift+PageUp/PageDown/Home/End scroll the history while the normal screen is shown; a program on the
  alternate screen receives them.

### Bindings file

`keybindings.json` in the application's configuration directory, next to the remembered appearance and
under the same rules (private, bounded, atomic, not used by isolated starts):

```json
{ "bindings": { "screen.next": ["ctrl+tab"], "drawer.toggle": [] } }
```

Only what differs from the defaults is stored. A list replaces the command's defaults; an empty list
unbinds it. A chord given to a command is taken from the command that has it by default. Unknown
identifiers, chords that do not parse and chords that are plain typing are reported and skipped. The file
is local because keys belong to a keyboard and an operating system, not to a workspace.

### Arrange mode

Resizing and moving by direct chords would need a third modifier layer that collides with desktop keys
(Ctrl+Alt with arrows switches workspaces on common desktops). Instead one key enters a mode on the
focused pane: arrows move the nearest split edge in that direction by a step, Shift with an arrow moves
the pane into the neighboring group, Escape or Enter leaves. A line in the window says so while it is on.

### Keyboard page

A page of the settings lists the registry by category: title, current keys, and whether they are the
defaults. A row records a new chord from the next key press, resets to the default or removes the key.
Problems of the file and conflicts are shown where they apply; a chord terminal programs commonly use is
allowed with a note. The page saves the file and applies it at once. It replaces the palette's read-only
list.

## Architecture impact

`architecture/*.c4`: a new component for `splitty_client_core::keymap`; the native workspace, terminal
and entry point depend on it. `docs/architecture.md`: none.

## Provisional decisions

Taken 2026-11-20 without the maintainer's confirmation beyond the default table; each is also in
`openspec/open-decisions.md`.

- **NKB-1:** bindings are a local file of the application, not a server setting. Alternative: a settings
  section stored by the local service and shared by its clients.
- **NKB-2:** one chord per binding, no key sequences. Alternative: a prefix key with a second key.
- **NKB-3:** an override replaces a command's defaults and takes a chord away from the command that has
  it by default. Alternative: refuse a chord that is in use until the other command is rebound.
- **NKB-4:** resizing and moving a pane by keyboard happen in an arrange mode. Alternative: direct chords
  with a third modifier.
- **NKB-5:** history scrolling takes Shift+PageUp/PageDown/Home/End only on the normal screen.
  Alternative: always, or leave them to the terminal.
- **NKB-6:** Mod+Shift+1 to 9 stays as a second default on Linux and Windows. Alternative: remove it.
- **NKB-7:** chords that are plain typing cannot be bound; chords programs commonly use can, with a note.
  Alternative: refuse both, or allow both.
