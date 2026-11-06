## 1. Command

- [x] 1.1 `--hook claude-code` on `splitty review`, without a path, `--text`, `--title`, `--format` or `--json`
- [x] 1.2 The event read from standard input: the event and tool, the plan from `tool_input.plan` or `tool_input.planFilePath`, at most 1 MiB of JSON and 256 KiB of plan
- [x] 1.3 The review of the plan as `splitty review --text --title Plan` shows it, in the event's `cwd`; the wait shared with the plain command
- [x] 1.4 The decisions per event and verdict, the notes of an approval appended to the plan, the review's unsafe characters replaced
- [x] 1.5 No decision and status 0 for every failure, with the reason on standard error
- [x] 1.6 Unit tests (extend `cli/review.rs`): events of both kinds, another tool or event, a missing plan, the plan from a file, oversized input, and each decision (cli, Review Hook For Claude Code: A plan approved, Changes requested, Approved with notes, Before the tool runs, The plan in a file)
- [x] 1.7 Smoke test (extend the review case of `tests/http_surface.rs`): the real command with a `PermissionRequest` event, answered with a request for changes, prints the denial and exits 0; run without a pane it prints nothing and exits 0, which only the binary's exit path shows (cli, Review Hook For Claude Code: Changes requested, Outside Splitty)

## 2. Docs

- [x] 2.1 `docs/files-and-editors.md`: the hook entry for Claude Code's settings, what each verdict does, the fallback
- [x] 2.2 `README.md`: the hook in one line where `splitty review` is named
- [x] 2.3 `openspec/open-decisions.md`: bullets for D1 and D2
- [x] 2.4 The agent guide: a plan reviewed by the hook needs no `splitty review`, and a refusal of `ExitPlanMode` carries the review

## 3. Checks

- [x] 3.1 Targeted tests: `cargo test -p splitty-server review`, `cargo test -p splitty-server --test http_surface review`
- [x] 3.2 Full check: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, `openspec validate --specs`
