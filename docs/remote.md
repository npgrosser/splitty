# Remote development

Splitty reaches other machines only over SSH. On the host it runs its own
daemon, bound to the host's loopback, which keeps screens, panes and
processes alive while your computer sleeps or disconnects. Your computer only
displays them. Nothing has to be installed on the host beforehand: the client
installs a matching Splitty server over the same SSH connection.

There are three ways to look at a host:

| Way | Where | Needs locally |
|---|---|---|
| [Remote screens](#remote-screens) | Screens of the host next to your local screens, in the same window | The local Splitty server (desktop app on macOS/Linux, the WSL backend on Windows, or `splitty serve`) |
| [Remote windows](#remote-windows) | A window of its own per host, like VS Code's remote windows | Only the desktop app; no local server and no WSL on Windows |
| [`splitty connect`](#splitty-connect) | Your browser | The `splitty` command |

Host keys are checked strictly in all three: connect to a new host once with
plain `ssh` and verify its fingerprint.

## Host requirements

- Linux or macOS, `x86_64` or `aarch64`, reachable with your system `ssh`
  (including whatever `~/.ssh/config` sets up: aliases, `ProxyCommand`, jump
  hosts, DevPod workspaces).
- `sh`, `tar`, `gzip` and `sha256sum` (on a Mac `shasum`), which every
  distribution, busybox and macOS have.
- Any user, including root.

The client installs its server to `~/.local/share/splitty/remote/<version>/`
on the host (checksum verified there; the host downloads nothing). The released
desktop apps and the static `splitty` from the release carry the servers for
x86_64 and aarch64 Linux hosts and for Macs. A build of your own needs a payload,
see [Installing Splitty on hosts](#installing-splitty-on-hosts).

## Remote screens

A window can show screens of other hosts next to its local ones: the arrow next to **New screen** in the drawer, the **+** on a host's line there, or **New screen on host…** in the command palette. The picker lists your recent hosts and the hosts of `~/.ssh/config` (DevPod's workspaces, for example) and accepts any `[user@]host[:port]`. The screen appears at once and shows inside it how far the connection got: the local server installs its server on the host when it lacks it, starts the host's daemon and relays it over SSH, as `splitty connect` does, but without a tunnel or a browser tab. Errors show in the same place, with what to do about them and a retry.

- A remote screen's tabs, splits, shells and working directories are the host's: `splitty split` in one of its panes splits it there. Your theme, font and keybindings apply; your local shell profiles do not, so a new tab there is a plain terminal of the host.
- In the drawer a remote screen looks like a local one. Local screens come first; each host with open screens follows under one quiet line with its name and connection dot (connected, reconnecting, offline), its forwarded ports and a **+** for another screen there. Hosts without open screens are not in the list but in **Hosts**. Screen shortcuts and the mouse wheel over the drawer (or the title bar) switch screens in that order; drag to reorder screens within a group. A dropped connection shows "reconnecting…" inside the host's screens and comes back with what was printed meanwhile; a host that stays unreachable for two minutes is shown offline, with a retry.
- **Closing** a remote screen (the × in the screen list) detaches it without asking: it leaves the window and everything keeps running on the host. A notice says so and offers **Reopen**. The trash button next to it **ends** the screen there, with its panes and their processes, and asks in the row the first time. Quitting, closing the window or reloading only detaches, and running processes on a host never hold up quitting.
- **Remembered:** the local server remembers the window's remote screens. After a reload or a restart of the app they are back in the screen list in their host groups, with a hollow dot until you connect: a remote screen connects only when you open it, or when its host is connected for another screen anyway, so an unreachable host does not slow the start. A remembered screen the host no longer has says so, with **Remove**.
- **Getting back:** **Hosts** at the bottom of the drawer (or a click on a host's line) lists every host you used, connected or not. Unfold one to connect and see its running screens; **Open** attaches to an existing screen, and **New screen** creates another on that host. Disconnected hosts show the last seen names and pane counts, saved by the local server across restarts. Reading this list never connects hosts at startup. Browsing a host keeps its connection until you fold it or close the panel; open screens also keep it connected. A screen that no longer exists is reported as missing, never recreated silently.
- **Updates:** a host whose Splitty is another version than yours says "update available" in its tooltip, and its entry under **Hosts** offers **Update…** while it is connected; new features such as pasting images need it. A host too old or too new to use shows **Update now…** in its screens. Both ask first, naming the versions and how many screens and panes end on the host (or that the count is not known, when that Splitty cannot report it). After you confirm, the host's Splitty stops and your version starts. A host daemon with [session keepers](cli.md#splitty-daemon) hands its sessions over to the new version; an older one ends its processes.
- On Windows the local server runs in WSL, so remote screens use WSL's `ssh`, keys and SSH configuration.

## Ports

Dev servers on the host are one click away, as in VS Code:

- A host's line in the screen list shows how many of its ports are forwarded; a click lists them with **Open**, and the ports its Splitty sessions listen on with **Forward and open**. A new listener brings no notice. Each host in **Hosts** lists its forwarded and saved ports, then the detected ones; every row has one action (Open, Forward, Resume or Retry) and a **⋯** menu with Stop, Remember or labels, and Forget. Inspecting ports does not connect the host. **Open** uses `http://localhost:<local port>` in your browser. A running forward survives detached screens and reconnects on the same port, showing “Reconnecting…” while being rebuilt.
- **Add port** inside a host fixes that host for the form; **Forward a port on another host…** at the end of **Hosts** works for any host, including when nothing was detected. Choose a known host or type an SSH target there, enter the remote port and optionally an exact local port. This also works for services started outside Splitty. **Forward** starts it; an empty local port uses the remote number when free or another free port for a new, unsaved mapping.
- **Remember** is opt-in. Give saved mappings optional labels such as Frontend and API. **Forward** with Remember checked saves the actual local port after opening; if saving fails, the forward stays running and the error says it was not remembered. **Save without starting** saves metadata only and never connects the host. Without a specified local port it saves the existing forward's port, or the remote number. Saved mappings remain visible after restart and restore only when you successfully connect their host; reading or editing them never starts SSH.
- **Stop** ends a running forward and keeps a saved rule **Paused**, across restarts and reconnects. **Resume**, **Retry**, or an explicit remote link click can start it again. **Forget** removes the saved rule and leaves a running forward running; use Stop to end it. Label edits never resume a paused rule.
- **Stable local ports:** a saved or explicitly selected port must be available on both supported loopback families. A conflict leaves the saved port unchanged and offers **Retry** or **Use another local port…**. The latter starts on the numeric port you choose and updates the saved mapping only after success. Stop a running mapping before changing its port. No conflicting listener is terminated.
- **Links in a host's panes:** a link to `localhost`, `127.0.0.1`, `0.0.0.0` or `[::1]` printed in a remote pane verifies or opens that host's forward before opening its local address, preserving the scheme, path, query and fragment. A saved-port conflict opens no browser URL. Other links, and every link in a local pane, open as they are. Forwarding carries HTTP, HTTPS and WebSockets unchanged and adds no Splitty credentials; listeners bind only to local `127.0.0.1`.
- **Forward new ports** (Settings → Remote, `remote.autoForwardPorts`) forwards new detected ports without a notice. It is off by default, since every program on your machine can reach a forwarded port. It never remembers a mapping, resumes a paused rule or silently changes a saved local port. Port lists refresh across windows while a host, mapping or the drawer is in use, and on window focus.
- A Mac host detects no listening ports yet; forward its ports by number.

## Remote windows

The desktop apps (the Tauri app on Windows, macOS and Linux, and the Electron app on Linux) open a host in a window of its own, without any local server: **New window** in the host's entry under **Hosts** in the drawer, **New window on a host…** under the arrow next to **New screen**, **Open host in new window…** in the command palette (Cmd+Shift+P, Alt+Shift+P on Linux and Windows), or **Connect to host…** on the Windows app's setup page. The picker lists your recent hosts and the hosts of `~/.ssh/config` (DevPod's workspaces, for example) and accepts any `[user@]host[:port]`. The app then does what `splitty connect` does: it installs its server on the host when it lacks it, starts the daemon there and holds the SSH tunnel. The window opens once the host's UI is ready; until then the picker shows the progress, or the error with what to do about it. Started from the Dock, the Finder or a desktop launcher, the Tauri app on macOS and Linux first takes `PATH` from your login shell (at most 5 seconds), so a `ProxyCommand` in `~/.ssh/config` that runs, say, `gcloud` from Homebrew works as it does in a terminal.

- One window per host: opening the host again focuses its window. The title names the host and says `(reconnecting…)` while the tunnel is rebuilt. After a restart of the host's daemon, the window loads a new session by itself.
- After 10 minutes without a tunnel, the title says `(disconnected)`; opening the host again reconnects the same window.
- Closing the window ends the tunnel, never the host's daemon or its sessions.
- The window shows code that comes from the host, so it gets what a web page in a separate browser profile gets: no native functions of the app, a cookie store of its own that is kept in memory only, no navigation away from the host (links open in your browser) and no pop-out windows. Its title bar is the system's. It cannot hand files or folders to your local editor.

**Windows without WSL.** Remote windows need no WSL: they use Windows OpenSSH (`ssh.exe`, `%USERPROFILE%\.ssh` and the Windows ssh-agent), so the Windows keys and SSH configuration apply, including the entries DevPod Desktop writes. On a machine without WSL, the setup page offers **Connect to host…** next to setting up WSL, and stays open as a launcher; the app quits when its last window closes. WSL remains the way to work locally. The Windows app keeps its recent hosts in its own configuration directory.

## `splitty connect`

Opens a host's Splitty in your browser through an SSH tunnel:

```bash
splitty connect user@remote-host
```

`connect` asks the remote `splitty` for its daemon (`splitty remote-handshake --json`), forwards the same port on your machine to the remote loopback address (bound to `127.0.0.1` only, whatever `GatewayPorts` says), verifies the tunnel, asks the remote host for a single-use launch code (`splitty issue-bootstrap`), and opens the UI in your browser. No long-lived credential crosses the wire. `--no-open` prints the URL (`SPLITTY_URL=…`) without opening a browser.

`--forward` forwards ports of the host as well, in the same SSH connection, for the dev servers and web APIs you run there:

```bash
# Ports 3000 and 5173 of the host on the same ports here, its port 80 on 8080
splitty connect user@remote-host --forward 3000,5173 --forward 8080:80
```

Each forward is bound to `127.0.0.1` here and reaches `localhost:<port>` on the host, so a dev server that listens only on IPv6 there works too. When the port is taken here (on `127.0.0.1` or `::1`), the forward uses any free port and `connect` says so. `connect` prints each one as `Forwarding http://localhost:<port> -> <host>:<port>` and keeps it on its port while it rebuilds a dropped tunnel. Open forwarded servers as `localhost`, as announced, not as `127.0.0.1`: Splitty's session cookie belongs to `127.0.0.1`, so a dev server opened as `localhost` never receives it.

`--json` prints one JSON object per line on standard output instead of text, for programs that run `connect` (the Electron shell does): `{"event":"progress","step":"install|daemon|started|tunnel","message":…}`, `notice` (`message`), `ready` (`url`, `port`), one `forward` per `--forward` port after it (`remote`, `local`, `url`), `lost` (`message`), `retry` (`message`, `retryIn` in seconds), `restored`, `url` (`url`, a new session after the remote daemon restarted), `failed` (`message`, and `hint` where there is an instruction) right before exit status 1 (or 2 for a target or `--forward` list it rejects), and `closed` right before exit status 0. Everything else, including opening the browser unless `--no-open`, stays the same.

- When no daemon runs on the host, `connect` starts one, on a port that is free on both ends. A daemon older than the client that keeps its sessions (one with session keepers) is updated on the way: `connect` stops it and starts its own version, which takes the sessions over. Any other daemon of another version keeps running; `connect` says so and how to update it.
- The local port must be free; if you run a local daemon on the same port, start the remote one with `--addr 127.0.0.1:<port>`.
- When the tunnel drops (sleep, network change), `connect` rebuilds it and the open tab reconnects. If the remote daemon was restarted meanwhile, it prints a new URL and opens it; after a reboot of the host it starts the daemon again (and reinstalls the server when a rebuilt container lost it). After 10 minutes without a tunnel it gives up with exit status 1; Ctrl+C ends it with 0.
- Every successful `connect` records its target as a recent host, in `~/.config/splitty/recent-hosts.json` (the last 20, most recent first).
- Use a separate browser profile for remote sessions, or a remote window of the desktop app, which has a cookie store of its own: browsers do not separate cookies by port, so every server on `127.0.0.1` sees the session cookies of the others.

## Installing Splitty on hosts

- On a Linux or macOS host without Splitty (or with another version), the client installs its own server first, over the same SSH connection, to `~/.local/share/splitty/remote/<version>/` (checksum verified on the host; nothing is downloaded there). It needs a payload: the released static `splitty` (`splitty-wsl-x86_64.tar.gz`) installs itself on x86_64 hosts and carries the servers for aarch64 hosts and for Macs (Apple silicon and Intel); any build takes one from `SPLITTY_REMOTE_PAYLOAD=<archive>`, an archive as `packaging/windows/build-wsl-payload.sh <repo> <out> [commit] [x86_64|aarch64|darwin-aarch64|darwin-x86_64]` builds it (on Linux, the macOS ones cross-built with a pinned zig that `packaging/zig-toolchain.sh` fetches; on a Mac, all four without zig).
- Without a payload, `splitty` must be in the `PATH` of non-interactive SSH sessions on the remote machine, which often comes from `~/.profile` rather than `~/.bashrc`; `ssh user@remote-host splitty --version` shows whether it is.
- On a Mac, the server `connect` or the hub installed keeps its own directory, `~/.config/splitty` (it runs with `SPLITTY_CONFIG_DIR` set to it): its daemon, sessions and token stay apart from a Splitty desktop app on that Mac, which keeps `~/Library/Application Support/splitty`. Updating the host from elsewhere never stops the app's sessions, and the app never attaches to that daemon.
