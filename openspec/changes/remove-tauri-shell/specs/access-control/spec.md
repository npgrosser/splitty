## REMOVED Requirements

### Requirement: Remote Window Isolation
**Reason**: This invariant isolated the Tauri shell's separate remote windows: WebView documents that loaded packaged code, held a host-bound native connection and had their own storage, navigation and window references. With the shell removed no client has such a window, so there is nothing left to isolate. The native app renders no host-supplied code and reaches hosts only through its local service's hub, where each connection stays bound to one backend (`client-connections`, Fixed Backend And Connection Identity, and Enumerated Native Requests). The browser gateway's own rules for host connections are unchanged.
**Migration**: None.

### Requirement: Desktop Shell Command Scope
**Reason**: It scoped the Tauri shell's native commands to an authorized packaged document, its lifetime and its window role (main, setup, remote), and documented parent-owned popouts as sharing main authority. Those commands, documents, roles and popouts exist only in the removed shell. The native app has no web documents, child frames or popouts that could invoke native commands; its service calls stay limited by `client-connections`, Enumerated Native Requests.
**Migration**: None.

### Requirement: Showing A Saved Download
**Reason**: It bounded the Tauri shell's `reveal_download` command, the only way any client showed a saved file in the system file manager. The command goes with the shell and no client offers the action any more, so no page or host can reach the file manager. The native app saves an offered file to the download folder without a reveal action.
**Migration**: None.

### Requirement: Packaged Desktop Content Policy
**Reason**: It limited the packaged web assets, their resource protocol and the content security policy of the Tauri shell's WebView documents. The native app loads no packaged web document. The browser gateway's own asset and header rules are separate requirements and stay unchanged.
**Migration**: None.

## MODIFIED Requirements

### Requirement: Explicit Remote Editor Dispatch
Only a deliberate editor action in the trusted hub UI SHALL dispatch a local VS Code or Zed folder handoff for the selected local or SSH terminal, or an SFTP client handoff for the selected SSH terminal. The desktop app SHALL validate the fixed editor enum (`vscode`, `zed`, or `sftp` for remote folders only), an absolute directory, an optional restricted SSH target for remote folders, and an optional validated WSL distribution for local VS Code folders, and generate the fixed editor URI itself before it hands that URI to the operating system's handler; no arbitrary URI, executable, command string or shell interpolation SHALL be accepted. Terminal links, remote output, server commands and URL query parameters SHALL NOT invoke this dedicated path. Existing generic URL openers SHALL continue to admit only HTTP(S).

#### Scenario: Dedicated native command scope
- **WHEN** a server or host message, or terminal output, names an editor, a folder or an editor URI
- **THEN** no handoff occurs unless the user chooses the editor action in the window

#### Scenario: Hostile dispatch inputs
- **WHEN** the action receives an executable/URI as target, an option-like target, a relative directory, control characters, unsupported SSH syntax, or `sftp` for a local folder
- **THEN** dispatch rejects the request before opening anything

#### Scenario: Terminal link uses editor scheme
- **WHEN** a hub remote terminal link or Splitty's generic URL helper receives a `vscode:`, `zed:` or `sftp:` URI
- **THEN** the generic opener refuses it and no editor dispatch is invoked
