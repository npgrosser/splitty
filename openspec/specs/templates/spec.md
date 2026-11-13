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
Template launches SHALL preserve commands, restore modes and shell profile IDs and resolve relative directories against the supplied root on the target server. A command that is empty or holds only whitespace SHALL be no command: the session records none, gets no restore mode and types nothing. Shell profile selection and unavailable-profile fallback SHALL follow the target server’s settings contract. A failed individual launch SHALL be reported without inventing a successful session.

#### Scenario: Launch metadata and failure
- **WHEN** a template contains a command, restore mode, profile and relative directory
- **THEN** its session receives those values with the resolved directory
- **AND** a spawn failure identifies its pane and successful sibling launches remain attached

#### Scenario: Blank command
- **WHEN** a template pane's command is three spaces
- **THEN** its session starts an interactive shell without a command and without a restore mode, as for a pane without a command

### Requirement: Template Validation
Application SHALL reject duplicate pane references, missing definitions, unused terminal definitions, reused pane IDs, invalid ratios and floating split layouts without changing the workspace. Legacy web panes (pane type `webview`) SHALL be dropped wherever a template is read, from its file, from a request or from a host's answer: the pane goes, the layout keeps the remaining terminals with a split of one remaining child replaced by that child, and the template never loads a URL. A template left without a tiled terminal SHALL fail to apply.

#### Scenario: Invalid or legacy content
- **WHEN** an invalid template or an all-web template is applied
- **THEN** the old screen and sessions remain intact
- **AND** mixed legacy templates omit web panes and retain their terminal layout

#### Scenario: Saved template with a web pane
- **WHEN** a saved template file holds a terminal `a` and a `webview` pane `b` side by side, and a window loads it
- **THEN** the template it receives defines only `a`, its layout is the single slot `a`, and applying it starts one terminal

#### Scenario: Host of an earlier release
- **WHEN** a window loads a template from a host whose answer still holds a `webview` pane
- **THEN** the window drops that pane when it reads the answer and shows and applies the template without it

### Requirement: Extracting Templates
Extraction SHALL use the server's screen layout and floating bounds, with terminal commands, current directories, profiles and restore modes. Live non-shell foreground commands SHALL take precedence over stored launch commands; a live base shell SHALL extract without a command. Viewer panes SHALL be omitted and split ratios renormalized. Empty or viewer-only screens SHALL report an error.

#### Scenario: Extract and reapply
- **WHEN** a window extracts a screen with terminal tabs, floating terminals and viewers, optionally relative to a root
- **THEN** its template preserves the terminal layout and launch metadata, omits viewers, and relativizes only directories within that root
- **AND** a screen with no tiled terminals reports an error

### Requirement: Template Command Consent
The window SHALL show and request confirmation for every template command before applying it. Cancellation SHALL send no apply operation. Layout-only application SHALL strip commands and their restore modes before sending the op. Approval SHALL bind the exact displayed template content and SHALL be consumed once; absence of an approval handler SHALL NOT authorize commands.

#### Scenario: Consent choices
- **WHEN** a template contains commands
- **THEN** approving runs only the displayed commands, layout-only runs none, and cancelling changes no screen

#### Scenario: Missing handler or changed source data
- **WHEN** command approval cannot be presented, or the source template changes after a request is presented
- **THEN** missing approval sends no apply operation and a later decision can apply only the captured displayed content

#### Scenario: Repeated decision
- **WHEN** an approval or cancellation has already been consumed
- **THEN** invoking that decision again or another choice from the same request sends no additional apply operation

### Requirement: Native Saved Template Library
The native window SHALL list and load the local server's saved templates and extract a Screen for saving with an optional relative root, chosen name and command subset; the file name SHALL come from the server (see Saved Template Names). Saving under a name whose file exists and deleting a saved template SHALL require explicit confirmation for the captured target. Commands omitted while saving SHALL lose their restore modes too.

#### Scenario: Extract, save and reload
- **WHEN** a Screen is extracted relative to a selected root and saved with a subset of its displayed commands
- **THEN** reloading the saved template preserves its terminal layout, floating bounds, directories and profiles, with exactly the chosen commands and their restore modes
- **AND** save uses the reviewed extraction rather than silently extracting changed processes again

