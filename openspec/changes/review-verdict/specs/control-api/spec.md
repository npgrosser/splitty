## MODIFIED Requirements

### Requirement: Review Requests
`POST /api/viewer/review/wait` SHALL take `{"screenId", "paneId", "viewerPaneId", "viewerRevision"?}`, the caller's screen and pane, the viewer pane whose review it waits for and optionally the `viewerRevision` the caller's `open_viewer` was answered with, start a wait for a review of that pane (see `viewer-annotations`, Waiting For A Review), and hold the request until the wait ends. It SHALL respond 200 with `{"outcome": "approved"}` when the user approves without notes, `{"outcome": "approved", "review": string}` when the user approves with notes, or `{"outcome": "changes_requested", "review": string}` when the user requests changes; 410 with `code` `closed` when the viewer pane closes first; 404 `not_found` for a screen, caller pane or viewer pane the screen does not hold, or a pane that is not a viewer pane; 409 `busy` while another wait for the same viewer pane exists; 409 `changed`, without a wait, when the viewer record carries another revision than `viewerRevision` as the wait would start; and 429 `busy` while 16 waits exist on the server. When the caller's connection ends before an answer, the server SHALL end the wait without an outcome. Grant sockets SHALL NOT expose this endpoint.

#### Scenario: Approved
- **WHEN** a pane process waits for viewer pane `v1` and the user approves in a window without notes
- **THEN** the request responds 200 with `{"outcome": "approved"}`

#### Scenario: Changes requested
- **WHEN** a pane process waits for `v1` and the user requests changes
- **THEN** the request responds 200 with `{"outcome": "changes_requested", "review": …}` holding the window's review

#### Scenario: Second wait
- **WHEN** a second request for `v1` arrives while the first waits
- **THEN** it responds 409 with `code` `busy`, and the first keeps waiting

#### Scenario: Replaced before the wait
- **WHEN** a process shows a text in `v1`, answered with `viewerRevision` 3, another process's text replaces it before the first waits, and the first then waits with `viewerRevision` 3
- **THEN** the request responds 409 with `code` `changed`, and `v1`'s viewer record names no wait

#### Scenario: Caller gone
- **WHEN** the waiting process is killed
- **THEN** the wait ends, and `v1`'s viewer record no longer names it
