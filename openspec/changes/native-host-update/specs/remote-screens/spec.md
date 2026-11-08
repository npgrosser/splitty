# Spec Delta

## MODIFIED Requirements

### Requirement: Host Update
`/ws/remote` SHALL accept `update=1`; an authorized native main host connection SHALL support the corresponding explicit intent. A client SHALL request it only after the user confirms the versions and keeper-session behavior. An older native daemon may be updated only with verified instance identity, session keepers and the matching installed replacement. An eligible legacy daemon SHALL use Confirmed Legacy Daemon Migration instead. A missing replacement, newer daemon, absent session keepers or blocked legacy migration SHALL stop nothing and return an actionable failure. No update SHALL deliberately end sessions.

The intent SHALL apply to one connection attempt and be consumed before a potentially mutating command; reconnect and ambiguous completion SHALL NOT replay it. Successful replacement SHALL report update progress and require a fresh handshake for the intended native daemon. A daemon already running the intended version SHALL remain running. Migration offers SHALL be structured metadata rather than inferred from message text; blocked legacy migration SHALL use `migration_unavailable` and SHALL NOT offer an ordinary update that cannot succeed.


The hub SHALL report ordinary update availability only when the daemon version has lower SemVer precedence than its own version, accepting one optional `v` prefix and ignoring build metadata. Unknown versions SHALL not imply an available update. Availability SHALL also require the verified instance, session preservation and matching prepared replacement required by the shared replacement plan. The window SHALL use this metadata, including the keeper behavior, rather than comparing versions itself. This metadata SHALL never bypass the replacement checks above.

The experimental native workspace SHALL retain actionable update metadata from its assigned host, including when `remote.ready` is followed by an unsupported workspace capability. An ordinary confirmation SHALL use the assigned local backend's version; missing, malformed or contradictory metadata SHALL NOT authorize an update. Native confirmation SHALL identify the host, observed and replacement versions, keeper preservation and connection interruption. A legacy session count SHALL describe the observation, not reserve those sessions.

Native confirmation SHALL be captured for the current local owner, host source, attempt and accepted offer, revalidated before admission and consumed once. Replacement of that owner/source/attempt, changed confirmed offer facts or dismissal SHALL revoke it. Unrelated host metadata changes SHALL NOT revoke unchanged consent. Confirmation authorizes the host-scoped update described above; fresh backend eligibility and conditional mutation remain authoritative.

A confirmed native update SHALL retain open remote Screen references and await owned connection cleanup before opening its replacement. Ordinary Connect, Retry and recovery SHALL NOT inherit update intent. Cancellation before update dispatch SHALL prevent that dispatch. Cancellation or failure after dispatch SHALL retain cleanup ownership and SHALL NOT claim rollback or unverified success. An unsuccessful dispatched update SHALL await explicit ordinary Retry; later observation SHALL report effective state without replaying maintenance or claiming the lost attempt succeeded.

#### Scenario: Update a compatible older host
- **WHEN** a user confirms updating an older native daemon with keepers and a matching replacement
- **THEN** the instance-bound update preserves keeper sessions and connects to the freshly verified replacement

#### Scenario: Nothing to update
- **WHEN** a confirmed update finds the host already running the intended version
- **THEN** it connects without stopping that daemon

#### Scenario: Eligible legacy migration
- **WHEN** a host failure includes a positive migration offer and the user confirms it
- **THEN** the next connection consumes the migration intent once and preserves keeper-managed sessions through the conditional migration

#### Scenario: Migration unavailable
- **WHEN** legacy process identity or keeper compatibility cannot be verified
- **THEN** the UI explains the blocked reason and does not present a working update action or end sessions

#### Scenario: Newer or unparseable host version
- **WHEN** the daemon version is newer, equal in precedence, or unparseable
- **THEN** update availability is false and the window offers no ordinary update

#### Scenario: Native offer survives workspace capability rejection
- **WHEN** the assigned host reports an actionable ordinary update in `remote.ready` and then lacks `workspace-catalog-v1`
- **THEN** the native client preserves that attempt's offer alongside the incompatibility explanation and permits explicit confirmation after cleanup
- **AND** ordinary Retry alone does not request an update

#### Scenario: Native versions come from the assigned backend
- **WHEN** the native UI and its assigned local backend have different build versions, or the backend version cannot be read
- **THEN** ordinary update confirmation names the assigned backend's reported version or remains unavailable until that information is valid
- **AND** no version guess or local comparison grants update eligibility

#### Scenario: Native unavailable or malformed offer
- **WHEN** update metadata is missing, malformed, contradictory or unavailable, including an equal-version daemon lacking a workspace capability
- **THEN** the native client offers no update from those observations and retains the applicable diagnostic
- **AND** neither inspection nor Retry bypasses the backend's update policy

#### Scenario: Native stale or dismissed confirmation
- **WHEN** a confirmation is dismissed, its local owner or host attempt is replaced, or its confirmed offer facts change before admission
- **THEN** confirming the old request starts no update and a late result cannot reopen or retarget that confirmation
- **AND** unchanged alias, pin/order or unrelated catalog metadata alone does not invalidate a current confirmation

#### Scenario: Native duplicate confirmation and connected replacement
- **WHEN** a connected host's update is confirmed more than once while its prior connection cleanup is pending
- **THEN** at most one update-enabled replacement begins, only after cleanup is observed, and existing remote references remain available for reobservation
- **AND** the old connection no longer accepts input while local work and other hosts continue

#### Scenario: Native cancellation before and after dispatch
- **WHEN** a confirmed native update is cancelled before dispatch, or after its update-enabled connection has started
- **THEN** cancellation before dispatch starts no update, while cancellation after dispatch preserves an unconfirmed outcome and joins setup, late returned connections and cleanup
- **AND** late completion cannot revive cancelled demand or authorize another update

#### Scenario: Native uncertain update and ordinary reconciliation
- **WHEN** a dispatched native update fails or its completion becomes uncertain
- **THEN** no automatic attempt replays maintenance, and explicit Retry reconnects without update intent
- **AND** a later observation reports the effective host state without claiming the earlier update succeeded; another update requires a fresh offer and confirmation

