# Spec Delta

## MODIFIED Requirements

### Requirement: Screen Default Names Per Host
The client SHALL assign new default screen names as `Screen N`, using the smallest unused positive integer independently for local screens and for each remote host. Known unopened screens on that same host SHALL reserve their names. Existing server, restored and user-selected names SHALL remain unchanged. A pending default name that collides with a newly discovered screen on its host SHALL be adjusted before creation; names on another host SHALL NOT affect it.

For explicit creation in an already opened ready host group, the experimental native workspace SHALL delegate default naming to the host by omitting `name` from `create_screen` and adopt the canonical returned name. It SHALL NOT present a guessed name as confirmed. The other clients' naming and pending-name rules above remain unchanged.

#### Scenario: Independent numbering
- **WHEN** local screens already use Screen 1 through Screen 3 and a new screen is created on an otherwise empty remote host
- **THEN** the new remote screen is named Screen 1

#### Scenario: Existing unopened screen
- **WHEN** a host already has an unopened Screen 1 and a new screen is created there
- **THEN** the new screen takes the smallest available number on that host

#### Scenario: Existing names survive
- **WHEN** a workspace restores a screen named Default or a user-selected name
- **THEN** that name is preserved

#### Scenario: Native default naming is decided on the host
- **WHEN** a native group-local creation and another client's creation reach the same host while it already has unopened named Screens
- **THEN** each admitted operation without a name uses the host's smallest unused Screen N at its application point, and the native reference adopts its canonical name
- **AND** no local or other-host name changes that choice, and no existing name is rewritten

## ADDED Requirements

### Requirement: Native Explicit Remote Screen Creation
The experimental native workspace SHALL support explicit creation within an already opened, ready host group with a complete live catalog. Admission SHALL capture and validate its local owner, host identity and current remote connection, reserve bounded window capacity, and send `create_screen` once with fresh Screen and Pane IDs and no local launch settings. Only one unresolved creation per host SHALL be admitted. This action SHALL NOT connect or retry a host.

#### Scenario: Create within the captured host group
- **WHEN** a user chooses New Screen within one ready opened host group while another source is selected
- **THEN** only that host receives the creation, using its own default launch settings
- **AND** existing opened and unopened Screens remain unchanged, the explicit creation skips the general picker reopen offer, and only its new reference joins that host's opened group

#### Scenario: Creation is unavailable or its capture is obsolete
- **WHEN** the host is disconnected, its live observation is unavailable or still reconciling, the initiating group has gone, the owner or remote connection has changed, or an assessed close prevents mutation
- **THEN** the captured creation submits no operation and reports unavailability without connecting, retrying or using another source
- **AND** rendering, inspection and dismissal create no Screen or attachment

#### Scenario: Creation navigation is admitted as one intent
- **WHEN** creation is activated while Hosts or Settings is displayed
- **THEN** successful UI enqueue enters Screens and the accepted current-owner worker intent releases browsing demand, even if remote creation admission is subsequently refused
- **AND** failed enqueue preserves the previous destination, while later destination departure revokes that request's follow even when the worker queue is full

#### Scenario: Duplicate creation or exhausted admission
- **WHEN** a creation is unresolved for that host or aggregate Screen/pane admission is exhausted
- **THEN** another creation is refused before mutation, while unrelated admitted work can progress
- **AND** failure to enqueue the mutation leaves no new reference or reservation

### Requirement: Native Remote Creation Observation
A submitted native creation SHALL remain an explicit unresolved reference until authoritative observation establishes its current state. Only a coherent catalog and Screen snapshot from the owning connection SHALL establish executable routes. Observations predating submission SHALL NOT settle it. Invalid or incomplete observations SHALL preserve uncertainty, and neither observation failure nor reconnect SHALL replay creation or recreate a missing Screen.

#### Scenario: Ordered confirmation and stale observation
- **WHEN** an older catalog response or a standalone Screen snapshot arrives around submission, followed by a complete observation ordered after it
- **THEN** the older or incomplete data cannot erase the pending reference, release its reservation or claim success
- **AND** the complete observation adopts only that explicitly requested Screen's canonical data and actual sessions without opening other catalog entries

#### Scenario: Rejection or bounded observation failure
- **WHEN** the mutation is rejected or the bounded confirmation pass fails, times out or returns invalid data
- **THEN** the reference reports rejection or uncertainty as supported by the evidence, no mutation is repeated, and other sources remain usable
- **AND** passive observation can establish current presence or absence without fabricating an acknowledgement or treating failure as a successful empty result

#### Scenario: Reconnect after an unobserved creation
- **WHEN** connection loss follows submission and a fresh connection observes the same host
- **THEN** the client retains the originally requested Screen identity as observation intent, accepts no old-connection route or completion, and sends no replacement creation
- **AND** a coherent current catalog and snapshot can expose that Screen's actual state, while current absence leaves a removable missing reference and does not prove that the earlier operation never ran

#### Scenario: Capacity through uncertain recovery
- **WHEN** a submitted creation has no conclusive current observation during disconnect or failed refresh
- **THEN** its bounded unresolved reservation remains accounted for across connection replacement
- **AND** successful canonical adoption converts rather than duplicates that reservation; conclusive current absence or explicit reference removal releases its unused local reservation without claiming that remote work was stopped

### Requirement: Native Remote Creation Selection And Detach
Native creation SHALL follow its confirmed reference only while the original selection intent remains current. Later navigation, disconnection, detachment or owner shutdown SHALL revoke that follow. Detach SHALL remove only the reference and its client-owned claims; late observations SHALL NOT reopen it. Normal window shutdown SHALL preserve remote sessions even when creation completion was not observed.

#### Scenario: Creation completes after navigation
- **WHEN** the user selects another Screen or leaves the Screens destination before creation is confirmed
- **THEN** the new reference may become available in its host group but completion does not replace the user's later selection or focus
- **AND** without intervening navigation the confirmed Screen becomes selected through ordinary workspace selection and attachment

#### Scenario: Detach while creation is unresolved
- **WHEN** the user detaches the new unresolved reference or closes the native window after submission
- **THEN** pending follow and local reference authority are revoked without sending a remote close or a compensating creation
- **AND** late snapshots cannot reinsert the reference, a Screen created on the host keeps its sessions, and owned stream cleanup completes normally

#### Scenario: A later explicit creation is a new request
- **WHEN** a previous creation has been resolved by current observation or its reference was explicitly removed and a new deliberate creation is admitted
- **THEN** it uses fresh Screen and Pane IDs with current authority and capacity checks
- **AND** it does not retry the earlier operation, clear uncertainty by inference or attach work whose reference was removed
