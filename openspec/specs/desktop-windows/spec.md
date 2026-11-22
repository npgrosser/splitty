# desktop-windows Specification

## Purpose
The windows of the desktop application: they are views of one client, each open Screen is held by exactly one of them, a Screen moves between them without its server hearing of it, and a window opens and closes without ending what the others show.

## Requirements

### Requirement: One Client For All Windows
The desktop application SHALL show its workspace in one or more windows of one process. All windows SHALL share one local service and, for each server (the local service and every connected host), one connection, so a server sees one client however many windows there are. Opening a window SHALL start no service and open no connection, and closing one SHALL end none while another window stays open.

#### Scenario: Second window
- **WHEN** the user opens a second window while the first holds a local screen and a screen of `vm1`
- **THEN** neither the local service nor `vm1` sees a new connection

### Requirement: State Shared By All Windows
Settings, appearance, zoom and key bindings SHALL apply to every window of the application. A question of the local service (see `remote-hosts`, SSH Questions Of The Local Service) SHALL be asked once, not once per window.

#### Scenario: Setting changed in one window
- **WHEN** the user changes the theme in one of two windows
- **THEN** both windows show the new theme

#### Scenario: SSH question with two windows
- **WHEN** the local service asks for a passphrase while two windows are open
- **THEN** one dialog asks, and its answer or dismissal goes back once

### Requirement: Screens Held By Windows
Every screen the application has open, a local screen or a reference to a host's screen, SHALL be held by exactly one of its windows. A window's screen list, shortcut numbers and screen navigation SHALL cover only the screens it holds: local screens in the service's order and a host's screens in the order of their references. Each window SHALL have its own shown screen, which no other window's choice changes.

#### Scenario: Independent selection
- **WHEN** window A holds `work` and `logs`, window B holds `notes`, and the user switches A from `work` to `logs`
- **THEN** B keeps showing `notes`
- **AND** the command for the first screen shows `work` in A and `notes` in B

#### Scenario: Order of a window's local screens
- **WHEN** the service's order is `work`, `notes`, `logs` and window A holds `work` and `logs`
- **THEN** A lists `work` before `logs`, and moving `logs` before `work` in A makes the service's order start with `logs`, `work`

### Requirement: Window Of A New Screen
A screen created or opened from a window SHALL be held by that window. A screen that enters the workspace otherwise (through the CLI, the control API or another client) SHALL be held by the window that was active last. Opening a screen that another window holds SHALL activate that window with the screen shown instead of holding the screen a second time.

#### Scenario: Screen created in a window
- **WHEN** the user creates a screen in window B while window A is open
- **THEN** the screen appears in B's screen list and not in A's

#### Scenario: Screen created through the CLI
- **WHEN** a pane's process creates a screen with the CLI while window B was active last
- **THEN** the screen appears in B's screen list and not in A's

#### Scenario: Screen held by another window
- **WHEN** the user opens, from window A, a screen of `vm1` that window B holds
- **THEN** B becomes the active window and shows that screen
- **AND** A's screen list is unchanged

### Requirement: Moving A Screen Between Windows
The user SHALL be able to move a screen to another window of the application or to a new one, with `screen.move-to-next-window`, `screen.move-to-new-window` and the screen's menu in the screen list. The moved screen SHALL be shown in its new window at the place the orders of Screens Held By Windows give it, and the window it left SHALL show a neighboring screen or close as Closing A Window says.

#### Scenario: Move to another window
- **WHEN** window A shows `logs` and also holds `work`, and the user moves `logs` to window B
- **THEN** B holds and shows `logs`, and A shows `work`

#### Scenario: Move to a new window
- **WHEN** the user runs `screen.move-to-new-window` for one of the two screens of a window
- **THEN** a new window opens that holds and shows that screen
- **AND** the first window shows its other screen

### Requirement: A Move Changes No Server State
A move SHALL change only which window holds the screen: the application SHALL send no workspace operation, SHALL neither detach nor attach a session, and SHALL keep each terminal's content, history and modes as they were. The only messages a move causes SHALL be the size reports of the screen's panes at their size in the new window (see `terminal-sessions`, Client Size Reports).

#### Scenario: Running program
- **WHEN** a screen whose pane runs a program and holds 5000 lines of history moves to another window
- **THEN** the program keeps running and its server receives no workspace operation, `detach` or `attach`
- **AND** the pane shows the same content and scrolls back through the same history in the new window

#### Scenario: Remote screen
- **WHEN** a screen of `vm1` moves to another window
- **THEN** `vm1`'s connection stays as it is and the screen appears in `vm1`'s group in its new window

### Requirement: Opening A Window
`window.new` SHALL open another window of the application that holds one new local screen, created as any new local screen is. A window opened for a moved screen (`screen.move-to-new-window`) SHALL hold only that screen. A new window SHALL NOT change the screens or the shown screen of the windows already open.

#### Scenario: New window
- **WHEN** the user runs `window.new` in a window that shows `work`
- **THEN** a second window opens showing a new local screen that only it holds
- **AND** the first window still shows `work`

### Requirement: Closing A Window
Closing a window (its close control or `window.close`) while the application has another one SHALL end nothing and ask nothing: its screens SHALL be handed to the window that was active last among the others, each with its sessions and terminal state, and the receiving window SHALL keep its shown screen. A window whose last screen was closed or moved away SHALL close unless it is the application's last, which stays open without a screen.

#### Scenario: One of two windows closed
- **WHEN** window B, which holds `notes` with a running program, is closed while window A is open
- **THEN** B closes without a question
- **AND** `notes` is in A's screen list with its program running, and A still shows the screen it showed

#### Scenario: Last screen of a window
- **WHEN** the only screen of window B is closed or moved to window A
- **THEN** B closes and A stays open

#### Scenario: Last screen of the last window
- **WHEN** the only screen of the application's only window is closed
- **THEN** the window stays open without a screen and offers to create one

### Requirement: Closing The Application
Closing the application's last window, and `window.quit` in any window, SHALL close the application for all windows as `client-connections` (Native Local Terminal Close Assessment, Native Owned Window Close Acceptance) and `remote-screens` (Collapse And Terminate) specify.

#### Scenario: Quit with two windows
- **WHEN** `window.quit` is confirmed while two windows are open
- **THEN** both windows close and the application ends

#### Scenario: Quit declined
- **WHEN** `window.quit` asks because a terminal runs a program and the user declines
- **THEN** every window stays open with its screens
