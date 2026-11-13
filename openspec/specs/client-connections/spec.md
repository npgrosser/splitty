# client-connections Specification

## Purpose

Defines how clients reach a Splitty service, exchange requests and terminal streams, and recover from failure without changing backend identity, authority or application semantics.

## Requirements

### Requirement: Equivalent Client Operations
Authorized native and browser clients SHALL observe the same results, validation errors and workspace/session effects for equivalent operations. Transport selection SHALL NOT change job ownership, scratchpad access, file/editor admission or session lifetime.

#### Scenario: Shared state across transports
- **WHEN** a native client changes a screen and an authorized browser client and owner CLI query that service
- **THEN** all observe the same resulting workspace and session identifiers

#### Scenario: Invalid operation through another transport
- **WHEN** equivalent invalid requests arrive through native IPC and an authenticated browser connection
- **THEN** both reject them with equivalent error/status semantics and no effects

### Requirement: Independent Client Lifetimes
Each application connection through a gateway or an app-owned helper SHALL retain its own service client identity, attachments, resize contributions, subscriptions and file/editor registration. Closing or stalling one client SHALL not close another or transfer its pending responses, registrations or authority. Closing or reloading a document SHALL NOT end the app-owned embedded service. Gateway HTTP operations SHALL retain their existing actor semantics independently of WebSocket attachments.

#### Scenario: Two browser tabs
- **WHEN** two tabs attach to one service through a gateway and one closes
- **THEN** only that tab's registrations and subscriptions are removed, the other tab continues and sessions keep running

#### Scenario: Multiple remote browser clients
- **WHEN** two browser clients use a remote gateway and one stream stalls or fails
- **THEN** the other retains its own ordered stream, identity and pending responses

#### Scenario: Documents share an app-owned embedded service
- **WHEN** multiple document connections use the same app-owned helper and one closes or reloads
- **THEN** only that connection is revoked, the service instance and local sessions remain available to the other and replacement connections
- **AND** an invalid host-open or saturated data stream cannot terminate the other document connections
- **AND** closing the app-owned outer pipe ends the embedded service and its local sessions/jobs

### Requirement: Native Transport Negotiation
A native connection SHALL negotiate transport version 1 independently of mux and verify the service instance/limits before application requests. Each framing/service handshake SHALL fail within 5 seconds if incomplete. A launcher MAY report preparation progress under its existing startup deadlines, but SHALL admit no application input until service negotiation completes. Unsupported versions, invalid framing and mismatched discovery SHALL fail before dispatch without a fallback listener.

#### Scenario: Unsupported endpoint
- **WHEN** a client reaches an endpoint with an unsupported native transport version
- **THEN** it reports incompatibility and sends no application operation or TCP fallback request

#### Scenario: Stale discovery
- **WHEN** the endpoint's service instance differs from the instance in the discovery record
- **THEN** the client rejects it and refreshes discovery without sending queued input

#### Scenario: Helper still preparing its backend
- **WHEN** a native helper has negotiated framing but is still starting WSL or preparing SSH
- **THEN** it can report bounded progress, but no application request is dispatched before the verified service generation is ready

#### Scenario: Owner host-open intent is pinned before preparation
- **WHEN** an authorized main broker or authenticated local gateway opens a separate host stream through a private owner endpoint
- **THEN** the endpoint verifies the expected local service instance before preparing SSH and assigns the validated target for the lifetime of that stream
- **AND** a stale local instance cannot initiate SSH work through a replacement hub

#### Scenario: Remote carriers cannot open a local host stream
- **WHEN** a peer sends host-open intent on an assigned direct or SSH remote carrier
- **THEN** it is rejected before transport acceptance and causes no local host preparation

### Requirement: Fixed Backend And Connection Identity
Each client connection SHALL bind to one backend and one connection generation. Request IDs, subscriptions and replies SHALL be resolved only within that generation. A peer-supplied target, path, handle or session identifier SHALL NOT select another backend or change the caller's authority.

