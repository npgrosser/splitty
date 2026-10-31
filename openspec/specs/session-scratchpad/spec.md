# session-scratchpad Specification

## Purpose
TBD - created by archiving change session-scratchpad. Update Purpose after archive.

## Requirements

### Requirement: Scratchpad Per Session
Every terminal session SHALL carry one plain-text scratchpad, empty when the session starts. A pane created by any operation SHALL start with an empty scratchpad; nothing is inherited from the pane it came from. The text SHALL be at most 65,536 bytes of UTF-8: the server SHALL refuse a longer one without changing the scratchpad, and a client SHALL NOT send one. Panes without a session have no scratchpad.

#### Scenario: Split pane starts empty
- **WHEN** a pane whose scratchpad holds text is split
- **THEN** the new pane's session has an empty scratchpad, and the first pane keeps its text

#### Scenario: Text over the limit
- **WHEN** a client sets a scratchpad to a text of 65,537 bytes
- **THEN** the server rejects the operation, and the scratchpad keeps its previous text

### Requirement: Scratchpad Lifetime
A scratchpad SHALL live exactly as long as its session record: through the session's process exiting, a client's reload or reconnect, the pane being moved, stacked, floated, docked or moved to another screen, and, in daemon mode, a daemon restart, update or crash, kept sessions included (see `session-keepers`). It SHALL end with its session: when its pane or screen is closed, or when an embedded server stops.

#### Scenario: Daemon restart
- **WHEN** a daemon restarts while a session's scratchpad holds text
- **THEN** the session's scratchpad holds the same text afterwards

#### Scenario: Pane moved to another screen
- **WHEN** a pane whose scratchpad holds text moves to another screen
- **THEN** the scratchpad moves with its session and keeps its text

#### Scenario: Pane closed
- **WHEN** the pane is closed
- **THEN** its scratchpad ends with its session

### Requirement: Saving Through The Window
A client SHALL save a scratchpad with the screen-level operation `set_scratchpad`, carrying the session's `sessionId` and its whole `text`, over `mux-protocol`; an empty text clears it. The server SHALL store the text in the session record, which screen snapshots carry as `scratchpad` (absent while empty), and SHALL let the other connected clients know (see `mux-protocol`, Change Propagation to Other Clients).

#### Scenario: Second client sees the change
- **WHEN** client A saves text in a session's scratchpad while client B shows that session
- **THEN** client B shows the text after its resync, and A receives nothing for its save

### Requirement: Concurrent Edits
Saves of one scratchpad from several clients SHALL resolve as last write wins. A client SHALL replace the text it shows with a newer one from the server only while it holds no unsaved edit of its own.

#### Scenario: Unsaved edit is kept
- **WHEN** client B holds an unsaved edit of a scratchpad while a save of client A arrives
- **THEN** B keeps its edit, and B's following save becomes the scratchpad's text

### Requirement: Scratchpad Capability
The server SHALL announce `scratchpad-v1` in `mux.welcome.capabilities`. A client SHALL offer a server's scratchpads only when that server announced it, so that no text is typed into a scratchpad a server would not keep.

#### Scenario: Host without scratchpads
- **WHEN** a window connects to a host whose daemon does not announce `scratchpad-v1`
- **THEN** the window offers no scratchpad for that host's panes

### Requirement: The Window Is The Only Channel
Scratchpads SHALL be shown and changed only through a window's `mux-protocol` connection. The control API, and with it the CLI and the processes in panes, SHALL neither return a scratchpad's text nor change it (see `control-api`), so that text written for later does not reach an agent before the user sends it.

#### Scenario: Agent in a pane reads the workspace
- **WHEN** a process in a pane requests `GET /api/mux/state` over the control socket
- **THEN** the answer contains no scratchpad text

### Requirement: Scratchpad In The Pane
A client SHALL show a session's scratchpad in a collapsible area inside the session's terminal pane, not in a pane of its own, toggled by a keyboard shortcut and by a pane control. The area SHALL follow its session wherever the pane goes. Whether the area is open belongs to the window, not to the session. While a scratchpad holds text, its pane's tab SHALL show a marker, whether the area is open or not.

#### Scenario: Pane stacked into tabs
- **WHEN** a pane with an open scratchpad area moves into another pane's tab stack
- **THEN** its area is open with the same text whenever its tab is shown

#### Scenario: Two windows
- **WHEN** two windows show the same session and one of them opens its scratchpad area
- **THEN** the other keeps its area as it was, and both show the same text

### Requirement: Confirmation Before Text Is Lost
Closing a pane whose scratchpad holds text SHALL ask first, as closing a pane with a running program does; closing several panes at once SHALL ask once for all. Quitting a window whose embedded server would end sessions whose scratchpads hold text SHALL ask first as well. Quitting a window of a daemon SHALL NOT ask for scratchpads, since its sessions continue.

#### Scenario: Close a pane with text
- **WHEN** the user closes an idle shell pane whose scratchpad holds text
- **THEN** a confirmation names the scratchpad, and the pane closes only when the user confirms

#### Scenario: Quit an embedded window
- **WHEN** the user quits a window of an embedded server while a session's scratchpad holds text
- **THEN** the window asks before quitting

### Requirement: Images In The Scratchpad
When the user pastes into a scratchpad and the clipboard data holds files and no text, or drops files onto it, the client SHALL store each file on the server of the session's connection as a pane does (see `terminal-sessions`, Image Paste In A Pane and Files Into A Pane) and SHALL insert the returned paths into the text at the cursor, in place of a selection, several paths separated by spaces. The inserted path is an edit like typed text. A paste without a file SHALL behave as before. When the server refuses a file, or does not know the message, the client SHALL say so in a notice and insert nothing.

#### Scenario: Screenshot into the scratchpad
- **WHEN** the clipboard holds a screenshot and the user pastes into a session's scratchpad
- **THEN** the image is stored on the session's server, its path appears in the text at the cursor, and the terminal receives nothing

#### Scenario: Text in the clipboard
- **WHEN** the clipboard holds text and the user pastes into the scratchpad
- **THEN** the text is inserted and no `save_image` is sent

#### Scenario: A log dropped into the scratchpad
- **WHEN** the user drops `build.log` onto a scratchpad
- **THEN** the file is uploaded to the session's server and its path appears at the cursor
