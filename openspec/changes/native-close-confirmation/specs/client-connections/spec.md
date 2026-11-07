## ADDED Requirements

### Requirement: Native Local Terminal Close Assessment
The experimental native local workspace SHALL use one terminal close policy for pane, Screen and normal owned-window close requests. Busy or unresolved affected sessions SHALL require confirmation; known idle targets SHALL not. Window assessment SHALL include hidden Screens and pending local creations. Cancellation SHALL preserve affected sessions and layout. This policy covers the owned local terminal workspace only.

#### Scenario: Busy pane and Screen close
- **WHEN** a pane button, pane-close shortcut or Screen close would end a session reported busy
- **THEN** the native window asks for confirmation before submitting the close
- **AND** cancellation leaves the sessions and authoritative layout intact
- **AND** confirmation uses the existing close operation for the selected scope

#### Scenario: Known idle and unresolved impact
- **WHEN** the affected local sessions are known idle
- **THEN** the close can proceed without a prompt
- **WHEN** an affected session or local creation is unresolved
- **THEN** the close requires confirmation rather than treating missing state as idle

#### Scenario: Hidden busy session on window close
- **WHEN** normal owned-window close is requested while a hidden Screen contains a busy terminal
- **THEN** confirmation describes ending the owned local workspace
- **AND** cancellation leaves the window, hidden session and its output stream usable

### Requirement: Native Close Consent Scope
A native local close decision SHALL be bound to its request, connection generation, target and affected pane/session identities. The client SHALL revalidate its latest known impact before execution. Observed target replacement, changed affected identities or newly unconfirmed busy/unresolved impact SHALL reject the stale decision without closing other work. Unrelated metadata changes SHALL NOT invalidate consent.

#### Scenario: Target changes while confirmation is pending
- **WHEN** the client observes a replacement session, changed affected pane membership or connection generation while a close confirmation is pending
- **THEN** accepting the old prompt performs no close for the changed target
- **AND** the native window reports that another close request is needed

#### Scenario: Busy state changes before idle close execution
- **WHEN** an unconfirmed close was prepared for idle sessions and the client observes busy or unresolved impact before execution
- **THEN** the close is rejected without ending those sessions
- **AND** a subsequent close request assesses the current impact

#### Scenario: Repeated gestures and harmless metadata
- **WHEN** a close decision is already pending and the same gesture repeats or only titles change
- **THEN** the client retains one pending decision rather than overlapping prompts or submitting duplicate closes
- **AND** unchanged affected identities remain eligible for confirmation

### Requirement: Native Owned Window Close Acceptance
A normal close of the experimental owned local window SHALL keep its UI and service running until the current close request is accepted or owned cleanup is known complete. Stale decisions or failed command admission SHALL NOT stop the service or quit the UI. Once accepted, the native client SHALL execute owned shutdown once, cease further user mutations, and await owned cleanup before normal process exit. Forced termination and non-cancellable platform quit events are outside this confirmation guarantee.

#### Scenario: Stale or unadmitted window request
- **WHEN** a window-close decision is stale or its bounded command queue cannot admit it
- **THEN** the window stays open with its service running
- **AND** an approval for another request or connection cannot quit it

#### Scenario: Accepted owned shutdown
- **WHEN** the current window-close request passes confirmation and execution-time validation
- **THEN** the native window receives acceptance before quitting
- **AND** later queued user mutations are not executed
- **AND** the owned service and sessions are cleaned up once before normal process exit

#### Scenario: Close during startup or completed cleanup
- **WHEN** normal window close is requested before workspace discovery completes
- **THEN** unknown impact is assessed without assuming it is idle
- **WHEN** owned cleanup has already completed after a startup or connection failure
- **THEN** the window can close without waiting for an approval from a terminated owner
- **AND** a failed send or closed update channel alone is not treated as cleanup completion
