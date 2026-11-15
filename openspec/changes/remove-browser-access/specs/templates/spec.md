## ADDED Requirements

### Requirement: Confirmed Template Replacement
`apply_template` onto an existing screen that holds panes or sessions SHALL carry `replaceConfirmed: true`; without it the server SHALL reject the op with `mux.op_failed` and change nothing. A window SHALL set it only after the user agreed to the replacement: before replacing, it SHALL assess the screen (see `mux-protocol`, Close Assessment) and ask when the assessment asks, naming what ends; without the capability, or when the assessment fails, it SHALL ask a generic confirmation. Creating a screen needs no flag.

#### Scenario: Replacement without the flag
- **WHEN** a window sends `apply_template` for an existing screen with a running terminal and without `replaceConfirmed`
- **THEN** the window receives `mux.op_failed` for `apply_template`, and the old screen and its sessions remain intact

#### Scenario: Older host
- **WHEN** a window replaces a screen of a host whose `mux.welcome` lacks `close-assessment-v1`
- **THEN** the window asks a generic confirmation before it sends the op

## MODIFIED Requirements

### Requirement: Saved Template Names
A window SHALL save a template under its name with `POST /api/templates` or native `template.create`, carrying the template and `overwrite` (shapes in `docs/protocol.ts`), and SHALL NOT derive the file name itself. The server SHALL derive the file name from the template's trimmed name: its ASCII letters and digits, lowercased, every run of other characters as one `-`, without a leading or trailing `-`, at most 128 bytes; a name that yields no file name SHALL be refused with 400. Without `overwrite`, the server SHALL save only when no saved template has that file name and otherwise answer 409 with the file name and change nothing; with `overwrite`, it SHALL replace that file. The check and the save SHALL be one step, so that of two saves of the same new name exactly one succeeds. A successful save SHALL answer with the file name and whether it replaced a file. The server SHALL announce `template-names-v1` in `mux.welcome.capabilities`. The existing `POST /api/templates/{slug}` SHALL keep its behavior.

#### Scenario: Same name from both windows
- **WHEN** one client of a local service saves a template named `v1.2` and a second client of that service then saves another one named `v1.2` without `overwrite`
- **THEN** the first is saved as `v1-2` and the second is answered with 409 naming `v1-2`, and the first file is unchanged

#### Scenario: Confirmed overwrite
- **WHEN** a window, answered with 409, saves the same name again with `overwrite`
- **THEN** the file is replaced and the answer says it replaced one

#### Scenario: Concurrent saves of a new name
- **WHEN** two saves of the same new name without `overwrite` arrive at the same time
- **THEN** one of them saves and the other is answered with 409

#### Scenario: Name without letters or digits
- **WHEN** a window saves a template named `...`
- **THEN** the server answers 400 and saves nothing

#### Scenario: Older local service
- **WHEN** the native window's local service does not announce `template-names-v1`
- **THEN** the native window saves no template and asks for the local service to be updated

## REMOVED Requirements

### Requirement: Confirmed Screen Replacement
**Reason**: Restated as Confirmed Template Replacement without the web window, which is deleted; the flag and the confirmation rule are unchanged.
