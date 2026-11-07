# Spec Delta

## ADDED Requirements

### Requirement: Experimental Native Settings Editor
The experimental native workspace SHALL offer an editor for the local service's supported Appearance, Terminal, and Workspace preferences. It SHALL read effective values through `home.get`, defaults and limits through `home.metadata`, and theme choices through `theme.list`. It SHALL submit only explicit supported changes through `home.update`; selecting a remote Screen SHALL NOT change the settings source.

#### Scenario: Local source while viewing remote content
- **WHEN** a remote Screen is selected and the native Settings editor opens
- **THEN** it shows the assigned local service's effective values and changes only that service's settings

#### Scenario: Supported controls and reset
- **WHEN** a user changes or resets UI theme mode or choice, UI font size or family, display zoom or scope, terminal palette or close-on-exit policy, or Drawer visibility or width
- **THEN** the native client submits a bounded patch for that setting and displays the confirmed effective value
- **AND** reset uses the local service's published default when available

#### Scenario: Unavailable metadata or theme catalog
- **WHEN** settings metadata or the theme catalog cannot be read
- **THEN** already loaded effective settings remain visible and terminal use continues
- **AND** controls that cannot establish valid choices or reset values do not submit guessed values

### Requirement: Native Settings Mutation Reconciliation
Native settings mutations SHALL be identified with their local connection and request, admitted within a bounded queue, submitted at most once, and reconciled with the service's complete effective settings and `settings.changed`. A rejected, uncertain, or stale result SHALL NOT be shown as saved. The editor SHALL retain a visible error or uncertain state and refresh from the local service without replaying the write.

#### Scenario: Confirmed edit or same-service event
- **WHEN** this window's settings change is confirmed or `settings.changed` arrives on its assigned local service connection
- **THEN** the native editor and current workspace display the latest effective settings
- **AND** terminal output continues on its separate stream

#### Scenario: Rejection or uncertain completion
- **WHEN** `home.update` rejects a change or its outcome is uncertain
- **THEN** the editor retains the last confirmed value, identifies the field and failure, and requests a fresh local settings read
- **AND** the mutation is not repeated automatically

#### Scenario: Old result and close
- **WHEN** a result belongs to an older local connection or an older settings generation, or the window closes during an update
- **THEN** the result cannot replace current presentation
- **AND** owned pending work is cancelled and joined without changing another service

### Requirement: Independent Native Terminal Palette
The native workspace SHALL use the active UI theme's terminal colors when `terminal.theme` is `default`. When it names an available theme, the workspace SHALL use that theme's resolved terminal colors while keeping the selected UI theme for chrome. Palette-only changes SHALL update existing and future terminals without recreating sessions, losing terminal history, or changing native terminal font metrics.

#### Scenario: Separate terminal choice
- **WHEN** the user chooses an available terminal palette different from the UI theme
- **THEN** terminal colors change and the workspace chrome retains its UI theme

#### Scenario: Inherit current UI theme
- **WHEN** the terminal palette is reset to `default` or automatic UI mode changes with `terminal.theme` set to `default`
- **THEN** terminal colors follow the active UI theme

#### Scenario: Unavailable saved palette
- **WHEN** a saved terminal theme is unavailable or its resolution fails
- **THEN** the native workspace keeps the last valid terminal palette or the active UI theme's terminal colors, reports the problem, and does not overwrite the saved setting
