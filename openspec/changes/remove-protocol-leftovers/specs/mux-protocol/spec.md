## MODIFIED Requirements

### Requirement: Notification Notices Capability
A server that classifies its notification events SHALL list `notification-notices-v1` in `mux.welcome.capabilities`. For a connection without it, a window SHALL add for each `process.finished`, `process.idle` and `session.exited` event an `info` notification titled `Activity in <process>`, or `Session activity` when the event names no process, while `notifications.enabled` is on, and SHALL derive nothing else from the event; a `program_status.changed` event without `notice` SHALL add no notification. No server builds a generic `notification` event, and a window SHALL add nothing for one.

#### Scenario: Current server
- **WHEN** a client connects to a server of this version
- **THEN** `mux.welcome.capabilities` lists `notification-notices-v1`

#### Scenario: Older server
- **WHEN** a window is connected to a host whose `mux.welcome` does not list `notification-notices-v1` and the host sends `process.finished` for `cargo`, which the user is not watching
- **THEN** the window adds an `info` notification titled `Activity in cargo`, prefixed with the host's name

#### Scenario: Generic notification event
- **WHEN** a connection delivers an event of the type `notification`
- **THEN** the window adds no notification and no unread mark for it

### Requirement: Template Extraction Over Session Connections
A window SHALL request extraction from its screen’s own connection using `mux.extract_template`, for a Screen of its local service as for a host's, and receive `mux.template_extracted` with its request ID and either a template or an error. This SHALL use the same semantics as authenticated HTTP/native extraction and SHALL never fall back to a different server.

#### Scenario: Extract a remote screen
- **WHEN** a window extracts a screen on a host connection
- **THEN** the request and response stay on that connection and a disconnect fails the pending extraction

#### Scenario: Extract a local screen
- **WHEN** the native window extracts a Screen of its local service
- **THEN** it sends `mux.extract_template` on that service's connection and no `template.extract` operation