#### Scenario: Reply from a previous connection
- **WHEN** a response arrives after its connection was replaced and its request ID is reused on a new connection
- **THEN** it settles no request and delivers no data to the new connection

#### Scenario: Host supplies local identifiers
- **WHEN** a host sends messages naming local panes or another host's connection handle
- **THEN** no operation is routed to either other backend

### Requirement: Enumerated Native Requests
Native requests SHALL name supported operations with validated arguments. Unknown operations and extra authority fields SHALL be rejected before effects. No generic native proxy, local filesystem or OS-execution escape hatch SHALL be exposed. Intended terminal/job/input/file operations on the assigned backend SHALL retain their existing authority and validation; caller-selected URLs, sockets or authentication headers SHALL not redirect them.

#### Scenario: Generic proxy request
- **WHEN** a renderer requests a generic URL/socket proxy or local OS-exec operation outside the enumerated backend API
- **THEN** the broker rejects it before connecting, reading a file or starting a process

### Requirement: Bounded Delivery
Native frames SHALL be at most 16 MiB. API bodies SHALL be at most 1 MiB or their existing smaller endpoint limit. Each connection SHALL admit at most 64 pending API requests and 32 MiB of application-owned queued/in-flight transport data; all connections together SHALL reserve at most 128 MiB of such transport data per process. Application-owned reassembly, IPC queues and retained IPC output, including encoding copies through completion or disposal of platform delivery, SHALL count against these limits. Incomplete negotiation SHALL hold no application state.

These numeric allowances do not bound framework-owned inbound invoke bodies allocated before application validation, generic framework command replies, engine storage outside the accounted transport buffers, or total process/renderer memory. These exclusions SHALL NOT relax application frame/body limits, transport-buffer accounting, request limits, connection/worker admission, or native role checks. A compromised packaged renderer can exhaust unaccounted framework storage; isolation of its authority does not guarantee availability of other windows sharing the process.

#### Scenario: Oversized frame
- **WHEN** a native transport peer announces a frame longer than the limit or exceeds its allowed reassembly budget
- **THEN** the connection ends before the application allocates the declared transport payload or partially applies it
- **AND** this preallocation guarantee does not cover framework ingress allocation that precedes the application handler

#### Scenario: Frozen client
- **WHEN** a client stops consuming output and reaches the delivery limit
- **THEN** it is disconnected without silently dropping part of a terminal message, other clients remain usable and the session keeps capturing bounded output

#### Scenario: Request flood
- **WHEN** a client already has 64 pending API requests
- **THEN** additional requests are refused with a busy result without starting their operations

### Requirement: Resource Admission
Admission SHALL precede application-owned per-client state allocation, backend dialing and worker spawn, with no unbounded wait queue. Framework ingress allocation before the application handler is outside this admission guarantee. Initial ceilings SHALL be:

- 256 accepted transport connections per process, including unauthenticated gateway HTTP and handshakes;
- 128 live application streams/native handles and 16 pending connection establishments per process;
- 64 live streams per assigned backend;
- 32 live streams and 4 pending opens per native window/shared trust domain or authenticated browser session;
- 32 owned SSH/transport-helper workers per process, including setup and deliberate forwarding workers.

Anonymous work SHALL consume process quotas; parent-owned popouts SHALL share their parent's quota. Excess authenticated work SHALL receive a busy/503 result; anonymous excess SHALL close before upstream work. Slots SHALL release exactly once after cleanup; worker slots only after observed exit. Cancellation and liveness of admitted clients SHALL remain usable. Handshakes SHALL time out in 5 seconds, incomplete native frames in 60 seconds, and unauthenticated HTTP header/idle reads in 15 seconds.

#### Scenario: Idle stream flood
- **WHEN** a client opens many idle streams that remain below the queued-byte budget
- **THEN** stream/handle/worker counts stay within all applicable quotas, excess work causes no extra dial/spawn, and existing clients retain cancellation and liveness