#### Scenario: Overwrite and delete choices
- **WHEN** a save is answered with an existing file name or a saved template is selected for deletion
- **THEN** cancellation leaves the file intact and confirmation saves the captured name with `overwrite` or deletes the captured file on its captured connection

### Requirement: Native Template Draft Ownership
Native library results and template decisions SHALL belong to their captured connection and request. Superseded replies SHALL NOT overwrite newer edits, reopen a dismissed dialog or authorize a mutation. Oversized or non-applicable content SHALL report an error without silent truncation or application.

#### Scenario: Stale or invalid draft
- **WHEN** a library reply arrives after its dialog was replaced, its connection changed, or the requested draft exceeds supported bounds or cannot be applied
- **THEN** the client preserves newer input and submits no apply operation from that reply
- **AND** an unusable extracted layout is explained before native saving rather than silently changing its structure

### Requirement: Native Template Replacement Consent
Before replacing an existing Screen, the native window SHALL disclose that both command-bearing and layout-only application end its existing terminals, with the running, starting, scratchpad and annotation impact from the server's close assessment of that Screen, or a generic notice when the host does not assess. Consent SHALL cover the captured Screen and its complete process impact, including hidden and floating tabs. Changed identities SHALL require a new review. Replacement SHALL use one apply operation with `replaceConfirmed: true` and without a preceding close operation.

#### Scenario: Replacement changed after review
- **WHEN** the target connection, Screen contents or affected identities no longer match the reviewed replacement
- **THEN** approval of the old review submits neither apply nor close and the current processes remain intact

#### Scenario: Confirmed replacement and command choice
- **WHEN** a current replacement review and its command choice are explicitly confirmed
- **THEN** one apply operation replaces the reviewed Screen, with commands omitted for layout-only
- **AND** an incidental Enter key does not authorize command execution

### Requirement: Native Template Apply Admission
The native client SHALL require template capability, reserve distinct client pane IDs and a new Screen ID when needed, and check projected workspace capacity before application. Conflicting structural work, stale consent and queue rejection SHALL submit no mutation. An admitted application SHALL be submitted at most once and SHALL NOT create a throwaway initial terminal.

#### Scenario: Admission failure
- **WHEN** capability, target, capacity, operation conflicts or command queue admission prevents application
- **THEN** no apply or preliminary create/close operation is sent and existing Screens remain intact

### Requirement: Native Pending Template Presentation
An admitted native apply SHALL immediately present its intended tiled and floating layout as pending while canonical sessions remain authoritative. Pending panes SHALL receive no input, attachment or visible-size claims. Canonical terminals hidden by the preview SHALL release visible-size claims while continuing output. Later navigation SHALL cancel deferred following, not replay or cancel the submitted mutation.

#### Scenario: Pending layout and later navigation
- **WHEN** application is pending and the window changes to another Screen
- **THEN** that Screen remains usable, the pending layout creates no speculative session routes and eventual completion does not steal the later selection
- **AND** unrelated terminal identities and retained content survive reconciliation

### Requirement: Native Template Application Outcomes
Native application outcomes SHALL be determined from authoritative layout and launch observations after submission, not solely from an error message. Successful sibling launches SHALL remain usable when another launch fails. Service-reported shell-profile fallback SHALL remain visible as a notice. Unverifiable outcomes SHALL be reported without replaying application or claiming the previous processes survived.

#### Scenario: Partial launch or post-commit failure
- **WHEN** the new layout commits but a launch or later effect fails
- **THEN** the accepted layout remains authoritative, successful sessions remain attached and the failure is visible without waiting indefinitely for every terminal to become ready
- **AND** a successful launch with a reported shell-profile fallback remains usable and reports that fallback separately from launch failures

#### Scenario: Unverified application
- **WHEN** current authoritative state cannot be established after submission
- **THEN** the client reports uncertainty, retains the last coherent routes and submits no automatic retry

### Requirement: Native Template Input and Shutdown
Template dialogs SHALL own their keyboard, pointer and wheel input without forwarding it to terminals. Pending application SHALL count as unresolved whole-window close impact. Scoped structural actions SHALL remain excluded while application is unsettled, but whole-window shutdown SHALL remain available and join cleanup of admitted work.

#### Scenario: Dismissal and close during pending work
- **WHEN** a template dialog is dismissed or the window is closed during template API/application work
- **THEN** dialog keys do not become PTY input, stale replies do not reopen it, and confirmed window shutdown joins its owned service and request cleanup

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
