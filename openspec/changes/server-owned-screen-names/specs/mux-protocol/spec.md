# Spec Delta

## MODIFIED Requirements

### Requirement: Client Resynchronization
A client MUST treat the server's workspace state as authoritative after connecting: on `mux.welcome` it SHALL create any listed screen it does not have locally and SHALL send `mux.resync` for every listed screen. A client SHALL build every screen listed in `mux.welcome` from its `mux.screen_snapshot`, also when it has prepared the screen locally before the snapshot arrives, and MUST NOT send `create_screen` for such a screen in its place. (Re-creating a screen that later ran empty stays allowed, see `workspace-model`.) Before the first `mux.welcome` of a connection a client MUST NOT send `create_screen`. It SHALL create an initial screen of its own only when the welcome lists no screens, with `onlyIfEmpty: true` (see `workspace-model`, First Screen of an Empty Workspace), and SHALL show only the listed screens when the welcome lists any. When the server refuses that initial screen, the client SHALL drop it and show the screens the server has instead. On a `mux.nudge` that carries a `screenId`, a client SHALL send `mux.resync` for that screen.

#### Scenario: Reconnect restores state
- **WHEN** a client reconnects and receives `mux.welcome` listing screens `s1` and `s2`
- **THEN** the client SHALL send `mux.resync` for `s1` and for `s2`

#### Scenario: Nudge triggers resync
- **WHEN** a client receives `mux.nudge` with `screenId` `s1`
- **THEN** the client SHALL send `mux.resync` for `s1`

#### Scenario: Page reload against an existing workspace
- **WHEN** a client starts while the server's workspace holds screen `s1` whose panes run sessions
- **THEN** the client SHALL send `mux.resync` for `s1` and no `create_screen`
- **AND** its view of `s1` SHALL show those panes connected to their existing sessions

#### Scenario: First start against an empty workspace
- **WHEN** a client starts and `mux.welcome` lists no screens
- **THEN** the client SHALL create one screen with `create_screen` and `onlyIfEmpty: true`

#### Scenario: Two windows start against an empty workspace
- **WHEN** two windows start together, both welcomes list no screens and both send their initial `create_screen` with `onlyIfEmpty: true`
- **THEN** the server holds exactly one screen
- **AND** the window whose create was refused drops its own screen and shows the one the server holds

#### Scenario: Older server
- **WHEN** a server without `screen-names-v1` ignores `onlyIfEmpty` and two windows start together against its empty workspace
- **THEN** each window's initial screen is created, as before

## ADDED Requirements

### Requirement: Server-Chosen Screen Names
A server SHALL list `screen-names-v1` in `mux.welcome.capabilities`. A client SHALL send `create_screen`, and `move_pane_to_screen` with `createScreen`, for a new screen without a name to such a server, SHALL show the name the server reports for the screen, and SHALL send no label of its own in place of it. Until the name arrives it SHALL show a placeholder that names no screen. A screen the server reports without a name SHALL be shown as "Unnamed Screen", never by its position.

#### Scenario: New screen named by the server
- **WHEN** a window creates a screen on a server that announces `screen-names-v1` and holds "Screen 1" and "Screen 3"
- **THEN** its `create_screen` carries no `name`
- **AND** the window shows the screen as "Screen 2" once the server reports it

#### Scenario: Two windows create screens
- **WHEN** two windows each create a screen on the same server at the same time
- **THEN** the two screens get different names, chosen by the server in the order it applies the ops

#### Scenario: Screen without a name
- **WHEN** the server reports a screen with an empty name in second place, and another screen is named "Screen 2"
- **THEN** the window shows the unnamed one as "Unnamed Screen", not as "Screen 2"

#### Scenario: Older server
- **WHEN** a window creates a screen on a server whose `mux.welcome` lacks `screen-names-v1`
- **THEN** its `create_screen` names it "Screen N" with the smallest positive N that no screen of that server uses as far as the window knows when it sends the op
