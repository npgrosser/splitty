# Spec Delta

## MODIFIED Requirements

### Requirement: Native Host Management Surface
An authorized native main client SHALL have equivalents of the existing host, preference, order, catalog, remembered-screen and forwarding API operations through its assigned service. Reads and metadata edits SHALL retain their existing absence of connection side effects. Remote-window and restricted-agent connections SHALL not gain local hub or forwarding authority.

The experimental native workspace SHALL provide passive inspection of the assigned local service's merged host directory. Cached Screens SHALL remain observations rather than open-screen references or evidence of current host reachability. Inspection and refresh SHALL neither contact a remote host nor mutate local or remote state. A failed refresh SHALL preserve the last valid directory from the same local connection and report its failure, distinguishing it from a successfully empty directory. Results superseded by a refresh or local connection replacement SHALL not replace the current directory. Directory reads SHALL not interrupt terminal event delivery, and closing their owner SHALL cancel pending reads and complete owned cleanup.

Beyond passive inspection, the experimental native workspace SHALL support deliberate connection of a known directory target and explicit opening of existing Screens as specified below. Passive reads, cached observations and remote active-Screen hints SHALL NOT acquire that connection or open-reference demand. Its native subset does not yet create remote Screens, persist open references, edit hosts or expose forwarding controls.

#### Scenario: Native host metadata
- **WHEN** a native main window reads or edits its saved host metadata without requesting a connection
- **THEN** the same validated metadata is returned or saved and no SSH connection or listener is started

#### Scenario: Native saved forwarding
- **WHEN** a native main window deliberately connects a host
- **THEN** only that host's enabled saved forwards/reverse forwards may restore, with the existing Stop, Forget, ordering and endpoint-exclusion rules

#### Scenario: Inspect cached native hosts
- **WHEN** the experimental native workspace inspects a merged directory containing aliases and previously observed Screens
- **THEN** those observations remain available without connecting, creating or attaching a Screen, restoring a forward, or claiming that a host is currently reachable

#### Scenario: Refresh fails after a successful observation
- **WHEN** a native directory refresh fails or returns an invalid or unadmittable response after a valid result from the same local connection
- **THEN** the last valid directory remains available with a refresh failure and local terminals continue processing their events
- **AND** a first-read failure is not presented as an empty directory, while a later successful empty response replaces the old observations

#### Scenario: Directory invalidated during a read
- **WHEN** `hub.changed` arrives while the native workspace is reading the directory
- **THEN** it performs a subsequent passive refresh and a superseded result does not overwrite the current directory
- **AND** repeated notifications do not require an unbounded queue of reads

#### Scenario: Local directory owner replaced
- **WHEN** the local service connection is replaced while a directory read or its presentation delivery is pending
- **THEN** the old connection's result is ignored, its retained directory is not attributed to the replacement, and the replacement reads its own directory when ready

#### Scenario: Close while a native directory read is pending
- **WHEN** the native workspace completes an accepted owner shutdown while directory setup or a read is pending
- **THEN** the pending directory work is cancelled and its owned stream cleanup completes without publishing a later directory result
- **AND** no remote host or saved forward is started by shutdown

## ADDED Requirements

### Requirement: Native Known Host Connection Demand
The experimental native workspace SHALL connect a known host only through explicit browse or opened-Screen demand. One connection per exact assigned target SHALL serve that window's demand. Cancelling browse SHALL preserve opened Screens; removing the final demand SHALL cancel recovery and release the owned connection. Aliases and host messages SHALL NOT change the target or local API owner.

#### Scenario: Deliberate connect and passive inspection
- **WHEN** a user inspects cached known hosts and then deliberately connects one
- **THEN** inspection sends no SSH work, the explicit action prepares only that exact target and local terminals remain usable
- **AND** successful readiness leaves enabled saved-forward restoration to the hub's existing policy

#### Scenario: Browse demand and open demand are independent
- **WHEN** the user stops browsing a connected host while one of its Screens remains open, including when hidden
- **THEN** that Screen retains its connection and output
- **WHEN** the final opened Screen is detached with no remaining browse demand
- **THEN** the host connection and recovery work are released without ending host sessions or independently owned forwards

