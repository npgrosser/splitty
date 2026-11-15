## REMOVED Requirements

### Requirement: Remote Windows
**Reason**: Separate per-host remote windows with packaged web UI existed only in the Tauri shell, which is removed. The native desktop app shows a host's screens in its own window, through its local service's hub (`remote-screens`), and opens no window per host.
**Migration**: Open a host from the native app's host picker or Hosts panel. On Windows the native app needs WSL; a desktop app for Windows without WSL is the draft `native-windows-without-wsl`.

### Requirement: Remote Window Shell API
**Reason**: The commands `remote_hosts`, `remote_open` and `remote_cancel` and the event `remote-progress` were the Tauri shell's API for its remote windows; no client offers them after its removal.
**Migration**: The native app lists known hosts and opens them through its local service (`remote-hosts`, Known Hosts, and `remote-screens`); scripts keep `splitty remote-hosts --json` (`cli`).

### Requirement: Remote Window Connection Lifetime
**Reason**: It described the lifetime of the SSH connection a Tauri remote window owned. The native app's host connections, their reconnect and their end are specified in `remote-screens`.
**Migration**: None.

### Requirement: Remote Window Entry Points
**Reason**: "Open host in new window…" and the Windows setup page's "Connect to host…" opened Tauri remote windows; both entry points go with the shell. The native app has no per-host window and no setup page.
**Migration**: None.

## MODIFIED Requirements

### Requirement: Remote Folder Editor Handoff
The trusted hub UI SHALL offer a deliberate user action to open a remote terminal's current directory in local VS Code with Remote-SSH, in Zed, or in the local SFTP client that opens `sftp:` links. The target MUST come from the terminal's owning remote connection, and the absolute directory from that connection's session. It MUST NOT fall back to a local directory or infer a target from a page's `host` query parameter. Missing or unsupported values SHALL disable the action with an explanation. A connection or session that disappeared before the click SHALL NOT cause another connection or folder to be opened.

Browser clients SHALL dispatch only a generated `vscode://vscode-remote/ssh-remote+...`, `zed://ssh/...` or `sftp://...` URI during the deliberate click. The native desktop app SHALL generate the same fixed URI itself from the connection's target, the session's directory and the chosen editor (`vscode`, `zed` or `sftp`) and hand only that URI to the operating system's handler. Neither path SHALL accept arbitrary URIs or executable names. Plain hostnames, SSH aliases, IPv4 and optional SSH usernames SHALL be supported. For VS Code and Zed, unsupported target forms including explicit ports and IPv6 literals, and all-hex target names for VS Code SHALL be refused with local SSH alias guidance, never silently rewritten to another host or port. For VS Code, all-hex targets SHALL explain that the alias must contain a non-hex character to avoid encoded-authority ambiguity. The SFTP link SHALL carry every target form the connection accepts, explicit ports and bracketed IPv6 literals included, unchanged as its authority; an alias SHALL stay an alias. Directory spaces, Unicode and URI delimiters SHALL remain directory data.

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
The trusted hub UI in browsers and the desktop app SHALL offer to open a local terminal's current directory in VS Code or Zed, with the same deliberate click, session identity rechecks and folder validation as the remote handoff. VS Code uses `vscode://file/<path>/`, or `vscode://vscode-remote/wsl+<distro>/<path>/` when the local server runs in a WSL distribution (named to browser clients by `/api/build`). Zed uses `zed://file/<path>/`; Zed for local WSL folders SHALL be disabled with an explanation until a supported URL handoff exists. A client whose local connection reaches a host, such as a browser on a `splitty connect` gateway, SHALL NOT offer it; the existing `host` display query SHALL not grant native authority. The desktop app SHALL independently validate the editor enum, directory and optional distribution before generating the fixed URI, and hand only that URI to the operating system's handler. Observable errors SHALL be reported without claiming successful opening or installation detection.

#### Scenario: Local server in WSL
- **WHEN** the local server runs in WSL `Ubuntu` and the user opens `/home/me/app` in VS Code
- **THEN** browser and desktop dispatch `vscode://vscode-remote/wsl+Ubuntu/home/me/app/`
- **AND** the Zed action is disabled with WSL guidance

#### Scenario: Local desktop folder
- **WHEN** the user opens `/srv/My App` from a local desktop terminal in Zed
- **THEN** the desktop app hands `zed://file/srv/My%20App/` to the operating system
- **AND** it rejects unknown editors, unsafe directories and invalid distribution names

#### Scenario: Local folder changes before click
- **WHEN** the terminal changes directory or disappears while its menu is open
- **THEN** the action uses the current directory of the same session, or refuses a missing/replaced session
