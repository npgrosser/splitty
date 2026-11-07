## ADDED Requirements

### Requirement: Native Screen Transfer Admission
The native client SHALL bind a Screen-transfer action to its captured connection, source Screen, terminal pane and session. It SHALL validate that source and the same-service destination before submitting `move_pane_to_screen` once. A menu for a non-selected tab SHALL target that tab. Stale, managed-job, conflicting or unsupported actions SHALL submit no transfer. Opening or dismissing the menu SHALL submit no operation.

#### Scenario: Transfer a non-selected tab
- **WHEN** a transfer is chosen for a non-selected terminal tab whose captured source still exists in the shown Screen
- **THEN** the submitted operation names that tab and its chosen destination, without first selecting a different source

#### Scenario: Stale or conflicting action
- **WHEN** the captured connection, Screen, pane or session changed, the pane is managed by a job, the destination is invalid, or an incompatible operation is pending
- **THEN** the client rejects the action without submitting a transfer or retargeting it to the focused tab

### Requirement: Native Cross-Screen Session Retention
The native client SHALL reconcile cross-Screen ownership changes without detaching an unchanged pane/session pair or replacing its retained terminal state merely because catalog and Screen observations arrive separately. This SHALL apply to local transfers and transfers initiated by other clients. Invalid or inconsistent observations SHALL NOT replace the last coherent workspace or authorize duplicate routes.

#### Scenario: Separate observations of a move
- **WHEN** a transfer's source, target and catalog changes arrive in any order, including removal of an emptied source or creation of a new target
- **THEN** the unchanged terminal retains its session, output, history and client state across the ownership change without a transient detach or duplicate attachment

#### Scenario: Unverified observations
- **WHEN** the client cannot verify a coherent bounded workspace after a transfer
- **THEN** it retains the last coherent routes, exposes an unverified outcome and continues terminal output without replaying the mutation
- **AND** later read-only refresh can recover the authoritative workspace

### Requirement: Native Screen Transfer Navigation
A locally initiated transfer SHALL follow and select the moved tab only after authoritative verification and only if later local navigation has not superseded that intent. Unrelated selections and valid maximized groups SHALL remain local choices. The client SHALL restore the target's full layout when needed to reveal a followed tab. An external transfer SHALL NOT create a local follow intent.

#### Scenario: Confirmed transfer reveals its target
- **WHEN** a locally initiated transfer is verified and no later local navigation superseded it
- **THEN** the target Screen and transferred tab are shown, restoring the target's full layout only if its maximized group would hide the tab

#### Scenario: Later navigation wins
- **WHEN** local navigation occurs before transfer verification, or another client initiated the move
- **THEN** the observation does not steal focus from the later valid local choice
- **AND** removal of the shown source uses normal Screen fallback when needed

### Requirement: Native Close During Transfer Reconciliation
While Screen ownership is unsettled, the native client SHALL reject pane and Screen close requests whose scope cannot be verified. Whole-window close SHALL remain available through the existing assessed confirmation and shutdown path, including unresolved reconciliation in its impact. Transfer admission SHALL reject an already pending close.

#### Scenario: Close while transfer is unsettled
- **WHEN** a transfer or cross-Screen observation is still being reconciled
- **THEN** pane and Screen close submit no destructive operation
- **AND** a confirmed whole-window close still joins shutdown of the owned service
