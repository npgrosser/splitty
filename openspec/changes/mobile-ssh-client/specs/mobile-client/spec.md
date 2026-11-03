# Spec Delta

## Purpose

Defines a native, remote-only mobile client's connection, presentation and input semantics for operating existing Splitty sessions from Android smartphones and tablets. iOS is outside the scope of this release.

## ADDED Requirements

### Requirement: Remote-Only Connection
The mobile client SHALL connect to a saved SSH host and use `splitty remote-bridge` to reach an already running compatible daemon. It MUST NOT start local shells, expose the daemon to the network, or automatically install, restart or upgrade the host's server. It SHALL distinguish connectivity, authentication, host verification, missing daemon and protocol compatibility failures.

#### Scenario: Existing remote workspace
- **WHEN** the user connects to a verified host with a compatible running daemon
- **THEN** its existing screens and sessions are available without creating replacement sessions or starting a local backend

#### Scenario: Host is unavailable or incompatible
- **WHEN** SSH is unreachable, authentication fails, verification fails, the daemon is missing, or `session-control-v1` is absent
- **THEN** the client reports the corresponding failure and sends no terminal input or destructive repair action

### Requirement: Device-Local Workspace Projection
Mobile presentation SHALL preserve server screen, pane and session identities. Selecting a terminal, rotating the device or changing presentation settings MUST NOT rewrite the shared layout, send shared activation ops or close sessions. All panes, including tab stacks and floating panes, SHALL remain addressable. Explicit creation and termination SHALL use the existing server-owned lifecycle.

#### Scenario: View a desktop split on a phone
- **WHEN** a phone displays and switches between terminals of a split screen containing tabbed and floating panes
- **THEN** their IDs and desktop layout remain unchanged and no session is closed by view replacement

#### Scenario: Create a terminal deliberately
- **WHEN** the user explicitly creates a terminal
- **THEN** the client submits a workspace operation and attaches to its server-spawned session rather than spawning a second session locally

### Requirement: Safe Snapshot Refresh
The mobile client SHALL refresh authoritative workspace snapshots on connection, resume and opening the terminal overview. A removed pane or session MUST NOT be silently recreated. Messages and selections SHALL be resolved only within their originating host connection.

#### Scenario: Desktop changes while phone sleeps
- **WHEN** the desktop renames a screen and removes a selected pane while the mobile app is suspended
- **THEN** resume displays the refreshed state and reports the missing selection without recreating its terminal

#### Scenario: Switch between hosts with colliding IDs
- **WHEN** two saved hosts report the same screen and session IDs and the user switches hosts
- **THEN** delayed output, actions and drafts from the former host do not affect the selected host

### Requirement: Deliberate Terminal Input
The client SHALL offer immediate terminal input and a separately edited multiline draft. Draft edits, composition and dictation MUST NOT reach the session before an explicit transfer. Text transfer SHALL use terminal paste semantics without adding an implicit Enter; sending Enter SHALL be an explicit action. Both paths MUST require current session control and preserve the intended destination.

#### Scenario: Compose and insert a multiline prompt
- **WHEN** the user composes text using an IME or dictation, edits it, and explicitly inserts it
- **THEN** only the final text is transferred to the selected controlled session using paste semantics, with no extra submit key

#### Scenario: Direct input and special keys
- **WHEN** the controlling user enters text, Tab, Escape, arrows or a control combination with a software or hardware keyboard
- **THEN** the selected terminal receives the intended text or mode-appropriate terminal key sequence once

#### Scenario: Control changes before transfer
- **WHEN** control is lost or the destination session disappears before a draft is transferred
- **THEN** transfer is refused and the draft remains associated with its original destination

### Requirement: Viewport Changes Do Not Take Control
Showing or hiding the keyboard, rotating, scrolling or selecting output MUST NOT claim session control or resize an observed PTY. A controlling view SHALL derive valid terminal dimensions from the visible usable area without transient zero-size updates. Merely opening or switching a view MUST NOT request software-keyboard input focus.

#### Scenario: Inspect with the keyboard hidden
- **WHEN** the user opens an observed terminal and rotates the phone or selects output
- **THEN** no keyboard-focus request, control claim or PTY resize is emitted

#### Scenario: Keyboard changes a controlling viewport
- **WHEN** the keyboard animates open in a controlling view
- **THEN** input remains accessible and any PTY resize uses a valid usable size rather than an intermediate collapsed area

### Requirement: Resume Without Input Replay
On disconnect or suspension the client SHALL stop sending input. Resume SHALL renegotiate the connection, resync and reattach as an observer before accepting new input. Uncertain or queued direct input MUST NOT be automatically resent. Drafts SHALL survive view switches and transient reconnects while the app process lives; the client MUST NOT promise draft survival after process eviction.

#### Scenario: Network changes during submission
- **WHEN** the network drops after a submit was sent but before its effect can be observed
- **THEN** reconnect does not resend it, the client explains uncertain delivery, and another write requires current control

#### Scenario: Return from suspension
- **WHEN** the user returns to the still-running app after locking the phone
- **THEN** its unsent draft is retained, host processes continue, and no draft or old keystrokes are automatically sent

### Requirement: Detach and Terminate Are Distinct
Closing a view, switching hosts or leaving the app SHALL detach without terminating host sessions. Ending a terminal SHALL require a distinct explicit action and the existing running-process confirmation policy.

#### Scenario: Leave the app during a command
- **WHEN** a command is running and the user leaves its mobile view
- **THEN** the command continues on the host and remains attachable

### Requirement: Honest Restoration and Link Targets
The client SHALL identify bounded replay as potentially incomplete after reconnect and MUST NOT claim it is a complete terminal snapshot. A remote loopback link without a supported forwarding path MUST NOT open an unrelated loopback service on the phone.

#### Scenario: Restored history is incomplete
- **WHEN** the client restores a view using bounded historical output
- **THEN** it identifies the restoration limitation and does not resize an observed PTY to force a redraw

#### Scenario: Remote localhost link
- **WHEN** the user opens a remote terminal's localhost URL without a mobile forwarding path
- **THEN** the client explains that the target is on the remote host and does not open phone localhost
