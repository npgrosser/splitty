## MODIFIED Requirements

### Requirement: Workspace and Screen Queries
`GET /api/mux/state` SHALL return the complete workspace state, and `GET /api/mux/screen/{id}` SHALL return the state of a single screen, with payload shapes as described by `WorkspaceState` and `ScreenState` in `docs/protocol.ts`, except that no session carries its `scratchpad` (see `session-scratchpad`) no viewer pane's `text` document carries its text (see `viewer-panes`) and no viewer pane carries its `annotations` (see `viewer-annotations`). A request for an unknown screen id MUST answer status 404 with the JSON body `{"error": "screen not found"}`.

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

### Requirement: Submitting Layout Operations
`POST /api/mux/op` SHALL accept a JSON body `{"screenId": string, "op": object}` and apply `op` to the workspace with the semantics defined in `workspace-model`. On success the server MUST respond with status 200 and `{"ok": true, "revision": n, "screenId": string, "created": array}`, where `screenId` is the actual affected screen, `n` is its post-op revision or zero if it was removed, and `created` lists the `paneId` and `sessionId` of successfully started sessions. For `open_viewer` the response SHALL also carry `paneId`, the viewer pane that shows the document, and `reused`, true when that pane existed before (see `viewer-panes`, Reopening A File). Managed-job operations SHALL use the same semantics, authorization and idempotency as the job endpoints. A session spawn failure MUST NOT be answered as `ok: true`. An op of type `set_scratchpad`, `put_annotations` or `remove_annotations` MUST be refused with status 400 and leave the workspace unchanged (see `session-scratchpad` and `viewer-annotations`).

#### Scenario: Split a pane
- **WHEN** a client posts `{"screenId": "s1", "op": {"type": "split_pane", "paneId": "p1", "axis": "horizontal"}}` and the op is valid for screen `s1`
- **THEN** the server responds with status 200, `ok` true, `screenId` `s1`, its new revision, and the created pane/session mapping
- **AND** screen `s1` contains a new pane

#### Scenario: Create a screen without an id
- **WHEN** a client posts `{"screenId": "", "op": {"type": "create_screen", "name": "backend"}}`
- **THEN** the server creates a screen named `backend` with a server-generated id
- **AND** responds with status 200, `ok` true, that actual `screenId`, revision 1 and its successfully created pane/session mapping

#### Scenario: Failed session spawn
- **WHEN** an accepted pane-creating operation cannot start its session
- **THEN** the API returns a 5xx error with `code` `spawn_failed` and the affected identifiers, and never reports that session as successfully created

#### Scenario: Scratchpad refused
- **WHEN** a client posts an op of type `set_scratchpad`
- **THEN** the server responds with status 400, and no scratchpad changes

#### Scenario: Open a viewer
- **WHEN** a client posts `{"screenId": "s1", "op": {"type": "open_viewer", "paneId": "p1", "source": {"kind": "file", "path": "/home/me/app/PLAN.md"}}}`
- **THEN** the server responds with status 200, `ok` true, an empty `created`, the new pane's `paneId` and `reused` false

#### Scenario: Open the same file again
- **WHEN** the same request is posted again while that viewer pane exists
- **THEN** the server responds with the same `paneId` and `reused` true, and the screen holds no new pane

#### Scenario: Annotations refused
- **WHEN** a client posts an op of type `put_annotations` or `remove_annotations`
- **THEN** the server responds with status 400, and no annotation changes
