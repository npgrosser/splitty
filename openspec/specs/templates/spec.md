# templates Specification

## Purpose

Templates capture reusable terminal layouts and launch data, and apply them through the authoritative workspace on the target server.

## Requirements

### Requirement: Authoritative Template Application
Applying a template SHALL replace an existing screen or create a new screen with its terminal tabs, splits, ratios and floating bounds. The target server SHALL own every resulting pane and session. The window SHALL project the layout immediately and attach only to server-confirmed sessions.

#### Scenario: Replace or create
- **WHEN** a window applies a valid template to an existing or new screen
- **THEN** one `apply_template` op registers its client-generated pane IDs, ends replaced sessions and launches every new terminal once
- **AND** another client and a subsequent reload see the same screen

### Requirement: Template Launch Data
Template launches SHALL preserve commands, restore modes and shell profile IDs and resolve relative directories against the supplied root on the target server. Shell profile selection and unavailable-profile fallback SHALL follow the target server’s settings contract. A failed individual launch SHALL be reported without inventing a successful session.

#### Scenario: Launch metadata and failure
- **WHEN** a template contains a command, restore mode, profile and relative directory
- **THEN** its session receives those values with the resolved directory
- **AND** a spawn failure identifies its pane and successful sibling launches remain attached

### Requirement: Template Validation
Application SHALL reject duplicate pane references, missing definitions, unused terminal definitions, reused pane IDs, invalid ratios and floating split layouts without changing the workspace. Legacy web panes SHALL be omitted with a visible notice and SHALL never load a URL; application with no remaining tiled terminal SHALL fail.

#### Scenario: Invalid or legacy content
- **WHEN** an invalid template or an all-web template is applied
- **THEN** the old screen and sessions remain intact
- **AND** mixed legacy templates omit web panes and retain their terminal layout

### Requirement: Extracting Templates
Extraction SHALL use the server's screen layout and floating bounds, with terminal commands, current directories, profiles and restore modes. Live non-shell foreground commands SHALL take precedence over stored launch commands; a live base shell SHALL extract without a command. Viewer panes SHALL be omitted and split ratios renormalized. Empty or viewer-only screens SHALL report an error.

#### Scenario: Extract and reapply
- **WHEN** a window extracts a screen with terminal tabs, floating terminals and viewers, optionally relative to a root
- **THEN** its template preserves the terminal layout and launch metadata, omits viewers, and relativizes only directories within that root
- **AND** a screen with no tiled terminals reports an error

### Requirement: Template Command Consent
The window SHALL show and request confirmation for every template command before applying it. Cancellation SHALL send no apply operation. Layout-only application SHALL strip commands and their restore modes before sending the op.

#### Scenario: Consent choices
- **WHEN** a template contains commands
- **THEN** approving runs only the displayed commands, layout-only runs none, and cancelling changes no screen
