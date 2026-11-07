## MODIFIED Requirements

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

## ADDED Requirements

### Requirement: Native Saved Template Library
The native window SHALL list and load the local server's saved templates and extract a Screen for saving with an optional relative root, chosen name and command subset. Saving over an existing slug and deleting a saved template SHALL require explicit confirmation for the captured target. Commands omitted while saving SHALL lose their restore modes too.

#### Scenario: Extract, save and reload
- **WHEN** a Screen is extracted relative to a selected root and saved with a subset of its displayed commands
- **THEN** reloading the saved template preserves its terminal layout, floating bounds, directories and profiles, with exactly the chosen commands and their restore modes
- **AND** save uses the reviewed extraction rather than silently extracting changed processes again

#### Scenario: Overwrite and delete choices
- **WHEN** a save would replace an existing slug or a saved template is selected for deletion
- **THEN** cancellation leaves the file intact and confirmation changes only the captured slug on its captured connection

### Requirement: Native Template Draft Ownership
Native library results and template decisions SHALL belong to their captured connection and request. Superseded replies SHALL NOT overwrite newer edits, reopen a dismissed dialog or authorize a mutation. Oversized or non-applicable content SHALL report an error without silent truncation or application.

#### Scenario: Stale or invalid draft
- **WHEN** a library reply arrives after its dialog was replaced, its connection changed, or the requested draft exceeds supported bounds or cannot be applied
- **THEN** the client preserves newer input and submits no apply operation from that reply
- **AND** an unusable extracted layout is explained before native saving rather than silently changing its structure

### Requirement: Native Template Replacement Consent
Before replacing an existing Screen, the native window SHALL disclose that both command-bearing and layout-only application end its existing terminals. Consent SHALL cover the captured Screen and its complete process impact, including hidden and floating tabs. Changed identities or increased busy/unresolved impact SHALL require a new review. Replacement SHALL use one apply operation without a preceding close operation.

#### Scenario: Replacement changed after review
- **WHEN** the target connection, Screen contents or affected process impact no longer matches the reviewed replacement
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
Native application outcomes SHALL be determined from authoritative layout and launch observations after submission, not solely from an error message. Successful sibling launches SHALL remain usable when another launch fails. Unverifiable outcomes SHALL be reported without replaying application or claiming the previous processes survived.

#### Scenario: Partial launch or post-commit failure
- **WHEN** the new layout commits but a launch or later effect fails
- **THEN** the accepted layout remains authoritative, successful sessions remain attached and the failure is visible without waiting indefinitely for every terminal to become ready

#### Scenario: Unverified application
- **WHEN** current authoritative state cannot be established after submission
- **THEN** the client reports uncertainty, retains the last coherent routes and submits no automatic retry

### Requirement: Native Template Input and Shutdown
Template dialogs SHALL own their keyboard, pointer and wheel input without forwarding it to terminals. Pending application SHALL count as unresolved whole-window close impact. Scoped structural actions SHALL remain excluded while application is unsettled, but whole-window shutdown SHALL remain available and join cleanup of admitted work.

#### Scenario: Dismissal and close during pending work
- **WHEN** a template dialog is dismissed or the window is closed during template API/application work
- **THEN** dialog keys do not become PTY input, stale replies do not reopen it, and confirmed window shutdown joins its owned service and request cleanup
