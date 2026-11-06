# viewer-panes Specification

## Purpose

Viewer panes show a file of the server's machine, or a text such as a terminal selection, inside a window next to the terminal it came from: Markdown and HTML rendered without running or loading anything, other text with highlighting, and images. A process in a pane opens one with `splitty open <file>`, the window reads the document over its own connection, so it works the same on a host, and a shown file follows its changes.

## Requirements

### Requirement: Viewer Panes
A viewer pane SHALL be a pane of the role `viewer` that carries a `viewer` record and no session. The record SHALL hold the pane's document as `source`, either a file on the server's machine (`kind` `file`, with its absolute `path`) or a text the server keeps for the pane (`kind` `text`, with its `title` and, for a text to be rendered as Markdown, the `format` `markdown`); the terminal pane it was opened from as `originPaneId`, when there is one; and a `revision` that starts at 1. The pane's title SHALL be the file's name or the text's title, and its working directory the file's directory or, for a text, the directory given when it was opened. A window SHALL give the viewer panes it creates IDs of the form `viewer-<16 hex>`. Payload shapes are in `docs/protocol.ts`.

#### Scenario: A file document
- **WHEN** `open_viewer` shows `/home/me/app/PLAN.md` next to the terminal pane `p1`
- **THEN** the new pane has the role `viewer`, the title `PLAN.md`, the working directory `/home/me/app`, `originPaneId` `p1`, `revision` 1 and no session
- **AND** the screen holds no new session

#### Scenario: A text document
- **WHEN** `open_viewer` shows a text titled `Selection from claude` with `cwd` `/home/me/app`
- **THEN** the new pane has that title and the working directory `/home/me/app`

#### Scenario: A plan in Markdown
- **WHEN** `open_viewer` shows a text titled `Plan` with the `format` `markdown`
- **THEN** the pane's record holds the source with that format, and snapshots show it without the text

### Requirement: Opening A Viewer
On the screen operation `open_viewer`, the server SHALL add a viewer pane for the op's `source` without starting a session and without any effect. With `paneId`, the pane SHALL go next to that pane: for `placement` `right` (the default), `left`, `above` or `below` as `split_pane` places a new pane next to a pane of the root layout, with the `horizontal` axis for `left` and `right`, the `vertical` axis for `above` and `below`, and before the anchor for `left` and `above`; for `tab` at the end of the anchor's slot, in the root layout or in a floating entry, as its active pane. A floating entry holds no splits, so every placement next to a pane of a floating entry SHALL add the viewer pane as `tab` does. Without `newPaneId`, a directional placement next to a terminal pane SHALL add the viewer pane as the active tab of the slot of the first viewer pane in layout order whose origin is that terminal, when there is one, so that the documents a terminal shows share one group instead of narrowing the terminal with every further split. Without `paneId`, the pane SHALL become the only pane of a root layout that holds no pane, or else the right child of a new `horizontal` split with sizes `[0.5, 0.5]` around the root layout. The server SHALL set `originPaneId` to `paneId` when that names a terminal pane, to that pane's own `originPaneId` when it names a viewer pane, and leave it out otherwise. A `file` source's `path` MUST be absolute, MUST NOT start with `//` or hold a backslash or a `.` or `..` segment, MUST be at most 4,096 bytes long, and MUST be free of control characters and of the invisible formatting characters U+200B to U+200F, U+202A to U+202E, U+2060 to U+206F and U+FEFF; a `text` source's `text` MUST be at most 262,144 bytes of UTF-8, its `title` 1 to 200 characters without control characters, and its `format`, when given, `markdown`; a `cwd`, when given, MUST be absolute. The server SHALL reject the op atomically when one of these fails or when `paneId` names no pane of the screen. `newPaneId` follows the identifier rules of `workspace-model`.

#### Scenario: To the right of a terminal
- **WHEN** `open_viewer` names pane `p1`, the only pane of its screen, without a placement
- **THEN** the root becomes a `horizontal` split `[slot(p1), slot(viewer pane)]` with sizes `[0.5, 0.5]`, and no session is spawned

