## ADDED Requirements

### Requirement: Saved Template Names
A window SHALL save a template under its name with `POST /api/templates` or native `template.create`, carrying the template and `overwrite` (shapes in `docs/protocol.ts`), and SHALL NOT derive the file name itself. The server SHALL derive the file name from the template's trimmed name: its ASCII letters and digits, lowercased, every run of other characters as one `-`, without a leading or trailing `-`, at most 128 bytes; a name that yields no file name SHALL be refused with 400. Without `overwrite`, the server SHALL save only when no saved template has that file name and otherwise answer 409 with the file name and change nothing; with `overwrite`, it SHALL replace that file. The check and the save SHALL be one step, so that of two saves of the same new name exactly one succeeds. A successful save SHALL answer with the file name and whether it replaced a file. The server SHALL announce `template-names-v1` in `mux.welcome.capabilities`. The existing `POST /api/templates/{slug}` SHALL keep its behavior.

#### Scenario: Same name from both windows
- **WHEN** the web window saves a template named `v1.2` and the native window then saves another one named `v1.2` without `overwrite`
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

## MODIFIED Requirements

### Requirement: Template Launch Data
Template launches SHALL preserve commands, restore modes and shell profile IDs and resolve relative directories against the supplied root on the target server. A command that is empty or holds only whitespace SHALL be no command: the session records none, gets no restore mode and types nothing. Shell profile selection and unavailable-profile fallback SHALL follow the target server’s settings contract. A failed individual launch SHALL be reported without inventing a successful session.

#### Scenario: Launch metadata and failure
- **WHEN** a template contains a command, restore mode, profile and relative directory
- **THEN** its session receives those values with the resolved directory
- **AND** a spawn failure identifies its pane and successful sibling launches remain attached

#### Scenario: Blank command
- **WHEN** a template pane's command is three spaces
- **THEN** its session starts an interactive shell without a command and without a restore mode, as for a pane without a command

### Requirement: Native Saved Template Library
The native window SHALL list and load the local server's saved templates and extract a Screen for saving with an optional relative root, chosen name and command subset; the file name SHALL come from the server (see Saved Template Names). Saving under a name whose file exists and deleting a saved template SHALL require explicit confirmation for the captured target. Commands omitted while saving SHALL lose their restore modes too.

#### Scenario: Extract, save and reload
- **WHEN** a Screen is extracted relative to a selected root and saved with a subset of its displayed commands
- **THEN** reloading the saved template preserves its terminal layout, floating bounds, directories and profiles, with exactly the chosen commands and their restore modes
- **AND** save uses the reviewed extraction rather than silently extracting changed processes again

#### Scenario: Overwrite and delete choices
- **WHEN** a save is answered with an existing file name or a saved template is selected for deletion
- **THEN** cancellation leaves the file intact and confirmation saves the captured name with `overwrite` or deletes the captured file on its captured connection
