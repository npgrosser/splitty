# The `splitty` command

One executable is the server, the daemon and the CLI. `splitty --help` lists
every public command. See [Remote development](remote.md) for host connections,
[Files and editors](files-and-editors.md) for `splitty open` and `splitty download`,
and [the jobs guide](cli-jobs.md) for managed jobs for agents.

## Server modes

| Mode | Started by | Sessions end when |
|------|------------|-------------------|
| **Embedded** | The app | The app ends |
| **Daemon** | `splitty daemon start` | You end them (`daemon stop --end-sessions`), a reboot, or a logout that kills user processes |

Workspace persistence is a property of the mode, not of a screen: a daemon
saves and restores its workspace in `state.json` in its configuration directory.
An embedded backend does not save its local workspace; settings and host
preferences are still persisted. The app owns its local embedded backend on
every platform and closes its sessions when it ends: on Linux and macOS the
backend runs inside the app's process, on Windows in WSL. A separately started
daemon remains independent of the app.

## `splitty daemon`

Run Splitty as a daemon. Layout and running sessions outlive disconnects, daemon restarts, updates and even a crash of the daemon: every session's program runs in a small keeper process of its own (`splitty session-keeper`), and a starting daemon takes the running keepers over, with their output.

```bash
splitty daemon start                          # background service on a private Unix socket
splitty daemon status                         # PID, endpoint and version; otherwise kept sessions
splitty daemon status --json                  # format-2 discovery, without credentials or a TCP port
splitty daemon restart                        # stop, then start this binary at the same endpoint
splitty daemon stop                           # stop the daemon; the sessions keep running
splitty daemon stop --end-sessions            # stop the daemon and end every session
```

So after installing a new version, `splitty daemon restart` updates without ending anything. While no daemon runs, `splitty attach` still shows a kept session, and the next daemon takes it back. Before downgrading to a version without keepers, run `splitty daemon stop --end-sessions`: an older daemon would start fresh shells and leave the keepers running. A reboot, a macOS logout or the end of a container still ends the processes; the workspace is restored as before. On Linux with systemd-logind's `KillUserProcesses=yes`, a logout ends daemon and keepers alike unless lingering is on (`loginctl enable-linger`).

`daemon start` reports `SPLITTY_CLIENT_SOCKET` and `SPLITTY_INSTANCE` only after native negotiation succeeds. If another process holds the instance lock before discovery is published, it exits with temporary-failure code 75. Native connection setup refreshes discovery within its existing retry limit; it never takes over the lock. The endpoint directory must be private and on a local Unix filesystem; unsafe paths fail without a TCP fallback. A live legacy daemon is reported as legacy. A legacy daemon whose sessions cannot survive a restart requires a deliberate `daemon restart --end-sessions`.

Corrupt or unreadable discovery is an explicit error, including for `daemon status --json`; it is never treated as permission to create a replacement workspace. Dead records are removed only while holding the native instance lock and after rereading the same record. A starting or running daemon's publication cannot be deleted by a stale status/stop request. A new start replaces stale publication only after acquiring that lock.

### Internal backend

On Windows the app starts `desktop-backend --transport native-v1 --backend embedded --owner` in WSL. Its stdout carries native frames, and `--owner` keeps the service across window clients. On Linux and macOS the app runs the service in its own process. Native daemon clients use the private client socket; the owner control socket carries CLI operations with HTTP framing over Unix sockets.

## `splitty attach`

Show a session of the daemon in the terminal you are in, for example over a plain SSH login, without the UI:

```bash
splitty attach                 # pick a pane from a numbered list
splitty attach 3f9a            # a pane or session id, or a unique prefix (4+ characters)
ssh -t me@192.0.2.10 splitty attach  # from another machine
```

From another machine, this needs `splitty` on the host's `PATH`. The copy Splitty installs on a host is not on it: it lives in `~/.local/share/splitty/remote/<version>/`, and that path changes with every update.

What you type goes to the session, and its output comes back; windows that show the same session keep showing it. **Ctrl+\\** detaches (`--detach-key ctrl-]` picks another key) and leaves the session running. When its process exits, `attach` prints the exit code and ends. Attaching takes over the PTY size, as the size badge in a window does, and the window gets it back once you detach and type there.

`attach` verifies the private client endpoint and server instance recorded in `daemon.json`. Without a daemon it controls a session's keeper directly, and a daemon started meanwhile takes over without the terminal noticing. It needs a terminal (`ssh -t`), runs on Linux and macOS, and is refused under an agent grant.

## Commands inside a pane

Every shell spawned by Splitty knows its pane and screen (`SPLITTY_PANE_ID`, `SPLITTY_SCREEN_ID`) and the path of a local control socket (`SPLITTY_SOCK`); it never receives a credential. Run these from inside any Splitty pane:

```bash
splitty split                # split below
splitty split -H             # split right
splitty close-pane           # close current pane
splitty new-screen --name dev
splitty list-panes
splitty list-screens
splitty send-keys --pane <id> "make test\n"
```

`list-panes` prints one tab-separated line per pane: its id, title, working directory, `session=<id>` and `status=` with the program status the pane's program last reported through OSC 7501, as Claude Code and Codex do: `working`, `blocked` with its kind, `done`, `error` or `idle`, and `-` without one. Control characters in a cell show as `�`.

```text
3f9a…	claude	/home/me/project	session=8c21…	status=blocked:permission
```

With `--json` each pane also carries its session's record as `programStatus` (`state`, `kind`, `progress`, `app`, `title`, `msg`, `updatedAt`; see `docs/protocol.ts`), absent without one.

## Visible background jobs for agents

`splitty agent guide` prints the installed CLI's short agent workflow guide, even
without a running server (`--json` is also supported). A single project instruction
is enough to point a coding agent at it: **Use Splitty for visible terminals and
background processes; first read `splitty agent guide`.** No separate skill is needed.

```sh
splitty run --tab --background --name backend --cwd ./backend --json -- npm run dev
splitty jobs --json
splitty logs <job-id> --tail 100 --json
splitty stop <job-id> --json
```

The Splitty server owns these processes: they keep running when Claude Code restarts,
and both you and the agent can read their output. Provision own-job access with
`splitty agent create --name claude --json`, then launch Claude with
`splitty agent exec <agent-id> -- claude`. See the [managed jobs guide](cli-jobs.md)
for cursors, reconnects, permissions and daemon-versus-app lifetime.

## Upgrading command-line usage

`splitty serve`, `splitty connect` and `splitty issue-bootstrap` are removed. Their temporary hidden stubs exit with status 2 and point to `splitty daemon start` for persistent local work, the desktop app for hosts, and `splitty attach` (also `ssh -t me@192.0.2.10 splitty attach`) for terminal access. The stubs remain until the next minor release after this change ships. `serve --desktop-stdio` points to `desktop-backend --transport native-v1 --backend embedded`.

The `--addr` and `--token` options are usage errors, and `SPLITTY_ADDR` has no effect. An existing `auth-token` file in the configuration directory is unused and left untouched; it can be deleted.

Details, guarantees and limits are in [`SECURITY.md`](../SECURITY.md).
