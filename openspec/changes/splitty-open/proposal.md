# Proposal

> Completed on 2026-11-06 from the draft of 2026-10-09; the draft's open questions are decided in `design.md`. Roadmap: 105 (`splitty open`). Depends on: Milestone 1 (done); builds on `workspace-model`, `mux-protocol`, `control-api`, `cli`, `access-control` and `server-modes`. Followed by `review-pane` (111), which adds annotations to the pane kind introduced here. The parts of the draft that this change does not need (renderer plugins, diagram formats, PDF, a transient overlay) moved to the draft `viewer-renderers`.

## Why

Agents and developers produce files that a terminal shows badly: plans and reports in Markdown, HTML pages, long logs, code to read next to the terminal that produced it. Today that means leaving Splitty for an editor or a browser, and on a remote host also finding the file again on another machine. An agent's answer in a terminal can only be read where it scrolls by.

## What Changes

- **Viewer panes**: a pane kind without a terminal session. It shows one document: a file on the server's machine, or a text the window handed over (a terminal selection). It splits, stacks, floats, moves to another screen and persists like a terminal pane.
- **Rendering in the window**: Markdown and HTML are rendered and can be switched to their source; other text is shown as source with syntax highlighting; images as images. Rendered documents run no script and load nothing; links go to the system browser or open the linked file in another viewer pane. No frame is involved, so the content security policy and the frame rules stay as they are.
- **Reading over the connection**: the window reads a document in chunks over the connection of the screen that holds the pane, so it works the same for a local server, a remote host, and a desktop app on Windows whose server runs in WSL. While a viewer is shown, it notices when its file changes and shows the new content.
- **`open_viewer`**: a screen operation that places a new viewer pane next to a pane (to the right by default, or as a tab), records the terminal it came from, and, when the request carries no pane ID, shows an existing viewer pane of the same file on that screen again instead of adding one.
- **`splitty open <file>`**: shows a file in a viewer pane next to the calling terminal, without a confirmation, since nothing native starts. Folders, `--with` and the new `--editor` keep the existing hand-over to VS Code or Zed. Agents in panes use the same command.
- **Terminal selection in a viewer**: the terminal's context menu opens the current selection as a text document in a viewer pane next to the terminal, for example a long answer of an agent.

Out of scope:
- Annotations and sending them to a terminal: `review-pane` (111), next.
- Editing files and showing diffs: drafts `viewer-quick-edit` and `review-diffs`.
- Renderer plugins, diagram formats (Mermaid, Graphviz), PDF, a transient overlay above a pane, rendering URLs or dev servers: draft `viewer-renderers`. Web-app panes and the `/proxy/` route stay removed (SR-01/SR-02).
- Opening viewers through a restricted agent grant socket, which serves job operations only.

## Capabilities

### New Capabilities
- `viewer-panes`: the pane kind, `open_viewer` and reopening a file, the pane's lifetime, reading a document over the connection, keeping a shown document current, how documents are shown, the terminal selection as a document, document text outside snapshots, and the capability `viewer-v1`.

### Modified Capabilities
- `workspace-model`: panes without a session ("Workspace Structure"), `open_viewer` in "Layout Operations", "Split Session Inheritance" from a viewer pane, and "Closing a Pane" for a pane without a session.
- `mux-protocol`: "Change Propagation to Other Clients" nudges after `open_viewer` and after closing a pane without a session.
- `control-api`: "Workspace and Screen Queries" omit a viewer's text, and "Submitting Layout Operations" answers `open_viewer` with the pane that shows the document.
- `cli`: "Open In Client Editor" keeps folders and explicit editors; the new "Open In A Viewer Pane" covers files.
- `access-control`: new requirements "Viewer Documents Stay With Their Connection" and "Rendered Documents Are Inert".
- `server-modes`: "Workspace Persistence In Daemon Mode" keeps viewer panes with their document reference and text.

## Impact

- `splitty-core`: the viewer record on panes, `open_viewer` in validation and reducer, splits from a viewer pane, the text kept out of snapshots.
- `splitty-server`: `viewer.read` on every client connection (with the native allowlist), the nudges, `viewer-v1`, the control API's answers, `splitty open`.
- Frontend: a viewer component in dockview with CodeMirror 6, markdown-it and DOMPurify as new dependencies; snapshots build viewer panes by role; the terminal context menu; tab titles, the drawer and template extraction know viewer panes.
- Docs: `docs/protocol.ts`, `docs/native-operations.json`, `docs/architecture.md`, `docs/files-and-editors.md`, `docs/cli.md`, the agent guide.
- Desktop shells: unchanged. Their content policies already allow `data:` images, and no frame is created.
