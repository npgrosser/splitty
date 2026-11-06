## MODIFIED Requirements

### Requirement: Screen Creation
On `create_screen` the server SHALL create a screen whose root layout is a single slot holding one new `terminal` pane backed by one new session. The session SHALL use the shell profile given in the op, or the server's default shell profile when none is given. A new screen SHALL be appended to the end of the screen order, and SHALL become the active screen only if the workspace had no active screen. If `screenId` names an existing screen without any pane, the server SHALL re-create that screen in place: its layout becomes a single slot with the new pane, it keeps its name unless the op gives one, and it keeps its place in the screen order. If `screenId` names an existing screen that has at least one pane, in its root layout or in a floating entry, the server SHALL reject the op with a "screen already exists" error. If no name is supplied or retained, the server SHALL assign the first unused name Screen N within this workspace.

#### Scenario: First screen becomes active
- **WHEN** `create_screen` is applied to a workspace without screens
- **THEN** the workspace contains one screen with one pane and one session
- **AND** that screen is the active screen

#### Scenario: Additional screen does not steal activation
- **WHEN** `create_screen` is applied while another screen is active
- **THEN** the new screen is appended to the screen order
- **AND** the active screen is unchanged

#### Scenario: Re-create a screen that ran empty
- **WHEN** `create_screen` names screen `s1`, whose last pane was closed, with `newPaneId` `p9`
- **THEN** `s1` holds the single pane `p9` with a new session
- **AND** `s1` keeps its name and its place in the screen order

#### Scenario: Existing screen with panes is not replaced
- **WHEN** `create_screen` names screen `s1`, which has two panes with running sessions
- **THEN** the op is rejected with a "screen already exists" error
- **AND** `s1`, its panes and its sessions remain exactly as before

### Requirement: New Screen from a Pane Move
With `createScreen: true`, `move_pane_to_screen` SHALL create the target as a new screen with that ID, named `screenName` when given, holding only the pane and placed right after the source in the screen order. When screenName is omitted, the server SHALL assign the first unused name Screen N within this workspace.

#### Scenario: Move to a new screen
- **WHEN** `move_pane_to_screen` moves `p2` from `s1` to `s9` with `createScreen` true and `screenName` "Logs"
- **THEN** a screen `s9` named "Logs" holds only `p2` and follows `s1` in the screen order

### Requirement: Split Session Inheritance
The session of a pane created by `split_pane` or `new_tab` SHALL start in the op's `cwd` if given, else in the source pane's known working directory, else in the source session's working directory. It SHALL use the op's shell profile if given, else the source session's shell profile, else, for a source pane without a session such as a viewer pane, the default shell profile. The source session's legacy shell value SHALL be inherited only when the op names no shell profile. On a server advertising serverDerivedPaneState, clients SHALL send cwd and shell profile only when explicitly selected; new tabs and splits use the same server inheritance.

#### Scenario: Inherit profile and directory
- **WHEN** a pane whose session uses profile `zsh` in `/work` is split without `cwd` or profile
- **THEN** the new session uses profile `zsh` and starts in `/work`

#### Scenario: Explicit profile overrides inheritance
- **WHEN** a pane whose session has a legacy shell value is split with an explicit shell profile
- **THEN** the new session uses the explicit profile and carries no legacy shell value

#### Scenario: Split of a viewer pane
- **WHEN** a viewer pane of `/home/me/app/PLAN.md` is split without `cwd` or profile
- **THEN** the new terminal's session starts in `/home/me/app` with the default shell profile

## ADDED Requirements

### Requirement: Server Close On Exit
The owning server SHALL apply its close_on_exit setting to ordinary terminal panes without requiring a client: always closes any exit, graceful only a known zero exit, never retains. Managed job panes SHALL retain their outcome. Automatic closure SHALL use the normal pane closing behavior, retaining an empty screen.

#### Scenario: Unattended exit
- **WHEN** an ordinary pane exits successfully while no client is attached and close_on_exit is graceful
- **THEN** its pane and session are removed and its empty screen remains

#### Scenario: Retained outcomes
- **WHEN** an ordinary pane fails under graceful, exits under never, or a managed job exits under any policy
- **THEN** the pane and its output remain available
