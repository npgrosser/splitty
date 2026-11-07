## ADDED Requirements

### Requirement: Experimental Native Local Shell Choice
The experimental native workspace SHALL offer the local service's enabled shell profiles from `shellProfiles.get` for explicit new-tab creation, disabling unavailable choices. Selection SHALL submit only `shellProfileId`; shell commands, arguments, working-directory inheritance and fallback remain service-owned. Ordinary tab and split actions SHALL retain service inheritance without explicit launch overrides.

#### Scenario: Explicit profile with inherited directory
- **WHEN** a user chooses an available profile for a new tab
- **THEN** `new_tab` carries that profile identifier and no explicit working directory
- **AND** the service uses its current known source directory and the selected profile's configured program and arguments

#### Scenario: Ordinary launch with stale client metadata
- **WHEN** the client has an older source directory or profile than the service and creates an ordinary tab or split
- **THEN** the service's current known launch defaults determine the new session

#### Scenario: Profile becomes unavailable before launch
- **WHEN** a selected profile becomes missing, disabled or unavailable before the service resolves the launch and the service uses its fallback
- **THEN** the native workspace visibly reports the effective profile and `shellProfileFallbackReason` for the admitted new pane
- **AND** the client neither reconstructs a command nor retries the launch

### Requirement: Native Profile Read Ownership and Isolation
Native profile reads SHALL belong to the local workspace connection, retain the latest valid result and refresh on same-service `settings.changed`. Superseded or foreign results MUST NOT replace current choices. Appearance and profile failures SHALL remain independent and visible while terminals remain usable. Refreshes SHALL coalesce without starving either read, and owned pending reads SHALL finish cleanup on shutdown without settings writes.

#### Scenario: Concurrent refresh and local preference change
- **WHEN** same-service settings change while a profile read is pending and the OS color preference changes repeatedly
- **THEN** superseded profiles are discarded and both requested settings reads eventually progress
- **AND** color preference alone does not reload profiles or consume terminal output

#### Scenario: Independent failure and recovery
- **WHEN** a profile read fails while appearance remains readable, or appearance fails while profiles remain readable
- **THEN** the successful feature updates independently, the failed feature retains its last valid state with a visible failure, and ordinary terminal actions remain usable
- **AND** a later same-service refresh can recover without an automatic retry loop or settings mutation

#### Scenario: Foreign result and shutdown
- **WHEN** a result belongs to another workspace connection, or the owned workspace closes during a read
- **THEN** foreign data is not applied and shutdown cancels pending reads and awaits owned stream cleanup without retaining delivery allocations