#### Scenario: As a tab of a floating group
- **WHEN** `open_viewer` names pane `p4` of a floating entry with `placement` `tab`
- **THEN** the floating entry's slot holds `[p4, viewer pane]` with the viewer pane active, under the same bounds

#### Scenario: A second document of the same terminal
- **WHEN** viewer pane `v1`, opened from terminal pane `p1` through the CLI, shows `PLAN.md` to the right of `p1`, and `open_viewer` without `newPaneId` names `p1` for `notes.md`
- **THEN** the new viewer pane is the active tab of `v1`'s slot, and the root layout gains no split

#### Scenario: Without an anchor
- **WHEN** `open_viewer` carries no `paneId` on a screen whose root is the slot `[p1]`
- **THEN** the root becomes a `horizontal` split `[slot(p1), slot(viewer pane)]`

#### Scenario: A relative path
- **WHEN** `open_viewer` carries the `file` path `docs/plan.md`
- **THEN** the op is rejected and the workspace is unchanged

#### Scenario: A text over the limit
- **WHEN** `open_viewer` carries a `text` of 262,145 bytes
- **THEN** the op is rejected and the workspace is unchanged

#### Scenario: Directional placement in a floating group
- **WHEN** `open_viewer` names floating pane `p4` with `placement` `below`
- **THEN** the floating entry's slot holds `[p4, viewer pane]` with the viewer pane active, as with `tab`

#### Scenario: A path with a dot segment
- **WHEN** `open_viewer` carries the `file` path `/home/me/app/../secrets.txt`
- **THEN** the op is rejected and the workspace is unchanged

#### Scenario: An unknown format
- **WHEN** `open_viewer` carries a `text` source with the `format` `html`
- **THEN** the op is rejected and the workspace is unchanged

### Requirement: Reopening A File
When `open_viewer` without `newPaneId` carries a `file` source whose path a viewer pane of the screen already shows, the server SHALL add no pane. It SHALL take the first such pane in layout order (the root layout depth first, then the floating entries), make it its slot's active pane, increase its `revision` by one, and, when the op's `paneId` yields an origin as for a new viewer pane, set its `originPaneId` to that origin. When `open_viewer` without `newPaneId` carries a `text` source with `replace` set, and a viewer pane of the screen shows a text document with the same title whose origin is the origin the op yields, the server SHALL take the first such pane in layout order, replace its text with the op's, make it its slot's active pane and increase its `revision` by one; the pane's annotations stay. While a process waits for the review of that pane (see `viewer-annotations`, Waiting For A Review), the server SHALL refuse the op and change nothing. With `newPaneId`, the server SHALL add a new pane in any case.

#### Scenario: An agent opens its plan again
- **WHEN** a viewer pane `v1` of screen `s1` shows `/home/me/app/PLAN.md` with `revision` 2 in a slot whose active pane is another tab, and `open_viewer` without `newPaneId` names the same path on `s1`
- **THEN** `s1` holds no new pane, `v1` is its slot's active pane, and its `revision` is 3

#### Scenario: Another terminal opens the plan
- **WHEN** viewer pane `v1` was opened from terminal pane `p1` and `splitty open` of the same file runs in terminal pane `p2` of the same screen
- **THEN** `v1` is shown again with `originPaneId` `p2`

#### Scenario: A window opens the same file
- **WHEN** a viewer pane shows `/home/me/app/PLAN.md` and `open_viewer` names the same path with `newPaneId` `viewer-0123456789abcdef`
- **THEN** the screen holds a second viewer pane `viewer-0123456789abcdef` for that file

#### Scenario: A revised plan
- **WHEN** viewer pane `v1` shows a text titled `Plan` opened from terminal pane `p1` and holds annotations, and `open_viewer` without `newPaneId` from `p1` carries a new text titled `Plan` with `replace` set
- **THEN** the screen holds no new pane, `v1` shows the new text with its `revision` raised by one, and it keeps its annotations

