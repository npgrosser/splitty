## MODIFIED Requirements

### Requirement: Ordered Native Zoom Updates
Native zoom actions SHALL target only their captured local service through bounded ordered settings work, preserving terminal event delivery. The actions that wait when zoom work starts SHALL be carried out together as one read and at most one write: the level SHALL be derived from current service settings by applying each action in order, and the write SHALL patch only zoom. Each admitted action SHALL keep its own outcome, and outcomes SHALL remain observable independently of appearance refreshes. Actions that wait behind a confirmed write SHALL follow it before the appearance is read again. Failed or uncertain writes SHALL be reported without automatic mutation retry, and a current read SHALL precede later actions.

#### Scenario: Rapid actions while settings refresh
- **WHEN** several zoom actions arrive while local settings or OS preference refreshes are in progress
- **THEN** admitted actions take effect in order without being replaced or losing their outcomes to read refreshes, and the saved level is the one all of them lead to
- **AND** terminal output and ordinary input continue, unrelated settings are preserved and queue overflow is visible

#### Scenario: Steps that wait together
- **WHEN** three zoom-in actions and a reset wait behind a running zoom write
- **THEN** one read and one write follow, the write sets 100 %, and each of the four actions reports that confirmed level

#### Scenario: Save response is uncertain
- **WHEN** a zoom save may have been applied but its outcome cannot be confirmed
- **THEN** the client reports uncertainty and reconciles with current service settings before later actions
- **AND** it does not resend the uncertain action automatically or claim that it was definitely unsaved

#### Scenario: Wrong owner or close during update
- **WHEN** a zoom action names an obsolete connection or the owning workspace closes during settings work
- **THEN** no action is redirected to another service, and owned pending API work is cancelled and joined during shutdown

## ADDED Requirements

### Requirement: Native Zoom Shown Ahead Of Confirmation
The native window SHALL show the step of a zoom action it sent at once, derived from the zoom it shows. While actions are outstanding, confirmations and settings reads SHALL NOT replace what is shown; once none is outstanding the window SHALL show the level the service holds. After a rejected or uncertain write with nothing else outstanding it SHALL show the last confirmed level again. During consecutive steps a shown terminal's grid SHALL follow each step, and its PTY size SHALL be reported once, after the last.

#### Scenario: A held zoom key
- **WHEN** the user holds the zoom-in key over several steps
- **THEN** each step is visible without waiting for the service, the display never returns to an earlier step meanwhile, and the level saved at the end is the one shown

#### Scenario: A step that was not saved
- **WHEN** the write of a single zoom step is rejected
- **THEN** the window returns to the last confirmed level and reports the failure
