# Spec Delta

## MODIFIED Requirements

### Requirement: Feature Parity Without Hidden Listeners
Released native clients SHALL support the existing settings, themes, shell profiles, templates, notifications, files, editor confirmations, popouts, zoom and host-management features allowed by their role. Using those features SHALL NOT start a fallback HTTP listener or grant arbitrary native privileges. Explicitly experimental development executables MAY implement a documented subset; they SHALL retain the same transport admission, backend identity and authority boundaries and SHALL NOT replace released clients.

#### Scenario: Native desktop workflow
- **WHEN** a desktop user saves settings, changes a theme, transfers a file, confirms an editor request and moves a terminal into a popout
- **THEN** the operations retain their existing validation and lifetime behavior without starting a terminal-server TCP listener

#### Scenario: Experimental executable
- **WHEN** a development-only native executable provides a terminal and document probe
- **THEN** it identifies its experimental scope, keeps released clients available and introduces no fallback HTTP listener

## ADDED Requirements

### Requirement: Isolated Native Probe Lifetime
The experimental `splitty-gpui` executable SHALL use an isolated app-owned workspace. Closing a client connection SHALL revoke only its stream; reconnecting to that service SHALL reattach existing sessions. Closing the owned application SHALL await its service and process cleanup without ending sessions of another service. A Windows client SHALL use an owned helper or explicit native connection, never silently substitute a different backend.

#### Scenario: Reattach in the probe
- **WHEN** the probe reconnects to its still-running service
- **THEN** it resynchronizes the existing screen and attaches its existing session without starting a replacement process

#### Scenario: Probe shutdown
- **WHEN** the isolated probe exits normally
- **THEN** its local sessions and owned service are stopped, and another service's sessions remain unaffected

### Requirement: Native Probe Data Paths
The native probe SHALL send input and size reports through the existing session protocol, preserve replay/live ordering and refrain from replay-triggered terminal replies or clipboard actions. Documents SHALL be read with versioned `viewer.read` chunks over their own connection. Native selections used for annotations SHALL retain Unicode-safe quote/context and source positions and be saved through the existing mux operations.

#### Scenario: Live terminal
- **WHEN** a user types in the probe terminal and resizes its view
- **THEN** the assigned service receives the input and size report, and its actual output is rendered in order

#### Scenario: Native document annotation
- **WHEN** a passage containing multibyte text is selected in the probe's Markdown document and annotated
- **THEN** the annotation is stored on that viewer pane and remains represented after a fresh screen snapshot

### Requirement: Scripted Probe Isolation
An optional scripted-content probe SHALL run only an explicit local fixture. Its engine SHALL receive no service connection, authentication material or native application command bridge. Its pixels SHALL participate in native clipping and overlays; an unavailable engine SHALL be reported as unavailable rather than replaced with a fabricated success result. Static Markdown SHALL remain inert, with no implicit resource loads or external actions.

#### Scenario: Offscreen composition
- **WHEN** the scripted fixture runs under a native overlay and inside clipped pane bounds
- **THEN** its real rendered frames respect those bounds and cannot cover the overlay or invoke service operations

#### Scenario: Unavailable engine
- **WHEN** the optional engine cannot be initialized
- **THEN** the probe reports that failure and makes no claim that scripted composition was validated