#### Scenario: Incomplete handshake and frame flood
- **WHEN** callers hold connections with incomplete handshakes, headers or frames
- **THEN** process admission remains bounded before authentication and the specified deadlines release those connections without upstream work

#### Scenario: Quota recovery
- **WHEN** an admitted open is cancelled, times out, fails, or its owned worker exits
- **THEN** its capacity returns exactly once after cleanup and can serve another caller without leaking or prematurely reusing a worker slot

### Requirement: Ordered Stream And Cancellation
Each connection SHALL preserve application-message order and existing spawn/replay ordering. Orderly peer closure SHALL deliver already accepted application frames before reporting end of input; explicit revocation and protocol failure SHALL still discard undelivered input immediately. Cancellation and liveness SHALL remain deliverable under saturation. A request cancellation or timeout SHALL release pending waits, but SHALL NOT claim an already admitted mutation was undone or automatically repeat it.

#### Scenario: Cancel while output is saturated
- **WHEN** an upload or pending request is cancelled while output fills the data allowance
- **THEN** cancellation is processed or the connection is explicitly closed within its deadline, without an unbounded wait

#### Scenario: Response lost after mutation
- **WHEN** a mutation may have applied but its response is lost
- **THEN** the client reports an uncertain outcome and resynchronizes rather than automatically performing it again

### Requirement: Reconnect Does Not Replay Input
A reconnect SHALL discard unsent terminal input, uploads, editor actions and non-idempotent mutations from the previous connection. After fresh authentication/negotiation it SHALL resynchronize workspace state and reattach existing sessions. Existing explicitly idempotent job requests SHALL retain their established deduplication semantics.

#### Scenario: Typing during a disconnect
- **WHEN** the connection drops while input is pending and later reconnects
- **THEN** old input is not sent to the recovered or a replacement session

#### Scenario: Recovered daemon
- **WHEN** a daemon restarts and adopts kept sessions
- **THEN** a reconnecting client obtains fresh connection identity and displays those sessions without spawning replacements

### Requirement: Native Document Lifetime
A native connection handle SHALL be bound to its actual window, top-level document generation, role and backend. Reload, navigation, destruction or backend replacement SHALL revoke the handle, subscriptions and pending requests, including parent-owned popout views. A new document SHALL obtain its own authorized connection. Direct native grants SHALL not be transferable; same-origin Dockview popouts share their parent's script trust domain rather than forming an isolation boundary.

#### Scenario: Handle copied to another window
- **WHEN** another window or child frame directly invokes the native broker with a copied connection handle
- **THEN** the broker rejects the call before any backend request
- **AND** calls executed through a same-origin popout's parent belong to the shared parent trust domain, not a claimed isolated popout principal

#### Scenario: Distribution switch
- **WHEN** the Windows app replaces its WSL backend
- **THEN** outstanding handles and replies from the old distribution cannot address the new one

### Requirement: Feature Parity Without Hidden Listeners
Released native clients SHALL support the existing settings, themes, shell profiles, templates, notifications, files, editor confirmations, popouts, zoom and host-management features allowed by their role. Using those features SHALL NOT start a fallback HTTP listener or grant arbitrary native privileges. Explicitly experimental development executables MAY implement a documented subset; they SHALL retain the same transport admission, backend identity and authority boundaries and SHALL NOT replace released clients.

#### Scenario: Native desktop workflow
- **WHEN** a desktop user saves settings, changes a theme, transfers a file, confirms an editor request and moves a terminal into a popout
- **THEN** the operations retain their existing validation and lifetime behavior without starting a terminal-server TCP listener

#### Scenario: Experimental executable
- **WHEN** a development-only native executable provides a terminal and document probe
- **THEN** it identifies its experimental scope, keeps released clients available and introduces no fallback HTTP listener