#### Scenario: Another title
- **WHEN** the same op carries the title `Notes` instead
- **THEN** a new viewer pane shows the text, and `v1` stays as it was

#### Scenario: A revision while a review is pending
- **WHEN** a process waits for the review of `v1`, and `open_viewer` from `p1` carries another text titled `Plan` with `replace` set
- **THEN** the op is refused, and `v1` keeps its text, its `revision` and the wait

### Requirement: Viewer Pane Lifetime
A viewer pane SHALL be closed, moved, stacked, floated, docked, arranged and moved to another screen by the same operations as any other pane, and SHALL keep its document, text, title, origin and revision through all of them. Closing it SHALL end no session. When its origin pane is closed, it SHALL keep its document and keep naming that origin. In daemon mode it SHALL survive a daemon restart with its document and text, and no process SHALL start for it. The server SHALL never store a file's content for a viewer pane; a `file` document is read when a window shows it.

#### Scenario: Origin closed
- **WHEN** the terminal pane `p1` is closed while a viewer pane opened from it shows a file
- **THEN** the viewer pane stays and still shows the file

#### Scenario: Daemon restart
- **WHEN** a daemon restarts while a screen holds a terminal pane and a viewer pane of a text document
- **THEN** after the restart the screen holds both panes with their IDs, the viewer pane's text can be read, and only the terminal pane's session is started

#### Scenario: Moved to another screen
- **WHEN** a viewer pane moves to another screen with `move_pane_to_screen`
- **THEN** it shows the same document there, and no session is started or ended

### Requirement: Reading A Document
A client SHALL read a viewer pane's document with `{"type":"viewer.read","requestId","paneId","offset"}` over the connection whose screen holds the pane, starting at offset 0 and then at the next offset, every later message carrying the `version` of the first answer. The server SHALL answer every such message with `{"type":"viewer.chunk","requestId","offset","size","version","data","eof"}`, holding at most 1 MiB of the document as base64 and `eof` true for the last chunk; with `{"type":"viewer.unchanged","requestId","version"}` when a read at offset 0 carries an `ifVersion` equal to the document's current version; or with `{"type":"viewer.failed","requestId","code","error"}`. For a `file` document the server SHALL follow symbolic links, read only a regular file of at most 8 MiB without waiting on a FIFO or a device, and take the version from the file it opened, so that a file which changes between two chunks fails the read with `changed` rather than mixing two versions. A `text` document SHALL be read in the same way from the server's copy. The codes SHALL be `unknown_pane` (no viewer pane with that ID), `not_found`, `not_a_file`, `too_large`, `unreadable`, `changed`, `busy` (four reads of the connection are already in flight, or the connection's output has no room for the chunk) and `invalid` (a malformed message, or a later chunk without `version`). Reading SHALL change no workspace state and SHALL send nothing to other clients.

#### Scenario: A file in two chunks
- **WHEN** a client reads a viewer pane's file of 1.5 MiB
- **THEN** it receives a chunk at offset 0 with 1 MiB and `eof` false, asks for offset 1,048,576 with that chunk's `version`, and receives the remaining 0.5 MiB with `eof` true

#### Scenario: Nothing changed
- **WHEN** a client reads at offset 0 with an `ifVersion` equal to the file's current version
- **THEN** the server answers `viewer.unchanged` without data

#### Scenario: Changed between two chunks
- **WHEN** the file is replaced after the first chunk of a read and the client asks for the next chunk
- **THEN** the server answers `viewer.failed` with `changed`

#### Scenario: Too large
- **WHEN** a viewer pane names a file of 9 MiB
- **THEN** a read answers `viewer.failed` with `too_large` and no data

#### Scenario: A FIFO
- **WHEN** a viewer pane names a FIFO
- **THEN** a read answers `viewer.failed` with `not_a_file` at once

