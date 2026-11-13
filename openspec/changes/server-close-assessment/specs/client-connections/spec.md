# Spec Delta

## MODIFIED Requirements

### Requirement: Native Local Terminal Close Assessment
The experimental native local workspace SHALL use one terminal close policy for pane, Screen and normal owned-window close requests. Before it asks or closes, it SHALL obtain the server's close assessment of the target (see `mux-protocol`, Close Assessment) and SHALL require confirmation when that assessment asks, when the server does not assess, or when an affected creation, Screen or reconciliation of the client is still unresolved; known idle targets SHALL not. Window assessment SHALL include hidden Screens and pending local creations. Cancellation SHALL preserve affected sessions and layout. This policy covers the owned local terminal workspace only.

#### Scenario: Busy pane and Screen close
- **WHEN** a pane button, pane-close shortcut or Screen close would end a session the server's assessment reports running
- **THEN** the native window asks for confirmation before submitting the close
- **AND** cancellation leaves the sessions and authoritative layout intact
- **AND** confirmation uses the existing close operation for the selected scope

#### Scenario: Known idle and unresolved impact
- **WHEN** the server's assessment lists nothing for the affected local sessions
- **THEN** the close can proceed without a prompt
- **WHEN** an affected session is still starting or a local creation is unresolved
- **THEN** the close requires confirmation rather than treating missing state as idle

#### Scenario: Hidden busy session on window close
- **WHEN** normal owned-window close is requested while a hidden Screen contains a busy terminal
- **THEN** confirmation describes ending the owned local workspace
- **AND** cancellation leaves the window, hidden session and its output stream usable

#### Scenario: Service without assessment
- **WHEN** the close target's service does not announce `close-assessment-v1`
- **THEN** the native window asks a generic confirmation, also for an idle target

### Requirement: Native Close Consent Scope
A native local close decision SHALL be bound to its request, connection generation, target and affected pane/session identities, and to the server's close assessment it was shown with. The client SHALL revalidate the identities and its own pending state before execution. Observed target replacement, changed affected identities or newly unresolved client state SHALL reject the stale decision without closing other work. Unrelated metadata changes SHALL NOT invalidate consent.

#### Scenario: Target changes while confirmation is pending
- **WHEN** the client observes a replacement session, changed affected pane membership or connection generation while a close confirmation is pending
- **THEN** accepting the old prompt performs no close for the changed target
- **AND** the native window reports that another close request is needed

#### Scenario: Busy state changes before idle close execution
- **WHEN** an unconfirmed close was prepared from an assessment that lists nothing and the client observes unresolved impact before execution
- **THEN** the close is rejected without ending those sessions
- **AND** a subsequent close request assesses the current impact

#### Scenario: Repeated gestures and harmless metadata
- **WHEN** a close decision is already pending and the same gesture repeats or only titles change
- **THEN** the client retains one pending decision rather than overlapping prompts or submitting duplicate closes
- **AND** unchanged affected identities remain eligible for confirmation
