# Remote development

Splitty reaches other machines only over SSH. On the host it runs its own
daemon, reached through a private Unix socket, which keeps screens, panes and
processes alive while your computer sleeps or disconnects. Your computer only
displays them. Nothing has to be installed on the host beforehand: the client
installs a matching Splitty server through SSH.

The [connection diagrams](networking.md) show the local/remote process boundaries,
where listeners exist and which sessions survive a disconnect.

There are two ways to look at a host:

| Way | Where | Needs locally |
|---|---|---|
| [Remote screens](#remote-screens) | Screens of the host next to your local screens, in the same window | The app, or `splitty serve` for the [browser UI](cli.md#browser-ui) |
| [`splitty connect`](#splitty-connect) | Your browser | The `splitty` command |

Host keys are checked strictly in both: connect to a new host once with plain
`ssh` and verify its fingerprint. The Tauri shell built from source can also
open a host in a window of its own
([development](development.md#the-tauri-shell)).

## Host requirements

- Linux or macOS, `x86_64` or `aarch64`, reachable with your system `ssh`
  (including whatever `~/.ssh/config` sets up: aliases, `ProxyCommand`, jump
  hosts, DevPod workspaces).
- `sh`, `tar`, `gzip` and `sha256sum` (on a Mac `shasum`), which every
  distribution, busybox and macOS have.
- Any user, including root.

The client installs its server to `~/.local/share/splitty/remote/<version>/`
on the host (checksum verified there; the host downloads nothing). The released
app and the static `splitty` from the release carry the servers for x86_64 and
aarch64 Linux hosts and for Macs. A build of your own needs a payload,
see [Installing Splitty on hosts](#installing-splitty-on-hosts).

## Remote screens

A window can show screens of other hosts next to its local ones. **Hosts** at
the bottom of the drawer, or **Connect to Host…** in the command palette, lists
your recent hosts and the hosts of `~/.ssh/config` (DevPod's workspaces, for
example); type to filter, or type any `[user@]host[:port]` and choose
**Connect to …**. A host's panel then shows its
state and its running screens, with **New Screen**. For a host that already has
screens in the window, the **+** on its line in the drawer creates another one.
The local server installs its server on the host when it lacks it, starts the
host's daemon and relays it over SSH, as `splitty connect` does, but without a
tunnel or a browser tab. The host's panel shows how far the connection got, and
errors with a retry. When SSH asks for a password or a key's passphrase, the app
asks in a dialog of its own.

- A remote screen's tabs, splits, shells and working directories are the host's: `splitty split` in one of its panes splits it there. Your theme, font and keybindings apply; your local shell profiles do not, so a new tab there is a plain terminal of the host.
- In the drawer, local screens come first, under **This computer**. Each host with open screens follows under a line of its own with its name and connection dot (connected, reconnecting, offline), a ports button with the number of its ports, a **+** for another screen there and, on hover, an unplug button that closes the host. Hosts without open screens are not in the list but under **Hosts**. Screen shortcuts and the mouse wheel over the drawer switch screens in that order; drag to reorder screens within a group, and host groups among each other. A dropped connection shows reconnecting and comes back with what was printed meanwhile; a host that stays unreachable for two minutes is shown offline, with a retry.
- **Detaching** a remote screen (the unplug button on its row, or **Detach from This Window** in its menu) removes it from the window without asking, and everything keeps running on the host. Local screens use × to close. **Terminate Screen…** in a remote screen's menu **ends** the screen on the host, with its panes and their processes. Quitting or closing the window only detaches remote screens; their processes never hold up quitting.
- **Closing a host** (the unplug button on its line, or **Close host** in its panel) takes all of its screens out of the window; they keep running. The local server remembers which screens were open: the host's panel then offers to reopen them (**Reopen 1 Screen**, **Reopen 2 Screens**, …), and **Open all … Screens** for the host's screens that are not open here. With **Reconnect at start** on in a host's panel, the app connects that host once at startup and brings back its screens (a browser window connects it at once too when it brings back screens of it); otherwise a host connects only when you ask, so an unreachable host does not slow the start. A screen the host no longer has is reported as missing, never recreated silently.
- **The Hosts page** (Alt+H, Cmd+Shift+H on macOS, **Hosts and Ports** in the command palette, or **Manage hosts…** at the end of the host list) lists every host you used, connected or not, with its state, the version of a connected host's daemon, its screens and its ports. Disconnected hosts show the last seen names and pane counts, saved by the local server across restarts. Reading this list never connects hosts at startup.
- **Updates:** for a host that runs another server version, its entry on the Hosts page offers **Update…**. The dialog names the current and the replacement version, and **Update host** confirms. Eligible older native daemons update through their verified service identity. For a compatible legacy Linux daemon, the dialog says how many sessions were observed; the backend checks its process identity and session keepers again before changing anything. Confirming applies to one attempt; an ordinary retry never migrates a legacy daemon. If migration cannot be verified, including for legacy macOS daemons, the daemon keeps running. Newer daemons and daemons without session keepers are not replaced this way.
- On Windows the local server runs in WSL, so remote screens use WSL's `ssh`, keys and SSH configuration.
- The app passes its own environment to `ssh` and does not read `PATH` from your login shell. Started from the Dock, the Finder or a desktop launcher, a `ProxyCommand` in `~/.ssh/config` that runs a program outside the system's default `PATH` (say, `gcloud` from Homebrew) needs its full path.

## Ports

Dev servers on the host are one click away:

- The ports button on a host's line in the drawer lists the ports its Splitty sessions listen on, under **Open in the browser**: choosing one forwards it when needed and opens `http://localhost:<local port>` in your browser. Type a number to open a port that was not detected; a running forward also offers **Copy localhost:…**. A new listener brings no notice. **Manage ports…** at the end of the list goes to the host on the Hosts page.
- On the Hosts page, each host lists its forwarded, saved and detected ports under **Forward to this computer**. A row offers **Forward**, or **Copy address** while it is forwarded, and **Local port settings…**. **Add port…** forwards a port by number, also when nothing was detected, for example for a service started outside Splitty. Inspecting ports does not connect the host. A running forward survives detached screens and reconnects on the same port, showing “Reconnecting…” while being rebuilt.
- In a port's settings, the local port is optional: empty, it uses the remote number when free or another free port for a new, unsaved mapping. A personal label (Frontend, API) names the port in the lists. **Restore when I connect** is opt-in and remembers the mapping: **Forward** saves the actual local port after opening; if saving fails, the forward stays running and the error says it was not remembered. **Save without starting** saves the settings only and never connects the host; without a local port it saves the existing forward's port, or the remote number. Saved mappings remain visible after restart and restore only when you successfully connect their host; reading or editing them never starts SSH.
- **Stop forwarding** ends a running forward and keeps a saved mapping paused, across restarts and reconnects; forwarding it again, or an explicit click on a link in a remote pane, starts it again. **Forget settings** removes the saved mapping and leaves a running forward running; stop it to end it. Label edits never resume a paused mapping.
- **Stable local ports:** a saved or explicitly selected port must be available on both supported loopback families. A conflict leaves the saved port unchanged and reports it; choose another local port in the port's settings, which updates the saved mapping only after the forward succeeds. Stop a running forward before changing its port. No conflicting listener is terminated.
- **Links in a host's panes:** a link to `localhost`, `127.0.0.1`, `0.0.0.0` or `[::1]` printed in a remote pane verifies or opens that host's forward before opening its local address, preserving the scheme, path, query and fragment. A saved-port conflict opens no browser URL. Other links, and every link in a local pane, open as they are. Forwarding carries HTTP, HTTPS and WebSockets unchanged and adds no Splitty credentials; listeners bind only to local `127.0.0.1`.
- **Forward new ports** (Settings → Remote, `remote.autoForwardPorts`) forwards new detected ports without a notice. The local server does it for a host while a window is connected to it, once for all windows; ports that were already listening when the host connected are not forwarded. It is off by default, since every program on your machine can reach a forwarded port. It never remembers a mapping, resumes a paused mapping or silently changes a saved local port.
- A Mac host detects no listening ports yet; forward its ports by number.

### Sharing a local port with a host

The other direction: a service on this machine (a local API, a database, a model server) reachable from programs on the host, for example a dev VM.

- **Share a local port…** in the ports list of a host's line opens the host's shared local ports. Type the local port and, optionally, the port it gets on the host (`11434`, or `11434:8080`; without a host port it gets the same number), then choose **Share local port …**. Nothing is shared without that choice; detected ports, links and reconnects never start a share.
- On the host, the port listens on `127.0.0.1` and `::1` only, and Splitty accepts a connection only from a program running as the user you log in as. Other users on the host, and other machines, cannot reach it, whatever the server's `GatewayPorts` says. Programs running as you on the host can, and most local services ask for no password: share only what you would let those programs use. The list says so before you share, and warns more strongly for ports usually used by SSH, Docker, Kubernetes, Remote Desktop or VNC.
- Only `localhost` of this machine is shared, one port per share, never another machine of your network. Every active Splitty browser gateway in the supported local reservation domain is refused, including gateways from another configuration directory. A gateway also refuses a port already reserved for sharing.
- Linux hosts only for now: the owner check reads `/proc` on the host, and a Mac host refuses the share. A port already in use on the host fails the share; nothing on the host is stopped.
- A share's row stops or resumes it. On a running share, **Share again when this host connects** remembers it: a remembered share starts again only when its host connects, and **Stop and forget** ends it and removes it. A lost connection frees the host's port within about a minute, and a running share is rebuilt on the same port.
- Reverse sharing is unavailable on Windows and WSL: their reachable localhost services have no shared Splitty reservation authority. It fails before starting SSH, including with mirrored networking. Forwarding a host service to this machine remains available.
- On Linux, reservation coordination requires the initial PID and user namespaces and a verifiable common root mount view. Unsupported or ambiguous namespace environments fail with an explicit error. The reservation registry coordinates Splitty processes of the same OS account across configuration directories; it does not restrict other forwarding tools or processes running as that user.

### Forwarding registry requirements

On native Linux and macOS, reverse shares and explicit `serve`/`connect` browser
gateways use one persistent registry rooted in the OS account's home directory:
`.local/state/splitty/exposure` on Linux and
`Library/Application Support/Splitty/exposure` on macOS. `HOME`, XDG variables and
`SPLITTY_CONFIG_DIR` do not select a separate registry.

Every ancestor must be a directory owned by the account or root, without a
symlink or non-sticky group/world-write permission. The final registry directory
must belong to the account with mode `0700`, and its files must be private owned
regular files with mode `0600`. Supported persistent local filesystem semantics
and a verifiable shared process/network view are required as well.

For example, an existing `.local` or `.local/state` with mode `0775` prevents a
native Linux browser gateway or reverse share from starting. This applies even
when the gateway itself starts no share: another Splitty process may already be
sharing its prospective port. Splitty does not chmod shared parent directories
or bypass the registry. This restriction is separate from native terminal
connections, which support owned `0775` ancestors through pinned endpoint
directories and peer checks.

Windows and WSL instead refuse reverse sharing entirely; their explicit browser
gateways and forward-to-local application ports remain available. Their policy
is not a fallback for an unsupported or unreadable native Linux registry.

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

Preparing a host terminal connection has a fixed ten-minute ceiling. An archive upload may take up
to eight minutes, but sixty seconds without a successful write to the owned SSH
stdin pipe aborts it. After stdin closes, the install command has up to two
minutes to finish. Progress reports do not renew these limits. Native framing
and service handshakes retain their separate five-second deadlines.

- On a Linux or macOS host without Splitty (or with another version), the client installs its own server first, through SSH, to `~/.local/share/splitty/remote/<version>/` (checksum verified on the host; nothing is downloaded there). It needs a payload: the released static `splitty` (`splitty-wsl-x86_64.tar.gz`) installs itself on x86_64 hosts and carries the servers for aarch64 hosts and for Macs (Apple silicon and Intel); any build takes one from `SPLITTY_REMOTE_PAYLOAD=<archive>`, an archive as `packaging/windows/build-wsl-payload.sh <repo> <out> [commit] [x86_64|aarch64|darwin-aarch64|darwin-x86_64]` builds it (on Linux, the macOS ones cross-built with a pinned zig that `packaging/zig-toolchain.sh` fetches; on a Mac, all four without zig).
- Without a payload, `splitty` must be in the `PATH` of non-interactive SSH sessions on the remote machine, which often comes from `~/.profile` rather than `~/.bashrc`; `ssh user@remote-host splitty --version` shows whether it is.
- On a Mac, the server `connect` or the hub installed keeps its own directory, `~/.config/splitty` (it runs with `SPLITTY_CONFIG_DIR` set to it): its daemon, sessions and private endpoints stay apart from a Splitty desktop app on that Mac, which keeps `~/Library/Application Support/splitty`. Updating the host from elsewhere never stops the app's sessions, and the app never attaches to that daemon.
