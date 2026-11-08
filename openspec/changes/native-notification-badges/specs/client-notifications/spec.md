# Spec Delta

## Purpose

Defines how a client window turns existing service notification facts into bounded, source-scoped unread activity without changing workspace state or server-owned notification delivery.

## ADDED Requirements

### Requirement: Native Notification Admission
An experimental native workspace SHALL admit `process.finished`, `process.idle`, `session.exited` and `notification` events for unread activity only under the effective local notification settings. The `notifications.enabled` switch SHALL gate all four types; `notifications.processFinished`, `notifications.processExited` and `notifications.processCrashed` SHALL gate their corresponding classified events. A disabled category SHALL have no unread indicator.

#### Scenario: Enabled process event
- **WHEN** an enabled `process.finished` event arrives for a pane the window is not watching
- **THEN** that pane gains one unread item and its Screen's unread count increases by one

#### Scenario: Disabled and refreshed settings
- **WHEN** an authoritative settings refresh disables notifications or a process category after unread items of that category exist
- **THEN** those items cease to contribute to unread indicators and are not restored by re-enabling the setting
- **AND** a failed or superseded settings read neither replaces the last accepted switches nor makes a settings write

#### Scenario: Settings have not settled
- **WHEN** notification events arrive before the first valid settings read
- **THEN** they create no unread entries, and a later valid read does not retroactively admit them

#### Scenario: Exit classification
- **WHEN** `session.exited` reports zero and nonzero exit codes, including `userInitiated: true`
- **THEN** zero is classified as an exit and nonzero as a crash for the respective switches and severity
- **AND** user initiation alone does not suppress an otherwise admitted unread item

### Requirement: Source-Scoped, Bounded Unread State
Each notification SHALL belong only to its sending source. A client SHALL admit a fact only from that source's current connection generation, without requiring command-send readiness. It SHALL resolve a session only within that source, sanitize remote text, name the remote host in its title, deduplicate retained event IDs per source, and keep at most 100 bounded entries per window. It SHALL retain no received transport event or charged payload after admission.

#### Scenario: Colliding server identifiers
- **WHEN** two sources send the same event ID and session ID for panes they each own
- **THEN** each source's unread state remains distinct and neither event changes the other's pane or Screen

#### Scenario: Stale connection generation
- **WHEN** an event from a replaced connection generation arrives after the new generation is active
- **THEN** it is ignored, even if its event ID and session ID match current objects

#### Scenario: Retained fact after readiness is revoked
- **WHEN** an event admitted by the current source connection is delivered to the window after that source stops accepting commands, while its exact session binding is still retained
- **THEN** the event may contribute unread activity for that retained pane and no command is sent

#### Scenario: Unknown session or unscoped event
- **WHEN** a valid event has no session ID or its session does not resolve within its sending source at admission
- **THEN** it cannot create a pane or Screen unread indicator, including if another source has that session ID or the sending source later gains a matching route

#### Scenario: Duplicate and oversized events
- **WHEN** an event ID is replayed while its prior entry is retained, or an event contains oversized or malformed display fields
- **THEN** a replay contributes no second unread item and retained text and identifiers remain bounded plain text

### Requirement: Window-Local Read Lifecycle
Unread state SHALL belong to one client window and SHALL never submit a workspace operation or settings mutation. A notification for the pane being watched in a focused, visible window SHALL arrive read; activating a pane or returning focus and visibility to its active pane SHALL mark that pane's retained items read. Read status for retained IDs SHALL survive a reconnect to the same source; removal of the source or replacement of the window's local service SHALL clear its inapplicable state.

#### Scenario: Watched pane and focus return
- **WHEN** an event arrives for the active pane while its window is visible and focused, or the window later regains focus on a pane with unread items
- **THEN** that pane has no unread indicator and no server command is sent to acknowledge it

#### Scenario: Pane activation and reconnect
- **WHEN** a user activates a pane with unread items, its source reconnects, and fresh canonical mapping under the current source token proves the same source, pane and captured session
- **THEN** the pane remains read without requiring those events to be delivered again

#### Scenario: Reconnect before route proof
- **WHEN** a source changes connection generation and the new catalog or snapshots are empty, incomplete or reconciling
- **THEN** its old pane and Screen indicators are hidden while up to 100 historical records and their read flags remain; missing routes in this interval do not prune them

#### Scenario: Fresh same-session proof after reconnect
- **WHEN** the current source token's fresh canonical mapping proves a retained record's exact source, pane and captured session after reconnection, even if the pane now belongs to another Screen
- **THEN** its unread indicator returns on the current Screen with its previous read status and no notification replay is required

#### Scenario: Source removal or local service replacement
- **WHEN** a source is removed from the window or the window changes its local service generation
- **THEN** unread state belonging to removed authority is cleared and cannot appear under a later source

#### Scenario: Pane moves or disappears
- **WHEN** an unread pane moves to another Screen of the same source, or a settled current model conclusively removes that pane
- **THEN** the unread Screen summary follows the moved pane while its session binding is unchanged, or the removed pane's unreachable records are pruned

#### Scenario: Pane ID reused for another session
- **WHEN** a settled current model replaces a pane's session while retaining its pane ID, including after reconnection
- **THEN** unread records captured for the old session are pruned and do not appear on the replacement pane or its Screen

### Requirement: Unread Projection
A native window SHALL expose the unread count for each Screen and the highest severity of unread items for each pane from the same source. Error SHALL outrank warning, then success, then info. A Screen's unread titles SHALL be available newest first for an accessible summary; reading a pane SHALL update both pane and Screen summaries together.

#### Scenario: Multiple unread events
- **WHEN** a pane has unread info and error events and another pane on its Screen has one unread event
- **THEN** the first pane indicates error and the Screen count is three, with titles in newest-first order

#### Scenario: Read one pane
- **WHEN** one of two panes on a Screen is read
- **THEN** only that pane's unread severity clears and the Screen count reflects the other pane's retained unread items
