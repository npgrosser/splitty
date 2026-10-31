# desktop-remote Specification

## Purpose
Defines remote windows in the desktop apps: a host's Splitty UI shown in a window of its own through a tunnel the app holds, without a local server. Covers the shell API the trusted UI uses to list and open hosts, one window per host, the connection's lifetime, and the entry points, including Windows without WSL. The isolation of remote windows is covered by `access-control`.

## Requirements

### Requirement: Remote Windows
The desktop apps (the Tauri app on Windows, macOS and Linux, and the Electron app on Linux) SHALL open a host in a remote window: a window of its own that shows the host's UI through an SSH tunnel the app holds. The app SHALL reach the host as `splitty connect` does (see `cli`, Remote Connect Tunnel and Remote Connect Session Handoff, and `remote-hosts`):
- install its own server when the host lacks it;
- ask for the daemon with the handshake, starting a missing one;
- forward the daemon's port on `127.0.0.1`;
- load a single-use session URL into the window.

It SHALL NOT need a local Splitty server for this. The window's title SHALL name the target. A successful open SHALL record the target as a recent host (see `remote-hosts`, Known Hosts). The session URL carries the target as its `host` parameter. The web UI SHALL then behave as in a browser, without the shell's title bar or native calls.

#### Scenario: Open a host
- **WHEN** the user opens `vm1` in a desktop app and `vm1` has no Splitty
- **THEN** the app installs its server on `vm1`, starts the daemon there, and opens a window titled after `vm1` that shows `vm1`'s screens

#### Scenario: Same host again
- **WHEN** a remote window for `vm1` is open and connected and the user opens `vm1` again
- **THEN** that window is focused, and no second window or tunnel is opened

