# The `splitty` command

One executable is the server, the daemon and the CLI. `splitty --help` lists
every command; this page covers the ones you use by hand. Remote access
(`splitty connect`) is described in [Remote development](remote.md#splitty-connect),
`splitty open` and `splitty download` in [Files and editors](files-and-editors.md),
and the managed jobs for agents in [the jobs guide](cli-jobs.md).

## Server modes

| Mode | Started by | Sessions end when |
|------|------------|-------------------|
| **Embedded** | The desktop app (when no daemon runs), `splitty serve` | The app or `serve` process ends |
| **Daemon** | `splitty daemon start` | You end them (`daemon stop --end-sessions`), a reboot, or a logout that kills user processes |

Persistence is a property of the mode, not of a screen: a daemon saves and
restores its whole workspace (`~/.config/splitty/state.json`), an embedded
server keeps nothing. On macOS and Linux the desktop app uses a running daemon
instead of starting its own server; on Windows the app always runs its own
backend in WSL, so its local sessions end with the app.

The server only ever listens on `127.0.0.1`. Other machines are reached over
SSH, never by exposing the server to the network.

## `splitty serve`

Serves the backend and the UI together, for a browser. Like [ttyd](https://github.com/tsl0922/ttyd) but with Splitty's full UI.

```bash
splitty serve                           # listen on 127.0.0.1:9800
splitty serve --addr 127.0.0.1:8080     # another loopback port
splitty serve --open                    # open the browser with a single-use launch code
splitty serve --oneshot-token           # random process token, not persisted to disk
SPLITTY_ADDR=127.0.0.1:8080 splitty serve
```

On startup `serve` prints `SPLITTY_URL`, a launch URL with a single-use code. Opening it signs the browser in; the code is useless afterwards. A non-loopback `--addr` is refused. The UI can be installed as a PWA from the browser.

## `splitty daemon`

Run Splitty as a daemon. Layout and running sessions outlive disconnects, daemon restarts, updates and even a crash of the daemon: every session's program runs in a small keeper process of its own (`splitty session-keeper`), and a starting daemon takes the running keepers over, with their output.

```bash
splitty daemon start                          # start daemon (background, 127.0.0.1:9800)
splitty daemon start --addr 127.0.0.1:8080    # custom port
splitty daemon status                         # show PID, port, uptime; without a daemon, the kept sessions
splitty daemon restart                        # the update: stop, then start this binary on the same port
splitty daemon stop                           # stop the daemon; the sessions keep running
splitty daemon stop --end-sessions            # stop the daemon and end every session
```

So after installing a new version, `splitty daemon restart` updates without ending anything. While no daemon runs, `splitty attach` still shows a kept session, and the next daemon takes it back. Before downgrading to a version without keepers, run `splitty daemon stop --end-sessions`: an older daemon would start fresh shells and leave the keepers running. A reboot, a macOS logout or the end of a container still ends the processes; the workspace is restored as before. On Linux with systemd-logind's `KillUserProcesses=yes`, a logout ends daemon and keepers alike unless lingering is on (`loginctl enable-linger`).

When a daemon is running, the desktop app on macOS and Linux connects to it instead of starting an embedded server, whose sessions end with the app.

## `splitty attach`

Show a session of the daemon in the terminal you are in, for example over a plain SSH login, without the UI:

```bash
splitty attach                 # pick a pane from a numbered list
splitty attach 3f9a            # a pane or session id, or a unique prefix (4+ characters)
ssh -t user@vm splitty attach  # from another machine
```

From another machine, this needs `splitty` on the host's `PATH`. The copy Splitty installs on a host is not on it: it lives in `~/.local/share/splitty/remote/<version>/`, and that path changes with every update.

What you type goes to the session, and its output comes back; windows that show the same session keep showing it. **Ctrl+\\** detaches (`--detach-key ctrl-]` picks another key) and leaves the session running. When its process exits, `attach` prints the exit code and ends. Attaching takes over the PTY size, as the size badge in a window does, and the window gets it back once you detach and type there.

`attach` talks to the daemon recorded in `daemon.json`, with the token file of the machine it runs on, or to a server named by `--addr` and `--token`. Without a daemon it controls a session's keeper directly, and a daemon started meanwhile takes over without the terminal noticing. An embedded server (`splitty serve`, the desktop app without a daemon) is reachable only with `--addr` and `--token`. It needs a terminal (`ssh -t`), runs on Linux and macOS, and is refused under an agent grant.

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

## Signing in

The browser never holds a long-lived secret:

1. The launcher (desktop app, `serve --open`, `connect`) opens a URL with a **single-use launch code** that expires after two minutes.
2. The page exchanges it for an **`HttpOnly` session cookie** and removes the code from the address bar.
3. Each WebSocket connection uses a **single-use ticket** valid for ten seconds.

The CLI and the launchers authenticate with the **process token** from `~/.config/splitty/auth-token` (created on first run, mode `0600`), or with `--token`. If you open the UI without a launch code, Splitty asks for that token once and turns it into a session. To rotate it, quit Splitty, delete the file and start again. `splitty issue-bootstrap` mints a fresh launch code for a running server.

Details, guarantees and limits are in [`SECURITY.md`](../SECURITY.md).
