# Proposal

## Why

Claude Code asks for approval of a plan when it leaves plan mode (its tool `ExitPlanMode`), in a dialog inside its terminal. In a Splitty pane, the plan deserves the viewer: rendered Markdown, annotations on passages, an overall comment and a verdict (`review-verdict`). `splitty review` can already wait for that verdict, but nothing connects it to Claude Code's approval, so the agent has to be told to call it, and the dialog still follows. Claude Code runs command hooks around its tools and lets a hook decide a permission request; `splitty review` can be that hook.

## What Changes

- **`splitty review --hook claude-code`**: reads a hook event of Claude Code for the tool `ExitPlanMode` from standard input, shows the plan in a viewer pane next to the agent's pane as `splitty review --text --title Plan` does, waits for the verdict, and prints Claude Code's decision as JSON.
  - Approve allows the tool call; Claude Code leaves plan mode without its own dialog. Notes of an approval go along appended to the plan.
  - Request Changes denies the tool call with the review as its message; the agent stays in plan mode and revises the plan, and its next attempt opens the same viewer pane with the threads.
- **A safe fallback**: whenever the command cannot review (outside a Splitty terminal, without a plan, for another tool or event, an old server, a closed pane, a lost connection), it prints no decision and exits 0, so that Claude Code shows its own dialog. The hook never approves on its own.
- **Both hook events**: `PermissionRequest` (recommended: it runs only when Claude Code would ask) and `PreToolUse`.
- **Docs**: the hook configuration for Claude Code in `docs/files-and-editors.md`.

Out of scope:
- Installing the hook into Claude Code's settings; the docs show the entry to add.
- Hooks of other agents and other tools of Claude Code (its questions to the user).
- Setting Claude Code's permission mode after an approval.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `cli`: a new requirement, Review Hook For Claude Code.

## Impact

- `splitty review` (`cli/review.rs`, the command's arguments in `main.rs`).
- Tests: unit tests of the event and the decisions, the smoke test of `splitty review`.
- Docs: `docs/files-and-editors.md`, `README.md`.

## Architecture impact

None: the hook is a mode of `cli::review`, with no new module or relationship.

## Decisions

- **D1 The plan's source** (provisional, tbd): the plan is the event's `tool_input.plan`; without it, the file `tool_input.planFilePath` names, when that is an absolute path of a regular file of at most 256 KiB. Claude Code's documentation does not describe the tool's input, and its recent versions keep the plan in such a file. Alternative: read the plan file only.
- **D2 Notes of an approval** (provisional, tbd): an approval with notes allows the call with the plan replaced by the plan, a rule and the review, as the hook's `updatedInput`, so that the agent finds the notes in the approved plan. Alternative: drop the notes, or deny with them, which costs another round.
- **D3 Never decide on a failure**: every failure ends with no decision and status 0, so that Claude Code asks the user itself; a hook that cannot show the plan must not approve it, and a non-zero status would be reported as a hook error.
- **D4 Text sent to the agent**: the review in a decision has control, bidirectional and invisible format characters replaced, as the plain output of `splitty review` does.
