## ADDED Requirements

### Requirement: Native Local Context Gesture
The native terminal SHALL offer local context actions only for a right-button gesture admitted to local UI handling. Application mouse reporting SHALL take precedence unless Shift selects the local override at gesture start. A local context gesture SHALL send no mouse bytes to the session, including if modifiers change before release.

#### Scenario: Application and local routes stay separate
- **WHEN** an application requests mouse-button reports and the user right-clicks without Shift
- **THEN** the terminal reports the gesture and opens no local menu
- **AND** a Shift-right-click opens local context actions without sending any part of that gesture to the application

### Requirement: Captured Native Terminal Context Actions
Native context actions SHALL retain the clicked displayed terminal's source, Screen, pane and session identity. Clipboard input SHALL use that terminal's existing bounded encoding and delivery path; structural and close actions SHALL retain their existing admission and process-impact checks. A changed or removed target SHALL revoke the action rather than redirecting it to current focus.

#### Scenario: Another split held focus
- **WHEN** a local context gesture targets a displayed terminal in a different split group
- **THEN** menu actions address the clicked terminal and model focus agrees with that terminal
- **AND** dismissal restores usable focus without sending menu navigation to the session

#### Scenario: Paste keeps its captured route
- **WHEN** Paste is activated for an unchanged captured terminal
- **THEN** one input request carries the clipboard bytes with that terminal's current paste framing to its captured session
- **AND** an obsolete owner, Screen, pane or session receives no action or replay

#### Scenario: Close retains assessment
- **WHEN** Close Pane is activated from a terminal context menu
- **THEN** the existing process-impact assessment and required confirmation run for that captured pane
- **AND** menu activation alone does not bypass close policy or terminate another pane

#### Scenario: Retained offline terminal actions
- **WHEN** a displayed terminal retains its buffer after its source goes offline
- **THEN** local copy and search remain available and focus remains owned by the client window model across unrelated updates
- **AND** that focus intent requires the same source owner, shown Screen, displayed pane and session binding
- **AND** it neither reconnects the source nor permits input, layout changes or close actions without live authority
- **AND** closing, disposed, replaced or non-displayed targets reject the intent