### Requirement: Isolated Native Probe Lifetime
The experimental `splitty-gpui` executable SHALL use an isolated app-owned workspace. Closing a client connection SHALL revoke only its stream; reconnecting to that service SHALL reattach existing sessions. Closing the owned application SHALL await its service and process cleanup without ending sessions of another service. A Windows client SHALL use an owned helper or explicit native connection, never silently substitute a different backend.

#### Scenario: Reattach in the probe
- **WHEN** the probe reconnects to its still-running service
- **THEN** it resynchronizes the existing screen and attaches its existing session without starting a replacement process

#### Scenario: Probe shutdown
- **WHEN** the isolated probe exits normally
- **THEN** its local sessions and owned service are stopped, and another service's sessions remain unaffected

### Requirement: Native Probe Data Paths
The native probe SHALL send input and size reports through the existing session protocol, preserve replay/live ordering and refrain from replay-triggered terminal replies or clipboard actions. Documents SHALL be read with versioned `viewer.read` chunks over their own connection. Native selections used for annotations SHALL retain Unicode-safe quote/context and source positions and be saved through the existing mux operations.

#### Scenario: Live terminal
- **WHEN** a user types in the probe terminal and resizes its view
- **THEN** the assigned service receives the input and size report, and its actual output is rendered in order

#### Scenario: Native document annotation
- **WHEN** a passage containing multibyte text is selected in the probe's Markdown document and annotated
- **THEN** the annotation is stored on that viewer pane and remains represented after a fresh screen snapshot

### Requirement: Scripted Probe Isolation
An optional scripted-content probe SHALL run only an explicit local fixture. Its engine SHALL receive no service connection, authentication material or native application command bridge. Its pixels SHALL participate in native clipping and overlays; an unavailable engine SHALL be reported as unavailable rather than replaced with a fabricated success result. Static Markdown SHALL remain inert, with no implicit resource loads or external actions.

#### Scenario: Offscreen composition
- **WHEN** the scripted fixture runs under a native overlay and inside clipped pane bounds
- **THEN** its real rendered frames respect those bounds and cannot cover the overlay or invoke service operations

#### Scenario: Unavailable engine
- **WHEN** the optional engine cannot be initialized
- **THEN** the probe reports that failure and makes no claim that scripted composition was validated

### Requirement: Native Local Screen Workflow

The experimental native desktop SHALL render each local screen from the assigned service's authoritative screen snapshot. Creating a split or tab, closing a pane and resizing a split SHALL use existing mux operations and preserve the service's ownership of terminal sessions. Selecting a tab SHALL change the window-local presentation state and, when the service announces `tab-select-v1`, SHALL persist the choice through `select_tab` as `mux-protocol` requires, without changing another window's shown tab. Unchanged panes SHALL retain their terminal view and session attachment across layout updates. Input and output SHALL remain scoped to the pane's assigned session and current connection generation. The local development workspace SHALL retain the isolated application lifetime rather than modifying an unrelated service's workspace. A screen the service removes with its last pane SHALL leave the window like a closed screen. Where a service of an earlier version keeps that screen without panes, the window SHALL close it with `close_screen` once the close of the pane it requested is confirmed; a screen without panes that the window only observes SHALL stay and SHALL offer one action that starts a terminal in it. A window without any screen SHALL offer one action that creates a screen.

#### Scenario: Split and add a tab
- **WHEN** a user splits a terminal and adds a tab in one of the resulting groups
- **THEN** the native window renders the service's resulting layout and each terminal pane streams its distinct service-created session

#### Scenario: Select and resize
- **WHEN** a user switches a tab or drags a split divider
- **THEN** the window shows the selected tab without changing another window's selection, and a divider resize is confirmed by the service's screen snapshot
- **AND** unchanged terminal panes retain their existing view and session state

#### Scenario: Close a local pane
- **WHEN** a user closes a terminal pane in the local screen
- **THEN** the service applies the existing close semantics and the window removes only views absent from the resulting authoritative snapshot
- **AND** output or queued input for the closed pane cannot be routed to another pane

