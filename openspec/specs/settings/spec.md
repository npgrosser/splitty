# settings Specification

## Purpose

Settings belong to each server and govern its terminal, workspace, appearance, shell, editor, and notification behavior for all connected windows.

## Requirements

### Requirement: Server Settings and Metadata
The server SHALL return its current settings at `GET /api/home/settings` and defaults and field limits at `GET /api/home/settings/metadata`, including numeric bounds and steps and allowed values. Defaults MUST reflect that server's platform and profile detection. A window SHALL derive controls and reset values from this metadata when available.

#### Scenario: Server-specific defaults and limits
- **WHEN** a window loads settings from a server
- **THEN** it uses that server's defaults and limits for the settings controls and reset actions

#### Scenario: Older server without metadata
- **WHEN** the metadata endpoint is unavailable on an older server
- **THEN** the window retains the settings returned by that server and continues to support compatible updates without requiring metadata

### Requirement: Atomic Settings Validation
`PATCH /api/home/settings` SHALL validate all submitted fields, including `editors`, before changing persisted or running settings. Invalid input MUST receive status 400 with `error` and a dotted `field` path. A valid update SHALL return the complete effective settings. Fractional `terminal.xterm.scrollSensitivity` and `fastScrollSensitivity` SHALL be accepted within published bounds.

#### Scenario: Invalid editor rule
- **WHEN** a client submits an invalid `editors.extensions` or `editors.projects` rule
- **THEN** it receives status 400 with the failing field and no settings change is persisted or applied

#### Scenario: Invalid numeric or enum field
- **WHEN** a client submits an out-of-range number or unsupported allowed value
- **THEN** it receives status 400 with the failing field and the prior settings remain effective

#### Scenario: Fractional scroll setting
- **WHEN** a client submits a fractional scroll sensitivity on a published step
- **THEN** the server persists and returns that fractional value

### Requirement: Settings Change Propagation
After a successful settings change, the server SHALL send `settings.changed` with the complete effective settings to every connected window, including the writer, over browser and native connections. A window SHALL apply that event, and SHALL restore its prior value after a failed update.

#### Scenario: Other window updates settings
- **WHEN** one window changes a setting
- **THEN** every connected window receives `settings.changed` with the saved settings

#### Scenario: Rejected optimistic update
- **WHEN** a window's settings update is rejected or its request fails
- **THEN** it restores its previous settings and exposes the failure

### Requirement: Removed Terminal Fields
`linkTarget` and `webviewMode` SHALL NOT be validated or returned as current terminal settings. Existing stored values SHALL be ignored when loading settings.

#### Scenario: Legacy stored fields
- **WHEN** a stored configuration contains `link_target` or `webview_mode`
- **THEN** the effective settings omit them and all other valid fields remain usable

### Requirement: Experimental Native Local Appearance
The experimental native workspace SHALL read local appearance through `home.get` and `theme.get`, applying appearance font overrides to workspace chrome and resolved colors to chrome and terminals without writing settings. Unset chrome font overrides SHALL use resolved UI theme defaults. Automatic mode SHALL select the saved light or dark theme using the local OS preference. Remote settings SHALL NOT select application appearance.

#### Scenario: Startup appearance
- **WHEN** the local appearance read succeeds
- **THEN** workspace chrome uses the supported appearance font overrides or resolved UI theme defaults
- **AND** existing and newly created terminals use the resolved terminal colors without recreating sessions or losing terminal history
- **AND** loading appearance does not submit a settings mutation

#### Scenario: Chrome font changes preserve terminal typography
- **WHEN** appearance font size or family changes, including selection of a proportional UI font
- **THEN** workspace chrome uses the supported font choice while terminal typography retains its native defaults
- **AND** the change does not reinterpret appearance font fields as terminal font settings or activate `terminal.theme`

#### Scenario: Automatic local theme
- **WHEN** automatic theme mode is configured and the local OS color preference changes
- **THEN** the matching configured light or dark theme is requested from the local service
- **AND** a remote service's settings cannot override that choice

### Requirement: Appearance Refresh and Failure Isolation
Native appearance refresh SHALL coalesce same-service `settings.changed` events, discard superseded results, preserve terminal event delivery and finish owned API work during shutdown. Read failures SHALL retain working defaults or the last valid appearance and expose a status while terminals remain usable. Unsupported presentation values SHALL use bounded native fallbacks.

#### Scenario: New settings during a refresh
- **WHEN** local settings change while an earlier appearance read is in flight
- **THEN** the superseded result is not applied and the latest refresh is eventually used
- **AND** terminal output continues through its existing stream

