# Spec Delta

## Purpose

Settings belong to each server and govern its terminal, workspace, appearance, shell, editor, and notification behavior for all connected windows.

## ADDED Requirements

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
