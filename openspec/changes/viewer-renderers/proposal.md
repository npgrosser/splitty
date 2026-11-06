# Proposal

> Draft created on 2026-11-06. Not planned yet. Specs and tasks are written when this change is next. Depends on: `splitty-open` (viewer panes, the inert rendering and the read path). Related: `review-pane` (annotations on rendered documents), `review-diffs`. Carries over the concept of the earlier note on `splitty open` (a pane by default, an overlay on request, renderers chosen per format) and the renderer parts of the `splitty-open` draft.

## Why

Viewer panes show Markdown, HTML, images and text. Agents also write diagrams (Mermaid, Graphviz) into plans and documentation, produce PDFs and tables, and a quick look at a file does not always deserve a pane in the layout. These need renderers of their own and a lighter way to show a file.

## What Changes

- **Diagrams**: Mermaid and Graphviz fences in Markdown, and `.mmd` and `.dot` files, rendered to SVG in the window with libraries loaded on demand; the SVG is sanitized and shown like any rendered document, with the source one switch away.
- **PDF**: pages drawn by pdf.js in the window with its text layer, document scripts and forms off.
- **Tables**: CSV and TSV as a table with a header row, falling back to the source for files beyond a size limit.
- **`--format <name>`** for `splitty open` when the extension is ambiguous (for example `--format mermaid flow.txt`).
- **Overlay**: `splitty open --overlay <file>` shows the file in a floating viewer over the current pane, closed with `Escape` or `q`, since a viewer is not a shell.
- **Renderer lookup**: one table that maps formats to renderers, so that a renderer from outside the bundle can register later by declaring its formats.

Out of scope:
- Rendering URLs, web applications or development servers in a pane. Web-app panes and the proxy route were removed for security (SR-01, SR-02) and stay removed; links open in the system browser.
- Renderers that run a document's code (JSX, MDX, notebooks with outputs that execute). They need an opaque-origin sandbox, which the frame policy forbids today (`frame-src 'none'`, SR-27 in the desktop shells); a change of its own decides that.
- The plugin tiers sketched as an outlook (interactive web plugins with a message API, system plugins with a backend process, a plugin API package and a permission manifest). They become a change when a renderer from outside the bundle needs them.

## Capabilities

To be decided when the change is next. Likely candidates:

### Modified Capabilities
- `viewer-panes`: the presentations for diagrams, PDF and tables, the format lookup, the overlay.
- `cli`: `--format` and `--overlay` for `splitty open`.
- `access-control`: "Frontend Security Headers" if pdf.js's worker needs `worker-src` in the three policies, and the inert-rendering rules for generated SVG.

## Impact

- Frontend: lazily loaded renderers (Mermaid, a Graphviz build in WebAssembly, pdf.js), their bundle size in the desktop apps, the overlay as a floating group.
- Security: diagram libraries turn text into SVG inside the window; their output passes the same sanitizer, and none of them may evaluate the document as code. pdf.js runs with `isEvalSupported` and scripting off.
- Open questions: which Graphviz build; whether the overlay is a floating entry of the layout or a transient view outside it; how a renderer from outside the bundle is installed and trusted.
