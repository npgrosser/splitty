## MODIFIED Requirements

### Requirement: Saving Through The Window
A client SHALL save a scratchpad with the screen-level operation `set_scratchpad`, carrying the session's `sessionId` and its whole `text`, over `mux-protocol`; an empty text clears it. The server SHALL store the text in the session record, which screen snapshots carry as `scratchpad` (absent while empty), and SHALL let the other connected clients know (see `mux-protocol`, Change Propagation to Other Clients). Every accepted save SHALL raise the session's `scratchpadVersion` by one; it starts at 0, is absent from snapshots while 0, and lives as long as the scratchpad. A save MAY carry `ifVersion`; the server SHALL then accept it only while the session's `scratchpadVersion` equals it, and otherwise refuse it with `mux.op_failed` without changing the scratchpad. A save without `ifVersion` SHALL be accepted as before. The server SHALL announce `scratchpad-versions-v1` in `mux.welcome.capabilities`.

#### Scenario: Second client sees the change
- **WHEN** client A saves text in a session's scratchpad while client B shows that session
- **THEN** client B shows the text after its resync, and A receives nothing for its save

#### Scenario: Save based on an older version
- **WHEN** a session's `scratchpadVersion` is 4 and a client saves with `ifVersion` 3
- **THEN** the client receives `mux.op_failed` for `set_scratchpad`, and the scratchpad keeps its text and version 4

#### Scenario: Accepted save
- **WHEN** a client saves with `ifVersion` 4 while the version is 4
- **THEN** the scratchpad holds the text, and the next snapshot carries `scratchpadVersion` 5

### Requirement: Concurrent Edits
A client SHALL replace the text it shows with a newer one from the server only while it holds no unsaved edit of its own; an unsaved edit SHALL take the version of the snapshots that arrive meanwhile, so that its save becomes the scratchpad's text. With `scratchpad-versions-v1`, a client SHALL save with the version its shown text is based on and SHALL then show a snapshot's text only when the snapshot's `scratchpadVersion` is at least that version plus one; when another client's save reached the server first, the client's save is refused and the other text shows. Without the capability, a client SHALL save without `ifVersion` and SHALL keep its saved text against snapshots for two seconds after the save. A client SHALL forget the versions it knows when its connection is made again.

#### Scenario: Unsaved edit is kept
- **WHEN** client B holds an unsaved edit of a scratchpad while a save of client A arrives
- **THEN** B keeps its edit, and B's following save becomes the scratchpad's text

#### Scenario: Snapshot from before the save
- **WHEN** a window saves a scratchpad with `ifVersion` 2 and then receives a snapshot with `scratchpadVersion` 2 and the old text
- **THEN** the window keeps showing its saved text, and a later snapshot with version 3 or higher replaces it

#### Scenario: Another window saved first
- **WHEN** window A and window B both base an edit on version 2 and B's save reaches the server first
- **THEN** A's save is refused, and A shows B's text once the snapshot with version 3 arrives

#### Scenario: Older server
- **WHEN** a window saves a scratchpad of a host whose `mux.welcome` does not list `scratchpad-versions-v1`
- **THEN** the save carries no `ifVersion`, and the window holds its saved text against snapshots for two seconds
