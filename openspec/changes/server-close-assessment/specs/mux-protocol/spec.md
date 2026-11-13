# Spec Delta

## ADDED Requirements

### Requirement: Close Assessment
A server SHALL list `close-assessment-v1` in `mux.welcome.capabilities` and answer `{"type": "mux.assess_close", "requestId", "scope"}` on the same connection with `{"type": "mux.close_assessed", "requestId", "assessment"}`, or with `error` for a malformed scope (shapes in `docs/protocol.ts`). The `scope` names the panes of one screen, whole screens, or the whole workspace; a screen or pane the server does not have adds nothing. The assessment SHALL list each pane, and each session of a named screen without a pane, that closing would make lose something, with its screen, pane and session IDs, its display title, and what it loses: a terminal's program that runs (`running`) or is still starting (`starting`), a scratchpad that holds text (`scratchpad`), and the number of annotations a viewer pane holds (`annotations`). Managed job panes SHALL NOT count as running. It SHALL carry the totals of these and `confirm`, true exactly when any entry loses something. The answer SHALL reflect the server's state after the preceding messages of that connection and SHALL change nothing. A window SHALL decide whether to ask before a pane, tab, screen or window close and before a template replaces a screen from this answer only, and SHALL send the request on the connection of the screen it closes, so a host's screen is assessed by its host through the hub's relay.

#### Scenario: A busy pane and an idle pane
- **WHEN** a window assesses the panes `p1` and `p2` of screen `s1`, where `p1` runs `vim` and `p2` is an idle shell
- **THEN** the answer lists `p1` with `running` true and not `p2`, and `confirm` is true

#### Scenario: A pane that is still starting
- **WHEN** a window assesses a pane whose session has not started yet
- **THEN** the pane is listed with `starting` true and `confirm` is true

#### Scenario: Nothing to lose
- **WHEN** every pane in the scope is an idle shell without scratchpad text and no viewer holds annotations
- **THEN** the answer lists nothing and `confirm` is false, and the window closes without asking

#### Scenario: The whole workspace
- **WHEN** a window of an embedded server assesses the workspace before it quits
- **THEN** the answer covers the panes and sessions of every screen, shown or not

#### Scenario: Older server
- **WHEN** a window closes a pane or screen of a connection whose `mux.welcome` does not list `close-assessment-v1`, or the assessment fails or does not come
- **THEN** the window asks a generic confirmation before closing, also for an idle pane, and offers no assessment-based text
