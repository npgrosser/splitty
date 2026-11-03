# Spec Delta

## MODIFIED Requirements

### Requirement: Remote Windows
Desktop apps on supported platforms SHALL open one window per host using packaged UI, a native host-bound connection and the existing installation/handshake flow over SSH. They SHALL start a missing private daemon, negotiate its native bridge and show its screens without forwarding a terminal-server TCP port or loading host-supplied application code. No local Splitty service or WSL SHALL be required for remote-only use. A successful open SHALL record the target as recent. The target label SHALL come from the native connection assignment, not grant authority through a query parameter.

#### Scenario: Open a host
- **WHEN** the user opens a host without Splitty
- **THEN** the app installs its server, starts its private daemon and shows its screens in the packaged remote window

#### Scenario: Same host again
- **WHEN** that host already has a connected window
- **THEN** the app focuses it without another window or SSH connection

#### Scenario: Remote only without local server
- **WHEN** the Windows app opens a remote host without WSL
- **THEN** packaged UI and native SSH provide the remote session without any local terminal service or TCP listener

### Requirement: Remote Window Shell API
The trusted UI of a desktop app (its authorized packaged main window, and on Windows its separately scoped bundled setup page) SHALL be able to:
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
A remote window SHALL own its SSH connection for its lifetime. Closing it or quitting SHALL revoke its native handles, end its SSH process and leave the host's daemon/sessions alive. Drops SHALL retry with the existing growing delay and 10-minute limit. Reconnect, including a restarted daemon, SHALL keep packaged UI loaded, verify the new instance and resync through a fresh connection without a bootstrap URL or stale input. At the retry limit the window SHALL stay disconnected; reopening the target SHALL retry in the same window.

#### Scenario: Close the window
- **WHEN** the user closes a host's remote window
- **THEN** its handles and SSH process end and host sessions continue

#### Scenario: Host daemon restarted
- **WHEN** the daemon restarts while its window is open
- **THEN** the window keeps its packaged document, negotiates a fresh connection and displays the recovered state without replaying input

#### Scenario: Reconnect after giving up
- **WHEN** the user opens the target of a disconnected window again
- **THEN** the same window retries and shows the host after a successful connection

### Requirement: Local Folder Editor Handoff
The trusted hub UI in browsers and desktop apps SHALL offer to open a local terminal's current directory in VS Code or Zed, with the same deliberate click, session identity rechecks and folder validation as the remote handoff. VS Code uses `vscode://file/<path>/`, or `vscode://vscode-remote/wsl+<distro>/<path>/` when `/api/build` names a WSL distribution. Zed uses `zed://file/<path>/`; Zed for local WSL folders SHALL be disabled with an explanation until a supported URL handoff exists. A remote-target client, identified by its assigned native role or browser gateway, SHALL NOT offer it; the existing `host` display query SHALL not grant native authority. Desktop apps SHALL use `open_local_editor` (Tauri) or `editor:open-local` (Electron) with `{ editor, cwd, wslDistro? }`, independently validating the editor enum, directory and optional distribution before generating the fixed URI. Observable errors SHALL be reported without claiming successful opening or installation detection.

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
