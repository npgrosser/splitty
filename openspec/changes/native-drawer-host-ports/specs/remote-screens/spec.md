# Spec Delta

## MODIFIED Requirements

### Requirement: Native Outbound Port Workflow
The experimental native main workspace SHALL offer manual and detected outbound forwards through its assigned local service, with explicit local-port selection, Stop/Resume, opt-in Remember, metadata-only Save, label editing and Forget. Existing mapping, conflict, persistence and restoration rules SHALL apply. Screen detachment and closing the Ports surface SHALL NOT implicitly stop forwards.

#### Scenario: Native manual mapping without a Screen
- **WHEN** a user enters a valid target and port without discovery or an open Screen, and explicitly starts it with Remember
- **THEN** the assigned local service opens that exact mapping and saves its actual local port only after confirmed opening
- **AND** another mapping can progress independently, while starting without Remember creates no saved preference

#### Scenario: Native save and label without connection
- **WHEN** a native user saves a mapping without starting it, or edits or clears its saved label
- **THEN** only the selected mapping's metadata is changed, omitted fields and paused state are preserved, and no SSH connection or forward starts

#### Scenario: Native Stop Forget and alternate port
- **WHEN** a native user stops or forgets a mapping, or explicitly selects an alternate local port after a conflict
- **THEN** Stop durably pauses it, Forget removes only saved metadata and leaves a live forward running, and an alternate updates the saved port only after confirmed success
- **AND** no conflicting listener or open Screen is terminated

#### Scenario: Native Remember partly fails
- **WHEN** opening succeeds but the separately requested Remember operation fails
- **THEN** the live mapping remains visible and usable, the failed save is reported separately, and the client does not claim the preference was saved

#### Scenario: Native Drawer host and port access
- **WHEN** a native user inspects a Drawer host's ports or navigates to that host's details or port management
- **THEN** the exact canonical host is used independently of the selected Screen or display alias, without connecting a host or mutating a mapping
- **AND** missing, partial or empty observations are distinguished while saved and active mappings remain available

#### Scenario: Native Drawer deliberate port opening
- **WHEN** a user deliberately chooses a current port entry in a native Drawer host group
- **THEN** the assigned local service ensures and verifies forwarding for that exact mapping before any browser dispatch
- **AND** pending, obsolete or unadmittable entries submit no duplicate operation, saved local-port conflicts remain explicit, and no new saved rule is created without Remember consent


### Requirement: Native Forwarded Browser Activation
Native browser opening for a remote loopback link or host-port action SHALL require a deliberate captured source action, a validated opening acknowledgement and subsequent passive confirmation of the same up mapping. Only the admitted HTTP(S) scheme, path, query and fragment SHALL survive replacement with `localhost` and the confirmed local port. Browser consent SHALL be separately revocable and SHALL NOT follow a replacement terminal or owner. These checks do not reserve the listener across OS browser dispatch.

#### Scenario: Native exact-source loopback link
- **WHEN** a remote terminal link is deliberately activated while another Screen becomes selected during forwarding
- **THEN** only the original target and mapping can authorize the rewritten URL, and the original scheme, path, query and fragment are preserved
- **AND** a malformed acknowledgement, conflict, down mapping or failed verification opens no browser URL

#### Scenario: Native stale browser continuation
- **WHEN** Stop, a superseding mapping action, explicit cancellation, initiating-surface closure, source detachment or replacement, or owner shutdown revokes a pending open
- **THEN** a late reply cannot open a browser or restart forwarding from that continuation
- **AND** closing only the initiating surface does not imply that an already admitted forward was stopped

#### Scenario: Native cross-window Stop before open
- **WHEN** another client stops a mapping and passive reconciliation observes that stop before browser dispatch
- **THEN** the observed absent or changed mapping prevents browser dispatch from the old request
- **AND** verification does not repeat the opening mutation to undo the Stop

#### Scenario: Native Drawer continuation belongs to its host group
- **WHEN** a native Drawer port opening has been admitted and the retained initiating host group remains the same
- **THEN** Screen selection, host/settings navigation, ordinary popup dismissal, Drawer visibility changes and ordinary reconnect do not retarget or independently authorize that opening
- **AND** dispatch still requires the admitted request's validated acknowledgement and passive confirmation of the same up mapping

#### Scenario: Native Drawer group closes before dispatch
- **WHEN** the initiating host group loses its final open reference, is disposed or changes canonical target, or its local owner closes or is replaced
- **THEN** pending browser intent and unsubmitted popup authority for that group are revoked immediately, even if a cancellation command cannot be queued
- **AND** reopening a reference under the same retained connection does not revive an old popup or continuation, while existing service-owned forwards are not implicitly stopped

#### Scenario: Native Drawer menu becomes obsolete
- **WHEN** a popup is dismissed without choosing an action or its local owner, group identity, displayed action class or local-port choice no longer matches at activation
- **THEN** the old popup cannot submit a new forwarding action and a new deliberate choice is required
- **AND** normal dismissal after an admitted choice does not revoke the separately retained group-scoped browser intent

