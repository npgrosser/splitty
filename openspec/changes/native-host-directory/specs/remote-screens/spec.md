# Spec Delta

## MODIFIED Requirements

### Requirement: Native Host Management Surface
An authorized native main client SHALL have equivalents of the existing host, preference, order, catalog, remembered-screen and forwarding API operations through its assigned service. Reads and metadata edits SHALL retain their existing absence of connection side effects. Remote-window and restricted-agent connections SHALL not gain local hub or forwarding authority.

The experimental native workspace SHALL provide passive inspection of the assigned local service's merged host directory. Cached Screens SHALL remain observations rather than open-screen references or evidence of current host reachability. Inspection and refresh SHALL neither contact a remote host nor mutate local or remote state. A failed refresh SHALL preserve the last valid directory from the same local connection and report its failure, distinguishing it from a successfully empty directory. Results superseded by a refresh or local connection replacement SHALL not replace the current directory. Directory reads SHALL not interrupt terminal event delivery, and closing their owner SHALL cancel pending reads and complete owned cleanup.

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
