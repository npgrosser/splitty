# Managed terminal jobs

`splitty run` starts a program in a terminal owned by the Splitty server. You and an
agent can read the same output, and the program continues when the invoking CLI or
Claude Code exits. It requires a running Splitty server with `managed-jobs-v1` support.

## Start and observe

For a concise, version-matched guide, run `splitty agent guide`. It works offline,
including with an unavailable/revoked scoped socket, and makes no configuration or
credential reads. `splitty agent guide --json` returns `{version, guide}`. It is
local documentation, not proof that a server supports jobs or grants access.

Inside an ordinary Splitty pane:

```sh
splitty run --tab --background --name backend --cwd ./backend --json -- npm run dev
splitty jobs --json
splitty inspect <job-id> --json
splitty logs <job-id> --tail 100 --json
splitty logs <job-id> --since <nextCursor> --wait 5s --json
splitty stop <job-id> --json
splitty forget <job-id> --json
```

Use the `jobId` returned by `run`; the response also includes the screen, pane and
session IDs. A successful start means the program was spawned, not that its service
is ready. `inspect` reports `starting`, `running`, `stopping`, `exited`, `failed` or
`interrupted`, with an actual exit code or signal when known.

- Placement defaults to a **new screen**. `--tab` adds a tab in the source pane's
  group; `--split left|right|above|below` adds a split. All preserve your focus.
- Tab/split targeting uses explicit `--screen`/`--pane`, an agent grant's anchor,
  or the ordinary pane's environment. It never chooses the globally active pane.
  Floating anchors are not supported yet.
- Cwd defaults to the CLI's current directory. Relative paths are resolved there;
  the directory must exist on the server. Run the CLI on the host for remote jobs.
- Arguments after `--` are literal. There is no implicit login shell or startup
  command injection. Shell syntax is explicit: `-- sh -lc 'your pipeline'`.
  PATH and environment come from the server, not the invoking shell's virtualenv.
- `--request-id <UUID>` makes retries refer to one launch attempt. Transport errors
  after sending a launch report that ID and an unknown outcome. Retry with the same
  ID and arguments, or list existing jobs; do not blindly start a new copy.
- `input <job-id>` reads up to 16 KiB of UTF-8 from stdin, unchanged. It neither
  interprets backslash escapes nor appends Enter. For example:
  `printf 'hello\n' | splitty input <job-id> --json`.
  Input is a terminal byte stream: an I/O error can follow a written prefix. A full
  input buffer fails promptly instead of blocking revocation; input is not retried
  automatically. Use `run` for a tracked, idempotent process launch.

## Output and lifetime

Log reads do not consume or resize the terminal. Text mode strips terminal controls
incrementally, preserves printable Unicode and normalizes newlines; it is a log
transcript, not a reconstructed fullscreen TUI. `--format raw --json` returns exact
PTY bytes as base64. Raw and text cursors are not interchangeable.

The default read is the last 100 text lines, capped at 64 KiB. `--limit` allows at
most 256 KiB (text needs at least 4 bytes); `--tail` allows at most 10000 lines and
cannot be combined with `--since`. `--tail 0` obtains an end cursor. `--wait` accepts
seconds or milliseconds, up to 30 seconds. `hasMore` indicates another available
page; `lostBytes` reports output overwritten since a cursor. Each job retains bounded
raw and text rings, 1 MiB each by default, controlled by `SPLITTY_SCROLLBACK_SIZE` on
the server. `outputComplete` is separate from process exit, so keep reading final
bytes even after `state` becomes `exited`.

Stopping leaves the pane and output available. It sends TERM to the owned launch
process group, then KILL after a three-second grace period if needed, and confirms
termination. This does not contain deliberately daemonized/escaped processes.
`forget` removes an ended, fully drained job's pane/output; its request ID remains a
deduplication tombstone until that server incarnation ends.