#### Scenario: Close the final pane of a screen
- **WHEN** a user closes the only pane of a screen while the window holds another screen
- **THEN** the screen leaves the window and the window shows another screen, chosen as after a closed screen

#### Scenario: Close the final pane of the only screen
- **WHEN** a user closes the only pane of the window's only screen
- **THEN** the window shows no screen and offers to create one

#### Scenario: Service of an earlier version keeps the screen
- **WHEN** a user closes the only pane of a screen and the service confirms the close with the screen still listed, without panes
- **THEN** the window sends `close_screen` for that screen and it leaves the window

#### Scenario: New terminal after closing the final pane
- **WHEN** the shown screen has no panes, as a service of an earlier version keeps it after a process exited, and the user creates a terminal
- **THEN** the service recreates the screen with its existing ID and name and owns the new terminal session

### Requirement: Native Local Screen Catalog and Selection
The experimental native desktop SHALL create, switch, rename and close local screens through its assigned service on one connection. Screen membership, names and order SHALL follow the authoritative catalog. The service's active-screen hint SHALL initialize local choice only; later hints SHALL NOT change a valid local choice. Closing the last screen SHALL leave an empty workspace from which a new screen can be created.

#### Scenario: Lifecycle and local choice
- **WHEN** the window creates and renames a screen, switches between existing screens and closes one
- **THEN** names, membership and ordering follow the confirmed service state
- **AND** surviving screens retain their local tab and focus choices
- **AND** another client's activation does not switch this window

#### Scenario: Close the final screen
- **WHEN** the final local screen is closed
- **THEN** the service ends its sessions and the native workspace shows no screen
- **AND** creating a new screen uses new screen and pane identities

#### Scenario: Delayed catalog during creation
- **WHEN** a response to a catalog request sent before a local create arrives after that create was submitted
- **THEN** it does not cancel the new screen's pending pane or discard its arriving output
- **AND** a subsequent ordered refresh confirms the operation

#### Scenario: Unsupported workspace catalog
- **WHEN** the assigned service lacks `workspace-catalog-v1`
- **THEN** the native window reports that multi-screen control is unavailable rather than fabricating an authoritative catalog

### Requirement: Native Screen Switching Preserves Sessions
Switching the shown local screen SHALL preserve surviving terminal views, attachments and emulation state across all screens. Inactive screens SHALL continue receiving their output and sending live terminal replies, while withdrawing visibility and size claims. A returning screen SHALL publish sizes for its shown panes. User actions and replies SHALL remain scoped to their original screen, pane, session and connection generation.

#### Scenario: Hidden output and return
- **WHEN** a screen becomes inactive while its terminal produces output and requests a terminal reply
- **THEN** its terminal state advances and the reply reaches its own session without a hidden resize
- **AND** switching back shows that state with the same session and terminal view

#### Scenario: Stale callback after switching or removal
- **WHEN** an obsolete visibility, resize or input callback arrives after its screen was switched or removed
- **THEN** it cannot resize, focus or send input to a different session
- **AND** a removed screen cannot be resurrected by a delayed snapshot or spawn

### Requirement: Native Workspace Admission Across Screens
The native connection's attachment and pending-pane admission bound SHALL cover all local screens together. A pending pane SHALL reserve capacity before its creation is submitted, and a failed submission or rejected creation SHALL release it. Closing a screen SHALL release its routes and attachments only through authoritative reconciliation, and queued actions SHALL NOT redirect to another screen.

#### Scenario: Global capacity and recovery
- **WHEN** attached sessions and pending panes across several screens exhaust the connection's admission bound
- **THEN** another pane or screen creation is rejected visibly before submission
- **AND** a rejected creation or confirmed closure restores usable capacity without detaching surviving screens

