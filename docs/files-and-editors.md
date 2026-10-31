# Files and editors

A terminal on a remote host is only useful if files get in and out easily.
Everything below works the same for local terminals and for terminals on an SSH
host; a host needs a current version of Splitty for it (see **Update…** in
[Remote development](remote.md#remote-screens)).

## Drop files onto a terminal

Drop files onto a terminal (or its scratchpad) to put them on that terminal's server, local or SSH host: each file lands in Splitty's paste cache (normally `~/.cache/splitty/paste/`) for 24 hours, and its path is inserted. Up to 512 MB per file.

This is how Claude Code on a host reads a PDF or a log from your computer: drop it, and the agent gets a path it can open.

## Paste screenshots

Ctrl+V (Cmd+V on macOS) or Ctrl+Shift+V with an image in the clipboard stores the image on that pane's server, in `~/.cache/splitty/paste/` for 24 hours, and pastes the file's path. Claude Code takes a pasted image path as the image, so screenshots work in Claude Code on a host, where its own Ctrl+V finds no clipboard. The same works in a session's scratchpad, where the path lands in the text at the cursor. Text in the clipboard pastes as before, and Ctrl+V then still reaches the program as `^V`. PNG, JPEG, GIF and WebP up to 10 MB.

## Download a file from a terminal

```sh
splitty download build/report.pdf
splitty download build/            # a folder goes as build.zip
```

Splitty shows the offer with **Save** and **Dismiss** in the windows that show this terminal; Save stores the file as a download of the window, and the desktop app can then show it in the file manager. The command waits until the file is saved, dismissed or the offer expires after 60 seconds. A folder is packed into a zip first (in `~/.cache/splitty/download/`, removed when the command ends) with everything below it except symbolic links and special files; up to 512 MB per file or packed folder.

## Open files in your editor: `splitty open`

From a Splitty terminal, open files or folders in VS Code or Zed on your computer:

```sh
splitty open                      # current directory
splitty open ../another-project
splitty open src/main.rs
splitty open --with zed .
```

Confirm the request in Splitty within 30 seconds. For remote screens, the path stays on the SSH host and your local editor connects there. Use remote screens in the **local hub**: isolated host windows and pages opened through `splitty connect` cannot request native editor launches. If the same terminal is attached in several capable clients, detach the others before retrying. Old host versions need updating.

Settings → **Editors** configures file and folder defaults (VS Code initially), extension rules, and project rules. Project connections are `local` or `host:<exact SSH target>`, for example `host:me@devbox`; paths are absolute directories on that connection. Selection order is `--with`, the longest matching project directory, extension, then file/folder default. Every request still asks for confirmation. Settings belong to the local hub.

Install the selected editor locally; VS Code needs Remote - SSH or its WSL extension as applicable. Zed handoff for local WSL paths is unavailable. SSH targets with explicit ports/IPv6 need an alias in the local editor's SSH config. Files containing colons cannot be handed off unambiguously. Only existing regular files and folders are supported; no downloads or arbitrary app commands run. CLI success means the handoff was dispatched, not that the editor connected.

## Open the current folder: the terminal's context menu

Right-click a terminal or its tab and choose **Open Folder in → VS Code / Zed**. This works for local and remote terminals in the browser and desktop apps. Splitty uses that terminal's current directory and, for remote terminals, its owning SSH target. Missing directories disable the editor entries with an explanation. Install the chosen editor locally and allow the browser/OS prompt; handing off the request does not confirm that the editor opened or connected. VS Code remote folders need [Remote - SSH](https://code.visualstudio.com/docs/remote/ssh); [Zed has built-in SSH support](https://zed.dev/docs/remote-development). Windows editors use SSH configuration separate from WSL. For explicit SSH ports or IPv6 literals, configure the same SSH alias locally and in Splitty, then reconnect using it. VS Code also needs aliases to contain a non-hex character. Isolated remote windows receive no native editor permission.

Local WSL folders open through the server's distribution in VS Code (requires its WSL extension). Zed’s URL handler cannot open a WSL connection yet, so that entry is disabled with guidance to use Zed’s Remote Projects.

**SFTP:** in a remote terminal's menu, **Open Folder in → SFTP Client** opens its folder as `sftp://user@host:port/path/` in whatever opens `sftp:` links on this computer: [WinSCP](https://winscp.net) or [Cyberduck](https://cyberduck.io) on Windows, Cyberduck on macOS, the file manager on Linux. Ports and IPv6 addresses go into the link as they are. An SSH alias stays an alias, so the client has to resolve it itself, which clients that read the SSH configuration do (on Windows the Windows one, not WSL's); otherwise connect Splitty to `user@host`. Splitty only hands the link over and cannot tell whether a client opened it.

**Copy SSH Path** in a remote terminal's menu copies its folder as `user@host:/path` (or `ssh://user@host:port/path`), the form `scp`, `rsync` and `sshfs` take; an SSH alias stays an alias.
