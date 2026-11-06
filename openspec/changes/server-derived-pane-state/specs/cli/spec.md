## MODIFIED Requirements

### Requirement: Layout and Screen Commands
`split`, `close-pane`, `new-screen [--name <name>]`, `close-screen` and `rename-screen <name>` SHALL each submit one multiplexer operation through the mux operation endpoint (see `control-api`), whose effect is specified in `workspace-model`, and print a one-line confirmation on success. `split` SHALL request a `vertical` split (new pane below, stacked top to bottom; see `workspace-model`, Layout Tree Shape) by default and a `horizontal` split (new pane to the right, side by side) with `-H`/`--horizontal`. Plain-text pane listings SHALL render control and bidirectional formatting characters in metadata as visible replacement characters rather than terminal instructions or extra lines; JSON output SHALL retain JSON escaping.

#### Scenario: Default split
- **WHEN** a user runs `splitty split` inside a pane
- **THEN** the CLI requests a `vertical` split of that pane and prints `Split <pane-id> (vertical)`

#### Scenario: Horizontal split
- **WHEN** a user runs `splitty split -H`
- **THEN** the CLI requests a `horizontal` split and prints `Split <pane-id> (horizontal)`

#### Scenario: Named screen
- **WHEN** a user runs `splitty new-screen --name dev`
- **THEN** the CLI requests a new screen named `dev` and prints `Screen created (dev)`

### Requirement: Attach Target Selection
`<target>` SHALL name a pane or a session by its id, or by a prefix of at least 4 characters that matches exactly one pane or session id of the server; a target that matches none or several SHALL fail with a command error naming the candidates. Without a target, on a terminal, the command SHALL list the server's panes bound to a session, numbered, with screen name, pane title and working directory, and attach to the one the user picks. It SHALL NOT default to `SPLITTY_PANE_ID`. Plain-text pane listings SHALL render control and bidirectional formatting characters in metadata as visible replacement characters rather than terminal instructions or extra lines; JSON output SHALL retain JSON escaping.

#### Scenario: Pane by prefix
- **WHEN** a pane's id starts with `3f9a` and no other pane or session id does, and the user runs `splitty attach 3f9a`
- **THEN** the CLI attaches to that pane's session

#### Scenario: Ambiguous prefix
- **WHEN** two pane ids start with `3f9a` and the user runs `splitty attach 3f9a`
- **THEN** the CLI prints an error listing both panes and exits with status 1 without attaching

#### Scenario: Pick from the list
- **WHEN** the user runs `splitty attach` without a target against a daemon with three terminal panes and answers `2`
- **THEN** the CLI lists the three panes numbered 1 to 3 with screen name, title and working directory, and attaches to the second

#### Scenario: Not the current pane by default
- **WHEN** the user runs `splitty attach` without a target inside a Splitty pane
- **THEN** the CLI shows the list instead of attaching to the pane it runs in

#### Scenario: Own session refused
- **WHEN** the target resolves to the session named by the command's own `SPLITTY_SESSION_ID`
- **THEN** the CLI prints `Error: cannot attach to the session this command runs in` and exits with status 1

#### Scenario: Process metadata contains terminal controls
- **WHEN** a pane title or working directory contains control or bidirectional formatting characters
- **THEN** the attach listing displays visible replacements and does not execute terminal instructions or forge extra rows
