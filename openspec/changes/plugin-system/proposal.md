# Proposal

> Draft created on 2026-11-12. Not planned yet. It is the third of three packages, after `session-stability-and-status` and `server-terminal-state`. Specs, design and tasks are written when this change is next; it will most likely be split into one change per stage. Related: `mcp-server`, `viewer-renderers`, `viewer-webview`, `viewer-sandboxed-html`, `terminal-intelligence` (its parser boundary was shaped for a later plugin interface), `notes-and-tags`.

## Why

Splitty's core is a terminal multiplexer whose sessions live on a server. Much of what is asked of it beyond that belongs to one tool, one workflow or one service: an integration with a particular coding agent, a notification to a phone, a worktree per task, a status line from a CI system, a renderer for one file type. Built into the core, each of these is code maintained against someone else's release schedule, and each widens what a host's daemon does. Left out, there is no way to add them.

A plugin system lets such features exist without being part of the core. It also gives a rule for what stays out: an integration with an individual agent or service is a plugin.

## What Changes

Principles:

- **No Rust and no build step**: a plugin is a folder, in practice a repository, with a manifest and files in a dynamically loaded language. Plugin authors never compile against Splitty's crates.
- **Plugins run where the sessions are**: a plugin's logic runs with the server it extends, the desktop's own backend or a host's daemon. It then works for hosts and while no window is open, and a host needs nothing installed for it beyond Splitty.
- **Contributions are data that every client draws**: commands, menu entries, key bindings, status items and badges are announced by the server, and the native window and the web shells draw them in their theme.
- **Standards before plugins, plugins before core**: what a program can say through its terminal (title, prompt marks, working directory, links, program status) needs no plugin. What needs knowledge of one tool is a plugin.
- **Authority is declared and granted**: a manifest lists what the plugin needs. Data installs without a question; code that runs on its own is approved per server. A program a plugin starts has the user's rights, and the interface says so instead of promising a sandbox.

Stages, each usable on its own:

1. **Contributions without a runtime**: a manifest declares commands (palette, context menus, key bindings) that run a program with the pane's context; hooks on events (a session starts or ends, a program's status changes, a notification is raised, a screen is created); status items a program updates through the CLI; and data such as templates, themes, shell profiles, link patterns and renderers that map a file type to a converter. The interface is the CLI and the control API. A plugin's own user interface is a terminal program in a pane or a floating pane.
2. **An embedded scripting runtime**: plugin logic inside the server process, with no interpreter on the host, against a host interface with capabilities (panes and sessions, status, notifications, storage, settings, processes, network).
3. **Components from scripts**: a script describes a view as a tree from a fixed catalog (lists, rows, text, buttons, inputs, badges, progress). Clients draw it with their own widgets and theme, and events return to the script. Because the tree is data, the script runs with the server and the view appears in every client.
4. **Web panes**: free-form user interface in a web view on an origin of its own, with no access to Splitty beyond a narrow message channel and with the theme's tokens as style variables. It rests on the isolation `viewer-webview` and `viewer-sandboxed-html` define.

Candidates to prove the interface with: pasting a Windows path converted for WSL; a message to a phone when a program is blocked; a worktree with a screen from a template and an agent in it; connection providers that reach a container, a pod or a virtual machine through a command yielding a byte stream to the host bridge; status from Git, CI or usage limits; pickers for sessions, hosts and history as terminal programs in a floating pane; renderers for diagrams, PDF, tables and notebooks; task sources (pick an issue, start an agent with it); and a source of program status for agents that do not report, fed by a local tool that observes them out of band.

Out of scope:
- Plugins as native code or compiled against the Rust crates.
- A sandbox for programs a plugin starts.
- A hosted registry; repositories and a catalog file come first.

## Capabilities

To be decided when the change is next. Likely new: `plugins` (manifest, installation, authority, lifecycle) and one capability per kind of contribution. Likely modified: `control-api`, `cli`, `access-control`, `settings`, `mux-protocol` or `client-connections`.

## Impact

- A contract: every command, event and field a plugin may use becomes versioned and stable. That is the main cost and the reason to start with what is already public.
- Security: plugin code on hosts, approval per server and per version, untrusted text from plugins shown in clients.
- Hosts: how a plugin's files reach a host's server.
- Clients: drawing contributed commands, status items and, from stage 3, component trees; from stage 4 a web view in the native window.

## Open questions for the design

- The language of the embedded runtime. Candidates are Lua and JavaScript; the criteria are size and build for every host target, sandboxing and limits, typing, and how well people and agents write it.
- Whether web panes come before components from scripts.
- How a plugin reaches a host: installed per host, or carried by the client like the server payload.
- Which parts of the CLI and control API become the versioned contract, and how a plugin declares the versions it needs.
- What data-only means exactly, and what an approval covers (plugin, version, server).
- Distribution: a catalog file, pinned revisions, updates.
- Which contributions the web shells draw.