### Requirement: Native Local Terminal Close Assessment
The experimental native local workspace SHALL use one terminal close policy for pane, Screen and normal owned-window close requests. Busy or unresolved affected sessions SHALL require confirmation; known idle targets SHALL not. Window assessment SHALL include hidden Screens and pending local creations. Cancellation SHALL preserve affected sessions and layout. This policy covers the owned local terminal workspace only.

#### Scenario: Busy pane and Screen close
- **WHEN** a pane button, pane-close shortcut or Screen close would end a session reported busy
- **THEN** the native window asks for confirmation before submitting the close
- **AND** cancellation leaves the sessions and authoritative layout intact
- **AND** confirmation uses the existing close operation for the selected scope

#### Scenario: Known idle and unresolved impact
- **WHEN** the affected local sessions are known idle
- **THEN** the close can proceed without a prompt
- **WHEN** an affected session or local creation is unresolved
- **THEN** the close requires confirmation rather than treating missing state as idle

#### Scenario: Hidden busy session on window close
- **WHEN** normal owned-window close is requested while a hidden Screen contains a busy terminal
- **THEN** confirmation describes ending the owned local workspace
- **AND** cancellation leaves the window, hidden session and its output stream usable

### Requirement: Native Close Consent Scope
A native local close decision SHALL be bound to its request, connection generation, target and affected pane/session identities. The client SHALL revalidate its latest known impact before execution. Observed target replacement, changed affected identities or newly unconfirmed busy/unresolved impact SHALL reject the stale decision without closing other work. Unrelated metadata changes SHALL NOT invalidate consent.

#### Scenario: Target changes while confirmation is pending
- **WHEN** the client observes a replacement session, changed affected pane membership or connection generation while a close confirmation is pending
- **THEN** accepting the old prompt performs no close for the changed target
- **AND** the native window reports that another close request is needed

#### Scenario: Busy state changes before idle close execution
- **WHEN** an unconfirmed close was prepared for idle sessions and the client observes busy or unresolved impact before execution
- **THEN** the close is rejected without ending those sessions
- **AND** a subsequent close request assesses the current impact

#### Scenario: Repeated gestures and harmless metadata
- **WHEN** a close decision is already pending and the same gesture repeats or only titles change
- **THEN** the client retains one pending decision rather than overlapping prompts or submitting duplicate closes
- **AND** unchanged affected identities remain eligible for confirmation

### Requirement: Native Owned Window Close Acceptance
A normal close of the experimental owned local window SHALL keep its UI and service running until the current close request is accepted or owned cleanup is known complete. Stale decisions or failed command admission SHALL NOT stop the service or quit the UI. Once accepted, the native client SHALL execute owned shutdown once, cease further user mutations, and await owned cleanup before normal process exit. Forced termination and non-cancellable platform quit events are outside this confirmation guarantee.

#### Scenario: Stale or unadmitted window request
- **WHEN** a window-close decision is stale or its bounded command queue cannot admit it
- **THEN** the window stays open with its service running
- **AND** an approval for another request or connection cannot quit it

#### Scenario: Accepted owned shutdown
- **WHEN** the current window-close request passes confirmation and execution-time validation
- **THEN** the native window receives acceptance before quitting
- **AND** later queued user mutations are not executed
- **AND** the owned service and sessions are cleaned up once before normal process exit

#### Scenario: Close during startup or completed cleanup
- **WHEN** normal window close is requested before workspace discovery completes
- **THEN** unknown impact is assessed without assuming it is idle
- **WHEN** owned cleanup has already completed after a startup or connection failure
- **THEN** the window can close without waiting for an approval from a terminated owner
- **AND** a failed send or closed update channel alone is not treated as cleanup completion

### Requirement: Native Local Group Maximization
The native local workspace SHALL support maximizing and restoring a tab group as window-local state. Maximization SHALL preserve the authoritative layout, pane/session identities and tab selection. Only displayed terminals SHALL claim visibility and size; hidden terminals SHALL continue receiving output. Each Screen SHALL retain its own valid maximize choice while hidden.

