# The `splitty` command

One executable is the server, the daemon and the CLI. `splitty --help` lists
every command; this page covers the ones you use by hand. Remote access
(`splitty connect`) is described in [Remote development](remote.md#splitty-connect),
`splitty open` and `splitty download` in [Files and editors](files-and-editors.md),
and the managed jobs for agents in [the jobs guide](cli-jobs.md).

## Server modes

| Mode | Started by | Sessions end when |
|------|------------|-------------------|
| **Embedded** | The desktop app, `splitty serve` | The app or `serve` process ends |
| **Daemon** | `splitty daemon start` | You end them (`daemon stop --end-sessions`), a reboot, or a logout that kills user processes |

Persistence is a property of the mode, not of a screen: a daemon saves and
restores its whole workspace (`~/.config/splitty/state.json`), an embedded
server keeps nothing. The desktop app owns its local embedded backend on every
platform and closes its sessions when the app ends. On Windows that backend runs
in WSL. A separately started daemon remains independent of the app.

Native daemon clients use a private Unix socket, and desktop clients use native
IPC or inherited child pipes. Browser access is explicit: `serve` and `connect`
open a loopback browser gateway. Remote native connections travel through SSH;
a daemon itself opens no TCP listener.

## `splitty serve`

Serves the backend and the UI together, for a browser. Like [ttyd](https://github.com/tsl0922/ttyd) but with Splitty's full UI.

```bash
splitty serve                           # listen on 127.0.0.1:9800
splitty serve --addr 127.0.0.1:8080     # another loopback port
splitty serve --open                    # open the browser with a single-use launch code
splitty serve --daemon --open           # browser view of an existing daemon
splitty serve --oneshot-token           # random process token, not persisted to disk
SPLITTY_ADDR=127.0.0.1:8080 splitty serve
```

On startup `serve` prints `SPLITTY_PORT`, `SPLITTY_BOOTSTRAP` and `SPLITTY_URL`, using the port actually bound. Opening the launch URL exchanges its single-use code for the gateway's HttpOnly cookie. The code is useless afterwards. Listeners accept canonical `127.0.0.1` or `::1` addresses, including a localhost name resolving to one of those addresses; other addresses are refused. For IPv6, use `--addr '[::1]:8080'`. The UI can be installed as a PWA from the browser.

Plain `serve` owns its embedded workspace; `serve --daemon` instead opens independent native client connections to the existing daemon and never creates another workspace. Every tab has its own attachment, file/editor state and lifetime. The gateway serves local assets and terminates browser credentials itself. Its `splitty-session-<instance>` cookie name and one-use credentials are distinct from those of other gateways. Cookies are scoped by hostname, not port; use the advertised numeric loopback URL, and open forwarded development servers through their advertised `localhost` URLs. This separation does not isolate a gateway from every other service on the same hostname. Host connections opened from the main workspace retain the same SSH setup progress and fixed host assignment.

`serve --desktop-stdio` is refused. Desktop launchers use the internal `desktop-backend --transport native-v1 --backend embedded` pipe protocol, whose stdout carries frames rather than text launch lines. Electron and the Windows WSL launcher keep their service in `--owner` mode across window clients. Tauri on Linux and macOS uses the in-process service directly. The SSH helper uses `--backend ssh --target <host>` and detaches on pipe closure. The launcher's `--remember-target` flag marks an explicit successful host open; automatic reconnects preserve removed hosts. These internal modes accept no browser address or token.

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

`daemon start` reports `SPLITTY_CLIENT_SOCKET` and `SPLITTY_INSTANCE` only after native negotiation succeeds. The endpoint directory must be private and on a local Unix filesystem; unsafe paths fail without a TCP fallback. A live legacy daemon is reported as legacy. A legacy daemon whose sessions cannot survive a restart requires a deliberate `daemon restart --end-sessions`.

Corrupt or unreadable discovery is an explicit error, including for `daemon status --json`; it is never treated as permission to create a replacement workspace. Dead records are removed only while holding the native instance lock and after rereading the same record. A starting or running daemon's publication cannot be deleted by a stale status/stop request. A new start replaces stale publication only after acquiring that lock.

`daemon start --addr` and `--token` are usage errors. For a browser, use `splitty serve --daemon --addr 127.0.0.1:8080 --open`; stopping that gateway leaves the daemon and its sessions running. The desktop app continues to own its separate local workspace.

## `splitty attach`

Show a session of the daemon in the terminal you are in, for example over a plain SSH login, without the UI:

```bash
splitty attach                 # pick a pane from a numbered list
splitty attach 3f9a            # a pane or session id, or a unique prefix (4+ characters)
ssh -t user@vm splitty attach  # from another machine
```

From another machine, this needs `splitty` on the host's `PATH`. The copy Splitty installs on a host is not on it: it lives in `~/.local/share/splitty/remote/<version>/`, and that path changes with every update.

What you type goes to the session, and its output comes back; windows that show the same session keep showing it. **Ctrl+\\** detaches (`--detach-key ctrl-]` picks another key) and leaves the session running. When its process exits, `attach` prints the exit code and ends. Attaching takes over the PTY size, as the size badge in a window does, and the window gets it back once you detach and type there.

`attach` verifies the private client endpoint and server instance recorded in `daemon.json`, without reading a token file. An explicitly named browser gateway remains available with `--addr` and `--token`. Without a daemon it controls a session's keeper directly, and a daemon started meanwhile takes over without the terminal noticing. A plain `splitty serve` instance is reachable through that explicit gateway path. The desktop app exposes no implicit browser endpoint. It needs a terminal (`ssh -t`), runs on Linux and macOS, and is refused under an agent grant.

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

1. The browser launcher (`serve --open`, `connect`) opens a URL with a **single-use launch code** that expires after two minutes.
2. The page exchanges it for an **`HttpOnly` session cookie** and removes the code from the address bar.
3. Each WebSocket connection uses a **single-use ticket** valid for ten seconds.

Explicit HTTP CLI callers authenticate with the **process token** from `~/.config/splitty/auth-token` (created on first run, mode `0600`), or with `--token`. If you open the UI without a launch code, Splitty asks for that token once and turns it into a session. To rotate a file-backed token, stop the gateways using it, delete the file and restart them. `connect` uses a random process-local token and never reads this file. `splitty issue-bootstrap --addr 127.0.0.1:9800` mints a fresh launch code for an explicit gateway. Native desktop and daemon connections use no browser cookies, tokens or launch codes.

Details, guarantees and limits are in [`SECURITY.md`](../SECURITY.md).
