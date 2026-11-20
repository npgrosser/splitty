# key-bindings Specification

## Purpose
TBD - created by archiving change native-key-bindings. Update Purpose after archive.

## Requirements

### Requirement: Window Commands
The native window SHALL name every action a key can start as a command with a stable identifier (lowercase words joined by `.` and `-`, such as `screen.next`, `tab.move-left`, `screen.select.3`). Every command other than the palette's own SHALL be reachable from the command palette whether or not a key is bound to it. Wherever the window shows the key of a command (the palette, menus, tooltips, hints, the Keyboard page), it SHALL show the key currently bound, and none for an unbound command. The `--help` text SHALL list the default keys.

#### Scenario: Rebound key in a menu
- **WHEN** the user binds `pane.split-right` to another key
- **THEN** the terminal's context menu and the palette show the new key beside Split Right

#### Scenario: Unbound command
- **WHEN** a command has no key
- **THEN** the palette lists it without a key and running it there carries it out

### Requirement: Default Keys
The window SHALL bind default keys per platform. Defaults SHALL NOT take a character, editing or navigation key without a modifier, nor Ctrl with a letter alone, from the programs in a terminal; the one function key they take is F11 for full screen on Linux and Windows. History scrolling with Shift and PageUp, PageDown, Home or End SHALL apply only while the terminal shows its normal screen. A key press bound to a command SHALL NOT reach the terminal; a key press bound to none SHALL. The next and previous Screen SHALL follow the order of the Drawer and wrap around.

#### Scenario: Keys left to terminal programs
- **WHEN** a terminal has the focus and the user presses Ctrl+B or Ctrl+F with the default bindings
- **THEN** the program in the terminal receives the key

#### Scenario: Next Screen
- **WHEN** the last Screen of the Drawer is shown and the user presses the key of `screen.next`
- **THEN** the first Screen of the Drawer is shown and the terminal receives no input

#### Scenario: History keys on the alternate screen
- **WHEN** a program shows the alternate screen and the user presses Shift+PageUp
- **THEN** the program receives the key

### Requirement: Bindings File
The user's bindings SHALL be read from `keybindings.json` in the configuration directory of the computer the application runs on: an object whose `bindings` maps command identifiers to lists of chords. A chord SHALL be modifier names (`ctrl`, `alt`, `shift`, and `cmd` on macOS or `super` elsewhere) and one key name joined by `+`. A listed command SHALL have exactly the listed chords in place of its defaults, and none for an empty list; a chord listed for a command SHALL no longer start the command that has it by default. An unknown identifier, a chord that cannot be read, and a chord that is plain typing (a character, Enter, Tab, Escape, Backspace or a navigation key without a modifier other than Shift; Shift with PageUp, PageDown, Home or End is not) SHALL be reported and skipped while the rest applies; a command whose list holds no usable chord SHALL keep its default keys. A missing or unreadable file SHALL leave the defaults. The file SHALL NOT be read or written by the starts that leave the remembered appearance alone.

#### Scenario: Replaced and removed keys
- **WHEN** the file binds `screen.next` to `alt+n` and `drawer.toggle` to an empty list
- **THEN** Alt+N shows the next Screen, Ctrl+Tab reaches the terminal, and the drawer has no key

#### Scenario: Chord taken from another command
- **WHEN** the file binds `tab.new` to the default chord of `pane.maximize`
- **THEN** the chord opens a new tab and `pane.maximize` has no key

#### Scenario: Problems in the file
- **WHEN** the file names an unknown command, a chord that cannot be read and binds a command to `a`
- **THEN** these three are reported, the other entries apply, typing `a` reaches the terminal, and the command bound to `a` keeps its default keys

### Requirement: Keyboard Settings
The settings SHALL list every command with its current keys, grouped and searchable, and SHALL let the user record a new chord for a command, remove its key and return it to the default. A change SHALL apply at once and be written to the bindings file. Recording a chord another command holds SHALL say which command loses it; recording a chord that terminal programs commonly use (Ctrl with a letter, Alt with a letter on Linux and Windows) SHALL be allowed with a note. While a chord is being recorded, the pressed keys SHALL NOT start commands or reach a terminal.

#### Scenario: Record a key
- **WHEN** the user records Ctrl+Shift+N for `screen.new`
- **THEN** the chord creates a Screen at once, the page shows it, and the bindings file holds it

#### Scenario: Reset
- **WHEN** the user resets a command whose key was changed
- **THEN** the command has its default keys and the file no longer lists it

### Requirement: Arrange Mode
The command `pane.arrange` SHALL start a mode on the focused pane in which the arrow keys move the nearest split edge in the arrow's direction, Shift with an arrow moves the pane into the neighboring group in that direction, and Escape or Enter ends the mode. While the mode is on, the window SHALL say so, and keys SHALL NOT reach the terminal. Leaving the Screen or losing the pane SHALL end the mode.

#### Scenario: Resize from the keyboard
- **WHEN** arrange mode is on in the left pane of two side by side and the user presses the right arrow
- **THEN** the left pane grows and the right pane shrinks by one step

#### Scenario: Move into the neighboring group
- **WHEN** arrange mode is on and the user presses Shift with the right arrow
- **THEN** the pane becomes a tab of the group on its right

#### Scenario: End of the mode
- **WHEN** the user presses Escape in arrange mode
- **THEN** the next key press reaches the terminal again
