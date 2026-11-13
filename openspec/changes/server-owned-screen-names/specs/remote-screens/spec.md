# Spec Delta

## MODIFIED Requirements

### Requirement: Screen Default Names Per Host
A new screen SHALL get its default name `Screen N` from the server that holds it, using the smallest unused positive integer among that server's screens, so local screens and each remote host count independently and a host's screens the window does not show reserve their names. The window SHALL create a host's screen without a name when the host announces `screen-names-v1` (see `mux-protocol`, Server-Chosen Screen Names). Existing server, restored and user-selected names SHALL remain unchanged; names on another host SHALL NOT affect a host's choice.

#### Scenario: Independent numbering
- **WHEN** local screens already use Screen 1 through Screen 3 and a new screen is created on an otherwise empty remote host
- **THEN** the new remote screen is named Screen 1

#### Scenario: Existing unopened screen
- **WHEN** a host already has an unopened Screen 1 and a new screen is created there
- **THEN** the host names the new screen Screen 2

#### Scenario: Existing names survive
- **WHEN** a workspace restores a screen named Default or a user-selected name
- **THEN** that name is preserved

#### Scenario: Older host
- **WHEN** a new screen is created on a host whose `mux.welcome` lacks `screen-names-v1`
- **THEN** the window sends `create_screen` named with the smallest `Screen N` that neither the host's screens it knows nor the window's screens of that host use at that moment, and does not rename the screen later
