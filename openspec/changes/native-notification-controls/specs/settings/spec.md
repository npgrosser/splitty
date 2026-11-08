# Spec Delta

## MODIFIED Requirements

### Requirement: Experimental Native Settings Editor
The experimental native workspace SHALL offer an editor for the local service's supported Appearance, Terminal, Workspace and Notification preferences. It SHALL read effective values through `home.get`, defaults and limits through `home.metadata`, and theme choices through `theme.list`. It SHALL submit only explicit supported changes through `home.update`; selecting a remote Screen SHALL NOT change the settings source.

#### Scenario: Local source while viewing remote content
- **WHEN** a remote Screen is selected and the native Settings editor opens
- **THEN** it shows the assigned local service's effective values and changes only that service's settings

#### Scenario: Supported controls and reset
- **WHEN** a user changes or resets UI theme mode or choice, UI font size or family, display zoom or scope, terminal palette or close-on-exit policy, Drawer visibility or width, or notification enablement or process categories
- **THEN** the native client submits a bounded patch for that setting and displays the confirmed effective value
- **AND** reset uses the local service's published default when available

#### Scenario: Unavailable metadata or theme catalog
- **WHEN** settings metadata or the theme catalog cannot be read
- **THEN** already loaded effective settings remain visible and terminal use continues
- **AND** controls that cannot establish valid choices or reset values do not submit guessed values

#### Scenario: Notification switches preserve other settings
- **WHEN** a user changes `notifications.enabled`, `notifications.processFinished`, `notifications.processExited` or `notifications.processCrashed`
- **THEN** the native client submits only that explicit boolean field through `home.update` on its assigned local service
- **AND** unrelated preferences, including toast and desktop switches, remain unchanged


### Requirement: Native Settings Mutation Reconciliation
Native settings mutations SHALL be identified with their local connection and request, admitted within a bounded queue, submitted at most once, and reconciled with the service's complete effective settings and `settings.changed`. A rejected, uncertain, or stale result SHALL NOT be shown as saved. The editor SHALL retain a visible error or uncertain state and refresh from the local service without replaying the write.

#### Scenario: Confirmed edit or same-service event
- **WHEN** this window's settings change is confirmed or `settings.changed` arrives on its assigned local service connection
- **THEN** the native editor and current workspace display the latest effective settings
- **AND** terminal output continues on its separate stream

#### Scenario: Rejection or uncertain completion
- **WHEN** `home.update` rejects a change or its outcome is uncertain
- **THEN** the editor retains the last confirmed value, identifies the field and failure, and requests a fresh local settings read
- **AND** the mutation is not repeated automatically

#### Scenario: Old result and close
- **WHEN** a result belongs to an older local connection or an older settings generation, or the window closes during an update
- **THEN** the result cannot replace current presentation
- **AND** owned pending work is cancelled and joined without changing another service

#### Scenario: Independent settings completion
- **WHEN** notification and appearance edits overlap with same-service settings events or refresh failures
- **THEN** each edit settles only from its matching request and a sufficiently fresh authoritative read for that setting
- **AND** an appearance-only read, OS preference change or failed theme read cannot confirm, reject, supersede or unlock a notification edit


## ADDED Requirements

### Requirement: Independent Native Notification Preferences
Native notification controls SHALL use the assigned local service's effective switches independently of appearance and theme resolution. A valid boolean read SHALL establish compatible edit choices; reset SHALL require that service's published default. Failed reads SHALL preserve last-good switches without guessing values or writing settings.

#### Scenario: Appearance unavailable
- **WHEN** the local service returns valid notification switches while a theme or appearance read fails
- **THEN** notification admission and compatible explicit edits remain usable
- **AND** a later accepted notification read updates effective controls independently

#### Scenario: Metadata unavailable
- **WHEN** notification switches are valid but `home.metadata` is unavailable or has no valid notification defaults
- **THEN** boolean switches remain editable and reset actions requiring those defaults are unavailable
- **AND** the window does not manufacture or persist default values

#### Scenario: Reset a notification field
- **WHEN** a user resets a supported notification field and its server default is available
- **THEN** only that field's exact published boolean default is submitted
- **AND** neither client defaults nor another service's metadata are used

#### Scenario: Owner replacement or failed refresh
- **WHEN** the local service owner changes or a notification read fails after a prior valid read
- **THEN** replacement discards the previous owner's editor authority and failed same-owner reads retain the last valid switches with visible failure
- **AND** stale results cannot settle edits for the new owner or automatically replay a write
