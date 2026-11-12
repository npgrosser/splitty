# Spec Delta

## ADDED Requirements

### Requirement: Program Status Capability
A server that reads program status (see `program-status`) SHALL list `program-status-v1` in `mux.welcome.capabilities`. A client SHALL show program status only for a connection whose `mux.welcome` lists it, and SHALL otherwise behave as before.

#### Scenario: Current server
- **WHEN** a client connects to a server of this version
- **THEN** `mux.welcome.capabilities` lists `program-status-v1`

#### Scenario: Older server
- **WHEN** a client connects to a server whose `mux.welcome` does not list `program-status-v1`
- **THEN** the client shows no program status for that connection and works as before

### Requirement: Program Status Events
The server SHALL send every connection `{"type": "program_status.changed", "sessionId", "paneId"?, "screenId"?, "status", "notify"?}`, where `status` is the session's root record or `null` and the pane and screen are the session's, if it has one (shapes in `docs/protocol.ts`): right after `mux.welcome` for each session that holds a root record, and when a root record changes, goes, or its pane moves to another screen. Per session, events SHALL be at least 100 ms apart and carry the latest record. A client SHALL ignore server messages whose type it does not know.

#### Scenario: Status on connect
- **WHEN** a session holds a `blocked` root record and a client connects
- **THEN** right after `mux.welcome` the client receives `program_status.changed` for that session with the state `blocked`, the session's `paneId` and `screenId`, and no `notify`

#### Scenario: Every connection receives the change
- **WHEN** two clients are connected, only one of them streams a session, and the session's root record changes
- **THEN** both clients receive `program_status.changed` for that session

#### Scenario: Paced changes
- **WHEN** a program writes 100 reports within one second
- **THEN** a client receives no two events for that session less than 100 ms apart, and the last carries the state of the last report

#### Scenario: Record cleared
- **WHEN** a session's program clears its root record
- **THEN** every client receives `program_status.changed` for that session with `status` `null`

#### Scenario: Session removed
- **WHEN** a session that holds a `done` root record is closed
- **THEN** every client receives `program_status.changed` for that session with `status` `null`

#### Scenario: Child records only
- **WHEN** a session holds no root record and its program reports a child record
- **THEN** no `program_status.changed` is sent for that session

#### Scenario: Pane moved to another screen
- **WHEN** a pane whose session holds a root record moves to another screen
- **THEN** every client receives `program_status.changed` for that session with the new `screenId` and no `notify`
