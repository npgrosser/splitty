# Keyboard

Every action of the window that a key can start is a command. Commands have
default keys, can be given other keys, and are all in the command palette,
with or without a key.

## Changing keys

Settings, Keyboard lists every command with its keys. A row records a new
key, removes the key or returns to the default; a change applies at once.

The same bindings live in `keybindings.json` in the configuration directory
(`~/.config/splitty/`, macOS `~/Library/Application Support/splitty/`,
Windows `%APPDATA%\splitty\`), on the computer the app runs on. The file holds
only what differs from the defaults:

```json
{
  "bindings": {
    "screen.next": ["alt+n"],
    "tab.new": ["ctrl+shift+t", "alt+t"],
    "drawer.toggle": []
  }
}
```

- A command's list replaces its default keys; an empty list leaves it without
  a key.
- A key is modifier names and one key name joined by `+`: `ctrl`, `alt`,
  `shift`, and `cmd` on macOS or `super` (the Windows key) elsewhere; then a
  letter, a digit, a punctuation character (`plus` for `+`), `space`, `tab`,
  `enter`, `escape`, `backspace`, `delete`, `home`, `end`, `pageup`,
  `pagedown`, `left`, `right`, `up`, `down` or `f1` to `f12`.
- A key given to one command no longer starts the command that has it by
  default.
- A key that is plain typing (a character, Enter, Tab, Escape or a navigation
  key without Ctrl, Alt or the platform key) is refused; Shift with PageUp,
  PageDown, Home or End is allowed. Keys that terminal programs commonly use,
  such as Ctrl with a letter, are allowed; the settings page marks them.
- Entries the app cannot read are skipped and named on the settings page. A
  command whose list holds no usable key keeps its default keys.

A key bound to a command does not reach the terminal. Every other key does.

## Arrange mode

Arrange Panes puts the focused pane into a mode for the keyboard: the arrow
keys move the nearest edge of its split, Shift with an arrow moves the pane
into the neighboring group, and Escape or Enter ends the mode.

## Default keys

The defaults leave typing keys and Ctrl with a letter to the programs in the
terminal; F11 for full screen is the one function key they take. Scrolling the history with Shift and PageUp, PageDown, Home or
End applies while the terminal shows its normal screen; a full-screen program
receives those keys.

<!-- keys:begin -->

### Screens

| Command | Identifier | Linux, Windows | macOS |
|---|---|---|---|
| New Screen | `screen.new` | `Ctrl+Shift+Space` | `⌃⇧Space` |
| Next Screen | `screen.next` | `Ctrl+Tab` | `⌃Tab` |
| Previous Screen | `screen.previous` | `Ctrl+Shift+Tab` | `⌃⇧Tab` |
| Switch to Screen 1 | `screen.select.1` | `Alt+1`, `Alt+Shift+1` | `⌘1` |
| Switch to Screen 2 | `screen.select.2` | `Alt+2`, `Alt+Shift+2` | `⌘2` |
| Switch to Screen 3 | `screen.select.3` | `Alt+3`, `Alt+Shift+3` | `⌘3` |
| Switch to Screen 4 | `screen.select.4` | `Alt+4`, `Alt+Shift+4` | `⌘4` |
| Switch to Screen 5 | `screen.select.5` | `Alt+5`, `Alt+Shift+5` | `⌘5` |
| Switch to Screen 6 | `screen.select.6` | `Alt+6`, `Alt+Shift+6` | `⌘6` |
| Switch to Screen 7 | `screen.select.7` | `Alt+7`, `Alt+Shift+7` | `⌘7` |
| Switch to Screen 8 | `screen.select.8` | `Alt+8`, `Alt+Shift+8` | `⌘8` |
| Switch to Screen 9 | `screen.select.9` | `Alt+9`, `Alt+Shift+9` | `⌘9` |
| Close Screen | `screen.close` | `Alt+Shift+W` | `⇧⌘W` |
| Rename Screen | `screen.rename` | `Alt+Shift+R` | `⇧⌘R` |
| Move Screen Up | `screen.move-up` | none | none |
| Move Screen Down | `screen.move-down` | none | none |
| Move Screen to New Window | `screen.move-to-new-window` | none | none |
| Move Screen to Next Window | `screen.move-to-next-window` | none | none |

### Tabs

| Command | Identifier | Linux, Windows | macOS |
|---|---|---|---|
| New Tab | `tab.new` | `Alt+T` | `⌘T` |
| New Tab with Profile… | `tab.new-with-profile` | `Alt+Shift+T` | `⇧⌘T` |
| Close Tab | `tab.close` | `Alt+W` | `⌘W` |
| Close Other Tabs | `tab.close-others` | none | none |
| Next Tab | `tab.next` | `Alt+PageDown` | `⌘PageDown` |
| Previous Tab | `tab.previous` | `Alt+PageUp` | `⌘PageUp` |
| Move Tab Right | `tab.move-right` | `Alt+Shift+PageDown` | `⇧⌘PageDown` |
| Move Tab Left | `tab.move-left` | `Alt+Shift+PageUp` | `⇧⌘PageUp` |
| Move Tab to Screen… | `tab.move-to-screen` | `Alt+Shift+M` | `⇧⌘M` |

### Panes

| Command | Identifier | Linux, Windows | macOS |
|---|---|---|---|
| Split Right | `pane.split-right` | `Alt+D` | `⌘D` |
| Split Down | `pane.split-down` | `Alt+Shift+D` | `⇧⌘D` |
| Focus Pane Left | `pane.focus-left` | `Alt+Shift+←` | `⇧⌘←` |
| Focus Pane Right | `pane.focus-right` | `Alt+Shift+→` | `⇧⌘→` |
| Focus Pane Above | `pane.focus-up` | `Alt+Shift+↑` | `⇧⌘↑` |
| Focus Pane Below | `pane.focus-down` | `Alt+Shift+↓` | `⇧⌘↓` |
| Maximize Pane | `pane.maximize` | `Alt+M` | `⌘M` |
| Float or Dock Pane | `pane.float` | `Alt+Shift+F` | `⇧⌘F` |
| Arrange Panes | `pane.arrange` | `Alt+R` | `⌘R` |
| Toggle Scratchpad | `pane.scratchpad` | `Alt+J` | `⌘J` |

### Terminal

| Command | Identifier | Linux, Windows | macOS |
|---|---|---|---|
| Find in Terminal | `terminal.find` | `Ctrl+Shift+F` | `⌘F` |
| Copy | `terminal.copy` | `Ctrl+Shift+C`, `Super+C` | `⌘C` |
| Paste | `terminal.paste` | `Ctrl+Shift+V`, `Super+V` | `⌘V` |
| Clear Buffer | `terminal.clear` | `Alt+K` | `⌘K` |
| Scroll Page Up | `terminal.scroll-page-up` | `Shift+PageUp` | `⇧PageUp` |
| Scroll Page Down | `terminal.scroll-page-down` | `Shift+PageDown` | `⇧PageDown` |
| Scroll to Top | `terminal.scroll-top` | `Shift+Home` | `⇧Home` |
| Scroll to Bottom | `terminal.scroll-bottom` | `Shift+End` | `⇧End` |
| Copy Working Directory | `terminal.copy-directory` | none | none |
| Open Selection in Viewer | `terminal.open-viewer` | none | none |

### View

| Command | Identifier | Linux, Windows | macOS |
|---|---|---|---|
| Toggle Drawer | `drawer.toggle` | `Alt+Shift+B` | `⇧⌘B` |
| Focus Drawer | `drawer.focus` | `Alt+E` | `⌘E` |
| Hosts and Ports | `view.hosts` | `Alt+H` | `⇧⌘H` |
| Templates… | `view.templates` | `Alt+P` | `⌘P` |
| Settings | `view.settings` | `Ctrl+,` | `⌘,` |
| Command Palette | `view.palette` | `Alt+Shift+P` | `⇧⌘P` |
| Keyboard Shortcuts | `view.keyboard` | `Alt+Shift+K` | `⇧⌘K` |
| Connect to Host… | `host.connect` | none | none |

### Window

| Command | Identifier | Linux, Windows | macOS |
|---|---|---|---|
| Zoom In | `zoom.in` | `Ctrl+=`, `Ctrl++` | `⌘=`, `⌘+` |
| Zoom Out | `zoom.out` | `Ctrl+-` | `⌘-` |
| Reset Zoom | `zoom.reset` | `Ctrl+0` | `⌘0` |
| Toggle Full Screen | `window.fullscreen` | `F11` | `⌃⌘F` |
| New Window | `window.new` | `Ctrl+Shift+N` | `⌘N` |
| Close Window | `window.close` | none | none |
| Quit | `window.quit` | none | `⌘Q` |

<!-- keys:end -->