#### Scenario: Duplicate or obsolete host actions
- **WHEN** Connect repeats for an already demanded target or an older Cancel or Retry arrives after the attempt was replaced
- **THEN** no duplicate stream is opened and the obsolete action does not cancel or restart the replacement
- **AND** a stale local-owner command or a target absent from its accepted known directory starts no host connection

### Requirement: Native Live Host Catalog
The experimental native workspace SHALL distinguish cached/handshake observations from a live catalog obtained through `workspace-catalog-v1`. Catalog refresh SHALL not open, create, attach or select Screens. Missing capability or failed/incomplete observations SHALL be reported without claiming an empty live catalog. Counts not supplied by a current observation SHALL remain unknown or explicitly last-observed.

#### Scenario: Live catalog without opening Screens
- **WHEN** a ready host reports several Screens, including an active-Screen hint
- **THEN** the window lists the live membership and names but opens and attaches none of them
- **AND** cached pane counts are not represented as current counts

#### Scenario: Missing capability and bad catalog
- **WHEN** the host lacks `workspace-catalog-v1` or returns an invalid, oversized or incomplete catalog
- **THEN** the window reports unsupported or failed live browsing without creating a Screen, claiming successful emptiness or endlessly repeating the read
- **AND** other sources and the passive directory remain usable

### Requirement: Native Explicit Remote Screen References
The experimental native workspace SHALL open only the existing remote Screens the user selects from a live catalog. It SHALL keep those references distinct from directory entries and local Screens, and navigate in local-first host-group order. Missing Screens SHALL be reported without recreation. Stopping and restarting this experimental workspace SHALL not restore or write remote open references in this increment.

#### Scenario: Open an existing Screen beside local work
- **WHEN** a user opens one live Screen on a host with several Screens
- **THEN** only that existing Screen joins the window's opened list and its actual sessions attach
- **AND** local selection and sessions survive, repeated Open selects the existing reference, and later host activation does not switch the window

#### Scenario: Same identifiers on several hosts
- **WHEN** local work and two hosts use the same Screen, pane and session identifiers
- **THEN** selection and navigation distinguish their owning sources, and output, replies, size claims and operations reach only the selected or captured owner

#### Scenario: Missing Screen during opening or recovery
- **WHEN** a Screen disappears before its requested snapshot and a subsequent ordered catalog confirms its absence
- **THEN** the window marks its reference missing, retains no executable route for it and offers removal without recreation
- **AND** a delayed snapshot after detachment cannot reopen the reference

#### Scenario: External move to an unopened Screen
- **WHEN** a remote pane moves from an opened Screen to an unopened Screen
- **THEN** it leaves the opened routes without opening its destination
- **AND** a move between two opened Screens on the same current connection preserves the surviving terminal view and session

### Requirement: Native Remote Screen Interaction
Supported native terminal and layout actions SHALL operate on the owning remote Screen through its current connection. Default tabs and splits SHALL inherit the remote source pane's launch settings without sending local profile identifiers. Hidden opened Screens SHALL continue consuming output and live replies without visible size claims. Local appearance/settings ownership SHALL remain local regardless of the shown Screen.

#### Scenario: Remote default tab and split
- **WHEN** a user creates a default tab or split from a live remote pane
- **THEN** the host creates its session using remote inheritance and the window renders its authoritative layout
- **AND** no local profile identifier, local CWD or local pane-creation request is sent

#### Scenario: Hide, return and resize
- **WHEN** an opened remote Screen becomes hidden while producing output and a live terminal query
- **THEN** its emulation advances and its reply reaches its own session, while hidden visibility and size claims are withdrawn
- **AND** returning preserves that view and session and publishes fresh shown geometry

#### Scenario: Local settings while remote work is shown
- **WHEN** the shown Screen is remote and the user changes a supported local appearance or Drawer preference
- **THEN** the fixed local service handles the request and its confirmed appearance applies to the remote view
- **AND** remote settings or hub messages cannot redirect the local request or refresh owner

#### Scenario: Unsupported remote actions
- **WHEN** a queued action requests a local profile/template, new or recreated remote Screen, or a transfer outside the supported remote scope
- **THEN** it is rejected before mutation or capacity-consuming creation, even if its control is no longer visible
- **AND** a remote loopback URL click is explained as unavailable until forwarding is supported and does not open an unrelated local service

