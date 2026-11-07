# Spec Delta

## MODIFIED Requirements

### Requirement: Native Host Management Surface
An authorized native main client SHALL have equivalents of the existing host, preference, order, catalog, remembered-screen and forwarding API operations through its assigned service. Reads and metadata edits SHALL retain their existing absence of connection side effects. Remote-window and restricted-agent connections SHALL not gain local hub or forwarding authority.

The experimental native workspace SHALL provide passive inspection of the assigned local service's merged host directory. Cached Screens SHALL remain observations rather than open-screen references or evidence of current host reachability. Inspection and refresh SHALL neither contact a remote host nor mutate local or remote state. A failed refresh SHALL preserve the last valid directory from the same local connection and report its failure, distinguishing it from a successfully empty directory. Results superseded by a refresh or local connection replacement SHALL not replace the current directory. Directory reads SHALL not interrupt terminal event delivery, and closing their owner SHALL cancel pending reads and complete owned cleanup.

The experimental native workspace SHALL also provide explicit target entry, display-name editing and removal from its saved host list through its assigned local service. A display name SHALL NOT change the SSH target. Removing host metadata SHALL preserve open Screens, sessions and independently owned forwards. Current connection work MAY remain visible separately from the saved directory.

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

### Requirement: Native Entered Host Connection
The experimental native workspace SHALL accept a deliberately entered valid SSH target in addition to its known directory entries. It SHALL validate the exact target before effects, reveal a previously hidden target through the local host preferences API, and connect only after a confirmed preparation result while the original browse request remains active. An optional display name SHALL be metadata only. This action SHALL NOT promise an offline bookmark for an unnamed target before successful connection.

#### Scenario: Enter a new target or restore a hidden target
- **WHEN** a user submits a valid SSH target that is absent from the accepted directory or was previously forgotten
- **THEN** the native client deliberately reveals that exact target and starts its browse connection only after the local service confirms preparation
- **AND** existing target validation, source admission, session ownership and saved-forward restoration rules remain in effect

#### Scenario: Leave during preparation
- **WHEN** the user cancels the pending connection request, leaves Hosts, or closes the window before preparation completes
- **THEN** late preparation completion cannot start a new host connection
- **AND** already submitted metadata work is reconciled without repeating it, while existing open Screens retain their connection demand

#### Scenario: Invalid target or preparation failure
- **WHEN** the target or display name is invalid, preparation is rejected, or its completion is uncertain
- **THEN** no host connection starts from that request, its failure remains visible, and a failed unlisted connection is not represented as a saved or reachable host

### Requirement: Native Host Metadata Mutation Ownership
Native host metadata actions SHALL carry their assigned local owner, exact target and request identity, use bounded admission, and submit each admitted action at most once. Directory refresh and mutation outcomes SHALL remain distinct. A rejected, uncertain or stale result SHALL NOT become a claimed success; fresh reads and hub.changed SHALL reconcile effective metadata without replaying a mutation or a connection continuation.

#### Scenario: Rename or clear a display name
- **WHEN** a user changes or clears the display name of a host
- **THEN** the local service receives only that target metadata change, the confirmed directory reflects it, and neither SSH target identity nor open work changes

#### Scenario: Remove metadata while work remains open
- **WHEN** a user removes a host with an open Screen or independent forward from the saved list
- **THEN** its saved metadata is forgotten through host.forget while its running Screen, sessions and forwards continue
- **AND** later cache observations do not silently restore the forgotten directory entry

#### Scenario: Duplicate, uncertain or obsolete mutation
- **WHEN** an action duplicates pending work, loses its reply, or arrives from an obsolete local owner
- **THEN** it cannot create overlapping duplicate effects or alter another owner, uncertainty remains explicit, and a subsequent refresh does not claim the lost request succeeded

#### Scenario: Mutation during directory refresh and shutdown
- **WHEN** hub.changed overlaps a mutation or shutdown starts while API work is pending
- **THEN** stale directory data cannot replace the latest confirmed view, terminal delivery continues, and shutdown joins the owned work without replay or late connection creation