#### Scenario: Failed or unsupported appearance
- **WHEN** an appearance read fails or a requested font or color form is unsupported
- **THEN** terminals remain usable with the last valid appearance or native defaults and the failure is visible
- **AND** no corrective settings write is made

#### Scenario: Close during a read
- **WHEN** the owned workspace closes during an appearance request
- **THEN** outstanding appearance work is cancelled and its stream cleanup is awaited without leaking delivery allocations or altering another service

### Requirement: Experimental Native Local Shell Choice
The experimental native workspace SHALL offer the local service's enabled shell profiles from `shellProfiles.get` for explicit new-tab creation, disabling unavailable choices. Selection SHALL submit only `shellProfileId`; shell commands, arguments, working-directory inheritance and fallback remain service-owned. Ordinary tab and split actions SHALL retain service inheritance without explicit launch overrides.

#### Scenario: Explicit profile with inherited directory
- **WHEN** a user chooses an available profile for a new tab
- **THEN** `new_tab` carries that profile identifier and no explicit working directory
- **AND** the service uses its current known source directory and the selected profile's configured program and arguments

#### Scenario: Ordinary launch with stale client metadata
- **WHEN** the client has an older source directory or profile than the service and creates an ordinary tab or split
- **THEN** the service's current known launch defaults determine the new session

#### Scenario: Profile becomes unavailable before launch
- **WHEN** a selected profile becomes missing, disabled or unavailable before the service resolves the launch and the service uses its fallback
- **THEN** the native workspace visibly reports the effective profile and `shellProfileFallbackReason` for the admitted new pane
- **AND** the client neither reconstructs a command nor retries the launch

### Requirement: Native Profile Read Ownership and Isolation
Native profile reads SHALL belong to the local workspace connection, retain the latest valid result and refresh on same-service `settings.changed`. Superseded or foreign results MUST NOT replace current choices. Appearance and profile failures SHALL remain independent and visible while terminals remain usable. Refreshes SHALL coalesce without starving either read, and owned pending reads SHALL finish cleanup on shutdown without settings writes.

#### Scenario: Concurrent refresh and local preference change
- **WHEN** same-service settings change while a profile read is pending and the OS color preference changes repeatedly
- **THEN** superseded profiles are discarded and both requested settings reads eventually progress
- **AND** color preference alone does not reload profiles or consume terminal output

#### Scenario: Independent failure and recovery
- **WHEN** a profile read fails while appearance remains readable, or appearance fails while profiles remain readable
- **THEN** the successful feature updates independently, the failed feature retains its last valid state with a visible failure, and ordinary terminal actions remain usable
- **AND** a later same-service refresh can recover without an automatic retry loop or settings mutation

#### Scenario: Foreign result and shutdown
- **WHEN** a result belongs to another workspace connection, or the owned workspace closes during a read
- **THEN** foreign data is not applied and shutdown cancels pending reads and awaits owned stream cleanup without retaining delivery allocations

### Requirement: Native Local Drawer Preferences
The native local workspace SHALL restore Drawer visibility and supported width from the assigned local service's `drawer.pinned` and `drawer.sidebarWidth` settings through `home.get`. An explicit visibility change SHALL update only `drawer.pinned` through `home.update`. Loading preferences SHALL NOT write settings, and foreign or superseded reads SHALL NOT replace current presentation.

#### Scenario: Restore saved preferences
- **WHEN** the native workspace starts or receives a same-service settings change
- **THEN** the latest valid local Drawer visibility and bounded width are applied
- **AND** loading or applying those values performs no settings mutation

#### Scenario: Explicit visibility update
- **WHEN** the user changes Drawer visibility and the local service confirms the update
- **THEN** the effective value is retained by the service and can be restored by a later workspace
- **AND** unrelated settings remain unchanged

### Requirement: Native Drawer Mutation Outcomes
Native Drawer updates SHALL retain connection and request identity, admit at most one pending visibility update and never automatically repeat a mutation. Explicit rejection SHALL retain confirmed presentation and expose failure. An unavailable, uncertain or invalid mutation reply SHALL expose uncertainty and refresh current settings without claiming a persisted outcome. Shutdown SHALL join owned API cleanup.

#### Scenario: Rejection and uncertainty
- **WHEN** a Drawer update is rejected or its outcome cannot be confirmed
- **THEN** the window keeps its last confirmed presentation, reports rejection or uncertainty as appropriate, and resynchronizes settings
- **AND** it does not replay the write or apply a result from another connection or request

#### Scenario: Repeated toggle and close
- **WHEN** another toggle arrives while a Drawer update is pending or the window closes during that update
- **THEN** no overlapping or repeated mutation is submitted
- **AND** shutdown retains ownership until pending API cleanup is joined
