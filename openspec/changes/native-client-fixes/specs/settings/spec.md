## MODIFIED Requirements

### Requirement: Server Settings and Metadata
The server SHALL return its current settings at `GET /api/home/settings` and defaults and field limits at `GET /api/home/settings/metadata`, including numeric bounds and steps and allowed values. Defaults MUST reflect that server's platform and profile detection. A window SHALL derive controls, reset values and the validation of an edit from this metadata when available, so that a limit the server widens or a choice it adds is offered and taken as it is; only without metadata SHALL it fall back to its own built-in limits.

#### Scenario: Server-specific defaults and limits
- **WHEN** a window loads settings from a server
- **THEN** it uses that server's defaults and limits for the settings controls and reset actions

#### Scenario: Older server without metadata
- **WHEN** the metadata endpoint is unavailable on an older server
- **THEN** the window retains the settings returned by that server and continues to support compatible updates within its built-in limits, without requiring metadata

#### Scenario: A server widens a limit
- **WHEN** a server's metadata allows font sizes up to 32 and lists an editor the window's built-in limits do not know
- **THEN** the window offers and sends a font size of 30 and that editor, and refuses 33
