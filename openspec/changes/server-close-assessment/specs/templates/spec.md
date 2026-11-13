# Spec Delta

## ADDED Requirements

### Requirement: Confirmed Screen Replacement
`apply_template` onto an existing screen that holds panes or sessions SHALL carry `replaceConfirmed: true`; without it the server SHALL reject the op with `mux.op_failed` and change nothing. A window SHALL set it only after the user agreed to the replacement: before replacing, it SHALL assess the screen (see `mux-protocol`, Close Assessment) and ask when the assessment asks, naming what ends; without the capability, or when the assessment fails, it SHALL ask a generic confirmation. Creating a screen needs no flag.

#### Scenario: Replacement without the flag
- **WHEN** a window sends `apply_template` for an existing screen with a running terminal and without `replaceConfirmed`
- **THEN** the window receives `mux.op_failed` for `apply_template`, and the old screen and its sessions remain intact

#### Scenario: Web window replaces a busy screen
- **WHEN** the user applies a template to the current screen in the web window while a program runs there
- **THEN** the window asks before applying, and only a confirmation sends the op with `replaceConfirmed: true`

#### Scenario: Older host
- **WHEN** a window replaces a screen of a host whose `mux.welcome` lacks `close-assessment-v1`
- **THEN** the window asks a generic confirmation before it sends the op

## MODIFIED Requirements

### Requirement: Native Template Replacement Consent
Before replacing an existing Screen, the native window SHALL disclose that both command-bearing and layout-only application end its existing terminals, with the running, starting, scratchpad and annotation impact from the server's close assessment of that Screen, or a generic notice when the host does not assess. Consent SHALL cover the captured Screen and its complete process impact, including hidden and floating tabs. Changed identities SHALL require a new review. Replacement SHALL use one apply operation with `replaceConfirmed: true` and without a preceding close operation.

#### Scenario: Replacement changed after review
- **WHEN** the target connection, Screen contents or affected identities no longer match the reviewed replacement
- **THEN** approval of the old review submits neither apply nor close and the current processes remain intact

#### Scenario: Confirmed replacement and command choice
- **WHEN** a current replacement review and its command choice are explicitly confirmed
- **THEN** one apply operation replaces the reviewed Screen, with commands omitted for layout-only
- **AND** an incidental Enter key does not authorize command execution
