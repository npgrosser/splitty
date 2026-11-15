# desktop-remote Specification

## Purpose
Defines how the folder of a terminal is handed to programs outside Splitty: opening a remote or local folder in an editor or an SFTP client, the SSH path of a remote folder, editor requests from the CLI that need the user's confirmation, and the local editor preferences. The rules for dispatching such a handoff are covered by `access-control`.

## Requirements

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

### Requirement: SSH Path Of A Remote Folder
The context menu of a remote terminal and of its tab SHALL offer to copy the terminal's folder as an SSH path: the owning connection's target and the session's absolute working directory as `target:/path`, or as `ssh://target/path` when the target carries a port. The target SHALL be the one the connection was opened with, never resolved or rewritten. Without an absolute working directory, or for a local terminal, the entry SHALL be absent.

#### Scenario: Copy a remote folder
- **WHEN** the user chooses Copy SSH Path in a terminal of `me@vm1` whose working directory is `/srv/app`
- **THEN** the clipboard holds `me@vm1:/srv/app`

#### Scenario: Target with a port
- **WHEN** the terminal belongs to `me@vm1:2222`
- **THEN** the clipboard holds `ssh://me@vm1:2222/srv/app`

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
