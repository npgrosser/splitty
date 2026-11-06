## ADDED Requirements

### Requirement: Comments Command
`splitty comments [<document>] [--all] [--json]` SHALL print what an agent may read of the annotations on the viewer panes of the caller's screen (see `viewer-annotations`, Agents Read And Reply To Handed-Over Threads), or of one document. A document is a path, resolved against the current directory and following symbolic links, of a file that a viewer pane of the screen shows, or the ID of a viewer pane of the screen. The command SHALL print each document with its path or title and its viewer pane's ID, and under it each annotation with its number, lines, kind, quote, remark, replacement and replies, as Markdown in the form of the review, the open ones only unless `--all` is given; with `--json` it SHALL print the response of the comment endpoint (see `control-api`, Viewer Comment Requests). It SHALL exit with status 0, also when there is nothing to print, which it SHALL say. `splitty comments reply <document> <number> <text>` SHALL add `<text>`, or standard input when `<text>` is `-`, as an agent's reply to the annotation with that number, print `Replied to #<number> in <document>` and exit with status 0. The caller's pane and screen SHALL come from `--pane` and `--screen`, else from `SPLITTY_PANE_ID` and `SPLITTY_SCREEN_ID`. A document that no viewer pane of the screen shows, a path that several viewer panes of the screen show, an unknown number, an annotation the user has not handed over and a reply that breaks the limits SHALL fail with a message that names the cause, and exit status 1. A server that does not know the endpoints SHALL be reported as needing an update. Under `SPLITTY_AGENT_SOCK` both commands SHALL be refused (see Scoped Administrative Commands Are Refused).

#### Scenario: Read a sent review
- **WHEN** the user has sent a review of `/home/me/app/PLAN.md` to an agent's terminal and the agent runs `splitty comments PLAN.md` in `/home/me/app`
- **THEN** the command prints the handed-over annotations with their numbers, quotes, remarks and replies, and exits 0

#### Scenario: Reply
- **WHEN** the agent runs `splitty comments reply PLAN.md 2 "Switched to on-demand computation."`
- **THEN** annotation #2 holds that text as an agent's reply, and the command prints `Replied to #2 in /home/me/app/PLAN.md`

#### Scenario: Nothing sent yet
- **WHEN** the agent runs `splitty comments PLAN.md` before the user has handed anything over
- **THEN** the command says that there are no comments and exits 0

#### Scenario: Two viewers of one file
- **WHEN** two viewer panes of the screen show `PLAN.md` and the agent replies naming the path
- **THEN** the command fails with a message that names both viewer panes' IDs, and nothing changes

#### Scenario: Under a grant
- **WHEN** a process with `SPLITTY_AGENT_SOCK` set runs `splitty comments`
- **THEN** the command is refused

### Requirement: Review Command
`splitty review <path>` SHALL show a file as `splitty open <path>` does (see Open In A Viewer Pane), and `splitty review --text [--title <title>]` SHALL show the text read from standard input, at most 256 KiB, as a text document titled `<title>`, `Review` by default, with `replace` set (see `viewer-panes`, Reopening A File). The command SHALL then wait until the user answers in a window (see `viewer-annotations`, Waiting For A Review), and print to standard error which viewer pane it waits for. When the user sends a review, the command SHALL print the review to standard output and exit with status 0; when the user approves, it SHALL print nothing to standard output and exit with status 0. With `--json` it SHALL print `{"outcome":"feedback","review":<review>}` or `{"outcome":"approved"}` instead. When the viewer pane closes before an answer, when a process already waits for that pane, or when the connection to the server ends, the command SHALL say so and exit with status 1. Interrupting the command SHALL end its wait. The caller's pane and screen SHALL come from `--pane` and `--screen`, else from `SPLITTY_PANE_ID` and `SPLITTY_SCREEN_ID`. Under `SPLITTY_AGENT_SOCK` the command SHALL be refused.

#### Scenario: A plan reviewed
- **WHEN** an agent pipes its plan into `splitty review --text --title Plan` and the user comments on two passages and sends the review
- **THEN** the command prints the review with both comments and exits 0

#### Scenario: The next revision of the plan
- **WHEN** the agent pipes a revised plan into the same command from the same pane
- **THEN** the viewer pane of the first plan shows the revised text with its annotations, and the command waits again

#### Scenario: Approved
- **WHEN** the user approves the document
- **THEN** the command prints nothing to standard output and exits 0

#### Scenario: Outcome as JSON
- **WHEN** the command runs with `--json` and the user approves
- **THEN** it prints `{"outcome":"approved"}` and exits 0

#### Scenario: The pane closed
- **WHEN** the user closes the viewer pane while the command waits
- **THEN** the command says that the review was closed without an answer and exits 1
