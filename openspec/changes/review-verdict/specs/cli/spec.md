## MODIFIED Requirements

### Requirement: Review Command
`splitty review <path>` SHALL show a file as `splitty open <path>` does (see Open In A Viewer Pane), and `splitty review --text [--title <title>] [--format markdown|text]` SHALL show the text read from standard input, at most 256 KiB, as a text document titled `<title>`, `Review` by default, with `replace` set (see `viewer-panes`, Reopening A File), and with the `format` `markdown` unless `--format text` asks for plain text. The command SHALL then wait until the user answers in a window (see `viewer-annotations`, Waiting For A Review), and print to standard error which viewer pane it waits for. When the user approves, the command SHALL print the review to standard output when the approval carries one, nothing otherwise, and exit with status 0. When the user requests changes, it SHALL print the review to standard output and exit with status 3. With `--json` it SHALL print `{"outcome":"approved"}`, `{"outcome":"approved","review":<review>}` or `{"outcome":"changes_requested","review":<review>}` instead, with the same exit status. The wait SHALL be for the revision of the document the command showed. When the viewer pane closes before an answer, when a process already waits for that pane, which leaves the pane's document as it was, when another command's document took the pane before the wait began, or when the connection to the server ends, the command SHALL say so and exit with status 1. Interrupting the command SHALL end its wait. The caller's pane and screen SHALL come from `--pane` and `--screen`, else from `SPLITTY_PANE_ID` and `SPLITTY_SCREEN_ID`. Under `SPLITTY_AGENT_SOCK` the command SHALL be refused.

#### Scenario: A plan reviewed
- **WHEN** an agent pipes its plan into `splitty review --text --title Plan` and the user comments on two passages and requests changes
- **THEN** the command prints the review with its verdict and both comments and exits 3

#### Scenario: A plan rendered
- **WHEN** an agent pipes a Markdown plan into `splitty review --text --title Plan`
- **THEN** the viewer pane renders it as Markdown; with `--format text` it shows the plan as plain text

#### Scenario: The next revision of the plan
- **WHEN** the agent pipes a revised plan into the same command from the same pane
- **THEN** the viewer pane of the first plan shows the revised text with its annotations, and the command waits again

#### Scenario: Approved
- **WHEN** the user approves the document without notes
- **THEN** the command prints nothing to standard output and exits 0

#### Scenario: Approved with notes
- **WHEN** the user approves the document with a new comment
- **THEN** the command prints the review with the comment and exits 0

#### Scenario: Outcome as JSON
- **WHEN** the command runs with `--json` and the user approves without notes, or requests changes
- **THEN** it prints `{"outcome":"approved"}` and exits 0, or prints `{"outcome":"changes_requested","review":…}` and exits 3

#### Scenario: Two reviews at once
- **WHEN** two `splitty review --text --title Plan` commands run at once in one pane, and the second's text replaces the first's before the first command waits
- **THEN** the first command says that another document took the pane and exits 1, and the second waits for the review of its own text

#### Scenario: The pane closed
- **WHEN** the user closes the viewer pane while the command waits
- **THEN** the command says that the review was closed without an answer and exits 1
