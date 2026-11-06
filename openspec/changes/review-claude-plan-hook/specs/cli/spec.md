## ADDED Requirements

### Requirement: Review Hook For Claude Code
`splitty review --hook claude-code` SHALL serve as a command hook of Claude Code for the hook events `PermissionRequest` and `PreToolUse` of the tool `ExitPlanMode`. It SHALL read the event, at most 1 MiB of JSON, from standard input, and take the plan from the event's `tool_input.plan`, or else from the file that `tool_input.planFilePath` names when that is an absolute path of a regular file of at most 256 KiB. It SHALL show the plan and wait for the user's verdict as `splitty review --text --title Plan` does (see Review Command), and then print one JSON object for Claude Code and exit with status 0:
- on an approval without notes, a decision that allows the tool call;
- on an approval with notes, a decision that allows the tool call with the tool's input, whose `plan` is followed by a rule and the review;
- on a request for changes, a decision that denies the tool call with the review as its message.

The decision SHALL have the form the event asks for: for `PermissionRequest` a `hookSpecificOutput` with a `decision` of `behavior` `allow` or `deny`, the latter with a `message`; for `PreToolUse` a `hookSpecificOutput` with `permissionDecision` `allow`, carrying the tool's input as `updatedInput`, or `deny` with a `permissionDecisionReason`. The review in a decision SHALL have control characters other than tab and line feed, characters that change the direction of text, and invisible format characters replaced by U+FFFD. When the command cannot review, because it runs outside a Splitty terminal, the event is for another event or tool or holds no plan, the server cannot be reached or waits for no reviews, the viewer pane closes before an answer, or the connection ends, it SHALL print nothing to standard output, say why on standard error and exit with status 0, so that Claude Code asks the user itself. It SHALL NOT decide without the user's verdict.

#### Scenario: A plan approved
- **WHEN** Claude Code, running in a Splitty pane, calls `ExitPlanMode` with a plan and the hook `splitty review --hook claude-code` for `PermissionRequest`, and the user approves the plan in the viewer pane
- **THEN** the command prints `{"hookSpecificOutput":{"decision":{"behavior":"allow"},"hookEventName":"PermissionRequest"}}` and exits 0

#### Scenario: Changes requested
- **WHEN** the user comments on a step of the plan and requests changes
- **THEN** the command prints a decision with `behavior` `deny` and the review, which starts with its heading and `Verdict: changes requested`, as its `message`, and exits 0

#### Scenario: Approved with notes
- **WHEN** the user comments on a step and approves
- **THEN** the decision allows the call, and its `updatedInput` holds the event's tool input with the plan followed by a rule and the review

#### Scenario: Before the tool runs
- **WHEN** the hook runs for `PreToolUse` and the user approves without notes
- **THEN** the command prints a `hookSpecificOutput` with `permissionDecision` `allow` and the event's tool input as `updatedInput`

#### Scenario: Outside Splitty
- **WHEN** the hook runs in a terminal that is not a Splitty pane
- **THEN** the command prints nothing to standard output, says why on standard error and exits 0, and Claude Code shows its own dialog

#### Scenario: The plan in a file
- **WHEN** the event's `tool_input` holds no `plan` but a `planFilePath` of a Markdown file
- **THEN** the viewer pane shows that file's text as the plan