### Requirement: Keeping A Shown Document Current
While a viewer pane of a `file` document is shown in a visible window, the client SHALL read its document again with `ifVersion` at most every 2 seconds, and at once when a snapshot carries a higher `revision` for it, and SHALL show a changed content in place of the old one, keeping the reading position as far as the new content allows. A pane that is not shown, and every pane of a hidden window, SHALL send no reads. When a read fails, the pane SHALL say why in place of the content and offer to read again.

#### Scenario: An agent rewrites the plan
- **WHEN** a process rewrites the file a shown viewer pane displays
- **THEN** within a few seconds the pane shows the new content

#### Scenario: Reopened from the CLI
- **WHEN** a snapshot raises the `revision` of a shown viewer pane
- **THEN** the pane reads its document at once

### Requirement: Showing A Document
A client SHALL render a document named `.md`, `.markdown`, `.mdown`, `.html` or `.htm` of at most 2 MiB and offer to switch to its source; show PNG, JPEG, GIF, WebP and SVG images as images; show other documents whose first 8 KiB hold no NUL byte as text with line numbers and syntax highlighting chosen by the file's name; show any other document as a notice with its size and without its content; show a `text` document with the `format` `markdown` as a rendered Markdown document; and show any other `text` document as plain text. Rendering follows "Rendered Documents Are Inert" of `access-control`.

#### Scenario: A Markdown plan
- **WHEN** a viewer pane shows `PLAN.md`
- **THEN** the window shows the rendered Markdown and offers its source

#### Scenario: A binary file
- **WHEN** a viewer pane shows a compiled program
- **THEN** the window shows its size and none of its bytes

#### Scenario: A plan from an agent
- **WHEN** a viewer pane shows a text with the `format` `markdown`
- **THEN** the window shows the rendered Markdown and offers its source, and annotations work in both views

#### Scenario: A terminal selection
- **WHEN** a viewer pane shows a terminal selection, a text without a format, that holds Markdown
- **THEN** the window shows it as plain text

### Requirement: Viewer Capability
The server SHALL announce `viewer-v1` in `mux.welcome.capabilities`. A client SHALL offer to open viewer panes on a server's screens, and SHALL build a pane of the role `viewer` as a viewer pane, only when that server announced it. A client SHALL never start a terminal session for a pane of the role `viewer`; when its record is missing or malformed, the pane SHALL say that it cannot be shown.

#### Scenario: Host without viewers
- **WHEN** a window shows a screen of a host whose daemon does not announce `viewer-v1`
- **THEN** the window offers no "Open Selection in Viewer" for that host's terminal panes

### Requirement: Terminal Selection In A Viewer
While a terminal pane of a server that announced `viewer-v1` holds a selection, its context menu SHALL offer to open the selection in a viewer. Choosing it SHALL send `open_viewer` with a `newPaneId`, the terminal pane as anchor and `placement` `right`, and a `text` source holding the selection with trailing whitespace removed from every line, titled after the terminal pane's title, with the terminal's last known working directory as `cwd`. A selection of more than 262,144 bytes SHALL be refused with a notice, and nothing SHALL be sent.

#### Scenario: An agent's answer
- **WHEN** the user selects an agent's answer in a terminal pane and chooses "Open Selection in Viewer"
- **THEN** a viewer pane to the right of the terminal shows the selected text, and its origin is that terminal

#### Scenario: Nothing selected
- **WHEN** the terminal pane holds no selection
- **THEN** its context menu offers no "Open Selection in Viewer"

### Requirement: Document Text Outside Snapshots
The text of a `text` document SHALL appear in no `mux.screen_snapshot` and no control API answer; the server SHALL send it only in `viewer.chunk` answers to the client connection that read it. The record SHALL otherwise appear in snapshots as it is.

#### Scenario: Snapshot of a screen with a text document
- **WHEN** a client resyncs a screen that holds a viewer pane of a text document
- **THEN** the snapshot carries the pane's `viewer` record with the source's `kind` and `title` but without its `text`
