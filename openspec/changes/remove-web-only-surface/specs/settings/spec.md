## MODIFIED Requirements

### Requirement: Removed Terminal Fields
`linkTarget` and `webviewMode` SHALL NOT be validated or returned as current terminal settings. Existing stored values SHALL be ignored when loading settings. An update naming them SHALL be refused like any unknown setting.

#### Scenario: Legacy stored fields
- **WHEN** a stored configuration contains `link_target` or `webview_mode`
- **THEN** the effective settings omit them and all other valid fields remain usable

#### Scenario: Legacy field in an update
- **WHEN** a client sends `PATCH /api/home/settings` with `{"terminal":{"linkTarget":"tab"}}`
- **THEN** it receives status 400 with the field `terminal.linkTarget` and no setting changes

## ADDED Requirements

### Requirement: Settings Update Validation
`PATCH /api/home/settings` SHALL validate all submitted fields, including `editors`, before changing persisted or running settings. Invalid input MUST receive status 400 with `error` and a dotted `field` path. A valid update SHALL return the complete effective settings. Fractional values SHALL be accepted where the metadata publishes them, such as the listed zoom levels.

#### Scenario: Invalid editor rule
- **WHEN** a client submits an invalid `editors.extensions` or `editors.projects` rule
- **THEN** it receives status 400 with the failing field and no settings change is persisted or applied

#### Scenario: Invalid numeric or enum field
- **WHEN** a client submits an out-of-range number or unsupported allowed value
- **THEN** it receives status 400 with the failing field and the prior settings remain effective

#### Scenario: Fractional setting
- **WHEN** a client submits `appearance.zoom` 1.25, a level the metadata lists
- **THEN** the server persists and returns that fractional value

### Requirement: Removed Setting Groups And Fields
The settings SHALL NOT contain, validate, describe in their metadata or return `terminal.renderer`, `terminal.ligatures`, `terminal.windowPaddingBalance`, the group `terminal.xterm`, the groups `tabs`, `window` and `experimental`, or `drawer.resizable`. A stored `config.toml` holding them SHALL load without error, with every other valid value kept; the removed keys SHALL be ignored and SHALL be absent from the file the server writes next. An update naming one of them SHALL be refused with status 400 and that field as an unknown setting.

#### Scenario: Older configuration file
- **WHEN** `config.toml` holds `[terminal] renderer`, `[terminal.xterm]`, `[tabs]`, `[window]`, `[experimental]` and `[drawer] resizable` next to a valid `[appearance] font_size = 15`
- **THEN** the server starts with font size 15, the effective settings contain none of the removed keys, and the file the server writes next holds none of them

#### Scenario: Update naming a removed setting
- **WHEN** a client sends `PATCH /api/home/settings` with `{"terminal":{"xterm":{"cursorBlink":true}}}`
- **THEN** it receives status 400 with the field `terminal.xterm` and no setting changes

## REMOVED Requirements

### Requirement: Atomic Settings Validation
**Reason**: Its fractional example, `terminal.xterm.scrollSensitivity`, goes with the `terminal.xterm` group. The rules move to Settings Update Validation.
**Migration**: None.
