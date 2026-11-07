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
