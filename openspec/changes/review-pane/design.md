# Design

> Draft design notes from 2026-11-04; revisit against the specs before implementation.

## Context

See proposal.md for motivation and scope. Current state that shapes the approach:

- **No pane without a session.** The workspace model knows the roles `viewer` and `webview`, but no operation creates a pane without a session, and the frontend registers only the `terminal` component (dockview throws on unknown components by design). `splitty-open` and `notes-and-tags` need the same pane kind.
- **`splitty open`** is the confirmed external editor handoff (`splitty open [path] [--with vscode|zed]`, `cli` spec); `--in splitty` is reserved for an in-Splitty destination.
- **Files already move through the protocol**: drop upload (into the paste cache or a folder, never replacing a file), image paste, `splitty download`. The paste cache (`~/.cache/splitty/paste/`, cleaned after 24 hours or beyond the newest 100) is the model for a review cache, and "insert the path at the prompt" is the established hand-over to an agent.
- **The scratchpad** belongs to a session, is stored with it and is invisible to agents in panes.
- **The agent socket** (`SPLITTY_AGENT_SOCK`) scopes the CLI for agents; attachment, credential, administration and lifecycle commands are rejected.
- **No frames.** Web-app panes are gone and the CSP forbids frames (SR-01/SR-02); the desktop shells render no other origin (SR-27).

## Direction

1. **One pane for files and diffs.** Diff view, file viewer and annotations are one pane with an annotation gutter, not three features. The renderers of `splitty-open` cover the formats that are not text.
2. **Anchoring by quote.** Path, line range, quoted text and, for a diff, side and base, comparable to the text-quote selector of W3C Web Annotation. After a change the annotation is re-anchored by its quote; if the quote is gone it is marked stale and kept.
3. **The review round is the unit.** Annotations are pending until sent or discarded and live on the server, so a reload keeps them. After sending, only the cache file remains.
4. **Delivery as text, like drop and paste.** The review goes into a cache file on the target terminal's machine and its path is inserted at the prompt; a short review goes in as text; or both into the scratchpad first. Nothing presses Enter. Sending is the moment the content becomes visible to the agent, in line with the scratchpad being invisible to agents. Format sketch:

   ```markdown
   ## docs/remote.md:42-47
   > Remote sessions keep running when you close the app …
   Question: does this also hold when the host reboots?

   ## crates/splitty-server/src/ws/codec.rs:118 (diff, new side)
   > if len > MAX_FRAME {
   Suggestion: take the limit from the configuration instead of a constant.
   ```

5. **The agent enters through the CLI, not MCP.** Human and agent use the same command. Opening a pane only displays content and launches nothing native, so it needs no confirmation like `splitty open --with`. A pane an agent opens goes next to the agent's own pane on its screen and does not take focus. The agent never reads pending annotations, only what the user sends.
6. **Read-only first, quick edit later.** Writing is a new capability: a save is refused when the file changed since it was loaded (the agent may be editing the same file), only existing regular files, with a size limit.

## Open Questions

- **Editor component**: Monaco (feels like VS Code, the strongest diff editor, several MB, web workers) or CodeMirror 6 (lighter and modular, has a merge view).
- **Markdown**: annotate the rendered view (needs a mapping from rendered blocks to source lines) or the source.
- **Diff scope**: working tree, index, last commit, or everything since the agent's pane was opened (a baseline taken then); untracked files.
- **Where pending reviews live**: with the target session like the scratchpad, per screen, or per server; what happens when the target session ends; which clients of a multi-attached screen see them.
- **Review cache**: location and retention; whether an agent that reads outside its project only after a permission prompt needs the file inside the repository instead (for example a gitignored `.splitty/review/`).
- **Large and binary files**; live update while the agent edits a shown file.
- **Clients**: which may open review panes and send (the local hub, isolated remote windows, pages opened through `splitty connect`).
- **Command names**: `splitty open --in splitty`, and `splitty review` or `splitty diff` for diffs.

## Considered Alternatives

- **Splitty as a VS Code extension showing one session.** `splitty attach` in VS Code's integrated terminal already does that, also under Remote-SSH against the host's daemon. A headless attach adds nothing over tmux. Not pursued.
- **The full Splitty UI as a webview in VS Code's panel**, replacing the integrated terminal with persistent sessions and screens. Feasible, but VS Code also evaluates key presses in webviews for its own shortcuts; the webview's origin does not pass Splitty's origin and bootstrap checks, so the extension would relay the connection or, under Remote-SSH, map the port; and it would be a third frontend environment next to the browser and the desktop apps, where editor handoff, downloads and notifications each need their own path. Parked: it serves people who work mainly in VS Code, not Splitty as the main window.
- **Embedding a full VS Code** (openvscode-server or code-server on the host, shown in a pane). It reverses SR-01/SR-02 and SR-27; open builds use Open VSX and lack the extensions licensed only for Microsoft's build (Remote-SSH, Pylance, C/C++); and it adds a second server with its own authentication per host. Such a server can already be opened in a browser tab through a port forward.
- **VS Code's services around Monaco** (monaco-vscode-api, Theia): a large maintenance burden for what a review pane needs.
- **An "Open in Splitty" VS Code extension**: a small bridge from VS Code's explorer to a Splitty tab in that folder through the existing CLI. Independent of this change; only worth it once it is missed.
