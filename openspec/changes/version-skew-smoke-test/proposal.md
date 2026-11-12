# Proposal

## Why

Clients, host daemons, keepers and the `splitty` command in a pane change version independently, and each fallback is tested only against a simulated peer (see "version skew" in `docs/issues/openspec-baseline-findings.md`). Nothing shows a break between two builds before a release.

Roadmap row 125, the second of three changes split from one package with `replay-terminal-modes` and `program-status`. It comes before `program-status`, whose capability and event reach a window from a host's daemon, across the first of the boundaries below.

## What Changes

- **A smoke test across versions**: it starts the last released `splitty` next to the current build and creates, attaches, resizes and re-attaches a session across each boundary, in both directions: a client and a host's daemon; a daemon and the keepers an earlier version started; the `splitty` command in a pane and the daemon behind its `SPLITTY_SOCK`; the files one build writes and the other reads (`state.json`, settings).
- **The released build**: fetched by version from the project's releases and checked against the release's `SHA256SUMS` before it runs. Without it the test is skipped at runtime and says why, the way PTY tests skip without a PTY.
- **When it runs**: before a release, and in CI on Linux.
- It is a smoke test because an in-process test links one version of every type.

Out of scope:
- The desktop window as a peer: the `splitty` command's own clients (its session stream, the CLI commands) stand in for a client.
- Builds for macOS and Windows, and versions older than the last release.
- Fixing a break the test finds; each becomes a finding or a change of its own.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. The change adds a test and the step that provides it a released build; no behavior changes, and `.openspec.yaml` sets `skip_specs: true`.

## Impact

- Tests: a new smoke test in `crates/splitty-server/tests/`, isolated like `daemon_lifecycle.rs` (own runtime directories, own sockets, no use of a `splitty` on `PATH`).
- CI and release: a step that fetches and verifies the released build, in `.github/workflows/ci.yml` and before a release.
- Findings: the "version skew" entry closes, or narrows to the boundaries the first version leaves out.

## Architecture impact

None: a test and a CI step, no component or relationship changes.