A Claude restart leaves the job running. Closing Splitty itself leaves jobs running
only when its backend is a **daemon**. An embedded backend ends with its app. A daemon
restart (`daemon stop`, `daemon restart`, an update, a crash) leaves the job's process
running in its session keeper: the next daemon adopts it, the job stays `running` under
your control, and its retained output is readable again (cursors from before the
restart are invalid). Its agent grant does not survive. A job whose process ended with
the daemon (`daemon stop --end-sessions`, a reboot) is never replayed and gets no
replacement shell: it is `interrupted` and reports unavailable old output. Use a new
`run` to deliberately start work again.

## Give Claude access to its own jobs

Provision from your ordinary owner terminal, preferably inside the Splitty pane
beside which you want agent tabs/splits:

```sh
splitty agent create --name claude --json
splitty agent exec <agent-id> -- claude
```

The invoked CLI's directory is included in the client's PATH, so an agent launched
this way can use `splitty agent guide` by name even when you invoked `agent exec`
through an absolute CLI path. The selected grant remains its only control socket.

The grant remembers that pane as a placement anchor. Outside a Splitty pane you can
provide `--screen` and `--pane` explicitly, or leave the grant unanchored and create
new screens. After exiting Claude, run the same `agent exec` command to resume the
grant and find its jobs. Grant IDs and socket paths are not bearer credentials.

The restricted socket can start jobs, list/read/control its own jobs, and place them
beside its approved anchor. It cannot read or type into the anchor, inspect the whole
workspace, administer grants, mint credentials or reach settings/SSH APIs. Knowing a
pane ID, sharing a directory or choosing an owner label grants no authority.

From the ordinary owner terminal:

```sh
splitty agent list --json
splitty agent reclaim --job <job-id> --json
splitty agent assign <other-agent-id> --job <job-id> --json
splitty agent revoke <agent-id> --json
```

Reclaim before using an agent-owned terminal for privileged manual work. Transfer and
revocation invalidate queued access and wake pending log reads; the process keeps
running. All clients using one grant share its jobs. Grants expire with the server,
not with Claude; after a server restart create a new grant and explicitly assign any
retained job records you want it to inspect.

**Boundary:** these are server-enforced API scopes, not an OS sandbox. Unrestricted
processes of your Unix user can still use the owner socket or read its token file.
Jobs execute as the server user and do not inherit Claude's sandbox. Allowing a
generic `splitty run` is allowing its requested program to execute, not a read-only
permission. See [SECURITY.md](../SECURITY.md).

Suggested Claude project instruction:

> Use Splitty for visible terminals and background processes; first read `splitty agent guide`.

Add that sentence yourself to `CLAUDE.md`/`AGENTS.md` or the agent's initial prompt.
Splitty does not edit those files, install a skill or inject instructions into the
agent automatically. The CLI carries the workflow so every project need not copy it.

The integration does not automatically redirect Claude's built-in background tools.
MCP and automatic Claude Agent Teams adapters are follow-ups on the same API.

## Limits and platform support

New terminals and managed jobs can find the available CLI beside their backend via
PATH, including a version-specific Windows/WSL backend directory. Supported Bash/Zsh
integration preserves that entry after startup files reset PATH. `agent exec` also
makes its invoked CLI available by name to the launched client. These are child-only
environment changes: no `~/.local/bin` install or shell-config edits are made, and
existing terminals need reopening under the updated backend. A native GUI package
with no installed/bundled CLI still needs one installed; Splitty does not pretend
its GUI executable is a CLI. Other shells retain control of their own startup PATH.

There are at most 16 active/draining jobs per grant, 64 globally, 4096 retained job
records/tombstones per server incarnation, 64 live grants, and 8 pending readers per
grant/128 globally. Limits fail explicitly without evicting live jobs. CLI responses
are bounded to 16 MiB and have a 35-second transport deadline. JSON success/errors
occupy one stdout object; diagnostics go to stderr. Exit codes: 0 for successful
control, 1 for failed operations, 2 for usage errors. `inspect` itself exits 0 even
when the inspected program failed.

Linux and macOS use their local Unix server. Windows local processes run inside WSL;
native remote-only windows still need no WSL. An embedded Windows backend ends with
the app; the CLI does not change that hosting policy.
