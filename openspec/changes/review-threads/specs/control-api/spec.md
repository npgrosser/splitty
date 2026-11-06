## MODIFIED Requirements

### Requirement: Submitting Layout Operations
`POST /api/mux/op` SHALL accept a JSON body `{"screenId": string, "op": object}` and apply `op` to the workspace with the semantics defined in `workspace-model`. On success the server MUST respond with status 200 and `{"ok": true, "revision": n, "screenId": string, "created": array}`, where `screenId` is the actual affected screen, `n` is its post-op revision or zero if it was removed, and `created` lists the `paneId` and `sessionId` of successfully started sessions. For `open_viewer` the response SHALL also carry `paneId`, the viewer pane that shows the document, and `reused`, true when that pane existed before (see `viewer-panes`, Reopening A File). Managed-job operations SHALL use the same semantics, authorization and idempotency as the job endpoints. A session spawn failure MUST NOT be answered as `ok: true`. An op of type `set_scratchpad`, `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies` or `mark_sent` MUST be refused with status 400 and leave the workspace unchanged (see `session-scratchpad` and `viewer-annotations`).

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
- **WHEN** a client posts an op of type `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies` or `mark_sent`
- **THEN** the server responds with status 400, and no annotation changes

## ADDED Requirements

### Requirement: Viewer Comment Requests
`GET /api/viewer/comments` SHALL take the query parameters `screenId` and `paneId`, the caller's screen and pane, and optionally `document`, an absolute path or a viewer pane's ID, and `all`. It SHALL respond 200 with `{"documents": [...]}`: for each viewer pane of the screen that shows the document, or for each viewer pane of the screen without `document`, its `paneId`, its source without a text's text, and the annotations an agent may read (see `viewer-annotations`, Agents Read And Reply To Handed-Over Threads), the open ones only unless `all` is `true`. Each annotation SHALL carry its number, kind, status, remark, replacement, target and the replies an agent may read, each with its author and, for an agent's reply, its pane and title; shapes are in `docs/protocol.ts`. `POST /api/viewer/comments/reply` SHALL take `{"screenId", "paneId", "document", "number", "body"}`, add `body` as an agent's reply with the caller's pane and that pane's title to the annotation with that number, and respond 200 `{"ok": true, "paneId": string, "number": n}`. Both endpoints SHALL respond 404 with `code` `not_found` for a screen, a caller pane or a document the screen does not hold and, for a reply, an unknown number or an annotation the user has not handed over; 409 `ambiguous`, naming the viewer panes, for a path that several viewer panes of the screen show; 400 `invalid` for a reply that is empty, only whitespace or over 16,384 bytes; and 409 `limit` for a reply the thread or the pane cannot take. Grant sockets SHALL NOT expose these endpoints.

#### Scenario: Comments of a screen
- **WHEN** a pane process requests `GET /api/viewer/comments?screenId=s1&paneId=p1` after the user sent a review on screen `s1`
- **THEN** the server responds 200 with the viewer panes of `s1` and their handed-over annotations

#### Scenario: A reply
- **WHEN** a pane process posts `{"screenId": "s1", "paneId": "p1", "document": "/home/me/app/PLAN.md", "number": 2, "body": "Done."}` to `/api/viewer/comments/reply`
- **THEN** the server responds 200, and annotation #2 holds the reply with author `agent` and pane `p1`

#### Scenario: Unknown number
- **WHEN** the same request names number 9, which the pane does not hold
- **THEN** the server responds 404 with `code` `not_found`, and nothing changes

### Requirement: Review Requests
`POST /api/viewer/review/wait` SHALL take `{"screenId", "paneId", "viewerPaneId"}`, the caller's screen and pane and the viewer pane whose review it waits for, start a wait for a review of that pane (see `viewer-annotations`, Waiting For A Review), and hold the request until the wait ends. It SHALL respond 200 with `{"outcome": "approved"}` when the user approves, or `{"outcome": "feedback", "review": string}` when the user sends a review; 410 with `code` `closed` when the viewer pane closes first; 404 `not_found` for a screen, caller pane or viewer pane the screen does not hold, or a pane that is not a viewer pane; 409 `busy` while another wait for the same viewer pane exists; and 429 `busy` while 16 waits exist on the server. When the caller's connection ends before an answer, the server SHALL end the wait without an outcome. Grant sockets SHALL NOT expose this endpoint.

#### Scenario: Approved
- **WHEN** a pane process waits for viewer pane `v1` and the user approves in a window
- **THEN** the request responds 200 with `{"outcome": "approved"}`

#### Scenario: Second wait
- **WHEN** a second request for `v1` arrives while the first waits
- **THEN** it responds 409 with `code` `busy`, and the first keeps waiting

#### Scenario: Caller gone
- **WHEN** the waiting process is killed
- **THEN** the wait ends, and `v1`'s viewer record no longer names it