### Requirement: Native Remote Detach And Assessed Termination
The experimental native workspace SHALL detach a remote Screen on its default close action without prompting or terminating its sessions. Explicit Screen termination SHALL assess current affected identities and require confirmation; pane close SHALL retain assessed destructive semantics. A lost or changed connection SHALL invalidate pending consent and SHALL NOT replay a submitted termination with uncertain outcome.

#### Scenario: Default Screen close
- **WHEN** the user closes a remote Screen with its normal close control or shortcut
- **THEN** only its open reference, unneeded attachments and claims are removed, without a destructive operation or confirmation
- **AND** its host sessions and independently owned forwards continue

#### Scenario: Explicit terminate or pane close
- **WHEN** the user explicitly terminates a remote Screen and confirms its assessed host, Screen and session impact
- **THEN** one `close_screen` is sent to that owning connection and completion follows authoritative observation
- **WHEN** an assessed remote pane close is confirmed where necessary
- **THEN** it affects only that pane's owning host and the other sources remain unchanged

#### Scenario: Reconnect while consent or completion is pending
- **WHEN** a host connection changes while a termination prompt is pending or its submitted outcome is unobserved
- **THEN** accepting the old prompt closes nothing on the replacement, and an uncertain submitted termination is not resent
- **AND** a late outcome with a reused request identifier cannot settle a different source's decision

### Requirement: Native Bounded Remote Recovery
After previous `remote.ready`, a demanded native host connection SHALL retry transient loss with increasing waits capped at ten seconds for at most two minutes. Initial failure, nonretryable failure or exhausted recovery SHALL wait for explicit Retry. Disconnect SHALL revoke input and mutations; fresh connection identity, catalog, snapshots and ordered session replay SHALL precede renewed interaction. No old input or mutation SHALL be replayed.

#### Scenario: Recover after prior readiness
- **WHEN** a previously ready demanded host disconnects transiently and becomes available during the recovery interval
- **THEN** its existing explicit references resynchronize and reattach without opening other catalog Screens or creating sessions
- **AND** replacement replay starts from reset emulation rather than appending duplicate history, including when session identifiers are reused

#### Scenario: Initial failure and nonretryable failure
- **WHEN** initial Connect fails, or recovery receives an authoritative nonretryable failure
- **THEN** no automatic attempt follows and the window preserves the bounded diagnostic and available hint with explicit Retry
- **AND** failure text is not used to override the reported retry classification

#### Scenario: Retry limit, cancellation and late completion
- **WHEN** recovery exhausts its interval or its final demand is removed during preparation
- **THEN** that attempt is revoked and its owned cleanup is awaited, with no later automatic attempt
- **AND** delayed progress, EOF, readiness or a timer from the old attempt cannot replace the newer source state

#### Scenario: Input during loss and stale callbacks
- **WHEN** input, geometry, live replies or mutation callbacks captured before loss arrive during or after reconnection
- **THEN** they cannot act on the replacement connection, and unsupported stale work is discarded rather than queued for replay

### Requirement: Native Remote Window Admission And Cleanup
The experimental native window SHALL bound sources, pending opens, opened Screens, attachments, pending panes and retained observations across all its sources. Refusal SHALL precede extra dialing or mutation. Normal window close SHALL assess only destructive local impact, detach remote work and await all owned setup/stream/service cleanup. Failed cleanup SHALL not be reported as successful completion.

#### Scenario: Aggregate capacity across sources
- **WHEN** local and remote opened work together exhaust a window admission bound
- **THEN** another open or pane creation fails before its extra dial or mutation and surviving routes remain usable
- **AND** cancellation or confirmed removal restores capacity only when its required cleanup is observed

#### Scenario: Pending remote setup during window close
- **WHEN** normal window close is accepted while a remote setup, read or stream close is still pending
- **THEN** those resources are revoked and retained until their cleanup completes before normal application exit
- **AND** remote sessions do not add a destructive busy-process prompt and are not terminated

#### Scenario: One host fails or cleanup fails
- **WHEN** one independently owned host stream fails while other sources are live
- **THEN** the other sources continue processing input/output and local API results
- **WHEN** an owned cleanup fails or the shared physical owner is lost
- **THEN** the failure remains observable and is not converted into successful aggregate cleanup
