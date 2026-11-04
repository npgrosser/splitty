# Remote development

Splitty reaches other machines only over SSH. On the host it runs its own
daemon, reached through a private Unix socket, which keeps screens, panes and
processes alive while your computer sleeps or disconnects. Your computer only
displays them. Nothing has to be installed on the host beforehand: the client
installs a matching Splitty server through SSH.

There are three ways to look at a host:

| Way | Where | Needs locally |
|---|---|---|
| [Remote screens](#remote-screens) | Screens of the host next to your local screens, in the same window | The local Splitty server (desktop app on macOS/Linux, the WSL backend on Windows, or `splitty serve`) |
| [Remote windows](#remote-windows) | A window of its own per host, like VS Code's remote windows | The desktop app and SSH; the remote window connects directly to the host, without WSL on Windows |
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
- **Updates:** the host menu offers an update for another server version. Eligible older native daemons update through their verified service identity. For a compatible legacy Linux daemon, the app offers **Update server and connect** after checking its process identity and session keepers. Confirming applies to one attempt; ordinary Retry never migrates a legacy daemon. The dialog names both versions and explains that keeper-managed sessions continue while clients reconnect. If migration cannot be verified, including for legacy macOS daemons, the app explains the reason and leaves the daemon running. Newer daemons and daemons without session keepers are not replaced by this flow.
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

### Sharing a local port with a host

The other direction: a service on this machine (a local API, a database, a model server) reachable from programs on the host, for example a dev VM.

- Each host in **Hosts** has **Shared with this host** and **Share a local port**. Enter the local port and, optionally, the port it gets on the host (empty: the same number). Nothing is shared without that click; detected ports, links and reconnects never start a share.
- On the host, the port listens on `127.0.0.1` and `::1` only, and Splitty accepts a connection only from a program running as the user you log in as. Other users on the host, and other machines, cannot reach it, whatever the server's `GatewayPorts` says. Programs running as you on the host can, and most local services ask for no password: share only what you would let those programs use. The form says so, and warns more strongly for ports usually used by SSH, Docker, Kubernetes, Remote Desktop or VNC.
- Only `localhost` of this machine is shared, one port per share, never another machine of your network. Every active Splitty browser gateway in the supported local reservation domain is refused, including gateways from another configuration directory. A gateway also refuses a port already reserved for sharing.
- Linux hosts only for now: the owner check reads `/proc` on the host. A Mac host answers “Not supported on this host”. A port already in use on the host fails with “Port in use on the host”; nothing on the host is stopped.
- **Stop**, **Remember**, **Resume** and **Forget** work as for forwarded ports. A remembered share starts again only when its host connects; **Start** on a remembered share that is not running starts it now. A lost connection frees the host's port within about a minute, and a running share is rebuilt on the same port.
- Reverse sharing is unavailable on Windows and WSL: their reachable localhost services have no shared Splitty reservation authority. It fails before starting SSH, including with mirrored networking. Forwarding a host service to this machine remains available.
- On Linux, reservation coordination requires the initial PID and user namespaces and a verifiable common root mount view. Unsupported or ambiguous namespace environments fail with an explicit error. The reservation registry coordinates Splitty processes of the same OS account across configuration directories; it does not restrict other forwarding tools or processes running as that user.

## Remote windows

The desktop apps (the Tauri app on Windows, macOS and Linux, and the Electron app on Linux) open a host in a window of its own, without using the app's local terminal backend: **New window** in the host's entry under **Hosts** in the drawer, **New window on a host…** under the arrow next to **New screen**, **Open host in new window…** in the command palette (Cmd+Shift+P, Alt+Shift+P on Linux and Windows), or **Connect to host…** on the Windows app's setup page. The picker lists your recent hosts and the hosts of `~/.ssh/config` (DevPod's workspaces, for example) and accepts any `[user@]host[:port]`. The app then does what `splitty connect` does: it installs its server on the host when it lacks it, starts the daemon there and holds a native SSH data connection. The window uses the packaged UI and appears once the host connection is ready; until then the picker shows the progress, or the error with what to do about it. Started from the Dock, the Finder or a desktop launcher, the Tauri app on macOS and Linux first takes `PATH` from your login shell (at most 5 seconds), so a `ProxyCommand` in `~/.ssh/config` that runs, say, `gcloud` from Homebrew works as it does in a terminal.

- One window per host: opening the host again focuses its window. The title names the host and says `(reconnecting…)` while the connection is rebuilt. After a restart of the host's daemon, the same document reconnects and reattaches its sessions.
- After 10 minutes without a connection, the title says `(disconnected)`; opening the host again reconnects the same window.
- Closing the window ends its SSH connection, never the host's daemon or its sessions.
- The window loads packaged UI and can connect only to its assigned host. It has private ephemeral storage, no local-backend or editor/reveal access, and no pop-out windows. Web links open in your browser; its title bar is the system's. Host data never supplies executable UI.

**Windows without WSL.** Remote windows need no WSL: they use Windows OpenSSH (`ssh.exe`, `%USERPROFILE%\.ssh` and the Windows ssh-agent), so the Windows keys and SSH configuration apply, including the entries DevPod Desktop writes. On a machine without WSL, the setup page offers **Connect to host…** next to setting up WSL, and stays open as a launcher; the app quits when its last window closes. WSL remains the way to work locally. The Windows app keeps its recent hosts in its own configuration directory.

## `splitty connect`

Opens a local browser gateway connected to one host over SSH:

```bash
splitty connect user@remote-host
```

The command installs or locates the host's server, asks for format-2 discovery
(`remote-handshake --json --ensure-daemon`), and opens a native bridge over SSH
stdin/stdout to its private daemon socket. The host opens no terminal-service
TCP listener. Your machine owns the browser listener, UI assets and one-use
launch code; browser credentials are created locally and never sent to the
host. `--no-open` prints the URL (`SPLITTY_URL=…`) without opening a browser.

The gateway chooses a free local port independently of the host. Its address,
cookie name and browser session store stay the same while SSH reconnects or the
host daemon is replaced. Existing tabs reconnect through that gateway. No new
URL or host-issued browser credential is needed.

`--forward` additionally opens the requested development ports through separate
owned SSH connections:

```bash
# Ports 3000 and 5173 of the host on the same ports here, its port 80 on 8080
splitty connect user@remote-host --forward 3000,5173 --forward 8080:80
```

Each forward binds `127.0.0.1` here and reaches `localhost:<port>` on the host,
including an IPv6-only host service. An occupied requested port gets a free
alternate; `connect` announces the actual mapping as
`Forwarding http://localhost:<port> -> <host>:<port>` and rebuilds it on that
port after a drop. Open forwarded servers as `localhost`, as announced; the
canonical gateway launch URL uses `127.0.0.1`. No Splitty credentials are added
to forwarded application traffic. Ctrl+C ends the gateway and its requested
forwards while the host daemon and sessions keep running.

`--json` prints one JSON object per line instead of text: `progress`
(`step`: `install|daemon|started|tunnel`, `message`), `notice` (`message`),
`ready` (`url`, `port`), one `forward` after it for each requested mapping
(`remote`, `local`, `url`), `lost` (`message`), `retry` (`message`, `retryIn`
in seconds), `restored`, `failed` (`message`, optional `hint`) and `closed`.
Daemon replacement reports `restored` with the original gateway URL. A failure
exits with 1, rejected arguments with 2, and Ctrl+C with 0. `--no-open` controls
browser launch in both output modes.

- If no daemon runs, the command starts one at the host's private endpoint.
  An older daemon can be updated automatically only when it has keepers, a
  verifiable native identity and the matching installed replacement. The stop
  request names that exact instance. Newer or non-kept daemons remain running
  and require explicit migration on the host.
  Versions follow SemVer precedence, so `0.2.0-dev.10` can replace
  `0.2.0-dev.9`, and `0.2.0` can replace either. Build metadata alone does not
  trigger replacement; an invalid version never authorizes an automatic update.
- Network loss retries with bounded backoff for at most ten minutes. Host-key,
  authentication and permanent protocol/installation errors require action.
  A rebuilt container can reinstall the server; a restarted host can start its
  daemon again, without changing the local browser origin.
- Successful connections record the target in
  `~/.config/splitty/recent-hosts.json` (the last 20, most recent first).
- Gateways have separate cookie names and credential stores. Browser cookies
  are still scoped by hostname rather than port; this does not isolate them
  from every other HTTP service on the same hostname.

## Installing Splitty on hosts

- On a Linux or macOS host without Splitty (or with another version), the client installs its own server first, through SSH, to `~/.local/share/splitty/remote/<version>/` (checksum verified on the host; nothing is downloaded there). It needs a payload: the released static `splitty` (`splitty-wsl-x86_64.tar.gz`) installs itself on x86_64 hosts and carries the servers for aarch64 hosts and for Macs (Apple silicon and Intel); any build takes one from `SPLITTY_REMOTE_PAYLOAD=<archive>`, an archive as `packaging/windows/build-wsl-payload.sh <repo> <out> [commit] [x86_64|aarch64|darwin-aarch64|darwin-x86_64]` builds it (on Linux, the macOS ones cross-built with a pinned zig that `packaging/zig-toolchain.sh` fetches; on a Mac, all four without zig).
- Without a payload, `splitty` must be in the `PATH` of non-interactive SSH sessions on the remote machine, which often comes from `~/.profile` rather than `~/.bashrc`; `ssh user@remote-host splitty --version` shows whether it is.
- On a Mac, the server `connect` or the hub installed keeps its own directory, `~/.config/splitty` (it runs with `SPLITTY_CONFIG_DIR` set to it): its daemon, sessions and private endpoints stay apart from a Splitty desktop app on that Mac, which keeps `~/Library/Application Support/splitty`. Updating the host from elsewhere never stops the app's sessions, and the app never attaches to that daemon.
