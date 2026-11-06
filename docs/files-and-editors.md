# Files and editors

A terminal on a remote host is only useful if files get in and out easily.
Everything below works the same for local terminals and for terminals on an SSH
host; a host needs a current version of Splitty for it (see **Update…** in
[Remote development](remote.md#remote-screens)).

## Drop files onto a terminal

Drop files onto a terminal (or its scratchpad) to choose how to put them on that terminal's server, local or SSH host. **Temporarily store and insert paths** is the default: files land in Splitty's paste cache (normally `~/.cache/splitty/paste/`) and their paths are inserted. Alternatively choose **Upload into a folder**, check or edit the suggested working directory, and optionally insert the paths. The destination must already exist; existing files are never replaced, and filenames are sanitized (spaces and special characters become underscores). Nothing is sent until you choose **Upload**.

A progress card in the pane shows bytes and file count, with **Cancel**. Cancelling removes the unfinished file and stops the remaining files; completed files stay, and no paths are inserted for a cancelled or failed batch. Temporary files are cleaned up on later uploads after 24 hours or beyond the newest 100. Up to 512 MB per file; archive folders before dropping them. Older hosts need updating for the drop dialog's upload controls.

This is how Claude Code on a host reads a PDF or a log from your computer: drop it, and the agent gets a path it can open.

## Paste screenshots

Ctrl+V (Cmd+V on macOS) or Ctrl+Shift+V with an image in the clipboard stores the image on that pane's server, in `~/.cache/splitty/paste/` for 24 hours, and pastes the file's path. Claude Code takes a pasted image path as the image, so screenshots work in Claude Code on a host, where its own Ctrl+V finds no clipboard. The same works in a session's scratchpad, where the path lands in the text at the cursor. Text in the clipboard pastes as before, and Ctrl+V then still reaches the program as `^V`. PNG, JPEG, GIF and WebP up to 10 MB.

## Download a file from a terminal

```sh
splitty download build/report.pdf
splitty download build/            # a folder goes as build.zip
```

Splitty shows the offer with **Save** and **Dismiss** in the windows that show this terminal; Save stores the file as a download of the window, and the desktop app can then show it in the file manager. The command waits until the file is saved, dismissed or the offer expires after 60 seconds. A folder is packed into a zip first (in `~/.cache/splitty/download/`, removed when the command ends) with everything below it except symbolic links and special files; up to 512 MB per file or packed folder.

## View files in Splitty: `splitty open`

From a Splitty terminal, `splitty open <file>` shows the file in a viewer pane to the right of that terminal:

```sh
splitty open PLAN.md              # rendered Markdown, switchable to its source
splitty open report.html          # rendered HTML
splitty open src/main.rs          # source with syntax highlighting
splitty open --tab build.log      # as a tab next to the terminal instead of a split
```

Markdown and HTML are rendered, other text files are shown as source with line numbers and highlighting, and images as images. The viewer reads the file from the terminal's server, so a file on an SSH host is shown from that host. While the pane is visible it follows changes to the file within a couple of seconds, and running `splitty open` again for the same file brings its pane to the front instead of opening another one, so an agent can show its plan or report and update it as it goes. Further files from the same terminal open as tabs next to the first one, so the terminal keeps its width. Nothing needs confirming, since only the file is shown. Files up to 8 MB.

Rendered documents run no scripts and load nothing: images show only when embedded as `data:` URLs, links to websites open in the system browser, and relative links open the linked file in another viewer tab.

**A terminal selection** opens the same way: select text in a terminal, for example a long answer of an agent, right-click and choose **Open Selection in Viewer**. The pane keeps the selected text, on a daemon also through a restart; processes in panes cannot read it through `splitty`.

A viewer pane splits, stacks, floats and moves like a terminal. Splitting it opens a terminal in the file's folder.

## Open files in your editor: `splitty open --editor`

From a Splitty terminal, open folders, or files with `--editor` or `--with`, in VS Code or Zed on your computer:

```sh
splitty open                      # current directory
splitty open ../another-project
splitty open --editor src/main.rs
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
