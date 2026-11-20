# Guide

Splitty brings terminals from your own machine and from SSH hosts into one
window. They are arranged in splits and tabs on named screens. Remote sessions
keep running when you close the app or lose the connection, so you reconnect
and continue where you left off.

Working with coding agents shaped it: keeping track of many agent sessions
running in parallel, sometimes across several projects, with dev servers and
tests beside them, locally or on remote machines.

## First steps

1. Start the app. It opens one screen with one terminal and the drawer pinned
   beside it. The pin button hides the drawer; your choice is remembered.
2. Split with **Mod+D** and **Mod+Shift+D**, open a tab with **Mod+T** and a
   screen with **Ctrl+Shift+Space**. **Ctrl+Tab** goes to the next screen,
   **Mod+1** to **Mod+9** to a screen by number.
3. For a remote screen, open **Hosts** at the bottom of the drawer (or
   **Connect to Host…** in the command palette), pick a host and choose
   **New Screen**. Connect to a new host once with plain `ssh` first to accept
   its key.
4. **Mod+Shift+P** opens the command palette with every command, and
   **Ctrl+,** (macOS: **Cmd+,**) the settings.

`Mod` is Cmd on macOS and Alt on Linux and Windows. Every command, its key
and how to change it are in [Keyboard](keyboard.md).

## Templates

**Templates** (Mod+P) opens the library of saved layouts. Saving the current
screen shows its extracted layout and lets you choose which commands to keep.
Loading a template creates a new screen or replaces the current one.
Replacing ends that screen's terminals, also with **Layout only**; use **Run
displayed commands** only after reading the exact command list.

## For coding agents

- **Let an agent use Splitty.** It can start dev servers, run tests or launch
  other agents in tabs and splits beside your own terminals, where you can
  follow and step in. Add this to your `CLAUDE.md` or `AGENTS.md`:

  > Use Splitty to run commands and launch other agents in tabs and splits;
  > first read `splitty agent guide`.

- **Viewer panes.** `splitty open PLAN.md` shows a file next to the terminal,
  Markdown and HTML rendered, and follows its changes, so an agent can show
  you its plan or report.
- **Status.** A program can report whether it works, waits for you or is
  done; tabs, screens and host groups show it.
- **Scratchpad.** Each terminal has one (Mod+J) for drafting the next prompt
  while the agent works. It stays with the session, and the agent cannot read
  it.

More in [Files and editors](files-and-editors.md) and
[Managed terminal jobs](cli-jobs.md).

## Files between machines

- Drop files onto a terminal to copy them to its machine, local or remote,
  and insert their paths.
- Paste a screenshot with Ctrl+V (macOS: Cmd+V). It is stored on the
  terminal's machine, so an agent on a remote host can read it.
- `splitty download <file>` in a terminal offers the file to save on your
  computer.
- A terminal's context menu opens its folder in VS Code or Zed on your
  computer, remote folders included.

## How it works

| Connection | Transport | Session lifetime |
|---|---|---|
| Desktop, local terminals | In-process service on macOS and Linux, owned child pipes to the WSL backend on Windows | End when the app quits |
| Explicit local daemon | Private Unix socket | Independent of the app |
| SSH host | SSH data channel to the host's private socket | Survive disconnects and app exit |

- The server holds the layout and the terminals, so several windows can show
  the same screens and stay in sync.
- The desktop app owns its local backend and closes its local sessions when
  you quit. A separately started daemon runs independently. On an SSH host,
  Splitty runs as a daemon and keeps each session in a small keeper process,
  so sessions survive disconnects and daemon restarts or updates.
- For remote screens, the local server is a hub: it installs and starts
  Splitty on the host and relays it over SSH.
- The server and the desktop app are Rust. The server uses Tokio, Axum and
  portable-pty; the desktop app is built with [GPUI](https://www.gpui.rs) and
  parses terminal output with libghostty-vt, Ghostty's terminal library. On
  Windows the server runs in WSL 2.

More in [Architecture](architecture.md).

## Security

Whoever controls Splitty gets a shell with your rights. Desktop connections
use in-process channels or owned pipes; daemons use private Unix sockets, and
remote connections travel over SSH. The terminal service has no TCP listener.
Owner, grant and keeper sockets use private paths and peer checks. Deliberate
application forwards and reverse shares have separate admission rules.
Processes running as your own user are not isolated from Splitty.

The full model is in [`SECURITY.md`](../SECURITY.md). Please report
vulnerabilities privately through
[GitHub security advisories](https://github.com/npgrosser/splitty/security/advisories/new).

## Limitations

- The binaries are not code-signed (see [Install](install.md)).
- On Windows, local terminals need WSL 2. Without a supported distribution
  the app says so and does not start.
- On a Mac host, Splitty does not detect listening ports yet; forward them by
  number.
