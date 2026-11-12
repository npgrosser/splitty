# Spec Delta

## MODIFIED Requirements

### Requirement: Workspace and Screen Queries
`GET /api/mux/state` SHALL return the complete workspace state, and `GET /api/mux/screen/{id}` SHALL return the state of a single screen, with payload shapes as described by `WorkspaceState` and `ScreenState` in `docs/protocol.ts`, except that no session carries its `scratchpad` (see `session-scratchpad`), no viewer pane's `text` document carries its text (see `viewer-panes`), and no viewer pane carries its `annotations` (see `viewer-annotations`). A session that holds a root status record SHALL carry it as `programStatus` (see `program-status`). A request for an unknown screen id MUST answer status 404 with the JSON body `{"error": "screen not found"}`.

#### Scenario: Existing screen
- **WHEN** a client sends `GET /api/mux/screen/{id}` for an existing screen
- **THEN** the server responds with status 200 and that screen's layout, panes and sessions

#### Scenario: Unknown screen
- **WHEN** a client sends `GET /api/mux/screen/does-not-exist`
- **THEN** the server responds with status 404 and `{"error": "screen not found"}`

#### Scenario: Scratchpad omitted
- **WHEN** a session's scratchpad holds text and a client sends `GET /api/mux/state` or `GET /api/mux/screen/{id}` for its screen
- **THEN** the session appears without `scratchpad`

#### Scenario: Viewer text omitted
- **WHEN** a viewer pane shows a terminal selection and a process in a pane sends `GET /api/mux/state` over the control socket
- **THEN** the viewer pane appears with its `viewer` record but without the selected text

#### Scenario: Annotations omitted
- **WHEN** a viewer pane holds annotations and a client sends `GET /api/mux/state` or `GET /api/mux/screen/{id}` for its screen
- **THEN** the viewer pane appears with its `viewer` record but without `annotations`

#### Scenario: Program status included
- **WHEN** a session's program has reported `working` and a client sends `GET /api/mux/screen/{id}` for its screen
- **THEN** the session appears with `programStatus` holding the state `working`
- **AND** a session without a root status record appears without `programStatus`
