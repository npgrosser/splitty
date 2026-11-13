## MODIFIED Requirements

### Requirement: Remote Window Shell API
The trusted UI of a desktop app (its authorized packaged main window, and on Windows its separately scoped bundled setup page) SHALL be able to:
- **list the known hosts:** Tauri command `remote_hosts`. The result is an array of `{ target, source, lastConnected?, alias? }`, in the order of `remote-hosts`, Known Hosts, with `source` `recent` or `ssh-config`.
- **open a host:** Tauri command `remote_open` with `{ target }`. It resolves once the remote window shows the host, and rejects with `{ message, hint?, migration? }`. A positively eligible migration offer SHALL support one separately confirmed update-and-connect launch. Only the authorized main/setup launcher may provide that intent; the remote document remains fixed to its assigned host and gains no launcher, executable or process-selection authority.
- **cancel an open in flight:** Tauri command `remote_cancel` with `{ target }`. The pending open then rejects.

While an open runs, the shell SHALL send progress to the window that called it: Tauri event `remote-progress`, each `{ target, step, message }` with the steps of `cli`, Remote Connect Machine-Readable Output.

A target that is not valid (see `access-control`, Remote Access Through SSH Tunnel) SHALL be rejected before anything runs. Concurrent ordinary opens of the same target SHALL share one connection attempt. A confirmed migration SHALL NOT be silently coalesced into an ordinary pending open or a focus operation; its native-owned intent SHALL be consumed once and omitted from reconnect or recreated helpers.

#### Scenario: Progress and failure
- **WHEN** the UI opens `vm1` and the SSH login fails
- **THEN** the UI receives progress events for `vm1`, then the open rejects with a message naming the failure

#### Scenario: Unknown host key
- **WHEN** the UI opens `vm1` and `ssh` reports a failed host key verification
- **THEN** the open rejects with a `hint` that tells the user to run `ssh vm1` once in a terminal and check the fingerprint

#### Scenario: Invalid target
- **WHEN** the UI opens `-oProxyCommand=x`
- **THEN** the open rejects without running `ssh`

### Requirement: Remote Folder Editor Handoff
The trusted hub UI SHALL offer a deliberate user action to open a remote terminal's current directory in local VS Code with Remote-SSH, in Zed, or in the local SFTP client that opens `sftp:` links. The target MUST come from the terminal's owning remote connection, and the absolute directory from that connection's session. It MUST NOT fall back to a local directory or infer a target from a page's `host` query parameter. Missing or unsupported values SHALL disable the action with an explanation. A connection or session that disappeared before the click SHALL NOT cause another connection or folder to be opened.

Browser clients SHALL dispatch only a generated `vscode://vscode-remote/ssh-remote+...`, `zed://ssh/...` or `sftp://...` URI during the deliberate click. Trusted desktop clients SHALL use `open_remote_editor` (Tauri) with `{ target, cwd, editor? }` (editor is `vscode` by default, `zed` or `sftp`). Neither the browser path nor the desktop command SHALL accept arbitrary URIs or executable names. Plain hostnames, SSH aliases, IPv4 and optional SSH usernames SHALL be supported. For VS Code and Zed, unsupported target forms including explicit ports and IPv6 literals, and all-hex target names for VS Code SHALL be refused with local SSH alias guidance, never silently rewritten to another host or port. For VS Code, all-hex targets SHALL explain that the alias must contain a non-hex character to avoid encoded-authority ambiguity. The SFTP link SHALL carry every target form the connection accepts, explicit ports and bracketed IPv6 literals included, unchanged as its authority; an alias SHALL stay an alias. Directory spaces, Unicode and URI delimiters SHALL remain directory data.

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

### Requirement: Local Folder Editor Handoff
The trusted hub UI in browsers and desktop apps SHALL offer to open a local terminal's current directory in VS Code or Zed, with the same deliberate click, session identity rechecks and folder validation as the remote handoff. VS Code uses `vscode://file/<path>/`, or `vscode://vscode-remote/wsl+<distro>/<path>/` when `/api/build` names a WSL distribution. Zed uses `zed://file/<path>/`; Zed for local WSL folders SHALL be disabled with an explanation until a supported URL handoff exists. A remote-target client, identified by its assigned native role or browser gateway, SHALL NOT offer it; the existing `host` display query SHALL not grant native authority. Desktop apps SHALL use `open_local_editor` (Tauri) with `{ editor, cwd, wslDistro? }`, independently validating the editor enum, directory and optional distribution before generating the fixed URI. Observable errors SHALL be reported without claiming successful opening or installation detection.

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
