# Spec Delta

## ADDED Requirements

### Requirement: Native Captured Remote Screen Rename
The native workspace SHALL permit deliberate renaming of an opened, canonically observed remote Screen through its current owning connection. The editor SHALL retain its original window, source and Screen identity independently of selection. An obsolete, disconnected, missing or unconfirmed target SHALL send no rename. Renaming SHALL neither connect a host nor create, detach or terminate work.

#### Scenario: Rename the captured remote Screen
- **WHEN** a user commits a valid name for one opened remote Screen while another source is selected, including sources with colliding raw Screen IDs
- **THEN** only the captured Screen on its owning host receives `rename_screen`
- **AND** local and other-host names, selection, layout and sessions remain unchanged by the rename

#### Scenario: Editor survives unrelated workspace updates
- **WHEN** unrelated selection, terminal output or workspace observations arrive while the editor remains open
- **THEN** the edit and its input focus remain associated with the original Screen
- **AND** committing cannot rename the newly selected Screen instead

#### Scenario: Obsolete or unavailable editor target
- **WHEN** the captured window/source connection is replaced, its Screen is removed, missing or unconfirmed, an applicable close prevents mutation, or the edit is cancelled
- **THEN** it sends no rename from that obsolete or unavailable edit and does not switch to another target or connect a host
- **AND** no rename is queued for replay after reconnect

### Requirement: Native Remote Rename Observation
An accepted native rename SHALL use the existing name validation and ordered canonical observation. Queue admission SHALL NOT be reported as a confirmed name change. Rejected or uncertain work SHALL NOT replay, and later observations SHALL determine the effective name without claiming a lost acknowledgement was received.

#### Scenario: Canonical name and bounded refusal
- **WHEN** a rename is accepted, rejected for an invalid name or refused by full admission
- **THEN** only a current canonical observation changes the displayed name, refusal leaves the prior name intact, and no duplicate operation is submitted
- **AND** an invalid name, old connection or full queue cannot mutate another source or silently substitute a different name
