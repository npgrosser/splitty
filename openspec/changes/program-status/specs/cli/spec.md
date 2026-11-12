# Spec Delta

## MODIFIED Requirements

### Requirement: Query Commands
`list-screens`, `list-panes` and `status` SHALL print human-readable output by default and the server's JSON response, pretty-printed, with `--json`. Human output of `list-screens` and `list-panes` is one tab-separated line per screen or pane, using `-` for missing values. A `list-panes` line SHALL end with `status=` and the state of the pane's program status (see `program-status`), followed by `:` and the kind for `blocked`, or `status=-` without one. With `--json`, a pane whose session holds a root status record SHALL carry it as `programStatus`.

#### Scenario: List screens
- **WHEN** a user runs `splitty list-screens` against a server with two screens
- **THEN** two lines are printed, each with id, name and pane count
- **AND** the active screen's id is followed by ` *`

#### Scenario: JSON output
- **WHEN** a user runs `splitty list-panes --json`
- **THEN** the CLI prints the target screen's panes as pretty-printed JSON

#### Scenario: List panes
- **WHEN** a user runs `splitty list-panes` inside a pane
- **THEN** each pane of that screen is printed as `<id>`, title, cwd, `session=<session-id>` and `status=<state>`, or `status=-` without a status

#### Scenario: Blocked agent in a pane
- **WHEN** the program of a pane has reported `blocked` with the kind `permission` and a user runs `splitty list-panes` for its screen
- **THEN** that pane's line ends with `status=blocked:permission`
- **AND** with `--json` the pane carries `programStatus` with the state `blocked` and the kind `permission`

#### Scenario: Status from inside a pane
- **WHEN** a user runs `splitty status` inside a Splitty pane
- **THEN** the output shows the control socket path as the server and `env` as the discovery source
- **AND** it shows the server mode, version, and the number of screens and sessions
