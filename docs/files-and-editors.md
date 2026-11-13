# Files and editors

A terminal on a remote host is only useful if files get in and out easily.
Everything below works the same for local terminals and for terminals on an SSH
host; a host needs a current version of Splitty for it (see **Update…** in
[Remote development](remote.md#remote-screens)).

## Drop files onto a terminal

Drop files onto a terminal (or its scratchpad) to put them on that terminal's server, local or SSH host. A notice asks first and sends nothing before you choose: **Send** stores the files in Splitty's paste cache (normally `~/.cache/splitty/paste/`) and inserts their paths; **Keep in folder**, offered when the terminal's working directory is known, stores them there instead and inserts those paths. Existing files are never replaced, and filenames are sanitized (spaces and special characters become underscores). **Cancel**, or no answer within a minute, drops the request.

A larger transfer shows its progress in a notice, with **Cancel**. Cancelling removes the unfinished file and stops the remaining files; completed files stay, and no paths are inserted for a cancelled or failed batch. Temporary files are cleaned up on later uploads after 24 hours or beyond the newest 100. Up to 512 MB per file; archive folders before dropping them. Older hosts need updating to keep files in a folder.

This is how Claude Code on a host reads a PDF or a log from your computer: drop it, and the agent gets a path it can open.

## Paste screenshots

Ctrl+V (Cmd+V on macOS) or Ctrl+Shift+V with an image or copied files in the clipboard stores them on that pane's server, in `~/.cache/splitty/paste/` for 24 hours, and pastes their paths. Claude Code takes a pasted image path as the image, so screenshots work in Claude Code on a host, where its own Ctrl+V finds no clipboard. The same works in a session's scratchpad, where the path lands in the text at the cursor. Text in the clipboard pastes as before, and Ctrl+V then still reaches the program as `^V`. PNG, JPEG, GIF and WebP up to 10 MB.

## Download a file from a terminal

```sh
splitty download build/report.pdf
splitty download build/            # a folder goes as build.zip
```

Splitty shows the offer with **Save** and **Dismiss** in the windows that show this terminal. The app saves the file into your Downloads folder and then offers **Show in folder**; a browser stores it as a download. The command waits until the file is saved, dismissed or the offer expires after 60 seconds. A folder is packed into a zip first (in `~/.cache/splitty/download/`, removed when the command ends) with everything below it except symbolic links and special files; up to 512 MB per file or packed folder.

## View files in Splitty: `splitty open`

From a Splitty terminal, `splitty open <file>` shows the file in a viewer pane to the right of that terminal:

```sh
splitty open PLAN.md              # rendered Markdown, switchable to its source
splitty open report.html          # rendered HTML
splitty open src/main.rs          # source with syntax highlighting
splitty open --tab build.log      # as a tab next to the terminal instead of a split
```

Markdown and HTML are rendered, other text files are shown as source with line numbers and highlighting, and images as images. The viewer reads the file from the terminal's server, so a file on an SSH host is shown from that host. While the pane is visible it follows changes to the file within a couple of seconds, and running `splitty open` again for the same file brings its pane to the front instead of opening another one, so an agent can show its plan or report and update it as it goes. Further files from the same terminal open as tabs next to the first one, so the terminal keeps its width. Nothing needs confirming, since only the file is shown. Files up to 8 MB.

Rendered documents run no scripts and load nothing. In the app their links open nothing. In the [browser UI](cli.md#browser-ui), images show only when embedded as `data:` URLs, links to websites open in the browser, and relative links open the linked file in another viewer tab.

**A terminal selection** opens the same way: select text in a terminal, for example a long answer of an agent, right-click and choose **Open Selection in Viewer**. The pane keeps the selected text, on a daemon also through a restart; processes in panes cannot read it through `splitty`.

A viewer pane splits, stacks, floats and moves like a terminal. Splitting it opens a terminal in the file's folder.

## Review a document

**Comments** in the viewer pane's bar opens its annotation panel. Select a passage, rendered or as source, and write a remark or a question in the panel; **Suggest change** adds a replacement for the passage (empty it to suggest deleting the passage), and **Comment only** takes it away again. `Ctrl+Enter` (`Cmd+Enter` on macOS) saves. Without a selection, the comment is a general comment on the whole document.

The panel lists each annotation with its number, its lines and its thread of replies, the open ones first and the resolved ones after them. Choosing an entry, or a marked passage in the document, selects its thread. When the file changes, annotations find their passage again by its text; a reworded passage is marked as changed where it most likely is now, and one that is gone stays in the list, marked as not found. **Move to selection** gives an annotation the passage selected in the document. **Resolve** keeps an annotation with its thread but leaves it out of reviews and marks; **Reopen** brings it back. **Hide marks** hides the marks in the document, except for the annotation chosen in the list; **Mark resolved too** marks the resolved ones as well. Both change only what this window shows. Annotations are kept with the pane, also on a daemon through a restart.

**Copy review** copies the review as Markdown. **Send to terminal** pastes it into the terminal the document was opened from, without Enter, so you can read it and add to it before submitting. **Send to…** lists the other ways for each terminal of the screen: pasted into it, as a file whose path is inserted at its prompt, or into its scratchpad. The review is Markdown: the document's path, then each annotation with its number and lines, the quoted passage, your remark, a suggested replacement and the replies of its thread, and a last line naming the command an agent replies with. It holds what you have not sent yet: new and edited annotations, and threads with a new reply of yours. Sending keeps the annotations and marks what the review held as sent; copying marks nothing.

An agent in a pane of the same screen reads what you sent and replies in a thread:

```sh
splitty comments PLAN.md                  # the sent annotations of PLAN.md with their threads
splitty comments reply PLAN.md 2 'Switched to on-demand computation.'
```

The reply appears in the thread as plain text, with the title of the agent's pane; answer it in the thread and send again. `splitty comments` shows an agent only what you sent, and an agent adds nothing but replies. That keeps drafts out of an agent's context; it is no boundary against your own programs, which can still read the workspace file, close the pane or type into a terminal. `splitty comments` lists the open annotations, `--all` the resolved ones as well, and `--json` prints the server's answer. An agent names the document by its path or by the viewer pane's ID, which the review's last line uses; a terminal selection shown in a viewer has only the ID.

An agent can also wait for your verdict on a file, or on a text it pipes in:

```sh
splitty review PLAN.md
splitty review --text --title Plan < plan.md
```

The pane's bar then reads **Review waiting**, and its annotation panel starts with `<title> waits for your review` and two verdicts:

- **Approve** lets the command go on with status 0. Comments not yet sent go along as notes, and the command prints them as a review; with none it prints nothing.
- **Request changes** answers with the review, which the command prints, and ends it with status 3. It also works without any comment, as a plain rejection.

A general comment written before the verdict goes along with the answer. The review starts with its verdict (`Verdict: approved` or `Verdict: changes requested`) and lists the general comments before the comments on passages; it notes when the file changed while the command waited. The answered annotations are marked as sent. `--json` prints the outcome as JSON with the same status. Closing the pane first ends the command with status 1. In the browser UI, the pane's tab shows an hourglass while a command waits. A text piped in is rendered as Markdown, or shown as plain text with `--format text`. Piped in again from the same pane with the same title, it replaces the text of its viewer pane, which keeps its annotations; while a command still waits for that pane, a second one fails and changes nothing. Two commands started at once cannot take each other's answer: the one whose text the other replaced before it waited fails.

### Plans of Claude Code

As a hook of Claude Code, `splitty review` takes over the approval of a plan when Claude Code leaves plan mode. Add this to Claude Code's settings (`~/.claude/settings.json`, or `.claude/settings.json` of a project):

```json
{
  "hooks": {
    "PermissionRequest": [
      {
        "matcher": "ExitPlanMode",
        "hooks": [
          { "type": "command", "command": "splitty review --hook claude-code", "timeout": 86400 }
        ]
      }
    ]
  }
}
```

When Claude Code in a Splitty terminal asks to approve its plan, the plan opens in a viewer pane next to it, titled `Plan`, instead of Claude Code's own dialog:

- **Approve** leaves plan mode. Comments not yet sent go along as notes: Claude Code gets the plan followed by a rule and the review.
- **Request changes** keeps Claude Code in plan mode with the review as the reason; its next plan replaces the text of the same pane, which keeps the annotations and their threads.

Wherever the hook cannot review, it decides nothing, and Claude Code shows its own dialog: outside a Splitty terminal, for another event or tool, when the server is too old, when the pane is closed before an answer or the connection ends. The hook never approves on its own. The `timeout`, in seconds, is how long Claude Code waits for your verdict before it falls back to its dialog. `PreToolUse` works as the event as well; it runs on every call of the tool, also where Claude Code's permission mode would not ask. The plan is taken from the tool's input, or from the plan file it names.

Terminal selections opened in a viewer can be annotated the same way, for example to answer a long reply of an agent point by point.

## Open files in your editor: `splitty open --editor`

From a Splitty terminal, open folders, or files with `--editor` or `--with`, in VS Code or Zed on your computer:

```sh
splitty open                      # current directory
splitty open ../another-project
splitty open --editor src/main.rs
splitty open --with zed .
```

Confirm the request with **Open** in the notice that appears; it expires after at most 30 seconds. For remote screens, the path stays on the SSH host and your local editor connects there. Pages opened through `splitty connect` cannot request editor launches; use a remote screen instead. If the same terminal is attached in several capable clients, detach the others before retrying. Old host versions need updating.

The `editors` settings in `config.toml` choose the editor: file and folder defaults (`vscode` initially), extension rules and project rules. The app has no settings page for them yet; the browser UI edits them under Settings → **Editors**. Project connections are `local` or `host:<exact SSH target>`, for example `host:me@devbox`; paths are absolute directories on that connection. Selection order is `--with`, the longest matching project directory, extension, then file/folder default. Every request still asks for confirmation. Settings belong to the local hub.

Install the selected editor locally; VS Code needs Remote - SSH or its WSL extension as applicable. Zed handoff for local WSL paths is unavailable. SSH targets with explicit ports/IPv6 need an alias in the local editor's SSH config. Files containing colons cannot be handed off unambiguously. Only existing regular files and folders are supported; no downloads or arbitrary app commands run. CLI success means the handoff was dispatched, not that the editor connected.

## Open the current folder: the terminal's context menu

Right-click a terminal and choose **Open Folder in → VS Code / Zed**. This works for local and remote terminals, in the app and in the browser UI. Splitty uses that terminal's current directory and, for remote terminals, its owning SSH target. Without a known working directory, Splitty says so instead of opening anything. Install the chosen editor locally and allow the system's prompt to open it; handing off the request does not confirm that the editor opened or connected. VS Code remote folders need [Remote - SSH](https://code.visualstudio.com/docs/remote/ssh); [Zed has built-in SSH support](https://zed.dev/docs/remote-development). Windows editors use SSH configuration separate from WSL. For explicit SSH ports or IPv6 literals, configure the same SSH alias locally and in Splitty, then reconnect using it. VS Code also needs aliases to contain a non-hex character.

Local WSL folders open through the server's distribution in VS Code (requires its WSL extension). Zed’s URL handler cannot open a WSL connection yet, so that entry is disabled with guidance to use Zed’s Remote Projects.

**SFTP:** in a remote terminal's menu, **Open Folder in → SFTP Client** opens its folder as `sftp://user@host:port/path/` in whatever opens `sftp:` links on this computer: [WinSCP](https://winscp.net) or [Cyberduck](https://cyberduck.io) on Windows, Cyberduck on macOS, the file manager on Linux. Ports and IPv6 addresses go into the link as they are. An SSH alias stays an alias, so the client has to resolve it itself, which clients that read the SSH configuration do (on Windows the Windows one, not WSL's); otherwise connect Splitty to `user@host`. Splitty only hands the link over and cannot tell whether a client opened it.

**Copy SSH Path** in a remote terminal's menu copies its folder as `user@host:/path` (or `ssh://user@host:port/path`), the form `scp`, `rsync` and `sshfs` take; an SSH alias stays an alias.
