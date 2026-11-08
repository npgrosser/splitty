# Spec Delta

## ADDED Requirements

### Requirement: Relative Screen Placement Capability
A supporting service SHALL announce `screen-placement-v1` in `mux.welcome.capabilities`. A native client SHALL require that capability and `workspace-catalog-v1` before sending `move_screen`. The operation SHALL use the existing ordered session protocol, rejection reporting and workspace-change propagation. Existing `reorder_screens` SHALL remain compatible.

#### Scenario: Supported relative placement
- **WHEN** a client submits a valid `move_screen` to a supporting service followed by `mux.workspace_resync`
- **THEN** the response reflects the operation against current state, the sender receives no success acknowledgement, and terminal delivery continues
- **AND** a changed order produces one workspace `mux_op` nudge without a Screen identifier to other clients

#### Scenario: Unsupported local service
- **WHEN** either required capability is absent from the assigned local service's welcome
- **THEN** native local Screen placement is unavailable and sends neither `move_screen` nor a full-list replacement fallback
- **AND** this does not disable window-local arrangement of remote references

#### Scenario: Parsed placement rejection
- **WHEN** `move_screen` carries an invalid moved Screen or insertion anchor
- **THEN** only the sender receives `mux.op_failed` naming the operation and moved Screen, and it requests authoritative catalog state without retrying the move
- **AND** other clients receive no change nudge

#### Scenario: Invalid anchor representation
- **WHEN** a recognized `move_screen` request supplies a non-string non-null insertion anchor
- **THEN** it changes no workspace state and is not treated as an omitted anchor or end placement

#### Scenario: Legacy full-list reorder
- **WHEN** an existing client sends `reorder_screens` with a partial list containing an unknown ID
- **THEN** the existing replacement semantics apply, filtering the unknown ID without appending omitted Screens
