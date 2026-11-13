## MODIFIED Requirements

### Requirement: Explicit Remote Editor Dispatch
Only a deliberate editor action in the trusted hub UI SHALL dispatch a local VS Code or Zed folder handoff for the selected local or SSH terminal, or an SFTP client handoff for the selected SSH terminal. Native editor dispatch SHALL be granted solely to the main packaged UI on the shell's assigned application origin, not bundled setup pages, remote windows or direct popout invocations; parent-owned popouts share main script authority as specified by Desktop Shell Command Scope. The native grant SHALL be scoped by window, actual caller, exact packaged origin and document generation; the no-frame content policy remains. It SHALL validate the fixed editor enum (`vscode`, `zed`, or `sftp` for remote folders only), an absolute directory, an optional restricted SSH target for remote folders, and an optional validated WSL distribution for local VS Code folders and generate the fixed editor URI itself; no arbitrary URI, executable, command string or shell interpolation SHALL be accepted. Terminal links, remote output, server commands and URL query parameters SHALL NOT invoke this dedicated path. Existing generic URL openers SHALL continue to admit only HTTP(S); remote windows SHALL retain their isolation.

#### Scenario: Dedicated native command scope
- **WHEN** a remote window, a web origin or popout directly invokes native editor dispatch
- **THEN** the request is rejected before any native handoff

#### Scenario: Hostile dispatch inputs
- **WHEN** a caller supplies an executable/URI as target, an option-like target, a relative directory, control characters, unsupported SSH syntax, or `sftp` for a local folder
- **THEN** native dispatch rejects the request before opening anything

#### Scenario: Terminal link uses editor scheme
- **WHEN** a hub remote terminal link, Splitty's generic URL helper or a native navigation guard receives a `vscode:`, `zed:` or `sftp:` URI
- **THEN** the generic opener refuses it and no editor dispatch is invoked

### Requirement: Showing A Saved Download
A desktop shell SHALL show a file in the system file manager only for a download of its main window that it saved itself and that finished, named by the page with that download's `blob:` URL (`reveal_download` in the Tauri app, under Desktop Shell Command Scope). It SHALL NOT take a path or a file name from the page, SHALL NOT tell the page where it saved the file, and SHALL refuse every other URL, an unfinished, failed or cancelled download, and a download beyond the most recent 32.

#### Scenario: A finished download
- **WHEN** the main window saved a download from its `blob:` URL and asks the shell to show that URL
- **THEN** the shell opens the file manager on the saved file and answers the page without its path

#### Scenario: A URL the shell did not save
- **WHEN** the page asks the shell to show a URL that is not one of its finished downloads
- **THEN** the shell refuses and shows nothing
