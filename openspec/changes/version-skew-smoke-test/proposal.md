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
- SSH: `connect`, and a client installing or updating a host's daemon. The current build's native client through either build's `remote-bridge` stands in for the link to a host (VS-3).
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

## Decisions

- **VS-1** (2026-11-12, provisional, tbd): the released build is the static `splitty` in the release's `splitty-wsl-x86_64.tar.gz`, the server and CLI the Linux packages ship as their command; the desktop bundle would add a large download and a window the test does not drive.
- **VS-2** (2026-11-12, provisional, tbd): "before a release" is a job of the release workflow that the publish job waits for. The CI run of the release commit runs next to the release workflow, and its failure would not stop the release.
- **VS-3** (2026-11-12, provisional, tbd): the link to a host is covered without SSH: the current build's native client through the `remote-bridge` of either build. The older build stands in as a client only through its `splitty` command; `connect` and a client updating a host need the private SSH server and network namespace of `tests/remote_connect.rs`, which a later step can give a released build.
- **VS-4** (2026-11-12, provisional, tbd): the variables are `SPLITTY_TEST_RELEASED_BINARY` and `SPLITTY_TEST_REQUIRE_RELEASED_BINARY`, apart from `SPLITTY_TEST_PREVIOUS_BINARY` of the pinned rollback smoke in `tests/daemon_lifecycle.rs`, which also needs a pinned SHA-256 and Bubblewrap; reusing it would turn that test on in CI.
- **VS-5** (2026-11-12, provisional, tbd): the test does not assert that a setting only the newer build knows survives a downgrade, since today it does not (a finding under server-modes); it asserts that the reading build keeps the values it knows beside a key it does not.
