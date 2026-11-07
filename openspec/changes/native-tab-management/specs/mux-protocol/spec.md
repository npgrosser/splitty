# Spec Delta

## ADDED Requirements

### Requirement: Exact Tab Placement Capability
A server supporting `move_tab` SHALL announce `tab-placement-v1` in `mux.welcome.capabilities`. A client SHALL NOT send exact tab placement without that capability. Existing `move_pane` semantics SHALL remain unchanged. Accepted placement SHALL use ordinary layout-change propagation and rejected placement SHALL use `mux.op_failed`, followed by client resynchronization.

#### Scenario: Supported exact placement
- **WHEN** a client connects to a supporting service and places a tab
- **THEN** the welcome announces `tab-placement-v1` and the operation is processed in stream order with ordinary layout propagation

#### Scenario: Older service
- **WHEN** the welcome lacks `tab-placement-v1`
- **THEN** exact placement is unavailable and the client sends neither `move_tab` nor a lossy fallback operation for that intent

#### Scenario: Rejected exact placement
- **WHEN** a parsed `move_tab` names an invalid source, destination or anchor
- **THEN** only its sender receives `mux.op_failed` naming `move_tab` and its Screen, and the client requests authoritative state
- **AND** other clients receive no change nudge

#### Scenario: Legacy move remains compatible
- **WHEN** an existing client sends `move_pane` with `targetSlotId`
- **THEN** the moved pane is appended and activated, including a same-slot move