#### Scenario: Maximize and restore a running group
- **WHEN** a client maximizes a local tab group and later restores it
- **THEN** the authoritative layout and sessions remain unchanged
- **AND** hidden terminals continue processing output without retaining visible size claims
- **AND** restoring republishes the displayed terminals' measured sizes

#### Scenario: Navigate while maximized
- **WHEN** the client selects or creates a tab within a maximized group
- **THEN** the group remains maximized with its current selected tab
- **WHEN** the client switches away from that Screen and returns
- **THEN** its still-valid local maximize choice is restored without affecting another Screen

#### Scenario: Reveal structural changes and reconcile removed targets
- **WHEN** a split or pane move is admitted, or the client explicitly focuses a pane outside the maximized group
- **THEN** the client restores the full viewport without rewriting the authoritative layout itself
- **WHEN** a snapshot removes or replaces the maximized group
- **THEN** the client clears the invalid view reference and focuses a surviving visible pane if one exists

#### Scenario: Close includes hidden groups
- **WHEN** Screen or window close is requested while a group is maximized
- **THEN** close assessment includes all affected sessions, including groups hidden by maximization
- **AND** cancelling the decision preserves both sessions and the current viewport

### Requirement: Native Terminal Application Pointer Input
The native terminal SHALL honor application-requested mouse tracking and coordinate encoding for button presses, releases, motion and supported wheel reports. A gesture SHALL remain assigned to its original terminal. Without applicable mouse reporting, normal scrollback and alternate-screen cursor-key scrolling SHALL remain available.

#### Scenario: Requested mouse formats
- **WHEN** an application enables press-only, normal, button-motion or all-motion tracking and selects legacy, UTF-8, SGR, urxvt or SGR-pixel encoding
- **THEN** supported pointer events produce reports in that requested format with positions relative to the terminal content
- **AND** cell-based motion reports are deduplicated while pixel reporting retains pixel changes

#### Scenario: Wheel routing
- **WHEN** an application requests wheel-capable mouse tracking
- **THEN** wheel input produces mouse reports rather than substituted arrow keys
- **WHEN** wheel reporting is absent, including press-only tracking
- **THEN** wheel input retains local scrollback or alternate-screen cursor-key behavior

### Requirement: Native Pointer Local Override And Lifetime
Holding Shift at gesture start SHALL retain local selection despite application mouse tracking. That choice SHALL remain stable until release. Lifecycle changes SHALL clear local gestures; an active application gesture SHALL be settled on its original live terminal where possible, without retries after failed input admission or forwarding to a replacement terminal.

#### Scenario: Local selection under application tracking
- **WHEN** a Shift-started selection continues after Shift is released
- **THEN** it remains local, can be copied and sends no mouse reports
- **WHEN** Shift is pressed during an application-started gesture
- **THEN** its release still settles that application gesture

#### Scenario: Leaving or replacing a terminal
- **WHEN** a drag leaves the terminal, focus is lost, the terminal is hidden, reset or replaced, or the window deactivates
- **THEN** no pressed gesture remains stuck and subsequent input cannot leak to another pane
- **AND** a failed input admission is surfaced without replaying the report

### Requirement: Explicit Native Windows Workspace Startup
The experimental Windows workspace SHALL accept an explicit existing WSL distribution, absolute Linux backend path and isolated configuration path. It SHALL validate complete configuration before starting its owned embedded backend over inherited native pipes. Invalid or missing configuration SHALL cause a clear failure without installing software, selecting another backend or opening a fallback listener.

#### Scenario: Configured Windows workspace
- **WHEN** the Windows executable starts with valid `--wsl-distro`, `--wsl-backend` and `--wsl-config-dir` arguments
- **THEN** the native window renders real sessions from that distribution's owned embedded backend
- **AND** configuration is assigned only to that backend child and no unrelated daemon or distribution is changed

