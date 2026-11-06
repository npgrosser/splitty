# Spec Delta

## ADDED Requirements

### Requirement: Template Extraction Endpoint
Authenticated windows SHALL extract a template with `POST /api/templates/extract/screen` or native `template.extract`, using the request and response in docs/protocol.ts. Missing screens SHALL return 404 and screens with no tiled terminals SHALL return 400. This endpoint SHALL NOT be exposed on owner control or restricted agent sockets.

#### Scenario: Transport parity
- **WHEN** the same extraction is requested over authenticated HTTP and native API
- **THEN** both return equivalent templates or equivalent errors without changing the screen

## MODIFIED Requirements

### Requirement: Submitting Layout Operations
`POST /api/mux/op` SHALL accept a JSON body `{"screenId": string, "op": object}` and apply `op` to the workspace with the semantics defined in `workspace-model`. On success the server MUST respond with status 200 and `{"ok": true, "revision": n, "screenId": string, "created": array}`, where `screenId` is the actual affected screen, `n` is its post-op revision or zero if it was removed, and `created` lists the `paneId` and `sessionId` of successfully started sessions. For `open_viewer` the response SHALL also carry `paneId`, the viewer pane that shows the document, `reused`, true when that pane existed before (see `viewer-panes`, Reopening A File), and `viewerRevision`, the revision of that pane's viewer record after the operation. Managed-job operations SHALL use the same semantics, authorization and idempotency as the job endpoints. A session spawn failure MUST NOT be answered as `ok: true`. An op of type `apply_template`, `set_scratchpad`, `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies` or `mark_sent` MUST be refused with status 400 and leave the workspace unchanged (see `session-scratchpad` and `viewer-annotations`).

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
- **THEN** the server responds with status 200, `ok` true, an empty `created`, the new pane's `paneId`, `reused` false and `viewerRevision` 1

#### Scenario: Open the same file again
- **WHEN** the same request is posted again while that viewer pane exists
- **THEN** the server responds with the same `paneId`, `reused` true and a higher `viewerRevision`, and the screen holds no new pane

#### Scenario: Annotations refused
- **WHEN** a client posts an op of type `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies` or `mark_sent`
- **THEN** the server responds with status 400, and no annotation changes

