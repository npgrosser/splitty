## ADDED Requirements

### Requirement: Viewer Documents Stay With Their Connection
A client SHALL read a viewer pane's document only over the connection whose screen holds the pane, and SHALL show it only in that pane. A server SHALL read for a viewer pane only the file its record names, only to answer `viewer.read` on the connection that sent it, and SHALL never write, create or delete a file for a viewer pane. Every client connection of a server MAY open viewer panes and read their documents, the authority it already has by starting a terminal on that server. The control API, and with it the processes in panes, MAY open viewer panes but SHALL NOT read a document's content or a `text` document's text. A restricted agent grant socket SHALL do neither.

#### Scenario: A host names a local path
- **WHEN** a host's snapshot holds a viewer pane naming `/home/me/.ssh/id_ed25519` and the window shows that host's screen next to local screens
- **THEN** the window reads the document only over the host's connection, the local server reads nothing, and the host answers from its own file system

#### Scenario: A process in a pane reads the workspace
- **WHEN** a viewer pane shows a terminal selection and a process in a pane sends `GET /api/mux/state` over the owner control socket
- **THEN** the answer holds the viewer pane without the selected text

#### Scenario: Grant socket
- **WHEN** an agent grant socket receives `POST /api/mux/op` with `open_viewer`
- **THEN** it answers 404 and no pane appears

### Requirement: Rendered Documents Are Inert
A client SHALL render a Markdown or HTML document without executing any script, event handler attribute or `javascript:` URL in it, without frames, objects, embedded documents or forms, and without loading anything over the network or from Splitty's own origin: images SHALL show only from `data:` URLs. The rendered document SHALL stay inside its pane: its styles SHALL apply only within the rendered document, and nothing it positions SHALL cover anything outside the pane. A link SHALL open an `http:` or `https:` URL through the external opener, a relative link the named file in a viewer pane, and a link to an anchor the position within the document; any other link SHALL do nothing. Rendering SHALL need no change to Frontend Security Headers: `frame-src 'none'` stays.

#### Scenario: A script in an HTML report
- **WHEN** a viewer pane renders an HTML file holding `<script>`, an `onerror` attribute and a `javascript:` link
- **THEN** no script runs and the link does nothing when clicked

#### Scenario: An image from the server's origin
- **WHEN** a Markdown document holds `![x](/api/mux/state)`
- **THEN** the window sends no request for it and shows the image's alt text

#### Scenario: A load hidden in a style
- **WHEN** an HTML document's style sheet or `style` attribute holds `background: var(--x, url(/api/mux/state))` or a `url()` spelled with CSS escapes inside `var()`
- **THEN** the window sends no request for it

#### Scenario: An overlay
- **WHEN** an HTML document holds an element with `position: fixed` covering the whole viewport
- **THEN** that element stays inside the viewer pane, and the rest of the window remains usable

#### Scenario: A link to a website
- **WHEN** the user clicks an `https:` link in a rendered document
- **THEN** the system browser opens it, and the window does not navigate