#### Scenario: Invalid or unavailable startup target
- **WHEN** required startup configuration is incomplete, malformed or unavailable
- **THEN** startup reports the failure and sends no application requests to an alternative backend
- **AND** any partially started owned helper is joined through cleanup

### Requirement: Native Windows Workspace Operation Parity
The experimental Windows workspace SHALL use the same validation, session identity, layout and application-outcome rules as the Unix development workspace for its supported terminal, Screen, settings, profile, zoom and template operations. API requests SHALL remain independent of terminal output while sharing the assigned backend lifetime.

#### Scenario: Windows terminal and API workflow
- **WHEN** a native Windows window creates splits/tabs, switches Screens and performs a supported settings or template operation
- **THEN** actual WSL terminal output/input and authoritative layouts remain usable with the same scoped identities
- **AND** an API failure or cancellation does not replay mutations or replace the terminal backend

### Requirement: Native Windows Owned Cleanup Evidence
Normal Windows preview shutdown SHALL join its owned streams and helper. The client SHALL mark cleanup complete only after a successful observed cleanup outcome. A failed or forced bridge exit SHALL be reported without claiming graceful backend cleanup. Shutdown SHALL never terminate the WSL distribution or unrelated services and processes.

#### Scenario: Normal and unsuccessful helper exit
- **WHEN** the preview closes normally after starting real WSL sessions
- **THEN** its helper and sessions exit while unrelated WSL work remains alive
- **WHEN** the helper exits unsuccessfully or cleanup cannot be established
- **THEN** the client reports the failure and does not publish a successful cleanup result

#### Scenario: Cancellation during startup
- **WHEN** the window closes while its owned helper or API stream is being established
- **THEN** admitted work remains owned until cleanup is joined and no request is delivered to a replacement backend

### Requirement: Native Relative Workspace Navigation
Native relative Screen navigation SHALL use the current authoritative catalog order and local selection. Relative tab navigation SHALL target its captured group on the shown Screen. Both SHALL stop at the first and last item, preserve sessions and reject foreign connection identities. Tab navigation SHALL NOT redirect to another group when its captured Screen or group is no longer current.

#### Scenario: Ordered relative Screen intents
- **WHEN** several relative Screen intents are admitted before the view receives the next update
- **THEN** each uses the latest local selection in catalog order without wrapping or creating sessions
- **AND** an intent from a replaced connection cannot navigate the replacement workspace

#### Scenario: Hovered group and stale target
- **WHEN** relative tab navigation targets a live group on the shown Screen, including an unfocused group
- **THEN** it selects and focuses that group's neighboring tab using the existing selection persistence semantics
- **WHEN** the captured Screen is no longer shown or its group was removed
- **THEN** the intent does not change another group's selection

### Requirement: Native Navigation Input Ownership
Native chrome navigation SHALL leave terminal wheel input with the terminal's existing application/scrollback routing. Editing, confirmation and drag interactions SHALL retain priority over chrome wheel navigation. Ordinary vertical wheel input over the Drawer SHALL select neighboring Screens. Deliberate list-overflow scrolling and horizontal tab-strip scrolling SHALL remain available independently of relative selection.

#### Scenario: Terminal and active editing retain input
- **WHEN** wheel input occurs over terminal content or while an editing, confirmation or drag interaction owns the workspace
- **THEN** chrome wheel navigation changes neither the shown Screen nor the selected tab
- **AND** terminal input retains its existing routing where applicable

#### Scenario: Drawer navigation owns its wheel input
- **WHEN** ordinary vertical wheel input occurs over the Drawer without another interaction owning input
- **THEN** the neighboring Screen is selected once in displayed order, without wrapping
- **AND** the event is not delivered to terminal content behind the Drawer

#### Scenario: Overflow browsing
- **WHEN** a user deliberately scrolls a long Screen list through a modified wheel gesture, or scrolls horizontally through an overflowing tab strip
- **THEN** the view reveals the overflow without changing the selected Screen or tab