### Requirement: Remote Window Shell API
The trusted UI of a desktop app (its main window on its own backend's origin, and on Windows its bundled setup page) SHALL be able to:
- **list the known hosts:** Tauri command `remote_hosts`, Electron channel `remote:hosts`. The result is an array of `{ target, source, lastConnected?, alias? }`, in the order of `remote-hosts`, Known Hosts, with `source` `recent` or `ssh-config`.
- **open a host:** Tauri command `remote_open` with `{ target }`, Electron channel `remote:open` with the target. It resolves once the remote window shows the host, and rejects with `{ message, hint? }`.
- **cancel an open in flight:** Tauri command `remote_cancel` with `{ target }`, Electron channel `remote:cancel`. The pending open then rejects.

While an open runs, the shell SHALL send progress to the window that called it: Tauri event `remote-progress`, Electron channel `remote:progress`, each `{ target, step, message }` with the steps of `cli`, Remote Connect Machine-Readable Output.

A target that is not valid (see `access-control`, Remote Access Through SSH Tunnel) SHALL be rejected before anything runs. Concurrent opens of the same target SHALL share one connection attempt.

#### Scenario: Progress and failure
- **WHEN** the UI opens `vm1` and the SSH login fails
- **THEN** the UI receives progress events for `vm1`, then the open rejects with a message naming the failure

#### Scenario: Unknown host key
- **WHEN** the UI opens `vm1` and `ssh` reports a failed host key verification
- **THEN** the open rejects with a `hint` that tells the user to run `ssh vm1` once in a terminal and check the fingerprint

#### Scenario: Invalid target
- **WHEN** the UI opens `-oProxyCommand=x`
- **THEN** the open rejects without running `ssh`

### Requirement: Remote Window Connection Lifetime
The tunnel of a remote window SHALL live exactly as long as the window. Closing the window or quitting the app SHALL end its tunnel and SHALL NOT stop the host's daemon or its sessions. When the tunnel drops, the app SHALL rebuild it with the rules of `cli`, Remote Connect Reconnect:
- When the same daemon answers, the window keeps its page, which reconnects by itself.
- When a different daemon answers, the app SHALL load a new single-use session URL into the window.
- While the tunnel is being rebuilt, the window's title SHALL say so.
- When the tunnel could not be rebuilt for 10 minutes, the window SHALL stay open and its title SHALL say that it is disconnected. Opening that target again SHALL run the connection again and load the new session into the same window.

#### Scenario: Close the window
- **WHEN** the user closes the remote window of `vm1`
- **THEN** the app's `ssh` process for `vm1` ends, and the daemon and sessions on `vm1` keep running

#### Scenario: Host daemon restarted
- **WHEN** the daemon on `vm1` restarts while its remote window is open
- **THEN** the window loads a new session to the new daemon without user action

#### Scenario: Reconnect after giving up
- **WHEN** the window of `vm1` shows that it is disconnected and the user opens `vm1` again
- **THEN** the app connects again and the same window shows `vm1`

### Requirement: Remote Window Entry Points
Every desktop app with a local server SHALL offer "Open host in new window…" in its command palette. The Windows app's setup page SHALL offer "Connect to host…" in every state, whether WSL is missing, being set up, failed or running. Both SHALL open a host picker that lists the known hosts, accepts a typed `[user@]host[:port]`, and shows the open's progress, its error with the hint, and a retry. A remote window SHALL NOT offer these entry points.

#### Scenario: Windows without WSL
- **WHEN** the Windows app starts on a machine without WSL
- **THEN** its setup page offers "Connect to host…", and connecting opens a remote window that uses Windows OpenSSH

#### Scenario: Remote window
- **WHEN** the command palette is opened in a remote window
- **THEN** it does not offer "Open host in new window…"

### Requirement: Remote Folder Editor Handoff
The trusted hub UI SHALL offer a deliberate user action to open a remote terminal's current directory in local VS Code with Remote-SSH, in Zed, or in the local SFTP client that opens `sftp:` links. The target MUST come from the terminal's owning remote connection, and the absolute directory from that connection's session. It MUST NOT fall back to a local directory or infer a target from a page's `host` query parameter. Missing or unsupported values SHALL disable the action with an explanation. A connection or session that disappeared before the click SHALL NOT cause another connection or folder to be opened.

Browser clients SHALL dispatch only a generated `vscode://vscode-remote/ssh-remote+...`, `zed://ssh/...` or `sftp://...` URI during the deliberate click. Trusted desktop clients SHALL use `open_remote_editor` (Tauri) or `editor:open-remote` (Electron) with `{ target, cwd, editor? }` (editor is `vscode` by default, `zed` or `sftp`). Neither interface SHALL accept arbitrary URIs or executable names. Plain hostnames, SSH aliases, IPv4 and optional SSH usernames SHALL be supported. For VS Code and Zed, unsupported target forms including explicit ports and IPv6 literals, and all-hex target names for VS Code SHALL be refused with local SSH alias guidance, never silently rewritten to another host or port. For VS Code, all-hex targets SHALL explain that the alias must contain a non-hex character to avoid encoded-authority ambiguity. The SFTP link SHALL carry every target form the connection accepts, explicit ports and bracketed IPv6 literals included, unchanged as its authority; an alias SHALL stay an alias. Directory spaces, Unicode and URI delimiters SHALL remain directory data.

The UI SHALL distinguish requesting a handoff from confirming that the editor or client connected. It SHALL explain the need for the selected local editor, Remote-SSH for VS Code, an SFTP client that opens `sftp:` links and knows an aliased target, and browser/OS permission, report observable dispatch failures, and explain that Windows applications use SSH configuration separate from WSL. It SHALL NOT start a server-side editor command.

#### Scenario: Selected remote terminal
- **WHEN** the user invokes the editor action for a remote pane while another host or local pane has the same pane id
- **THEN** only the selected pane's owning target and cwd are used
- **AND** creating the menu or receiving remote messages does not launch anything

#### Scenario: Special directory characters
- **WHEN** the remote cwd contains spaces, Unicode, `#`, `?`, percent signs, or a file-like suffix
- **THEN** handoff preserves those characters as part of the folder path and has no caller-supplied query or fragment

#### Scenario: Missing or unsupported input
- **WHEN** cwd is missing, relative or unsafe, or, for VS Code or Zed, the target requires an explicit port, IPv6 literal or (for VS Code) all-hex alias
- **THEN** no handoff occurs and an explanation is available, including alias guidance for unsupported targets

#### Scenario: Remote folder in an SFTP client
- **WHEN** the user opens `/srv/My App` of a terminal of `me@vm1:2222` in the SFTP client
- **THEN** the client dispatches `sftp://me@vm1:2222/srv/My%20App/`
- **AND** a local terminal's menu offers no SFTP client

#### Scenario: Session or connection changed
- **WHEN** the session or owning connection disappears or changes between building the menu and clicking
- **THEN** no cached folder or different connection is opened

#### Scenario: Browser or editor cannot complete handoff
- **WHEN** the browser/OS refuses handoff or the selected editor, its remote support or an SFTP client is absent
- **THEN** observable errors are reported and the UI offers setup guidance without claiming successful opening

#### Scenario: Remote window has a host query
- **WHEN** a host-owned page has a `host` query parameter and a local connection
- **THEN** that parameter does not enable the remote-folder editor action or any native permission

### Requirement: SSH Path Of A Remote Folder
The context menu of a remote terminal and of its tab SHALL offer to copy the terminal's folder as an SSH path: the owning connection's target and the session's absolute working directory as `target:/path`, or as `ssh://target/path` when the target carries a port. The target SHALL be the one the connection was opened with, never resolved or rewritten. Without an absolute working directory, or for a local terminal, the entry SHALL be absent.

#### Scenario: Copy a remote folder
- **WHEN** the user chooses Copy SSH Path in a terminal of `me@vm1` whose working directory is `/srv/app`
- **THEN** the clipboard holds `me@vm1:/srv/app`

#### Scenario: Target with a port
- **WHEN** the terminal belongs to `me@vm1:2222`
- **THEN** the clipboard holds `ssh://me@vm1:2222/srv/app`

### Requirement: Local Folder Editor Handoff
The trusted hub UI in browsers and desktop apps SHALL offer to open a local terminal's current directory in VS Code or Zed, with the same deliberate click, session identity rechecks and folder validation as the remote handoff. VS Code uses `vscode://file/<path>/`, or `vscode://vscode-remote/wsl+<distro>/<path>/` when `/api/build` names a WSL distribution. Zed uses `zed://file/<path>/`; Zed for local WSL folders SHALL be disabled with an explanation until a supported URL handoff exists. A page with a `host` query SHALL NOT offer it. Desktop apps SHALL use `open_local_editor` (Tauri) or `editor:open-local` (Electron) with `{ editor, cwd, wslDistro? }`, independently validating the editor enum, directory and optional distribution before generating the fixed URI. Observable errors SHALL be reported without claiming successful opening or installation detection.

#### Scenario: Local server in WSL
- **WHEN** the local server runs in WSL `Ubuntu` and the user opens `/home/me/app` in VS Code
- **THEN** browser and desktop dispatch `vscode://vscode-remote/wsl+Ubuntu/home/me/app/`
- **AND** the Zed action is disabled with WSL guidance

#### Scenario: Local desktop folder
- **WHEN** the user opens `/srv/My App` from a local desktop terminal in Zed
- **THEN** only the dedicated native handler dispatches `zed://file/srv/My%20App/`
- **AND** it rejects unknown editors, unsafe directories and invalid distribution names

#### Scenario: Local folder changes before click
- **WHEN** the terminal changes directory or disappears while its menu is open
- **THEN** the action uses the current directory of the same session, or refuses a missing/replaced session

### Requirement: Confirmed CLI Editor Handoff
Only the trusted hub UI SHALL offer CLI editor requests. Every launch SHALL require a local deliberate click showing the connection-derived host, exact path and locally selected editor. Receipt, terminal output and host-owned pages SHALL never launch an app. A click SHALL recheck request lifetime and session identity, consume the request once and use only the existing fixed editor handoff extended with `kind: file|folder` (default folder).

#### Scenario: Remote file accepted
- **WHEN** the user confirms a remote file request in the trusted hub
- **THEN** the local editor receives that file on the owning SSH connection, with delimiters encoded as path data and no folder slash

#### Scenario: Refusal, replay or stale session
- **WHEN** a request is rejected, expired, repeated, disconnected, or its session disappeared or was replaced
- **THEN** no handoff occurs and a stale confirm action cannot launch it

#### Scenario: Unsupported handoff
- **WHEN** a file path has ambiguous colon syntax, the SSH target is unsupported, or Zed is selected for local WSL
- **THEN** no handoff occurs and the reason is reported; existing folder restrictions and SFTP folder behavior remain in force

### Requirement: Local Editor Preferences
Editor selection SHALL use explicit editor, longest matching project directory rule for the local connection or exact SSH target, file extension rule, then file/folder default, in that order. Defaults SHALL be VS Code. Preferences SHALL be configured in the trusted hub's settings and SHALL never authorize automatic opening. Only VS Code and Zed SHALL be configurable for CLI requests.

#### Scenario: Preferences and precedence
- **WHEN** local settings map `.rs` to Zed and a longer matching project path to VS Code
- **THEN** a Rust file there selects VS Code, one outside selects Zed, and `--with zed` overrides the project rule
- **AND** every selection still requires local confirmation

#### Scenario: Host separation and malformed settings
- **WHEN** identical paths exist on different hosts or a saved editor value is invalid
- **THEN** project rules apply only to their exact connection and invalid preferences fall back safely without accepting arbitrary commands
