# Spec Delta

## ADDED Requirements

### Requirement: Template Operation Capability
`mux.welcome` SHALL advertise `server-templates-v1` when template application and extraction are supported. A window SHALL disable these actions without the capability and offer an update for an older host, without direct session spawning as a fallback.

#### Scenario: Older host
- **WHEN** a host welcome lacks `server-templates-v1`
- **THEN** template application and extraction are unavailable and the host update action is offered

### Requirement: Template Operation Results
`apply_template` SHALL use the payload in docs/protocol.ts on every session transport. Each successful launch SHALL produce `mux.pane_spawned` before other windows are nudged. Rejection SHALL produce `mux.op_failed` for the screen, and an individual launch failure SHALL also name its pane. The requesting window SHALL resynchronize after failure.

#### Scenario: Result ordering and rejection
- **WHEN** a template op succeeds or fails
- **THEN** successful spawn results precede the screen nudge and rejection triggers resynchronization without a client-created session

### Requirement: Template Extraction Over Session Connections
A window SHALL request extraction from its screen’s own connection using `mux.extract_template` and receive `mux.template_extracted` with its request ID and either a template or an error. This SHALL use the same semantics as authenticated HTTP/native extraction and SHALL never fall back to a different server.

#### Scenario: Extract a remote screen
- **WHEN** a window extracts a screen on a host connection
- **THEN** the request and response stay on that connection and a disconnect fails the pending extraction

## MODIFIED Requirements

### Requirement: Rejected Op Reporting
When the server rejects a recognized op that names a pane to create (`newPaneId`) because it fails validation (see `workspace-model`), it SHALL send `mux.op_failed` to the originating client only, carrying the op `type`, the `screenId`, that `paneId`, and an error message. When it rejects a recognized `apply_template`, `arrange_panes`, `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies` or `mark_sent` op because it fails validation, it SHALL send `mux.op_failed` to the originating client only, carrying the op `type`, the `screenId` and an error message, and for the annotation ops the viewer pane's `paneId`.

#### Scenario: Rejected pane-creating op
- **WHEN** a client sends a `mux.op` with `newPaneId` `p9` that the server rejects
- **THEN** the client SHALL receive `mux.op_failed` with `paneId` `p9` and a non-empty `error`
- **AND** no other client SHALL receive it

#### Scenario: Rejected arrangement
- **WHEN** a client sends `arrange_panes` on screen `s1` that misses one of its panes
- **THEN** the client SHALL receive `mux.op_failed` with `op` `arrange_panes`, `screenId` `s1` and a non-empty `error`
- **AND** no other client SHALL receive it

#### Scenario: Rejected annotation
- **WHEN** a client sends `put_annotations` for viewer pane `v1` on screen `s1` that would give the pane a 101st annotation
- **THEN** the client SHALL receive `mux.op_failed` with `op` `put_annotations`, `screenId` `s1`, `paneId` `v1` and a non-empty `error`
- **AND** no other client SHALL receive it

#### Scenario: Rejected reply
- **WHEN** a client sends `put_replies` for viewer pane `v1` on screen `s1` with the ID of an agent's reply
- **THEN** the client SHALL receive `mux.op_failed` with `op` `put_replies`, `screenId` `s1`, `paneId` `v1` and a non-empty `error`
- **AND** no other client SHALL receive it

