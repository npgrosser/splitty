## MODIFIED Requirements

### Requirement: Open In Client Editor
`splitty open [path] [--with vscode|zed] [--editor]` SHALL request opening an existing folder, or with `--with` or `--editor` an existing file, in the attached client editor; `--editor` uses the client's preferred editor. Omitted path means `.`; relative paths SHALL resolve against the caller's current directory on the originating machine. The command SHALL use `SPLITTY_SESSION_ID` or explicit `--session` and existing control discovery. A file without `--with` and `--editor` goes to a viewer pane instead (see Open In A Viewer Pane). Shell integration SHALL NOT define a `splitty-open` function or any other alias for it.

#### Scenario: Relative file and default folder
- **WHEN** a pane invokes `splitty open --editor src/main.rs`, or invokes `splitty open` without a path
- **THEN** it requests respectively the resolved file or current directory of the caller in the client editor, never a path on the client PC for a remote session

#### Scenario: Invalid input or missing context
- **WHEN** the file is missing, special, unsupported, or the session is absent, or an unknown editor is selected
- **THEN** the command exits unsuccessfully with an explanation without requesting a launch

#### Scenario: Outcome is a handoff
- **WHEN** the UI accepts, rejects, disconnects, expires, or cannot dispatch the request
- **THEN** the CLI reports the corresponding outcome and exits zero only for dispatched handoff, without claiming the editor connected

#### Scenario: No shell wrapper
- **WHEN** an integrated Bash or Zsh started by Splitty runs `splitty-open`
- **THEN** the shell reports that the command is not found, as for any unknown command

## ADDED Requirements

### Requirement: Open In A Viewer Pane
`splitty open <path>` SHALL show an existing regular file in a viewer pane when neither `--with` nor `--editor` is given (see `viewer-panes`). The CLI SHALL resolve the path against its current directory, following symbolic links, refuse anything but a regular file and a file over 8 MiB with a usage error, and submit `open_viewer` without `newPaneId` through the mux operation endpoint (see `control-api`) for the pane and screen given by `--pane` and `--screen`, else by `SPLITTY_PANE_ID` and `SPLITTY_SCREEN_ID`, with `placement` `right`, or `tab` with `--tab`. It SHALL ask for no confirmation and wait for no window, print `Opened <path> in pane <pane-id>`, or `Showing <path> in pane <pane-id>` when an existing viewer pane shows the file again, and exit with status 0. A server that does not know `open_viewer` SHALL be reported as needing an update. Under `SPLITTY_AGENT_SOCK` the command SHALL stay refused (see Scoped Administrative Commands Are Refused).

#### Scenario: A plan next to the agent
- **WHEN** a process in pane `p1` runs `splitty open PLAN.md`
- **THEN** a viewer pane showing the resolved file appears to the right of `p1`, and the command prints `Opened /home/me/app/PLAN.md in pane <id>` and exits 0

#### Scenario: Opened again after an edit
- **WHEN** the same process runs `splitty open PLAN.md` again while that viewer pane exists
- **THEN** no further pane appears, the existing one shows the file again, and the command prints `Showing /home/me/app/PLAN.md in pane <id>`

#### Scenario: As a tab
- **WHEN** a process in pane `p1` runs `splitty open --tab notes.txt`
- **THEN** the viewer pane becomes the active tab of `p1`'s slot

#### Scenario: A folder still goes to the editor
- **WHEN** a pane runs `splitty open src`
- **THEN** the CLI requests the folder in the client editor as before

#### Scenario: Too large
- **WHEN** a pane runs `splitty open build/app.log` for a file of 20 MiB
- **THEN** the command fails with a usage error naming the limit, and no pane appears
