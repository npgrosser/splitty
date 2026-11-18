# Splitty Multiplexer Architecture

## Overview

Splitty is a terminal multiplexer with a native desktop client (GPUI) and a Rust service (Tokio). The service owns layout and session state. The client projects that state and sends layout operations; the CLI uses the owner control socket.

Start with [Connections, listeners and session lifetime](networking.md) for the
runtime diagrams: local desktop IPC/pipes, independent daemons, SSH host
connections and private control sockets. The [LikeC4 model](../architecture/README.md)
maps those paths to crates and modules; the sections below explain their internals.

## Ops Model

Layout changes are expressed as **Operations** — typed structs that describe a mutation (`split_pane`, `close_pane`, `move_pane`, etc.). There is no CQRS-style separation between Commands and Events.

The flow is Redux-like:

```
Op → Validate → Reduce (pure) → new State + Effects
```

The Reducer is a pure function: `(ScreenState, Op) → (ScreenState, []Effect)`. Effects (`SpawnSession`, `KillSession`) are executed separately by the EffectRunner (`mux::effect_runner`) — no I/O inside the Reducer.

`move_screen` places one ordered Screen before another or at the current end.
The reducer resolves both identities against current state, collapses only
duplicates of the moved Screen, and preserves other order entries, Screen
documents, active identity and sessions, with no process effects. A missing or
unlisted identity rejects atomically. Both session and control entry points
use the same operation; a changed order nudges other clients without a Screen
identifier, while an unchanged order produces no nudge.

Floating geometry uses viewport units. `mux::FloatBounds::is_valid` is the shared
domain rule for direct float/move operations and template projection: every
component must be finite and width/height positive. Negative origins and sizes
above one remain valid. Reachability clamping belongs to each client's display
and does not rewrite the canonical bounds. Closing or transferring a pane uses
the same subtree removal for tiled and floating layouts, retaining sibling
panes, sessions and layout identities; a floating entry disappears only when
its last pane leaves. Existing float/new-tab operations create tab groups, and
split/resize/tab-placement operations still target only the tiled root.

An op is always "I want this to happen." Whether the client applies it optimistically or the server applies it authoritatively doesn't change the wire format. This makes the protocol multiplayer-ready without structural changes — only the delivery semantics change (fire-and-forget → ACK/REJECT).

## Layout operations and client state

The reducer and client connections follow these rules:

- **No screen ownership** — every connected client may change every screen; the server applies ops in arrival order and rejects invalid ones atomically (for example `create_screen` over a screen that still has panes)
- **Client-set IDs** — the client generates pane/screen IDs, no round-trip needed
- **Server-owned sessions** — the server spawns and kills the sessions of mux panes, so a client never has to create one before its pane exists on the server

`move_pane_to_screen` is a workspace op without effects: the reducer takes the pane and its session record out of the source like `close_pane` would, without `KillSession`, puts it into the target's first slot or a new screen, and removes a source left empty. The native client verifies the destination from its authoritative snapshot before following the moved pane. Other clients get `mux.nudge` for both screens and rebuild them from their snapshots. A pane's shell keeps the `SPLITTY_SCREEN_ID` it started with, so a screen op from the CLI that names a screen no longer holding its pane is applied to the screen that does; an owner's job placement follows the pane the same way, an agent's grant anchor does not. `move_tab_group_to_screen` (`tab-group-transfer-v1`) does the same for every pane of one slot in a single reduction: it checks the whole group first (no managed job's pane, the shown tab one of its own), then takes the panes out one after the other as `close_pane` would and appends them in tab order to the target's first slot, so a rejection changes nothing and other clients see one change.

Every op runs its effects, whether it arrives over a client connection, from the CLI (`POST /api/mux/op`) or through the control socket. For a pane it created, the client waits for `mux.pane_spawned` and attaches to that session through the session protocol. A rejected op triggers reconciliation; an uncertain result does not authorize another session creation.

### Start-up and reload

The server rejects `create_screen` for a screen that still has panes and re-creates a screen that ran empty in place.

### Attaching to sessions

Each connection streams a session at most once. `attach` subscribes to its output; replay precedes `attached`, then live output follows. Detaching releases that connection's subscription. Replay filtering removes clipboard operations and terminal report requests, so historical output cannot write the clipboard or inject query replies into the current shell.

Once an attached view is wired, it measures its pane again and reports that size with `resize`, which accounts for layout changes during the attach. A server announcing `pty-size-v1` sends the session's `size` before the replay, so the emulator has the PTY's size before the buffered output is written, and the first report carries `redraw`: when the PTY size does not change, the server resizes the PTY by one row and back itself, so TUIs redraw after the replay. With an older server the view resizes the backend PTY by one row and back to its grid size, keeping the emulator's dimensions; using `resize` also works with servers that ignore dimensions on `attach`.

## PTY Size

Several connections may show one session, and the PTY has one size. With `pty-size-v1`, `resize` is a report: the size at which the connection shows the session. `protocol::sizing::SizeArbiter`, owned by the dispatcher, keeps per session each connection's report and per connection its activity, and derives the PTY size with the server's `terminal.size_policy`: the smallest columns and rows of the active connections (`active`, the default; a connection is active within `terminal.size_idle_seconds` of its last `activity`, unless it sent `idle` since or another one took over), of the most recently active one (`latest`), or of all (`smallest`); with no active connection, the most recently active one decides. Computing the size, resizing the PTY and publishing the new size on the session's watch channel happen under one lock; every stream of the session forwards a published size as a `size` event, and a one-second tick (spawned with the first connection, holding the dispatcher weakly) settles connections that became idle. Settings updates reconfigure the arbiter. A connection that never sent `activity` or `idle` is an older client: each of its reports counts as an activity.

The client decides what counts as activity: terminal query replies through `input` alone do not prove user activity. A key, pointer button, window activation or resize reports activity; scrolling alone does not. Hidden windows report idle.

The native app splits presence and sizing between its crates. `splitty-client-core`'s source worker (`native_workspace::presence`, driven by `SourceRuntime` for the local source and each host source alike) keeps per connection whether its welcome announced `pty-size-v1` and what it still has to tell it: each welcome is greeted with `activity` or `idle` before the first `attach`, according to the window's presence (a cell the window writes: visible, focused) and whether the shown Screen belongs to that connection; an interaction the window marks in the same cell becomes, at the worker's next turn, an `activity` for the connection of the Screen shown then, at most every 5 seconds; a window that became hidden makes every connection `idle`; `TakeOverSize` sends `activity` with `takeover`. Without the capability it sends none of these and no `hide`, and the first report after an attach resizes the PTY by one row and back instead of asking for `redraw`. The worker validates `size` events of the sessions it attached (1–1000 columns, 1–500 rows, a known policy) and publishes each new one as `Update::PtySize`. `splitty-gpui` only observes: the workspace marks a key or pointer button anywhere in the window, its activation and its resizing in the cell (no command, so typing never fills the worker's queue), writes visibility and focus into it, and hands each size to the terminal of its session, also to one created later. The terminal keeps the pane's measured grid, which it reports, apart from the emulator's grid, which follows the latest `size` and else the pane; it paints and hit-tests the grid from the pane's origin, moved up as far as a taller grid's cursor row needs, hatches the pane's area outside a smaller one and shows the size in a badge that takes the size over unless the policy is `smallest`.

## Two Protocol Paths

The terminal service owns workspace state, jobs, configuration and the host hub.
`client_session` owns connection identities, session dispatch, editor/file registrations and cleanup. Native carriers create independent connections: closing one window does not detach another. Liveness belongs to the carrier, which checks every 15 seconds and ends a peer whose last matching pong is 30 seconds old (within 45 seconds of silence). The session loop has no heartbeat. Closing delivery ends that loop and releases its registrations; editor requests and file offers expire by their own deadlines.

The native operation catalog is published in [`native-operations.json`](native-operations.json), with method/path mappings, arguments, roles, body limits, deadlines, cancellation and error styles. These paths identify operations; only the `control` scope is served as HTTP on the owner Unix socket. `control::operation` translates requests and validates query/path arguments and JSON bodies (`Content-Type` must be JSON, else 415), then calls `State::execute` with the main role. Non-body arguments are strings; count arguments decode as decimal numbers. Invalid bodies/counts follow the catalog error style: plain 422, job 400 `invalid_request`, viewer 400 `invalid`. Job and grant routes live in `control::jobs`, shared with grant sockets. The route set remains narrow: host-directory, settings, build and command-dispatch operations are native-only. Session messages retain dispatcher validation and connection ownership. Remote-role connections cannot invoke the local host hub; owner/grant administration is separate from the native UI catalog.

Splitty has two coexisting application protocol layers on each client connection,
carried by native channels, Unix sockets or owned pipes:

| Layer | Messages | Purpose |
|-------|----------|---------|
| **Session protocol** | `create`, `attach`, `input`, `output`, `resize`, `close`, `save_image`, `upload`, `file.*`, `viewer.*`, `activity`, `idle`, `hide`, `size` | PTY I/O and file content — `output` is binary; other session messages are JSON |
| **Mux protocol** | `mux.op`, `mux.resync`, `mux.welcome`, `mux.screen_snapshot`, `mux.nudge` | Layout state — screen/pane management |

The mux handler intercepts `mux.*` messages; everything else falls through to the session protocol handler. Both share the same assigned client connection.

`splitty attach` (`cli::attach`) shows one session in the invoking terminal alongside windows showing it. On `mux.welcome` it sends `activity` with `takeover` and `attach`, filters replay through `pty::replay`, reports size with `resize` and typing with `activity`, and detaches with `detach`. It discovers and verifies the daemon's private native endpoint and instance, or claims a keeper when no daemon runs. The daemon or keeper continues owning the session.

### Connection Lifecycle

1. Client opens its assigned native connection
2. Server sends `mux.welcome` with its capabilities
3. Client requests the ordered workspace via `mux.workspace_resync` and uses `mux.resync` for individual screens
4. Server responds with `mux.screen_snapshot` containing full layout tree
5. Normal operation: client sends `mux.op` fire-and-forget, server mirrors state

No mux-level attach/detach — the client gets screen state via `mux.resync` on demand.

### Client connections

The native workspace uses one orchestration loop on Unix and Windows. Its concrete
`OwnedService` owns either an in-process service or an existing WSL distribution's
explicitly configured helper. Windows passes the distribution, absolute Linux
backend path and isolated configuration directory as separate `wsl.exe --exec env`
arguments; it performs no provisioning, daemon discovery or shell interpolation.
The helper carries independent session and API streams belonging to the same
service. Closing the window joins those streams and the owned helper; successful
cleanup requires an observed successful exit, not merely a closed carrier or a
forcibly terminated bridge. Existing released launchers retain their compatible
owner-close API.

The portable `core::native_api` catalog is the single source of native operation
names, role checks, argument bounds and deadlines, reexported by the service.
`transport::api_request` implements the API-only transaction over a concrete Peer:
monotonic IDs, cancellation-on-drop, old-reply rejection, charged matching replies
and explicit uncertain mutation failures. Embedded and owned clients delegate to
that implementation; the transport never imports domain operations. The native
`ApiStream` adapter and `LocalServiceClient` reuse it for settings, profiles, zoom
and templates without consuming terminal events or duplicating scheduling.
The shared catalog also owns `native_api::directory`, the unchanged merged-host
response shape, and the hub's other replies: remembered remote Screens
(`native_api::remembered`), forwards and their rules (`forwards`), shared local
ports (`reverse`) and host metadata (`hosts`). The hub projects its persistence
records into these contracts and keeps its files' records and strict checks to
itself; the reply types ignore unknown fields, and native clients do not depend
on server storage types.
Every `mux.welcome` capability name is defined once in `core::protocol::capability`,
which also assembles the welcome's list in its fixed order; the session handler only
says whether the connection is a local main one with a hub, and clients test the
same constants (`capability::offered`). The messages a server sends a window are
typed in `core::protocol` as well: events in `ServerEvent` (`messages`), one
connection's answers and requests in `session::SessionMessage`, and every message's
`type` in `message_type::MessageType`, which client-core and splitty-gpui match on
instead of strings. A type a build does not know reads as `MessageType::Unknown` and
is ignored; the handling module still reads the payload's fields itself, leniently
and within its budget, so a malformed known message is answered as before.

Outbound forwarding has a separate `client-core::ports` owner. Connection-scoped
port observations live in `SourceWorkspace`; the Ports controller reads those
observations and joins them with independently retained active and saved mappings.
The workspace never depends on that controller. Discovery is optional, and a
reported empty list differs from a connection that has not reported ports.
Disconnecting clears discovery authority without removing saved or live forwards.
The mapping identity is the canonical SSH target and remote port, independently
of host-directory membership and opened Screens.

The native worker composes Ports with its assigned local `OwnedService`.
Forwarding uses bounded, individually owned API jobs and a separate passive reader,
not the host-metadata queue: opening or stopping a tunnel may take substantially
longer than an ordinary metadata request. Per-mapping ordering preserves a later
Stop while unrelated mappings, metadata and terminal events can progress.
Read generations and an acknowledgement revision fence old passive reads: an
empty list started before Start cannot revoke a newer confirmed opening merely
because its response arrives later. Settled requests without a browser
continuation release their consent allocation; published continuations remain
revocable until consumption or cancellation. Uncertain mutations are reconciled
through reads without replay. The service remains the sole owner
of SSH forwarding, saved preferences and restoration. The native client uses the
existing `forward.*` and `forwardPreferences.*` operations without new wire types.

The native Ports view tracks browser origins separately from forwarding feedback.
Hosts actions belong to that management surface; terminal links retain their
exact terminal binding; Drawer actions retain the local owner and canonical host
group. A Drawer group remains valid through remote reconnect and ordinary popup,
visibility or destination changes, but losing its final opened reference, source
or owner revokes its pending browser intent. UI authority is removed even when a
best-effort cancellation command cannot be queued, without claiming the service's
forward stopped. Result handling still requires the exact pending mapping/request
and the Core's verified one-shot handle. Settled or duplicate results cannot
reopen a browser or overwrite an already recorded dispatch outcome.

The native app `splitty-gpui`, the default desktop app, uses `splitty-client-core` as a UI-free
adapter over one native connection per source and an isolated owned service. It names no other
crate of the workspace. Client-core links the server only on Linux and macOS, to start it
in-process (`embedded`) and connect to it over the same framed pipe and handshake a remote
client uses (`connection`, `api_stream`); every other module reaches the service through those
streams, and ending the service is one call of the server's runtime. The architecture check
enforces both (`architecture/README.md`, "What the test checks"); tests may build services
of their own anywhere. The explicit
`native_workspace` module owns the worker, command/update API and bounded retries
for attachment, visibility and size controls; GPUI only submits intents and
consumes updates. This runtime retries safe control admission, never user input
or layout mutations. Its `WindowWorkspace` owns the fixed local collection and
bounds admitted host sources, global selection and aggregate resources.

The bounded command and update channels require fair platform dispatch as well
as a bounded worker. Linux's pinned GPUI platform is patched at its native event
boundary: finite batches return to the event loop, and one client-owned weak
continuation drains buffered XCB events without depending on fd readiness.
Pending key-repeat normalization survives batch boundaries; XIM handling and
event order remain platform responsibilities. No second Splitty ingress queue,
input replay or scheduler policy is added to the terminal/domain model. The
published external source, license and patch provenance live under
`vendor/gpui-pre-linux`, outside workspace membership; the application still
depends on GPUI Kit through its existing seam. Dependency updates must preserve
the platform regressions and actual multi-source input/observation acceptance.

`SourceWorkspace` is the common connection collection: it holds ordered opened
Screens, source-local tab/focus choices, pending lifecycle intents and canonical
candidate admission. Local catalog policy opens every listed Screen; on an empty
first catalog it creates the first Screen with `onlyIfEmpty`, so when another
window's first Screen came first the service refuses this one and the next
catalog drops it. New local Screens go out without a name when the service
announces `screen-names-v1`, otherwise with the smallest free "Screen N" of the
catalog; a Screen without a name is labelled "Unnamed Screen", a pending local
creation "Creating Screen…", never by position. Remote policy keeps charged live catalog
metadata separate from explicit opened references, including removable missing
references; neither catalog entries nor host active hints create or select work.
The worker publishes the whole window after mutable source edits reconcile.
A common `SourceRuntime` owns one actual Client's catalog collector, attachment,
visibility and geometry controls, immutable connection token and close settlement.
It returns bounded effects to the worker, which publishes the reconciled window
before dependent Event/Attached/outcome deliveries. Binary output and immutable
input/geometry admission do not enter a structural source edit. Each deliberate
host has a retained setup context/future, optional common source driver, and
retained close future. Cancellation revokes the context but joins a late Client
and its cleanup before releasing the source reservation or enabling Retry.
Progress coalesces as one charged value; only fenced worker publication changes
the host presentation. Initial failure waits for explicit Retry. After previous
application readiness, retryable loss permits waits of 1, 2, 4, 8 and then at
most 10 seconds within one fixed 120-second interval, including preparation and
cleanup. Typed nonretryable or malformed failures stop recovery. A replaced
Client receives fresh execution tokens and reobserves only explicit references;
input, layout mutations and uncertain close operations are never replayed.
Host updates retain bounded semantic offers in the host presentation, including
an accepted `remote.ready` offer followed by a missing workspace capability.
The hub's update plan (`update: {available, keepsSessions, toVersion}`) names
the replacement version; only for an older hub without `toVersion` does the
local auxiliary API's `build.get` supply the assigned backend version; the UI
executable's version is not a replacement authority. One worker-owned
confirmation captures the local owner, exact host/source/attempt and accepted
offer. Dismissal, replacement or changed offer facts revoke it; unrelated display
metadata does not. The worker consumes confirmation once, revokes old execution
authority and awaits retained connection cleanup before dispatching one explicit
update-enabled host open. Existing references survive for fresh observation.
Connect, Retry and recovery always remain connect-only. Failed or cancelled
update dispatch is unconfirmed, never evidence of rollback; explicit Retry
observes effective state without replaying maintenance.

Explicit creation in an already opened ready remote host group uses one captured
`RemoteScreenCreateRequest`: fixed local owner, exact host/source/attempt, explicit
remote execution token and revocable selection permission. The local-owner command
composes Screens navigation with browsing withdrawal; only then does the common
SourceRuntime admit the remote mutation. A full UI queue preserves the destination,
and later navigation can revoke the shared permission without another queued command.
The collection reserves one reference and initial pane before sending the existing
`create_screen` once. Omitting name and launch fields delegates naming and defaults
to the host, including its unopened Screens. No speculative layout/session is exposed.
One bounded pending-or-latest record per host retains the requested IDs and mutation
watermark. The ordinary collector confirms current presence or absence only after
its catalog/snapshot/trailing-catalog fence. A complete candidate converts the
reservation using actual canonical content even if the first pane changed; invalid
candidates do not free current capacity or mutate shared follow permission.
Connection loss retains unresolved observation intent and unused capacity, clears
execution/routes/follow, and never resends Create. Current absence releases the
unused pane reservation but keeps a removable missing reference and historical
uncertainty; later presence must pass ordinary capacity checks again. Later coherent
observations update the retained record's current presence without reviving follow
or erasing historical uncertainty/rejection. Detach removes
client interest without closing host sessions. Follow remains revocable through
prepared source effects until final owner/request validation and atomic consumption
immediately before global selection. Dropping a cloned request or published model
does not revoke a live counterpart. A newer creation or explicit selection supersedes
older follows globally; accepted shutdown revokes them before stream cleanup.

The normal window-close assessment includes only local work. Accepted shutdown
revokes host demand and joins every retained host, local and API stream before
closing the owned service; published cleanup failures remain final failures.
Each `ScreenModel` keeps
its canonical `Arc<ScreenState>`, local tab/focus choices, an optional maximized
canonical slot ID and pane/session routes. Maximization borrows that slot, from
either tiled or floating roots, as the sole presentation root; it sends no
layout operation and maintains no second tree. Otherwise the projection borrows
the tiled root and all canonical floating entries. Selection, focus, cycling
and visible-size admission share that traversal.
An admitted split or cross-group pane move restores the full root, while a new tab
or reorder in the same group and a Screen switch preserve the local choice. Snapshot reconciliation
clears a removed slot and focuses a visible survivor. The close plan still reads
the complete canonical Screen, including hidden groups.
The GPUI view keeps one terminal entity registry across all Screens and renders
the shown Screen's presentation slots and splits directly, without a second docking
tree. Switching Screens preserves terminal emulation and attachments; hidden
terminals consume output and send protocol replies without claiming visibility
or PTY size. Returning panes publish their measured sizes again.

The native client distinguishes stable source identity from current connection
authority. `SourceId` names one admitted source; `SourceToken` combines that
namespace with the current connection UUID. Screen
and pane keys carry their source, while layout keys also carry their Screen.
Canonical IDs inside one owned model remain unchanged. These keys are client
context rather than wire fields or authorization grants. A local source uses its
initial connection UUID; a deliberately admitted remote source reserves a stable
namespace before setup, without inventing a transport connection. The source only
acquires an execution token after its actual transport and readiness prelude.

The window owns one authoritative `Option<ScreenKey>`. A narrow mutable
`SourceEdit` borrow reconciles the source's remembered choice with that selection
before publishing a model update. Removed selected references choose a surviving
neighbor in local-first grouped order. Terminal bytes, input and other read-only
admission do not take this structural mutation guard. Exact retained bindings
include early replay and offline views; execution additionally requires a ready
source and completed attachment. Session delivery resolves the captured source
and session even when its Screen is hidden.

Admission limits are per window: eight remote sources, four pending opens, 128
opened/model/pending Screen references, and 128 panes, sessions or pending pane
reservations. Offline/closing models and accepted template/transfer reservations
still count. A failed or cancelled opening retains its slot until owned cleanup
is observed. Source mutation receives the remaining allowance after all other
sources; candidate adoption is atomic. Simultaneous coherent refresh batches
share the existing 16 MiB staging budget, while each remote catalog is bounded to
200 rows and retains its connection-budget charge through view clones. A failed
catalog keeps last-observed metadata but cannot admit another remote reference;
only a current successful catalog enables Open. A new invalidation or explicit
Retry starts another bounded observation without replacing a healthy connection.

A native worker command envelope carries its captured source token. Admission
rejects a foreign or replaced connection before target lookup or effects, then
applies the existing membership, visibility and consent checks. Session deliveries
and attachment notices carry the same source context while retaining the original
charged event. Terminal tokens identify the exact source, connection, pane and
session; ordinary catalog refreshes do not invalidate unchanged terminal input.
Route generations fence geometry, Screen revisions fence structural gestures,
request IDs match pending actions, and document versions fence content. Their
distinct purposes remain explicit.

Window-close assessment before the first source publication stays a worker-channel
lifecycle intent. If readiness wins that race, the existing startup consent is
checked and current local impact is assessed; missing source authority cannot
authorize another command or skip confirmation.

GPUI retains terminal entities by source-qualified pane identity across moves
within that source. Callback bindings capture authority immutably; replacing a
geometry binding preserves its emulator and search state while requesting a fresh
size measurement. Each binding owns its pending resize slot, so an obsolete
closure cannot overwrite a newer measurement. Queue retries retain the original
capture and discard stale sizes instead of attaching current authority to old
dimensions. Connection or session replacement revokes the old executable view.
The worker reports a session only at a size its view measured in the current
route generation, never a placeholder, and sets `redraw` only on the first
report after an attach. A host source refreshes its routes after every layout
operation, so its views report again at unchanged sizes; those reports leave
the PTY, and the programs in it, alone. A metadata nudge (a program's title,
working directory or foreground process changed) does not refresh the routes,
as the `mux.workspace_resync` paragraph below describes. A refresh does not
`hide` a shown pane that keeps its session: the connection's last report stands
until the window's visibility for the new routes arrives, so another connection
showing the session at a different size never gets the PTY in between. The
emulator's grid follows the session's PTY size from the server, not the pane;
see [PTY Size](#pty-size).

The native Drawer is a view of that ordered catalog, not a second workspace
store. Relative Screen commands resolve the latest local selection on the
worker's ordered queue, so successive wheel steps do not depend on UI refresh
timing. Hovered-group tab navigation captures connection, shown Screen and group;
the existing focused-group keyboard guard remains separate. Navigation clamps
at both ends. Ordinary vertical wheel input anywhere over the Drawer navigates
Screens once; Shift-wheel input retains list-overflow browsing.
Tab headers retain horizontal/Shift overflow scrolling and terminal content
keeps its own wheel route. Editing, modal and drag ownership gates chrome
gestures. Drawer pane rows derive from the canonical layout and capture their
source execution token before dispatch; they cannot acquire replacement
authority from a later connection.

The native inline Screen name editor captures the window owner, source token
and exact Screen key when opened from the Drawer. It survives unrelated
selection and source updates, but closes when that binding is no longer ready,
opened or present in the current remote catalog. A valid Enter or blur sends
the existing `rename_screen` operation to the captured source; Escape cancels
it. The old name stays visible until the ordered canonical catalog reports a
new one. A rejected or lost operation is never replayed after reconnect.

Remote Drawer captions navigate passively to the exact current Host row; their
Ports menus use the same `ForwardSnapshot` mapping projection as Hosts. Navigation
recomputes the Host or Ports section index from the shared row projection and
canonical target, including when Hosts is already open. Aliases never route an
action, and navigating does not connect, browse or create a Screen. The bounded
menu distinguishes missing discovery from an empty report, keeps at most twelve
port actions visible and exposes the complete management view. An ephemeral
popup capture is established by the Kit open interaction, consumed at activation
and discarded on dismissal or group loss. Cached menu rows must still match the
current action class and local-port choice before forwarding admission; neither
rendering nor popup inspection submits a mutation. The Drawer retains the
actual Ports trigger focus through element-keyed state: cancelling returns to
that Button rather than the dropdown wrapper, while management navigation keeps
its destination focus and autohide returns to the remaining work area.

The native Drawer offers a new remote Screen beside each retained host group's
Ports action. Its Plus button captures that group's current source token and
uses the Core's pure availability check; activation queues one local-owner
creation request that combines leaving host browsing with remote mutation.
The view keeps only a bounded enqueue latch, a revocable follow permission and
the latest outcome interest. Accepted navigation or exact Detach revokes follow
immediately, while canonical pending, missing and rejected results remain
visible after destination changes. Unnamed remote references use neutral labels
until a canonical name arrives. Rendering, failed queue admission and result
projection never select a Screen or submit another mutation.

Native Screen ordering captures the local window owner and stable Screen keys.
Local placement requires `screen-placement-v1` and `workspace-catalog-v1`, sends
one relative operation and uses the existing mutation watermark to retain an
ordered catalog refresh through sender pressure. An earlier in-flight catalog
is followed by a post-operation read; no failed or uncertain move is replayed.
Remote placement changes only the source's opened-reference list, including
offline or missing entries, without remote commands or new browsing demand.
Live host catalog refresh and reconnect preserve that window order. Neither path
changes selection or terminal bindings, and neither adds restart persistence.
The Drawer retains only the captured drag identity and a transient insertion
marker measured against the current rows. It submits the same relative intent
as its Move Up/Move Down menu actions; changing order does not navigate.

Drawer wheel navigation uses a gesture gate: an idle gap longer
than 120 ms admits another Screen step, while accelerating or jittering frames
within the same continuous touchpad swipe stay debounced. Pixel noise below two
pixels does not start a swipe; native line events already represent wheel
movement. Ordinary selection and catalog acknowledgements preserve this gate so
residual momentum cannot repeat the change. Tab-strip distance handling and
terminal scrolling remain separate.

Screen navigation retains at most one outgoing `ScreenModel` projection for a
200 ms vertical Kit transition. Its terminal surfaces reuse the existing painter
with recorded geometry and source/route identity, without installing input,
resize or activity handlers. Only the current Screen owns live views and visible
routes. Reversal starts from the sampled position; a third Screen replaces the
outgoing projection, and owner/binding loss, removal, viewport changes, leaving
Screens or reduced motion discard it. A parent viewport guard preserves current
terminal focus when a click lands on the outgoing-only region. Layout still comes
from the canonical model; the animation owns no second mutable layout.

The Drawer uses two window-local Kit transition channels: visible overlay
translation and pinned content reservation. Revealing an unpinned Drawer never
reserves workspace width. Explicit pin admission changes the presentation target
immediately; the identified `home.update` outcome and latest accepted appearance
remain persistence authority. The selected `WindowWorkspace` Screen alone owns
visible terminal routes, size reporting and input. Split-drop feedback animates
the existing measured target bounds without submitting a layout operation.

New chrome uses the pinned GPUI Kit styled controls where their controlled
interfaces fit. The toolkit's complete DockArea owns its own PaneTree, so it is
not used for the canonical mux projection. Native event normalization stays in the view and
is tested for platform differences in line/pixel deltas and gesture phases.

Group chrome captures the displayed slot and its selected pane/session instead
of routing pointer actions through the globally focused group. Before a control
submits its existing tab, split, float or maximize command, it rechecks the
connection, Screen, pane/session and current slot selection. This presentation
boundary lives in `workspace/chrome.rs`; keyboard commands retain their focused
context. The toolkit supplies actual icon assets and button interaction states,
while the existing appearance projection and terminal scaling remain separate.

Cross-Screen transfers use the existing server `move_pane_to_screen` operation,
and `move_tab_group_to_screen` for a whole tab group.
The client captures connection, source Screen, pane and session identity from
the actual tab, submits once and verifies the authoritative destination. A
later local navigation cancels deferred follow without canceling verification.
The tab menu and its focus handling belong to GPUI; admission, observation and
navigation policy belong to the client workspace, and the server retains all
layout and session effects.

An ownership change cannot be committed one Screen at a time: source-first
delivery would remove a route, while target-first delivery would duplicate it.
The runtime therefore owns one bounded refresh collector outside the cloneable
UI workspace model. It retains the committed routes while collecting actual
Screen snapshots between ordered catalog queries. The trailing query also
settles missing-Screen reads, which send no snapshot. This orders reads on the
connection; it is not an atomic snapshot against concurrent clients. A complete
candidate must pass global uniqueness and admission checks before one route
delta is published. The collector limits aggregate serialized snapshot data to
16 MiB before retaining typed models, plus the existing Screen and attachment
counts. Failed collections release staged
data, report uncertainty and can recover through later reads without replaying
the mutation. External transfers use the same observation path. Stable
pane/session pairs retain readiness, attachments and terminal entities across
Screen changes; obsolete size and visibility claims are withdrawn until the
next actual layout. Unsettled ownership blocks scoped destructive actions but
keeps terminal output and assessed whole-window shutdown available.

Floating actions reuse `float_pane`, `dock_pane` and `move_float`. The client
captures the owner, Screen, floating ID, original bounds, structural layout and
pane/session identities. Metadata and local active-tab changes do not invalidate
a gesture; changed membership, geometry or ownership does. One identified
action is admitted at a time, excluding conflicting placement, transfer and
close actions. An ordered catalog fence and subsequent accepted Screen snapshot
settle its result; rejected or uncertain mutations are never replayed. Coherent
workspace refresh also settles pending floating actions. Later local navigation
cancels deferred float/dock focus, and moving bounds never changes focus.

GPUI owns overlay paint/hit order, display-only reachability clamping and
captured move/resize previews. Raising a float is local to the Screen. A changed
release sends one bounded geometry intent; Escape, loss of window activity or
changed owner/Screen/viewport/zoom cancels the preview. Terminal entities remain
keyed by pane/session identity through float, dock and transfer. Native Dock
uses the first tiled slot, including an empty one; it docks the whole group.
Nested floating trees received in snapshots are rendered defensively, while
native split, divider resize and tab drag placement remain tiled-only.

The native window retains one async receive task for the worker update channel.
New output wakes that task directly; a separate maintenance deadline still retries
visibility and size reports. Bounded batches preserve update order and retain
transport byte leases through terminal feeding. Large batches keep the existing
pacing, while repeated small batches also yield so input and drawing can run.
Dropping the task drops its sole receiver and releases queued delivery leases.

The same native runtime owns one auxiliary API-only local-service stream. After
publishing the session owner, it polls stream setup and separately charged
appearance, shell-profile and notification-settings reads alongside terminal events and commands.
The UI-free client settings module owns bounded field, value, edit and service
metadata contracts shared by that mutation path and the native editor. Appearance
keeps the charged settings projection with the theme and terminal palette names,
the OS preference it was read for and its terminal presentation preferences; it does not own the shared edit vocabulary.
Each feature has its own generation, dirty flag and result. Same-service settings
events refresh all three; owner-checked OS preference changes refresh appearance only.
Round-robin dirty reads prevent one feature from starving the others, and
superseded results cannot replace a newer choice. The same owned client admits
up to 32 queued zoom intents beside its active job and alternates ready reads
and writes. The intents that wait when a zoom job starts are that one job: it
reads the settings metadata and the current settings, applies each intent's
step in order through the service's levels (`zoom::ZoomSteps`) with the
UI-free `zoom` policy and patches only `appearance.zoom` when the result
differs, so a burst of steps costs two reads and one write. Intents that arrive
during a confirmed write follow it at once, and the appearance is read after
the last of them. Mutation outcomes carry independent request identities, one
per intent; a refresh cannot discard an acknowledgement. Failed or uncertain
writes are reported without retrying the mutation, and subsequent intents
begin with a current read. The window shows an admitted step at once
(`zoom::PresentedZoom`): confirmations and settings reads replace what is
shown only when no step is outstanding, and after a failed or uncertain write
the last confirmed value is shown again.
This is ordered within one client, not an atomic increment across independent
clients. Each feature reports its own failure while the others continue with
current data; terminals keep working. Setup and pending-job cleanup are
awaited on every worker exit before owned service shutdown, including update
receiver failure. The API stream never consumes terminal delivery. Appearance
font overrides remain chrome settings. Display zoom scales the native terminal's
14 px text and 18 px row baseline in both scopes, rounded to whole pixels of
text with the terminals scope; window scope also scales native
chrome through rem units without changing OS DPI. The styled toolkit Root owns
the rem baseline, so window scale updates its theme base font rather than relying
on a direct window rem assignment that the next Root render would overwrite.
The terminal view owns one
set of measured metrics for shaping, paint, pointer mapping and IME bounds, and
passes only grid and pixel geometry into the replaceable emulator. Hidden panes
retain desired zoom until measured on return. Changed cell pixels refresh the
emulator even when rounded grid dimensions stay the same; only changed visible
dimensions need a new PTY size report, and consecutive zoom steps report the
size once, shortly after the last. Applying zoom cancels gestures whose
captured pixel geometry is obsolete without replacing sessions, search queries
or the canonical workspace layout.

Drawer pinning shares the local presentation read: one charged `home.get`
projection retains appearance and Drawer settings. The
view restores `drawer.pinned`, edge-reveal policy and bounded saved width without
a startup write. Transient edge reveal belongs to the window: the unpinned
Drawer overlays the workspace without changing terminal geometry or writing
settings. Only the explicit pin command submits the identified visibility job,
which patches `drawer.pinned`, alternates fairly with zoom work and refreshes
presentation after its outcome. The view disables
overlapping toggles and retains confirmed state on rejection or uncertainty;
no ambiguous write is replayed. These preferences remain local-service-owned.

Host inspection and metadata management share a third stream, independent of
both ordered session delivery and the settings scheduler. `RuntimeHub` opens a local API stream only
after `hub-host-decisions-v1` readiness; embedded and owned adapters use the same
main-service authority without a remote target or update intent. Initial readiness,
ordered `hub.changed` events and explicit Refresh/Retry trigger `HubClient` reads.
One retained API request and a dirty generation coalesce invalidations across
cancelled receive branches. Directory errors wait for another trigger; a broken stream is
joined before reopening. A missing capability reports unavailable without polling
or a second directory merge in the client.

Directory snapshots retain their typed decode budget through shared immutable
ownership. Admission bounds the response to 1 MiB and validates version, identities,
duplicates and catalog field limits before publication. The API's merged host
count is not limited to the persistent catalog's 20 entries. A previous snapshot
may coexist with its replacement while either is still in use. Loading and
failure carry that same owner's last valid snapshot; a successful empty response
replaces it. Owner UUID and client read generation accompany every update. A
same-generation completion follows Loading, while an older or different-owner
result cannot replace current presentation. A connection replacement drops the
old view state instead of carrying its observations to another service.

Typed host management uses the same hub stream with a bounded FIFO of 16 pending
actions, excluding overlapping actions on the same exact target. Validated
requests distinguish explicit Connect, display-name change/clear and metadata
removal; omitted Connect names preserve an existing alias. Identified outcomes
carry the full local execution token and remain separate from directory read
generations, so `hub.changed` cannot invalidate a valid write acknowledgement.
Uncertain replies and potentially partial server failures trigger passive
reconciliation without repeating the mutation. Recent admitted request identities
are retained across stream replacement with bounded storage.

Explicit Connect first reveals the entered target through existing local host
preferences. Only a confirmed preparation acknowledgement allows the worker to
acquire browse demand through the shared remote admission path; directory-row
Connect still requires an accepted known target. Worker-owned request withdrawal
handles cancellation racing preparation or admission, while leaving Hosts releases
all browse demand. Open references keep their sources alive independently.
Metadata removal never detaches Screens or stops sessions or forwards. GPUI owns
retained input, focus and rendering, combining confirmed directory rows with
current connection work without claiming the latter is saved metadata. Revealing
an already connected unnamed target need not recreate a recent entry until a
fresh handshake; an explicit display name can keep it discoverable independently.

Hub setup, reads, submitted writes and stream cleanup remain owned across cancellation. Shutdown
discards unsent writes, revokes setup, joins pending work and finishes the hub stream alongside settings
and session cleanup before closing the service. A previous stream-cleanup failure
remains a failure of final shutdown even after it was displayed. Closing Hosts
alone leaves the local workspace and its terminals alive.

The same `LocalServiceClient` admits one identified template-library job alongside
settings and zoom work, with a fair turn between library requests. The template
module owns charged immutable drafts and catalog data; it reuses authenticated
list/load/create/delete operations and never reads files directly. Extraction is
asked on the shown Screen's own session connection, local or on a host, and its
draft comes back to the same dialog. Save
filters the reviewed draft rather than extracting again and sends it by name
through `template.create`; the service names the file, and an existing one comes
back as `OverwriteRequired`, saved over only by an explicit choice with
`overwrite`. Without `template-names-v1` from the local service the worker
refuses the save. Mutation uncertainty is reported
without replay. API setup, reads and writes retain their owned future through
poll cancellation and join cleanup on shutdown.

Template application belongs to the workspace model, not the library module.
A sealed plan captures the exact draft/root, client-generated pane and Screen IDs,
shared session-free projection and replacement close impact. Submission revalidates
capability, ownership, capacity and current affected identities before one session
`apply_template` operation. The pending view has no session routes and withdraws
canonical visible-size claims while retaining output. Later navigation cancels
following without resubmitting or cancelling the operation. Coherent post-fence
observations settle the layout separately from launch readiness; failed terminals
remain visible diagnostics and do not repeatedly attach nonexistent sessions.
Successful profile fallbacks retain bounded notices until the confirmed pane/session
identity can be published; these warnings do not suppress successful attachments.
Pending work adds an unresolved whole-window close identity, and scoped structural
actions remain excluded until settlement. A nonmatching layout is uncertain unless
a scoped rejection and unchanged captured Screen establish non-application.

The new-tab profile picker retains a charged service snapshot, with bounded
profile count, identifiers and labels. Its explicit choice captures connection,
Screen, source pane and session; the worker rejects a replaced or no longer
displayed source before submitting the profile identifier. It never reconstructs
commands or arguments. Ordinary tabs and splits omit launch overrides so the
service inherits its current known CWD and profile. Profile fallback remains
service-owned; a notice is shown only for an admitted pane/session route.

Native pane, Screen and window close gestures share a client-core close plan.
It captures the connection, affected pane/session identities and the client's own
unresolved work, including hidden Screens and pending creations. What the close
ends (running and starting programs, scratchpad text, annotations) comes from the
service's close assessment (see Close assessment): GPUI sends the plan's scope as
`mux.assess_close`, waits up to five seconds for the answer, attaches it to the
plan and words its one cancellable prompt from it; without the capability or an
answer it asks a generic question. The worker reassesses the identities and
pending state before submitting an existing close operation and keeps the
assessed impact. A submitted operation stays pending until an ordered catalog
refresh and, for a surviving Screen's pane, a subsequent Screen snapshot prove
removal or report rejection. A normal window-close request is vetoed until the
worker approves the matching plan. The worker enters closing before publishing
approval and then awaits its owned service cleanup; the application joins the
worker after the UI event loop exits.

The capability-gated `mux.workspace_resync` request runs in the serial session
loop and returns an ordered catalog plus an initial active-screen hint from one
store read. It therefore follows earlier lifecycle operations on that connection;
the separately dispatched native API request path is not a mutation barrier.
One outstanding refresh and a dirty follow-up coalesce requests. A local mutation
watermark prevents an older catalog response from retiring a newly submitted
Screen or its pending terminal. Existing mutation nudges refresh the catalog;
known-Screen `session_metadata` nudges only refresh that Screen. A host source,
which otherwise observes every change through the coherent refresh of all its
opened Screens, also reads only that Screen for such a nudge while its routes
are settled (no catalog read, creation, transfer, floating action, template or
tab placement pending), keeping its routes and their generation. The host
sends the same nudge when a pane closes on exit, so a read that shows another
layout, name or pane/session pairing than the Screen had starts the coherent
refresh instead of being applied. Later active-screen hints do not replace a
valid window-local choice.

Typed lifecycle operations create, rename, activate and close Screens. Typed
`ScreenOp`s request splits, tabs, pane closure and split resizing, followed by an
explicit screen snapshot because a successful op does not nudge its own connection.
Tab selection is window-local and also persisted through `select_tab` on servers
advertising `tab-select-v1`. The server removes a Screen with its last pane, so the
Screen leaves the window through the catalog like a closed one. A service of an
earlier version keeps it without panes: once the close of a pane the window
requested is confirmed and the snapshot lists no pane, the source sends
`close_screen`. A Screen without panes that the window only observes stays and can
receive a new terminal under its existing ID and name, on a host as well. A window
without any Screen shows an explicit empty workspace with one action that creates
a Screen.
Shared admission counts all attached and pending terminal routes across Screens,
with separate catalog retention bounds. Authoritative removal retires only that
Screen's routes and entities. Focus restoration is tied to navigation or terminal
replacement so unrelated updates cannot interrupt a rename editor or close prompt.

Exact native header placement uses capability-gated `move_tab`, carrying source
and destination slot IDs plus a before-tab anchor or append intent. The pure mux
reducer validates these against current state, preserves effective selection for
same-group reorder and derives surviving source proportions for cross-group moves.
It retains pane/session records and surviving node identities. Legacy `move_pane`
keeps append/activate and directional split behavior. Whole-group drags use
`arrange_panes`.

One pending exact placement lives in its `SourceWorkspace`. Admission commits its
mutation watermark; an older in-flight catalog cannot settle it. A same-owner
catalog covering that watermark is the ordered fence for failures and Screen
existence. After it, the first matching accepted Screen snapshot proves exact
adjacency/end placement or settles supersession. A matching but unaccepted older
revision, malformed verification, or removed Screen ends verification with an
error and retains the last accepted model. Read refreshes can retry admission;
mutations cannot. Later local navigation cancels deferred focus without dropping
verification. GPUI owns only tab-strip geometry, scrolling, insertion feedback and
selected-tab reveal. Previous/next tab commands capture owner, Screen and slot and
are clamped against the worker's current selection in the shared client model.

Terminal commands enter one shared bounded worker queue directly through a
terminal-local callback. Each workspace callback captures its pane and session
identity; the worker checks that route again before sending input. There are no
per-pane forwarding queues: successful admission reserves a slot in the worker
queue, and a full or closed queue rejects the command immediately with a visible
error in the affected terminal. Input messages are capped at 64 KiB and a paste,
including its bracketed framing, occupies one queue slot. All panes therefore
share the same 64-command bound rather than accumulating a separate input budget
per pane.

Native tab dragging captures its connection, screen, pane/session and immutable
layout snapshot. Metadata-only revisions remain admissible; changed slot
membership, node IDs, axes or sizes cancel the gesture. A drop submits one
`move_pane` through the same bounded worker queue, preserving surviving source
proportions. Terminal entities and attachments remain keyed by their existing
identities.

Native layout changes made with the pointer show at once, on local and remote
screens alike. `ScreenModel` keeps the service's last state beside the shown
one (`LayoutPreview`): a submitted `move_pane`, `move_tab` or `resize_split`
is reduced locally with the shared mux reducer and laid over every snapshot
until the service reports it, reports another structure, rejects it
(`mux.op_failed`) or five seconds pass; then the service's layout shows again.
Revisions stay the service's. A dragged divider is drawn from the drag itself
and keeps its origin whatever snapshots arrive; sizes go out every 32 ms and
at release. A split made by an unconfirmed move cannot be resized until the
service has named it.

Terminal input remains bytes from the native view through the generation-checked
workspace command queue. `Client::input_bytes` preserves UTF-8 via legacy `input`
and uses capability-gated `input_bytes` for other bytes. The shared dispatcher
bounds standard base64 before decoding, charges its decoded allocation through
the PTY write, and retains existing session authority and ordered writes. No
mouse-specific backend writer or automatic input retry exists.

The terminal adapter derives pointer encoding and tracking from the native VT
encoder. GPUI supplies fractional physical geometry and retains the gesture's
local/application choice and actually admitted presses beside view lifetime and
queue admission. Shift starts local selection; lifecycle cancellation settles
only the captured live terminal. Wheel uses application reports when requested,
otherwise the existing local history or alternate-screen cursor-key path.

A terminal body context gesture follows that same admitted pointer route: application
mouse reporting wins, while Shift admits a local popup without mouse bytes. The
terminal emits intent; Workspace owns one Kit popup captured to the displayed
Screen, source owner, pane/session, terminal entity and emulation generation.
Callbacks revalidate those identities. Clipboard and Find reuse keyboard paths;
split and assessed close reuse captured workspace admission. Dismissal restores
only the still-current clicked terminal while the popup owns focus.

Retained offline buffers still support Copy and Find. Their exact displayed
terminal identity admits pure focus through `WindowWorkspace::focus_terminal`;
the native worker handles this intent before transport dispatch and preserves it
in later model publications. The UI queues the captured intent before optimistic
focus. Closing, disposed, replaced and non-displayed targets fail validation.
This grants no live execution authority: Paste, split and close also require the
live pane authority captured when the menu opened. No focus or local search
operation reconnects a host or queues terminal input for replay.

The UI-free `NotificationCenter` retains at most 100 small owned notification
records per window. It borrows the existing charged event only while reading the
`notice` the server attached (`protocol::notice`, `notification-notices-v1`):
kind, severity, title, the `notifications.*` switch that governs it and, when no
window should notify, why. The server builds the notice once: the notification
engine for `process.finished`, `process.idle` (suppressed while the session holds
a root program status record) and `session.exited` (suppressed when the server
closed the session while its program ran, which the PTY session remembers as
`closed_on_request`), and the program status watcher for a `program_status.changed`
marked `notify`, titled with the screen's label from the server's catalog. The
center accepts only the source's current connection token, drops suppressed
notices, applies the window's own switches to the named setting (a switch turned
off also removes the records it governs) and records a notice for the watched
pane as read without a fresh notice. An event without a notice, from an older
server, gets a generic `info` notice ("Activity in <process>") governed by
`notifications.enabled` only; bells are no notification. Titles are bounded
plain text with window-supplied remote host attribution; bodies, arbitrary JSON
and transport leases are never retained. The UI supplies the watched pane and
reads source-qualified count, newest-title and severity projections. The GPUI
window supplies actual focus and visibility observations, marks its displayed
active pane read on return or activation, and projects count/title summaries and
severity dots in the Drawer. Pure read-state changes send no workspace command.

Notification switches use an independent coalesced `home.get` job on the same
auxiliary stream and budget. The worker stamps its local owner; the view accepts
only the matching connection and newest read generation. Admission starts only
after a valid read, retains the last good switches on failure and purges disabled
categories on successful refresh, without writing settings or depending on theme
resolution.

Read/dedup history belongs to the source namespace across a reconnect. Badge
bindings require the current canonical pane and captured session, independently
of terminal attachment or command readiness. Reset gaps hide bindings without
pruning history; fresh exact mapping restores them without replaying events.
Unknown-at-arrival sessions stay permanently unbound. A complete observation can
prune a removed or replaced-session pane; source disposal or local-owner
replacement clears inapplicable records, and the latter also resets settings
admission. Reading and reconciling this state never sends a server operation.

Native terminal search is local to the terminal adapter and view. The adapter
owns the search handle, checked allocator and tracked navigation anchor; its
public results contain only owned, viewport-clipped cell spans. A 16 MiB budget
covers native search allocations across primary and alternate buffers; a 20,000
active-buffer match guard limits expensive result operations. Every native call
checks a sticky allocator failure because the library can return success after
an internal allocation failure. Reaching a limit discards the index and presents
a stopped state, never a complete partial result. Small terminal-owned registry
and pin allocations are outside that search budget.
Owned result staging is bounded separately: at most 20,000 native match records,
an 80,000-cell viewport mask and coalesced row spans. Larger grids refuse search
without affecting terminal operation; the current native view fits that bound.

The view owns query editing, yielding incremental work and highlight painting
between cached backgrounds and glyphs, with local selection taking priority.
The workspace activates only the focused visible pane's index, releasing the
previous one first. Inactive panes retain query/open state, but rebuild their
index on activation. Output and viewport changes wake search work; completed
or failed searches do not poll while idle. Search has no session protocol or
service authority, and its input controls cannot send bytes to the PTY.

Native terminal links cross three separate boundaries. The terminal adapter
resolves an owned target from OSC 8 metadata or a complete soft-wrapped logical
line, without retaining native grid references. Retained explicit metadata takes
precedence even when rejected. The pinned VT parser discards OSC 8 commands
that exceed its 2,048-byte fixed capture buffer before storing cell metadata;
it exposes no shortened URI. Their visible labels are ordinary text and can
still contain detected URLs. The adapter does not duplicate the OSC parser. Plain detection uses byte ranges mapped back to
grapheme cells, including wide tails; extraction stops without a target when
16 KiB, 128 rows or 80,000 inspected cells cannot contain the complete line.
`client-core::external_links` admits and normalizes bounded absolute HTTP(S)
URLs without I/O. The adapter also reports the viewport cells a link covers,
which the view underlines on hover. The view previews that actual target and
owns the click. Without application mouse reporting a plain single click opens
it: the press starts an ordinary selection, and a release on the pressed cell
without a drag or a selection is held for the double-click interval, so that a
double click selects without opening; another press or any cancellation revokes
it. Ctrl/Cmd-click opens at once and is the only link gesture while the program
takes the mouse. Gesture completion resolves the target again; cached hover cannot
authorize a launch. Selection and application mouse reporting retain their
routes, while an admitted Ctrl/Cmd link gesture remains local through cancellation.
The view observes captured movement at window scope so leaving and returning
to a terminal element cannot rearm a click that the element did not receive.
There is no terminal-adapter dependency on client policy, no server-side browser
effect and no generic editor/file scheme dispatch.

Workspace ownership routes the admitted link. Local and non-loopback web links
keep direct validated browser opening. A remote loopback link captures its
terminal/source identity and asks the local Ports controller to open that exact
mapping. A matching opening acknowledgement and a subsequent passive observation
of the same up mapping are required before publishing a one-shot browser result.
The client constructs the localhost URL from the confirmed local port, retaining
only the original HTTP(S) scheme, path, query and fragment. Stop, replacement,
cancellation or closure revokes the pending consent; changing an unrelated Screen
does not redirect it. Final platform dispatch rechecks consent and ownership.
These checks do not reserve the listener through the browser's later connection.
Windows uses the existing selected-WSL helper and Windows browser contract;
WSL-side success alone is not evidence of Windows endpoint reachability.

The terminal view owns a thread-confined Ghostty VT engine behind an owned Rust
wrapper; the wrapper contains the C interface and hands the view bounded render
snapshots. Cached glyph shaping and change-driven GPUI notifications avoid
reshaping unchanged cells and repainting an idle window. While a program has
synchronized output set (DEC private mode 2026), a paint shows the snapshot from
before the update began; the mode's reset, a resize, a reset, a local scroll or
selection, or one second since the update began bring the terminal's state back
into the frame. The retained `--probe`
fixture shows the earlier single terminal and native Markdown document; `--smoke`
exercises that fixture. Received native frames and decoded messages keep their
delivery charges through bounded UI queues, and versioned document reads stay
with their connection generation. The optional CEF fixture owns only offscreen
pixels and has no service bridge. The embedded composition exists only on Unix;
the Windows launcher owns a native stream to its selected WSL helper. Closing the window joins its worker
after closing its stream and isolated service. See [the spike report](gpui-spike.md)
for platform evidence; released clients remain unchanged.

Messages are resolved only against objects of the source connection on which they arrive. Output, attachment replies, spawn results, snapshots and nudges cannot target another source's sessions or Screens. The native source runtime owns its connection generation; replacement invalidates pending work and never replays disconnected input. Window-local focus and tab selection remain separate from authoritative layout state.

## ID Ownership

Pane and screen IDs are **client-generated** and random. The client sends them as `newPaneId` and `screenId`; the server uses them as-is and rejects a pane ID already present anywhere in the workspace. When a CLI op omits an ID the server generates it. With `screen-names-v1`, the server names a new Screen with the lowest unused "Screen N", including Screens the window does not show. Older servers receive the lowest free name known to the client. `create_screen` with `onlyIfEmpty` refuses creation if another Screen already exists, preventing concurrent initial windows from starting duplicate work.

Client-generated IDs let the window identify pending work before the server processes it.

## CLI Flow

`splitty agent guide` is an offline CLI operation, handled before server discovery
and allowed as local information under scoped invocation. Its versioned Markdown
is embedded in the CLI; `--json` wraps that same text as `{version, guide}`. It does
not read server configuration, connect to an owner socket, or inject an agent prompt.

New PTYs discover an executable `splitty` in the host executable's installation
directory (the server itself, or a sibling of the native GUI). They prepend that
directory to their child PATH once and supply the non-secret `SPLITTY_CLI_DIR`
hint. A starting daemon first points `bin/splitty` of its run directory at its own
CLI (`cli_path::publish_stable_cli`, a link renamed into place); a spawner hands out
that `bin/` directory instead of its own while the link resolves to the same file as
its CLI, so a kept shell, or an agent started in one, reaches the next daemon's CLI
after an update. Bash restores it after its existing startup replay, and Zsh at its prompt
hook after user startup. The common PTY spawn path removes stale inherited hints;
no shell configuration or server-global environment is changed. `agent exec` uses
the same directory/PATH helpers for its child while retaining its grant-only socket.
GUI-only packages without an installed/bundled CLI keep their existing PATH; no
fake GUI alias or runtime CLI installation is created.

Bash integration defers its normal startup files to a bootstrap before the first
prompt, or before an explicitly interactive `-c` command. It preserves the actual
login-shell flag, sources the selected profiles/rc file once, and restores inherited
`PROMPT_COMMAND`; it does not use POSIX mode or replace `ENV`. Initial history-file
access is deferred until those files have selected `HISTFILE`, `HISTSIZE` and
`HISTFILESIZE`, preventing premature truncation. If a profile explicitly populates
the history list (for example with `history -r` or `history -s`), the integration
retains that list without an additional automatic read. This differs from native
Bash configurations that deliberately preload a file and then let Bash append a
second copy. Noninteractive commands retain Bash's own startup path.

CLI commands use HTTP framing over an owner or scoped Unix socket. Discovery
checks `SPLITTY_AGENT_SOCK`, then `SPLITTY_SOCK`, then the daemon control socket. Layout commands post `/api/mux/op`; the reducer applies the
intent, the effect runner returns actual outcomes, and clients receive successful spawn
mappings followed by a nudge naming the actual screen. No UI is needed to start a process.
Queries read the authoritative screen/workspace state. `reqwest` handles framing over Unix sockets, with bounded responses, a deadline, no redirects and no environment proxy.

### Managed jobs and local agent grants

`splitty_server::jobs::Jobs` owns accepted start tasks, idempotency, live handles and
authorization. Every `ClientState::new` wires the same coordinator, including embedded
services. Metadata remains in the pure mux store; clocks, PTYs, socket lifetimes and waiters
remain outside it. Typed effect outcomes prevent false successful-spawn events. An injected
notification bus delivers typed job changes through protocol senders without a dependency
back into the client session adapters.

The owner socket alone provisions/revokes grants and transfers jobs. A grant has its own
private listener and fixed job-only router. Listener identity supplies the principal;
JSON never does. Grant read/write gates and per-job lanes order input, transfer and revoke;
pending input also checks its authorization epoch. Log waits release action gates, hold
bounded semaphore permits, and reauthorize when they return. Weak coordinator references
in grant routers avoid listener/service reference cycles. Accepted spawn work survives a
request disappearing, while server shutdown closes admission and stops managed processes.

Directory validation runs outside admission/authorization locks, in a bounded blocking
pool with a three-second request deadline, followed by reauthorization. Managed PTY
masters use nonblocking writes so a program that never reads stdin cannot pin a grant's
action gate indefinitely. Their single reader waits with `poll` on an owned descriptor;
ordinary shell PTYs retain their existing blocking reader. A full input buffer reports
an I/O error synchronously rather than leaving an input task that could write after revoke.

Job snapshots project only bounded lifecycle metadata alongside pane job IDs, not every
stored launch argument. The incoming-message validator preserves these fields and fails
closed on unknown states. Background job additions/metadata updates preserve existing
terminal renderers, tabs and keyboard focus; failed/interrupted job views never create a
shell. New remote screens retain explicit host-directory/reopen semantics.

## 2D Grid Projection

`crates/splitty-core/src/mux/grid.rs` projects the layout tree into a flat list of `PaneRect` structs with relative coordinates (0–1). Each rect has `{pane_id, slot_id, x, y, w, h}`. Stacked panes in the same slot share identical rects.

`find_neighbor(root, pane_id, direction)` uses the grid with a 90° cone filter.

## MuxManaged Flag

When mux is active, `Connection.mux_managed = true`. This tells the session protocol's `Connection::close()` to NOT kill PTY sessions on disconnect — only output streams are canceled. PTY sessions continue running independently.

## Session Output

A `Session` (`pty/session.rs`) is the session layer over a terminal (`pty/terminal.rs`): the session builds the spec (program, the full environment, directory, size, managed or not), and a terminal factory starts the program on a PTY, in this process (`LocalTerminal`, embedded servers) or in a session keeper (`keeper::client::KeptTerminal`, daemons; see Session Keepers). Either feeds the session every chunk of output from the first on (`Feed`): it goes into the scrollback ring buffer, through the mode tracker and the OSC scanner (title, OSC 133, bell watch channels, program status records) and to each subscriber, all under one output lock. The local terminal reads the PTY on its own reader thread, `pty-reader-<id>`, until EOF whether or not any client is connected. So output is captured, prompts are tracked and a busy process never blocks on a full PTY buffer while nobody watches.

Ahead of all of these, the probe matcher (`pty/probe.rs`) removes each status probe, `ESC ] 7501 ; ?` with BEL or `ESC \` (the Program Status Protocol, revision 0.4), holding back at most 9 bytes of a probe split between chunks, and queues its fixed answer, `ESC ] 7501 ; ?` with the probe's terminator, at most 16 per session; no buffer, mode offset, subscriber or replay holds a probe, and no other byte is ever written back to a program. Every write to the terminal (client input, the control API's keys, the delayed startup command) goes through one asynchronous input gate per session (`Session::write`): holding it, a writer first writes the queued answers and then its own bytes, which a kept terminal may send in several frames; a task woken by a queued answer writes answers while nobody else writes. So the answer lands ahead of a client's reply to a query the program wrote after the probe, and never inside other input.

The OSC scanner (`pty/osc.rs`) reads every OSC to its end, BEL, `ESC \`, CAN, SUB or an ESC that begins another sequence, and collects payloads only for OSC 0, 2, 133 and 7501, each within its limit (a whole OSC 7501 is at most 4096 bytes); so the BEL that ends an OSC is never a bell, also for numbers and payloads it does not read. The byte 0x9d does not begin an OSC, since in UTF-8 output it is part of a character. `ESC c` outside a string is a full reset. The scanner's findings come in byte order: status reports go to the session's status store (`pty/status.rs`), which parses them (fields, limits, base64 with and without padding, sanitizing `title` and `msg`), keeps a root record and child records by `id`, at most 64 with the root never evicted, and counts a revision that changes with the root record. `working`, `blocked` and `idle` records end at OSC 133 `A`, at the end of the output and when the session stops being busy (see Server-derived Pane State); a full reset removes all records; `done` and `error` stay until the program replaces or clears them. The client sessions' watcher (`client_session::program_status`) checks every session's revision every 100 ms while a client is connected, one atomic read each, and sends `program_status.changed` with the session's pane and screen to every connection, streaming the session or not: the records held right after a client's `mux.welcome` and its ports, then each change, removal and move of a pane to another screen. It marks an event `notify` for a record that turned `blocked`, `done` or `error` or changed its kind or message, from live output, at most once per session and state in 10 seconds, and gives a marked event the notice windows show (`protocol::notice`); windows decide whether to show a notification. `GET /api/mux/state` and `GET /api/mux/screen/{id}` add each session's root record as `programStatus`; workspace snapshots do not carry it.

The mode tracker (`pty/modes.rs`) follows the terminal modes that outlive the output that set them: the alternate screen, autowrap, cursor keys and keypad, cursor visibility and style, bracketed paste, focus reports, mouse tracking and its encoding, alternate scroll, color-scheme and in-band size reports, `modifyOtherKeys`, and the kitty keyboard flags with a stack per screen. It reads 7-bit CSI and `ESC` sequences only, skips OSC, DCS, APC, PM and SOS strings, keeps its parser state across chunks, and notes the output offset of each change. For the alternate screen it keeps the switches within the retained output (at most 1024, pruned as the ring's oldest offset moves), so the screen at the oldest retained byte is known; for every other mode only its last change. `ModeTracker::restore(oldest)` returns the sequences that put a fresh terminal into the state the output from `oldest` on starts in: the normal screen's kitty stack, the alternate screen as it was at `oldest` with its stack, then every other mode whose last change lies before `oldest`; a mode changed at or after it is left to the replay. After a daemon restart the tracker is rebuilt, like the OSC state, from the keeper's oldest buffered byte, and `splitty attach` straight to a keeper gets the keeper's buffer without a restore.

Connections never read the PTY. `SessionHandle::subscribe_with()` returns a charged snapshot, the mode tracker's restore for the ring's oldest offset followed by the scrollback, and a bounded receiver for later chunks, taken under that same lock, so replay plus live stream contain each byte exactly once. `handle_create` and `handle_attach` send the snapshot in 32 KiB frames (attach: before `attached`), then `stream_output` forwards the receiver. Detaching, disconnecting or replacing a stream only drops that subscription; the reader is untouched, and independent admitted connections can stream one session. Stopping a connection's stream (detach, a replacing attach, close) waits until its task has ended, so nothing it was forwarding arrives after the next response. Once the output has ended or the process has exited, `stream_output` waits up to 2 s for the other before sending `closed`, so the exit code is known; the notification engine hears of the exit once per session (`claim_exit_report`).
Download chunks reserve raw read and Base64 scratch space before scheduling a blocking read. That lease stays with the worker through cancellation and through encoding; the serialized chunk has its own ordered-queue lease. A final chunk that cannot be queued ends the offer as disconnected, not saved. Session event JSON is measured first and allocated at its exact charged capacity.

Incoming JSON has a separate admission estimate before parsing: structural tokens cover container capacity and growth, raw string lengths cover owned strings, and escaped strings include serde's reusable scratch buffer. Typed native requests/replies and mux conversions additionally reserve error-formatting space, since a malformed scalar can expand when quoted in an error. These are conservative bounds for the current serde_json configuration and the protocol types, not a promise about arbitrary custom deserializers. Allocation-meter tests exercise compact arrays, sparse maps, repeated keys, escaped strings and typed error expansion. Native API arguments retain their lease through the executing future; relays and native requests/replies hold theirs through validation or delivery. Large serde errors are discarded under that reservation before returning a fixed diagnostic.
Session dispatch parses once and moves values into its typed request. Scalar checks use fixed errors, so image/file strings do not pay the error-formatting multiplier. Native text moves its existing frame allocation into the session queue; the input text is released before image decoding. Image workers retain the Base64 lease and a separately admitted raw buffer, and upload workers retain decoded-chunk and copied-metadata leases through actual completion, including cancellation. An in-process framed test stores and verifies a full 10 MiB image; separate tests refuse JSON amplification without disrupting a healthy client. Native controls use reserved control capacity for frame bytes and parsing. A strict object/scalar decoder rejects duplicate fields; sealed control schemas validate types and enum tags before conversion so malformed fields cannot allocate large serde diagnostics. The parser lease shrinks to retained struct/string capacities after conversion, leaving scratch capacity available for subsequent controls. Service identity, queued progress and preparation errors retain those smaller leases until their owners drop them. Maximum-sized escaped diagnostics remain usable alongside retained readiness/progress metadata. Desktop carrier reservations and the remaining framework allocation limitations are described below.


Handshake adapters reserve serialized control frames before allocating their buffers and retain the lease through partial writes. Structured remote diagnostics reserve both the inner JSON string and its escaped outer Closed frame; exhausted capacity propagates rather than triggering repeated diagnostic shortening. Fixed-host relays charge their assigned-purpose Ready copy; the direct daemon bridge borrows the already charged upstream metadata. Preparation progress shares the actual connection budget.

Notifications have no shared serialized-message ring. The core notification bus holds weak registration objects and synchronously offers each typed event to the subscriber's byte-bounded sender. Each session keeps its subscription alive and uses one pending notification queue with the same connection/process context as its main output. Job events, mux/service broadcasts and port changes all enter that queue, including during registration; welcome is sent directly before pending events are forwarded. Forwarding moves the existing buffer and lease without another allocation. A failed recipient is closed while delivery to others continues. Per-client saturation affects that client; aggregate process-budget exhaustion can reject additional recipients too. Dropped registrations cannot be revived by other aliases of their sender, and registration/send prunes stale weak entries. Mux batches retain pane-spawn-before-nudge order, and a job operation drains its notifications before the next request can resync.

For client mux operations, `Store::dispatch_inspect` compares the operation's before/after screen revisions and membership under one lock. The session handler resolves process effects and publishes a single batch: successful pane-spawn events, then one nudge per affected screen to other clients. Layout-only edits follow the same path; order or activation changes with no changed screen use a screenless nudge. `select_tab` persists the chosen pane in the server's slot but sends no nudge to other windows; each keeps its shown tab until it changes it. Every other parsed rejected operation reports `mux.op_failed` to its sender, whose mux client requests the named screen's snapshot. An absent screen has no per-screen snapshot response. The native client reconciles membership through the ordered workspace catalog described above.

All subscriptions of a client share its delivery context with the session and native queues. The snapshot and queued/live chunks remain charged until consumed, not just until dequeued. A stalled subscriber closes that client rather than dropping a partial application frame; capture, keepers and other clients continue. Each queue also has a 1024-item bound so tiny-message floods cannot evade byte accounting. API response vector capacity is reserved while serializing, including results waiting for their caller; control-socket response bodies retain those leases through HTTP delivery. File/editor request guards remove their registrations synchronously on cancellation, without leaving a detached expiry task.

SSH setup commands reserve their fixed stdout limit, stderr tail and read scratch before spawning. Their captured output retains the actual vector-capacity reservation after the worker exits and through parsing/inspection by the caller. Exhausted byte capacity reports the same retryable busy condition as exhausted worker capacity. Long-lived stderr readers reserve their tail, lossy-UTF-8 conversion and trimming scratch before spawn, then return a diagnostic with its retained string-capacity lease. Native bridge stdout wraps its 8 KiB reader and reservation together; spawn buffers, negotiation and peer traffic share the connection budget. Forward readiness reserves 24 KiB, reverse readiness/draining 40 KiB, before spawning. A reverse stdout drain owns its reader and lease as one value so cancellation destroys the reader before returning its capacity. Source diagnostics retain their charges through SSH failure classification, old-CLI conversion, installation-status parsing and unsupported-host errors; native failure copies acquire their own reservation before formatting.

The native adapter uses `splitty-transport`: a five-second framing handshake, bounded backend preparation, then a separately acknowledged service instance and generation. Application input is admitted only afterward. Its single persistent reader preserves partial-frame state across heartbeat ticks; a cancelled partial read/write always ends its carrier. Text/binary/request/reply frames retain application order, while cancellation, liveness and delivery credits have separate bounded control queues. Data includes a random receipt trailer; consumption returns that receipt once, and a still-active writer retains its own guard even if credit arrives concurrently. Consumed receipts wait as charged compact records in their own 1024-slot queue, matching the delivery window, rather than occupying the 64 ordinary control slots. The writer prioritizes cancellation and liveness, serializes one receipt at a time under the existing control-byte budget, and retains its storage through the write. Reassembly, queued data, outstanding receipts and IPC buffers share a 32 MiB connection / 128 MiB process allowance (including 64 KiB / 8 MiB reserved for controls). Frames are at most 16 MiB, API envelopes 1 MiB or their smaller operation cap, and partial frames expire after 60 seconds.

A main broker opens each hub host on a separate connection. In-process brokers supply the local service's hub directly. The desktop broker instead sends an open command to the private daemon owner helper with a validated host target, explicit update intent and the expected local `ownerInstance`. The helper checks that instance before SSH preparation. The resulting Ready has the fixed host purpose and the verified **remote** daemon instance; host setup may resolve the supported previous mux version without relaxing direct discovery's exact-version check. Host streams retain remote-only API authority, the shared hub's forwarding ownership and independent client lifetimes. Native errors preserve their code/retry classification; oversized diagnostics omit screen counts as unknown instead of presenting a partial list.

Increasing request IDs belong to one generation. At most 64 requests are pending; cancellation stops reads/registrations, reports uncertainty for an admitted mutation, and keeps the slot until its task actually finishes. Endpoint deadlines come from the operation catalog, not peer arguments. A new generation cannot receive an old result or replay old input. Idle resource admission separately caps process carriers (256), streams (128), simultaneous opens (16), per-backend streams (64), per-trust-domain streams/opens (32/4), and owned transport workers (32). Native quota objects come from trusted assignment; parent-owned popouts share their parent's domain. Worker capacity is released only after local exit is observed, never on a kill request alone. These are transport-resource limits, not limits on owner-created PTYs, application state or jobs.

The reader thread holds its own descriptor of the PTY master, so `close` hangs up the terminal explicitly (SIGHUP to the shell and the foreground process group) and the thread ends at the EOF that follows. A stopping embedded server closes all sessions this way after its server has stopped; otherwise a session still held by a client connection would keep its shell, and with it the process, alive. A stopping daemon releases its sessions instead (`Backend::release_all`): the keepers keep the programs running for the next daemon.

## Server-derived Pane State

The service owns an observer (`service::pane_state`) for all mux sessions, including screens with no attached client. It samples process and OSC information every half second and dispatches trusted `UpdateSessionMetadata` facts through the pure reducer. The session ID locates the current screen, so pane moves do not leave stale titles or close the former screen's pane. Session snapshots carry `displayTitle` and `busy`; ordinary pane titles follow the display title, while managed job labels and lifecycle records remain owned by the job runtime. Busy describes a known foreground process other than the original shell, and clears at exit. When a session's busy state changes to not busy, the observer tells the session, which ends its `working`, `blocked` and `idle` program status records: an agent killed without clearing its record leaves no block behind, also in a shell that marks no prompts. `TitleInfo::display_title` applies one OSC/process/shell/home-directory display policy to snapshots and per-connection title events, removes control and bidirectional formatting characters and bounds the title to 500 characters. CLI listing cells also replace controls in cwd and other fields before printing them.

Ordinary mux operations and observer updates share the effect runner's publication gate: dispatch, effects and spawn announcements complete before metadata can nudge a client. The observer applies the current host's `close_on_exit` setting, using normal close effects, as soon as a session it saw running has exited and its output has ended (at EOF all of its output is recorded): a task per session wakes it at the exit and at the end of output, so this does not wait for the next half-second sample. While a process that outlived the program holds the terminal open, and for an exit it did not see happen (one while no daemon ran, which a client attaching to the next daemon still reads), the policy applies after a three-second final-output grace from the first sample of the exit instead (longer than the session stream's two-second drain bound). Managed jobs are excluded. A screen whose last pane closes, by an op or by this policy, is removed in the same reducer step (`remove_screen_without_panes`); a floating, viewer or job pane keeps its screen. After a removal by the observer, clients get a nudge without a screen ID. The observer holds only weak ownership of the service between ticks.

The reducer names omitted new Screen names with the first unused "Screen N", leaving explicit names and retained empty-screen names intact. `CreateFirstScreen` (`create_screen` with `onlyIfEmpty`) refuses creation while the workspace holds a Screen.

The session dispatcher filters complete replay snapshots in place before framing with `pty::replay::filter_in_place`, preserving the snapshot's byte lease and requiring no second scrollback allocation. Its rules match the streaming CLI replay defense, including cancelled and incomplete sequences. The replay/attached/live boundary and the clients' legacy-server protection remain unchanged.

## Session Keepers

In daemon mode every session's PTY and program belong to a keeper process of their own (`splitty_core::keeper`), so a daemon restart, an update or a daemon crash ends no session. The keeper owns the process and its terminal; the daemon is its controller; clients connect to the daemon as before, which keeps the dispatcher, the effect runner and the jobs coordinator unaware of keepers (they see `SessionHandle`s).

- **Start.** The daemon's terminal factory (`keeper::client::Keepers`) runs its own executable as `splitty session-keeper` with `setsid`, a socket pair on standard input and standard error shared with `daemon.log` (`ProcessLauncher`, which also reaps the keepers it started while it runs; after the daemon exits, init takes them). `main` dispatches the subcommand before it builds the Tokio runtime: a keeper is a few blocking threads. The daemon sends the spawn spec over the socket pair, never on the command line; the keeper binds its socket, starts the program with exactly the spec's environment and answers `hello` with the PID, or `spawn_failed`, which tells a program that could not start (the session's error) from a keeper that could not set itself up (the session then runs in the daemon, with a warning in the log). The socket pair stays the daemon's controller connection.
- **Where.** `config::run_dir()` is `$SPLITTY_CONFIG_DIR/run` or `<user data dir>/splitty/run`, not `$XDG_RUNTIME_DIR`, which systemd-logind removes at the last logout while a remote host's daemon keeps running. Keepers sit in its `keepers/` (0700, sockets 0600), named after the first 16 letters and digits of the session id (a socket path is limited to 103 bytes on macOS); both sides check the peer's user id (`keeper::dir`).
- **Protocol** (`keeper::wire`). Frames of a 4-byte big-endian length, a kind byte and a payload, at most 1 MiB: JSON for control frames, raw bytes behind a sequence number (`input`) or an absolute offset (`output`). On every connection the keeper speaks first with `hello`. A controller claims (`claim`, optionally with `takeover` and the offset to resume from) and the keeper answers `claimed` or `busy`; a takeover tells the previous controller `taken_over`. The keeper reports `input_result` per write (blocking for ordinary sessions, failing at once on a full PTY for managed ones), `foreground` when the terminal's foreground process group changes (it checks twice a second; the session layer used `tcgetpgrp` on its own master), `exited` when the program has ended and `output_ended` when the terminal closed: two events, as the session layer has them. Version 1 is meant to last; changes stay additive.
- **The keeper** (`keeper::process`). It reads the PTY into an offset ring of the session's buffer size and queues frames per controller on a writer thread of their own; a controller more than 8 MiB behind is disconnected and resumes at its last offset (`KeptTerminal` reconnects with `takeover` from the offset it has, so its session sees no byte twice). `pty::lifecycle` runs in the keeper unchanged, which is the program's parent and so has the real exit status. Once the program has ended and its terminal closed, the keeper waits for `exit_ack` from a controller that recorded both (or `close`, or seven days without one), then removes its socket and exits.
- **Adoption.** A starting daemon reads the keepers directory (`keeper::client::find`), leaves keepers of another protocol version running, takes the others over before it loads the workspace (`Found::take_over`), applies `daemon::restore_jobs` with the adopted session ids (their jobs keep running), builds its state, registers a session on every adopted keeper (`Manager::adopt`, fed from the keeper's oldest buffered byte, so scrollback and OSC state come back; a keeper the workspace does not know becomes a session without a pane). The output below the `buffer_end` of the keeper's `hello` is what the keeper had buffered before this daemon connected, read again; `KeptTerminal` hands it to `Feed::record_again`, splitting a frame that straddles that offset, at an adoption and at every reconnect: it rebuilds the program status records but answers no probe, and its record changes notify nobody. A session on a keeper whose `hello` carries an exit result has exited from its start, before the keeper's `exited` frame is read. Then the daemon lets the jobs coordinator watch the adopted jobs again (`Jobs::resume_adopted`), and only then respawns, through `restore_effects`, the sessions the backend does not hold.
- **Stop.** `daemon stop` and SIGTERM release the keepers; job grants are revoked (`Jobs::release`), job processes run on. `daemon stop --end-sessions` then takes every keeper over and sends `close`, also without a daemon; `daemon restart` stops and starts the invoking binary at the configured private client endpoint. The daemon's control socket is `daemon.sock` in the run directory (`control::daemon_socket_path`), so an adopted shell's `SPLITTY_SOCK` reaches the next daemon; `daemon.json` says `keepsSessions: true`.
- **Attach without a daemon.** `splitty attach` lists the keepers by their `hello`s (the working directory looked up by PID) and claims one without `takeover`. When a starting daemon takes the keeper over, the command waits for the daemon and continues through its session stream, dropping that stream's replay.

Managed jobs use a direct argv launch in that same PTY layer, bypassing shell profiles, login arguments
and delayed command injection. The process environment/PATH is the server's; an explicit shell in argv
is the only source of shell interpretation. A single lifecycle worker owns process signalling and
reaping. On Unix it observes structured exit/signal status with `waitid(WNOWAIT)` before `waitpid`, so
a managed stop can retain the unreaped launch leader's identity through TERM, the three-second grace
period and KILL escalation of its launch process group. No saved foreground PID is used for managed
stop. Processes that escape that group are not a containment guarantee. Stopped job output remains
available; process completion and output EOF are separate facts. Ordinary close still resolves its
foreground group from an owned terminal descriptor for SIGHUP; owned service shutdown also signals that group during TERM/KILL escalation. Exit events carry the actual code or
signal rather than turning every failure into -1.

Managed output has two bounded rings: raw PTY bytes and incremental sanitized UTF-8 log text, each
using the configured scrollback capacity. The `vte` parser has its `std` feature disabled and an OSC
buffer of zero bytes; sequence payloads never accumulate. C1 controls are normalized without treating
Unicode continuation bytes as controls. Text retains printable Unicode, tabs and normalized newlines
(CRLF becomes one newline), not cursor placement or a reconstructed TUI screen. A cursor includes its
job, fresh output generation, representation and absolute byte position. Reads cap returned bytes,
report overwritten positions and keep UTF-8 boundaries. A long poll watches a bounded change signal
before reading, rather than adding an unbounded output subscription. EOF and the ring snapshot are
read under the capture lock together so a reader cannot miss final bytes by observing a newer EOF.

## Session Scratchpads

Every terminal session carries one plain-text scratchpad (`session-scratchpad`) in its record of the mux model, `SessionEntry::scratchpad`, so it lives exactly as long as the session: it survives the process exiting, moves with its pane to another screen, is persisted with `state.json` and comes back with a kept or respawned session, and ends with `close_pane`. A window replaces it whole with the screen op `set_scratchpad` (at most 64 KiB of UTF-8); the client session handler nudges the screen's other clients, which find the text in their next snapshot. Every accepted save raises `SessionEntry::scratchpad_version`, which persists with the text; `set_scratchpad` with `ifVersion` is a compare-and-set in the reducer (`MuxError::ScratchpadChanged` otherwise), announced as `scratchpad-versions-v1`. The window is the only channel: `POST /api/mux/op` refuses the op, and `GET /api/mux/state` and `/api/mux/screen/{id}` leave the text and its version out (`service::operations::without_scratchpads`), so an agent that reads the workspace through the control socket never sees text the user wrote for later.

The native editor (`splitty-gpui`, `workspace/scratchpad.rs`, `ScratchpadVersion`) tracks edits per connection and sends versioned saves after typing pauses. Snapshots older than a pending save do not overwrite it. Without `scratchpad-versions-v1`, a local edit outlasts snapshots for two seconds. The window keeps editor visibility and height per session. Close confirmation uses scratchpads reported by the server's close assessment.

## Pasted Images

The native client stores pasted images on the pane's own server, then inserts the returned path. The native workspace worker owns the transfer and captures the pane's source and connection generation; disconnect or replacement prevents insertion into another session.

The client sends `save_image` (base64, at most 10 MiB, which stays under the 16 MiB the hub and the bridge carry) on the pane's connection and waits 30 s for `image_saved` or `image_failed` with its request id; a server older than `save_image` may reject the message, which the client reports as needing an update. The answer's path must be absolute and plain text, since it comes from the host. A disconnected connection never queues `save_image`. On the server the dispatcher hands the image to the protocol's paste store, which checks the leading bytes, stores it under a name of its own in `splitty/paste/` of the cache directory (0700, the file 0600, created new) and first removes pastes older than 24 hours and beyond the newest 100. The hub relays both messages like any other.

## File Transfers

Other files take the same way as pasted images, in chunks, and come back the other way on the user's Save.

The native client uses its source connections. The workspace worker owns the transfers of each source (`splitty-client-core`, `native_workspace/files.rs`): pasted items go out one message per answer, an offered file is read only after Save and is written into the download folder under a name that replaces nothing. The GPUI workspace (`workspace/file_transfer.rs`) turns a paste without text, or Ctrl+V with only an image or files on the clipboard, into such a request for the pane's source and shows offers, drops awaiting Send and progress as window notices. It has no destination choice for drops yet. A local pane takes this computer's file paths as they are (`file_route`); with the local server in WSL it gets stored copies instead, except for files of that server's distribution named by their `\\wsl.localhost\<distro>\…` or `\\wsl$\<distro>\…` path, which go in as Linux paths (`wsl_file`), ahead of the stored ones.

**Into a pane.** The client sends files as chunked `upload` messages, one chunk per answer. Clipboard images use `save_image`; other clipboard files use temporary uploads. Drops await explicit Send. No path is inserted after failure or cancellation.

Uploads use base64 chunks of at most 1 MiB, each answered by `upload_progress`, then `upload_saved`, or `upload_failed`. `upload-control-v1` advertises durable destinations and cancellation. `upload_cancel` discards only that connection's unfinished upload and is idempotent. A final chunk completed before cancellation remains saved; uploads and cancellation are never queued across reconnects.

The protocol upload store bounds files to 512 MiB and sanitizes their names. Temporary uploads use new private `upload-<time>-<hex>/` directories in the paste cache, pruning entries older than 24 hours and beyond the newest 100. A first-chunk `dir` selects durable storage: require and canonicalize an existing absolute directory without changing its permissions, stage in a new private child directory, and publish the complete 0600 file using a same-filesystem hard link that cannot overwrite an entry (including a symlink). Remove staging after publication or on error/cancellation/disconnect; durable files have no retention cleanup. Filesystems without hard-link support fail explicitly. No bytes or paths are passed to a shell by the server.

**Out of a pane.** `splitty download <file>` posts the session ID and the file's absolute path to `POST /api/file/offer` over the owner control socket (grant sockets do not route it). For a folder, `cli::download` first packs a zip into a new `download-*` directory of `<cache>/splitty/download/` and offers that; the directory goes when the command ends (on SIGINT, SIGHUP and SIGTERM as well), and one left by a killed command goes with the next folder download after 24 hours. The server and the window see only a file. `client_session::files`, beside `client_session::editor`, opens the file once, and sends `file.offer` to every connection that registered (`file-offer-v1`, `file.register`) and streams that session. The first `file.accept` takes the offer and the others get `file.closed`; the accepting connection reads it with `file.read` at the next offset, answered by `file.chunk` (1 MiB, base64, `eof` on the last), and the HTTP request answers `saved` after the last chunk. Offers expire after 60 s without an answer and 60 s without a read. The native window treats offers as untrusted data and reads chunks only after Save, writing a new file without replacing an existing one.

## Viewer Panes

A viewer pane (`viewer-panes`) is a pane of the role `viewer` with a `viewer` record and no session: `PaneState::viewer` names its document, a file on the server's machine (`source.kind` `file`, an absolute path) or a text the server keeps for it (`source.kind` `text`, a title, the text and, for a text the window renders as Markdown, `format` `markdown`), the terminal it was last opened from (`originPaneId`) and a `revision`. The screen op `open_viewer` adds one next to an anchor pane. Its placement reuses the slot insertion of `split_pane` (directions, beside a pane of the root layout) and `new_tab` (`tab`, and any placement beside a floating pane, since floating entries hold no splits), and it has no effects. Without `newPaneId`, which only the CLI omits, a file that a viewer pane of the screen already shows selects the first such pane, raises its revision and takes the anchor's origin instead of adding another, and a further document of a terminal joins the group of the first viewer opened from it as a tab. With `replace`, which only `splitty review --text` sends, a text goes to the first viewer pane of the screen in layout order that shows a text of the same title and origin: the pane takes the text, raises its revision and becomes its slot's active pane, keeping its annotations; without such a pane the op adds one. The reducer checks file paths with the same pure rule as the read and the editor request (`mux::viewer_path_is_valid`), so no viewer pane names a path that cannot be read. A window always sends `newPaneId`, so its optimistic panel and the server's pane agree. Splits and tabs from a viewer pane start a terminal in its directory, through the existing fallback of `create_terminal_pane` to the source pane's `cwd`. The client session handler nudges the other clients after `open_viewer` and after a `close_pane` that ended no session, since neither has effects that would.

The model never holds a file's content. A `text` document's text sits in the pane record and so in `state.json`, but one helper strips it from every `mux.screen_snapshot` and from the control API's workspace and screen answers, as `without_scratchpads` does for scratchpads. Snapshots go to every other client after each nudge, and scratchpad saves nudge while the user types. Windows read a document over the connection of the screen that holds the pane (`client_session::viewer`): `viewer.read` at offset 0, then at the next offset with the first answer's `version`, answered by `viewer.chunk` (1 MiB, base64), `viewer.unchanged` (offset 0 with a matching `ifVersion`) or `viewer.failed` with a code. The server keeps nothing between chunks. It checks the path again, opens the file without blocking on a FIFO, takes the version from the opened file's metadata, and fails a later chunk with `changed` when the file was replaced in between. Reads run off the connection's message loop, at most four per connection. `viewer.read` is in `SESSION_OPERATIONS`, so native carriers, the hub and the relays pass it. The control API can open viewer panes (`splitty open <file>` posts `open_viewer` without `newPaneId` and prints the `paneId` the answer names) but cannot read a document. Grant sockets route neither.

The native viewer (`splitty-gpui`, `workspace/viewer.rs`) reads through the source worker and renders text and layout without scripts or network loads. Shown file panes poll every two seconds with `ifVersion`; a newer revision triggers a read. Viewer panes never start a terminal. Document links have no launch authority.

### Annotations

A viewer pane's record holds the annotations a window made on its document (`viewer-annotations`), in creation order: an ID the window generates, a number the server gives from the record's counter (`lastAnnotationNumber`, so a pane never gives a number twice), a kind (`comment`, or `change`, whose empty replacement suggests a deletion), the remark, the replacement, a target (the view, the quote with up to 32 characters of context on each side, and the source lines when it was made; a general comment has none), a status (`resolved`, else open), the mark `sent` once the user handed it over, and a thread of at most 50 replies by the user or an agent. A window writes all of it but the numbers and the agents' replies, with five screen ops: `put_annotations` (adds or replaces by ID, keeping the place, the number and the thread of a replaced one), `remove_annotations` (by ID, with their threads), `put_replies` (the user's replies in one thread), `remove_replies` (of either author) and `mark_sent` (the annotations and the user's replies a review held, each with the content it held: the reducer marks an entry only while it still reads so, field by field, so an edit another window saved meanwhile stays pending and the op never overwrites one); the removals and `mark_sent` ignore IDs the pane does not hold. No put marks an entry: `put_annotations` and `put_replies` keep `sent` only for an entry that reads as before and says so, and a new entry starts unmarked. They have no effects and bump only the screen revision, so they never bring a viewer to the front. The validator checks the kinds' fields, the ID forms, and the limits per field, per op, per thread and per pane (100 annotations, 256 KiB of text with the replies and their titles), so an op that breaks one changes nothing. The client session handler nudges the other clients after each and answers a rejected one, also one that does not parse, with `mux.op_failed` carrying the viewer pane. The kinds `question` and `delete` of earlier builds are read, from `state.json` and from a window, as a comment and as a change with an empty replacement, and never written. Annotations move and end with their pane like a scratchpad with its session, persist in `state.json`, and survive the reuse of the pane by `open_viewer`, `replace` included. The control API leaves them, the counter and `reviewWait` out of the workspace and screen answers (`service::operations::without_annotations`, beside `without_scratchpads`) and refuses the five ops.

Agents reach the threads through `service::review` alone, on the owner control socket and through equivalent native operations and never on a grant socket. `GET /api/viewer/comments` lists, for the viewer panes of the caller's screen (its `screenId` and `paneId`, following a pane that moved) in layout order, the annotations the user handed over, open ones unless `all`, each with the agents' replies and the user's handed-over ones; `POST /api/viewer/comments/reply` adds an agent's reply to a handed-over annotation by its number, without reopening a resolved one. A `document` names a viewer pane by its ID or by the absolute path of the file it shows; a path that several panes show is refused as ambiguous. A reply's `from` is the caller's pane with the title the server knows for it: the pane's title, else its session's process title, else its OSC title, else `agent`, without control and invisible formatting characters and at most 256 bytes. `POST /api/viewer/review/wait` holds its request until the user's verdict on a viewer pane, without a deadline. `client_session::reviews` keeps the waits in memory, at most 16 and one per viewer pane, each with a one-shot channel for its outcome. The handler registers one, names it in the record (`reviewWait`, with the caller's pane and title), refused with 409 `changed` when the request names a `viewerRevision` the pane no longer shows (the `open_viewer` answer carries it, so two commands started at once cannot take each other's answer), nudges every client, and waits for the outcome or for the store's change channel to show the pane or its wait gone (410 `closed`). A guard ends the wait when the handler returns or its future is dropped, as a closed HTTP connection or a cancelled native call (`viewer.reviewWait`, cancellation class `registration`) drops it: it leaves the registry first, so that no later answer is accepted, then clears the record and nudges. A window answers with `viewer.review_answer`, a session operation that native carriers, the hub and the relays pass; the client session handler settles the wait with the first well-formed answer that names it and its pane, and acknowledges every answer to its sender alone with `viewer.review_answered`. An agent's reply and the start and the end of a wait are trusted operations (`Op::AgentReply`, `Op::StartReviewWait`, `Op::EndReviewWait`): variants of `Op` that `ScreenOp`, which clients send, does not hold, so neither a client connection nor `POST /api/mux/op` can submit them. They run through the store's dispatch like any op, so the reply persists and the screen revision rises, and `service::review` nudges every client after them. `state.json` never holds a wait: saving strips it (`daemon::persistence`), and the record never reads one back. On the CLI's side, `cli::comments` (`splitty comments`, `splitty comments reply`) prints the listing as Markdown in the review's form, with control, bidirectional and invisible formatting characters replaced and a reply's further lines indented, `--json` with them escaped, and `cli::review` (`splitty review`) opens the document beside the caller with `open_viewer`, a text with `replace`, and then holds the wait on the revision it showed.

The server anchors the annotations and writes the review (`viewer-reviews-v1`). `splitty_core::mux::anchoring` decodes a document's bytes into its source text (UTF-8 with U+FFFD for invalid sequences, a leading byte order mark dropped, CRLF and lone CR as LF) and anchors an annotation by its quote and context in it, measured in characters: the exact quote among at most 100,000 occurrences, best context first, then the nearest line; then the whitespace-collapsed quote with collapsed context; then, where the quote is gone, the most similar span within 50 lines and 65,536 characters of the recorded start (Dice coefficient of character trigrams of at least 0.6, spans of half to twice the quote's length that start and end at word boundaries or at any character of a script without spaces, quotes of at most 4,000 characters, a budget of 1,000,000 span ends per annotation and 4,000,000 per document version). Ranges count UTF-8 bytes of that text. `splitty_core::mux::review` orders the pane's list and writes the review: the heading and document line, the verdict and the note on a document that changed during a wait, the general comments, a section per passage with its number, lines, kind and state, the remark, the replacement in a fence it cannot close, the replies under their authors, and the reply line naming the viewer pane, with control, bidirectional and invisible formatting characters replaced; it holds the pending annotations, or every open one when none is (none for an approval), and names the user's pending entries it holds as `mark_sent` takes them. `client_session::viewer` answers `viewer.anchor` with `viewer.anchored` and `viewer.review` with `viewer.reviewed` beside `viewer.read`: it takes the pane's record from the store when the message is processed, so the connection's earlier ops are in it, reads the whole document off the loop within the same four reads in flight, and answers the sender alone, or `viewer.failed`. A window's `passages` place annotations taken from its rendered view, which only the window can anchor; the server ignores passages of annotations it does not hold or that target the source.

The native annotation implementation is described under Native functions added after the remote slice. It uses the server's anchors and review text, scoped to the document version and source connection. An accepted `viewer.review_answered` permits marking the entries handed over; a changed entry stays pending.

## Server Modes

### Managed-job restore

The workspace's `jobs` map stores direct launch metadata and terminal outcomes, while job panes and
sessions carry a job ID and sessions explicitly use `restartPolicy: never`. Daemon restore marks
unfinished jobs `interrupted` unless the daemon adopted their keeper (then they keep running, under
trusted administration), keeps recorded terminal results, and never produces spawn effects for a job
(even if its stored session status is stale). Legacy states without jobs retain ordinary shell
restoration. Any surviving job reference or never-restart marker prevents a damaged record from
turning into an interactive shell. Client restarts are different: the running server keeps the job.
Output and live agent authority are not persisted. Before downgrading to a binary without job support,
stop managed processes and back up/reset the newer workspace; an older server cannot interpret these
panes safely.

Persistence is a server-mode property, not per-screen:

| Mode | `serverMode` | Behavior |
|------|-------------|----------|
| **Embedded** | `"embedded"` | The app owns the backend. Window clients can detach without ending it. The native app embeds it in-process on macOS/Linux; on Windows it runs it in WSL as an owned `desktop-backend --owner` child over native pipes. |
| **Daemon** | `"daemon"` | Server runs as background process. State persisted to `state.json` after every dispatch. Sessions run in keepers, which outlive the daemon and which the next daemon adopts; only sessions whose keeper is gone are respawned |

The desktop app always owns its local backend. On macOS/Linux, the native app constructs the shared service runtime in-process (`splitty-client-core`, `embedded`) with local terminals and no keeper directory; startup does not inspect daemon discovery or keeper sockets. Shutdown closes this runtime's client connections and waits for its jobs and terminals to end. A separately started daemon and its kept sessions are independent. The window connects over the same framed pipe and handshake a remote client uses (`connection`, `api_stream`); startup creates no terminal-service TCP listener.

**`PATH`.** Started from the Dock, the Finder or a Linux desktop launcher, the native app has the launcher's `PATH` (macOS: launchd's `/usr/bin:/bin:/usr/sbin:/sbin`), not the one the user's shell startup builds, and so has everything it starts without a shell: `ssh` and the `ProxyCommand` of the user's SSH configuration (such as `gcloud` from `/opt/homebrew/bin`), and the embedded server's managed jobs. Terminals are not affected; their login shells set their own. The app does not adopt the login shell's `PATH` (a finding in `docs/issues/openspec-baseline-findings.md`). A daemon has the `PATH` of whatever started it.

The hub records the hosts the native app opens as recent hosts in its service's configuration directory, shared with the CLI; on Windows that is the WSL backend's.

`splitty daemon start` takes the private instance lock before re-executing its binary as a detached `daemon run` (new session, logs in `daemon.log`). The child inherits only the lock descriptor, validates its exact configured inode, restores close-on-exec and binds `client.sock` before spawning keepers or shells. The parent retains its lock copy through readiness; closing it cannot unlock the child's copy. Format-2 `daemon.json` contains `clientSocket`, `instance`, `transportVersion`, PID, the optional owner-control `socket`, start time, version, mux protocol and `keepsSessions`, without a port or credential. After the child's readiness line, the launcher verifies native negotiation against that record, then prints `SPLITTY_CLIENT_SOCKET` and `SPLITTY_INSTANCE`. Failed startup cleans up only resources whose termination/identity is verified. Lost or damaged discovery cannot bypass the live instance lock.

`native::endpoint` checks every directory ancestor, refuses symlinks and unsupported mounts, requires an owner-only endpoint directory and 0600 socket/lock, and verifies peer UID in both directions (Linux `SO_PEERCRED`, macOS `getpeereid`; unavailable credentials fail closed). Stale removal requires a refused connection to an unchanged owned socket while holding the lock. Cleanup compares device/inode, so it cannot unlink a replacement. The daemon's accept loop acquires carrier/opening quotas before a connection task and stops acceptance before draining active clients. Its lock remains held through final workspace persistence, keeper release and control cleanup.

Native endpoint operations and daemon discovery/log access share `config::PrivateDir`: an `openat` no-follow directory walk pins the owned `0700` directory, and file opens, metadata checks, publication and cleanup stay relative to its descriptor. Owned `0775` ancestors are accepted unchanged; non-sticky world-writable ancestors remain refused. Linux binds/connects through `/proc/self/fd/<dirfd>/<name>`. macOS uses `pthread_fchdir_np` on a dedicated short-lived thread, never the process cwd or an executor thread. The launcher holds the instance lock across startup and passes only that lock descriptor. The daemon validates its exact configured inode, binds its own socket through the pinned directory and reports readiness over the existing stdout pipe. No inherited listener or startup probe consumes application connections. Conditional discovery cleanup rechecks the expected record inside the same retained directory. A replaced configured path fails verification, while cleanup can only reach its original directory. Other group members can still disrupt path availability; this does not change keeper paths or the SSH transport.

Authorized daemon start/restart and `remote-handshake --ensure-daemon` prepare the config directory before discovery. An existing owned final config directory such as `0755` is tightened to `0700` through `fchmod` on its verified descriptor; symlinks, foreign owners and group/world-writable final directories remain refused. Shared ancestor permissions stay unchanged. Ordinary native endpoint opens and read-only discovery/probes retain strict validation and never perform this repair.

`desktop-backend --transport native-v1 --backend embedded` is an owned child-pipe adapter. It negotiates framing before constructing the service, never discovers or adopts a local daemon or keeper, and sends only native frames on stdout. EOF, broken output or launcher death ends its sessions/jobs while an independent daemon keeps running. The service runtime exposes an optional owner-control socket with a narrow route set. Transport closure starts cleanup independently of pending API input. Shutdown seals PTY creation, waits for admitted spawns, and terminates both active sessions and children still exiting after a tab was closed. The lifecycle owner sends TERM, escalates to KILL after 500 ms, and confirms the child exit before the helper exits. Ordinary local input uses nonblocking ordered writes with a deadline, so a program that never reads cannot hold the executor or shutdown hostage. `splitty attach` uses the same verified native connector and retains delivery credit while consuming output.

The embedded `--owner` carrier retains one runtime across independent connections and fixed host streams through its Hub; the native Windows app starts its WSL backend this way (`splitty-client-core`, `owned_service`). Native framing carries bounded 64 KiB chunks with per-stream consumption waits; cancellation uses reserved controls and bounded cancellation-before-open tombstones. Invalid host opens affect only their stream. Inner service EOF drains final bytes before stream closure, while app-owner EOF shuts down the runtime.

Native client completion also joins its owned embedded service task. Client and child-bridge completion runs in separate cleanup tasks so abandoning a completion wait cannot release carrier/stream slots before cleanup. An unfinished embedded negotiation keeps its caller-side opening with the service task, including when its caller is abandoned. Native host relays keep an acquired bridge through failed final progress, rejected Ready, prelude errors and the initial-welcome timeout, and await its cleanup before returning.

SSH setup receives a native owner context for each attempt. It checks revocation before spawning, shares that owner's byte budget, and kills and waits for its setup command on cancellation. The native preparation loop signals cancellation and await setup completion instead of dropping the opening future. A backend peer has a child context: it shares the parent's budget and synchronously rejects new or queued work after parent revocation, but a host protocol failure does not revoke the owner before it can report that failure. A retained watcher wakes blocked IO and is joined during cleanup; pending receives and every resumed write also check the parent directly so watcher scheduling cannot admit stale input.

On Windows the native app reaches its WSL backend through `splitty-remote`'s `owner::Owner` over `wsl.exe` pipes. The owner client shares one embedded helper across independent streams; each inner stream has separate framing, credit and cancellation. App EOF closes the owner and its local jobs; on exit the app closes input, waits up to 3 seconds and then terminates only its bridge, never the distribution.

The memory contract covers application-owned transport buffers, including retained native output. Generic framework replies, unrelated engine storage and total process RSS are excluded. Application transport limits and delivery ownership remain enforced; they do not claim framework-wide memory bounds.

### Listener and authority inventory

| Surface | Source and authority | Lifetime |
|---|---|---|
| Native app local service | `splitty-client-core` `embedded` constructs the Unix service in-process; Windows uses `owned_service` and one owned `desktop-backend --owner` pipe into WSL | App-owned; no TCP bind |
| Daemon client endpoint | `native::endpoint`, private Unix listener, verified owner/instance | Explicit daemon; no TCP bind |
| Owner control socket | `control::router`, private Unix HTTP framing and peer UID check; existing control, job and grant-administration routes | Service-owned; no TCP bind |
| Agent grant socket | `control::jobs`, private per-grant Unix listener with job-only authority and peer UID check | Revocation ends access, not the job |
| Keeper socket | `splitty-core::keeper`, private Unix listener with peer UID checks | Owns one daemon session across daemon replacement |
| Forwarded development service | `tunnel::PortTunnel`, foreground owned SSH `-L 127.0.0.1:…:localhost:…` | Deliberate forward or opted-in saved rule; raw application traffic |
| Reverse share relay | `host::relay` binds host loopback IPv4/IPv6 behind its private Unix SSH relay | Deliberate share, host UID checks and heartbeat cleanup |

## Remote Hosts

The local service's hub reaches hosts through system `ssh` (`splitty-remote`), using WSL's SSH client on Windows. Every remote command is a `CommandLine` of constant words and values passed through `quote`, sent as one argument after `--`. Probe and install scripts are constant single lines run as `sh -c '<script>' <name> <quoted values>`, so the host login shell parses only that outer line.

Before connecting, the client probes the host once (kernel, architecture, `$HOME`, the marker of its own version) and, when its version is missing and it has a payload for the host's kernel (Linux or Darwin) and architecture, installs its server:

```
~/.local/share/splitty/remote/<version>/
  splitty              static server, run by its absolute path from then on
  shell-integration/   next to the binary, where the server looks for it
  .payload-sha256      checksum of the archive it came from
```

The archive goes over the connection's stdin into a `mktemp -d` directory (umask 077); `sha256sum -c` (or `shasum -a 256 -c` where that is missing, exit 5 without either) checks it before `tar` unpacks it, `splitty --version` must run, and only then is it moved to `<version>/`. Afterwards every other version directory whose binary no process runs is removed (the running executables from `/proc/*/exe`, or from `ps -axo comm=` on a host without `/proc`), so a daemon of an older version keeps running until the user restarts it. Payloads come, in this order, from `SPLITTY_REMOTE_PAYLOAD` (development), from archives built into the client (`crates/splitty-remote/build.rs` embeds the archives named by `SPLITTY_REMOTE_PAYLOAD_X86_64` and `SPLITTY_REMOTE_PAYLOAD_AARCH64` for Linux and `SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64` and `SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64` for macOS at build time, keyed by kernel and architecture, with their checksums), or from the client itself when it is a static Linux build of a Linux host's architecture; without one, `splitty` from the host's `PATH` is used. The release builds the static aarch64 server and the two macOS servers first and builds them into the static x86_64 server (the WSL backend), which thereby installs on every supported host; `packaging/windows/build-wsl-payload.sh <repo> <out> [commit] [x86_64|aarch64|darwin-aarch64|darwin-x86_64]` builds each, aarch64 cross-linked with `rust-lld`, the macOS ones with `cargo zigbuild` and a zig pinned by checksum (`packaging/zig-toolchain.sh`, no Apple SDK; zig's linker ad-hoc signs the arm64 binary, as macOS requires). On a Mac the same script links the x86_64 Linux server with `rust-lld` too and the macOS ones with Xcode's linker; the release builds the Linux and macOS native apps with all four, and `packaging/windows/build.ps1` builds them into the local Windows build's WSL backend.

On a macOS host, installed commands use `env SPLITTY_CONFIG_DIR=<home>/.config/splitty <server> …`; the daemon, workspace and private sockets remain separate from the platform's usual `~/Library/Application Support/splitty` directory. The daemon and its panes inherit that configuration selection. Linux hosts use the normal configuration directory. Listening-port discovery uses `/proc`, so macOS reports no discovered ports.

The hub uses the shared installation, native setup and SSH supervision primitives. Format-2 `remote-handshake --json --ensure-daemon` reports the answering binary's version/path/native capabilities separately from the running daemon's version/protocol/instance/keeper status and screens. It contains no port hint. `NativeBridge` then verifies transport v1 and the expected daemon instance through SSH exec pipes to `remote-bridge`. Stale discovery is refreshed within the setup deadline. An automatic update is limited to an older, keeper-backed, verifiable daemon with the matching replacement installed; `daemon stop --if-instance` is accepted only by that immutable instance's owner socket. A newer, unverifiable or non-kept daemon requires explicit migration. No PID signal substitutes for the conditional update.

Legacy recovery is a separate confirmed action. Handshake discovery can report a bounded `legacyMigration` offer or blocked reason. A renderer sees only availability, versions and a keeper-session count; the conditional proof and selected binary stay native-owned. The installed host binary's `remote-migrate --expected` command rechecks the proof, private owner socket, live session/keeper coverage and actual process generation. On Linux it pins a pidfd before owner verification and signals through that handle. Bounded descriptor inspection refuses daemon-owned PTY masters, including unmapped fallback terminals. This preflight does not freeze concurrent clients; the preservation promise covers checked keeper-backed sessions. Unsupported platforms or identity checks refuse without a PID-only signal or TCP fallback. An ambiguous result never replays the migration. The hub consumes `update:true` once.

The native client owns host reconnects. Each new hub host stream repeats install and discovery/setup, reinstalling a missing server or starting a missing daemon. Permanent host-key, authentication and protocol failures require action; observed framing errors take precedence over a generic child exit 255. Dropped bridge workers are terminated and reaped while the host daemon and its sessions continue. Forward and reverse-share supervisors own their separate retry loops and port reservations.

The hosts a user knows (`splitty_remote::hosts`) are the recent hosts and the configured ones. A ready hub host stream records its target in `recent-hosts.json` in the configuration directory (`{"version":1,"hosts":[{"target","lastConnected"}]}`, most recent first, at most 20, written to a fresh 0600 file and renamed over the old one; a file that does not parse counts as empty). The configured hosts are the concrete `Host` names of `~/.ssh/config` and the files its `Include`s name (`~`, paths relative to `~/.ssh`, `*` and `?`), read and never written; patterns, negations and `Match` are skipped. The hidden `splitty remote-hosts --json` prints both, recent first, for scripts and launchers. Host display aliases and hidden markers live in private atomic `remote-host-preferences.json`, keyed by the unchanged SSH target; the order in which the user arranged the host groups lives in `remote-host-order.json` beside it, a file of its own because the preference reader rejects unknown fields and an older Splitty would otherwise read every alias as gone. Aliased targets remain discoverable beyond recent-history eviction. Metadata mutations share an OS file lock across hub and desktop processes. Explicit desktop connections reveal targets; automatic hub reconnects only record visible hosts.

`splitty remote-bridge --transport native-v1` (module `host`) carries the host daemon over SSH stdin/stdout. After the framing handshake it opens the host-local private endpoint with peer-user, path and instance checks, then reports that verified service instance in Ready. Each hop has one ordered writer, bounded queues and its own delivery credits; the bridge holds incoming byte charges until forwarding has acquired its charge. It permits the remote UI operation catalog and rejects local hub/forwarding authority. EOF and pipe failure detach this client; host sessions and the daemon keep running. Invalid framing remains an error through carrier teardown, and stdout contains native frames only.

The exec client's `splitty_remote::bridge::NativeBridge` acquires trusted carrier/opening capacity before spawning its process, verifies Ready against the expected daemon instance, and exposes the shared charged native frames. Closing it finishes its carrier and waits for the owned worker. SSH setup commands share the process-wide 32-worker ceiling; capture is limited to 1 MiB stdout and an 8 KiB stderr tail. Commands without an upload have a two-minute completion deadline. An archive upload has an eight-minute ceiling and a sixty-second write-idle deadline, refreshed only when the owned SSH stdin writer accepts bytes; after stdin finishes, command completion has two minutes. Native host preparation has a fixed ten-minute ceiling that progress reports cannot renew. Framing and service negotiation retain their five-second deadlines. Error and cancellation paths terminate the owned worker and retain its worker slot until exit is observed. A lingering stderr reader is cancelled after its bounded drain wait.

## Remote Screens

The local server is a transport hub for screens of other hosts (`splitty_server::hub`). Desktop clients open a separate native hub stream with a fixed host target. For each connection the hub prepares the installation, then runs format-2 discovery with `--ensure-daemon`. The protocol check accepts the daemon's protocol if it is the hub's `PROTOCOL_VERSION` or the one before; otherwise the hub answers `incompatible` and leaves the daemon running. Then it starts `remote-bridge` on the host, by the installed server's path, and waits for the daemon's `mux.welcome` through it. Meanwhile it reports each step (`hub::Step`, whose names are spelled in one place) as the native stream's transport progress, repeated as a pulse while preparation runs. After acceptance it sends one `remote.ready` (the daemon's version and protocol, whether it started it, the host's screens) and records the target as a recent host. From then on it validates the remote-role client operations and relays their application payloads without merging IDs: ids stay apart because the client holds one connection per host, not because the hub translates them. When the bridge ends, the hub ends the client connection; when the client disconnects, it closes the bridge's input and terminates and reaps its owned `ssh` after a short grace. A failure before the relay is one `remote.error` with a code, a message and, where there is one, a hint (`hub::failure` maps the SSH, install and handshake errors).

Every hub connection has `ssh` processes of its own, opened with `Ssh::stdio_only` (`-T -a -x -o ClearAllForwardings=yes` on top of the hardening options), so the user's configuration adds no terminal and no agent, X11 or port forwards; two windows on one host hold two bridges. Nothing but the commands of the install, the handshake, the update's `daemon stop`, the bridge, the client's messages and, for a shared port, `remote-relay prepare` and `remote-relay serve` reaches the host. `remote.directory.get` includes the known hosts of the user the server runs as. The `Connector` trait separates the flow from `ssh`, so the hub's messages, error codes and relay are tested in-process against a scripted host whose bridge is a local `sh`.

With a confirmed native `update` intent, the hub requires keeper preservation, lower SemVer precedence, verified native instance identity and a matching installed replacement. Its conditional stop goes through the checked daemon's owner socket, followed by fresh discovery; a failed identity comparison cannot stop a replacement. Unsafe updates return an explicit incompatibility result. The hub's update plan in `remote.ready` and `incompatible` errors (`failure::UpdateAvailability`) is `available`, `keepsSessions` and, when available, `toVersion` (the hub's own version): `update_plan` offers an ordinary update only for a daemon that keeps its sessions, so no window counts or names screens that would end. The hub also keeps the client's remembered remote screens (`hub::remembered`): `GET` and `PUT /api/remote/screens` read and replace `remote-screens.json` in the configuration directory (`{"version":1,"screens":[{"target","screenId","name"?,"position"?,"closed"?}]}`, at most 200, each target valid and each (target, screen id) once, written like `recent-hosts.json`), apart from the workspace persistence, so it works with an embedded server too. Windows write it per host with `PUT /api/remote/screens/host` (`remoteScreens.updateHost`, body `splitty_core::native_api::remembered::HostScreens`): `replace_host` swaps that host's entries in the place of its first one under the hub's `remembering` lock, an empty list forgets it, `closed` marks each entry; the whole-list `PUT` stays for older clients. Both are announced as `hub-host-state-v1` on local main connections, together with observations without pane counts (`hub::catalog`, below).

Every daemon reports the TCP ports processes of its sessions listen on, which a client can forward. On Linux, `splitty_core::pty::ports` reads `/proc/net/tcp` and `tcp6` for the sockets in state LISTEN on loopback or wildcard addresses, the parent pid in every `/proc/<pid>/stat` for the process tree of each session's shell, and the `socket:[<inode>]` links in `/proc/<pid>/fd` of those processes; a port held in two sessions counts for the lower session id. `client_session::ports` runs the scan on a blocking thread every 2 seconds while a client is connected and sends `ports.changed` with the full list right after each `mux.welcome` (also when it is empty) and to every client when the list changes; other systems send nothing. Through the hub it is one of the host daemon's relayed messages, which the hub reads only for automatic forwarding (below).

The hub forwards discovered or manually entered ports (`hub::forwards`,
`/api/remote/forwards`, excluded from the control socket). Each uses an owned
foreground `ssh -N -T -a -x` (`Ssh::forward_only`, `tunnel::PortTunnel`) with
the hardening options and `-L <bind>:<local>:localhost:<port>`. The binding
defaults to `127.0.0.1`. The hub reserves the selected address and also checks
`::1` for IPv4 loopback and wildcard bindings. The checks cover alternate
ports and repeat before SSH handoff and after verification, including rebuilds.
An unavailable IPv6 stack permits IPv4. Occupancy, permission and uncertain
errors fail closed.

`PortTunnel` verifies ownership before announcing the confirmed local binding and port. OpenSSH runs local forwarding setup before `LocalCommand`; `ExitOnForwardFailure` prevents that command when binding fails. A fixed `LocalCommand=echo SPLITTY_FORWARD_READY_V1` produces a bounded stdout marker (4 KiB, LF/CRLF), followed by the TCP/liveness check. No target, label or other input is interpolated into executable text. `ControlMaster=no`, `ControlPath=none`, `ControlPersist=no` and `ForkAfterAuthentication=no` prevent reuse or backgrounding from bypassing this proof or losing process ownership. Configuration lookup and readiness each time out after 15 seconds. An authenticating process plus somebody else's listener cannot count as ready. `ssh -G` still rejects configured extra forwards because `ClearAllForwardings` would also clear the requested `-L`. Service HTTP, HTTPS and WebSocket traffic remains raw TCP with no added Splitty credentials.

Saved preferences are separate from live state. `hub::preferences` validates `remote-forwards.json` in the private config directory, with atomic owner-only replacement through `hosts::write_private`: version 1, canonical target and remote-port keys, local port, label (at most 200 Unicode characters), and enabled state. It bounds saved entries at 32 per host and 256 total, including disabled rules. Missing, unreadable or invalid files read empty without repair. GET/PUT/DELETE `/api/remote/forward-preferences` are guarded metadata operations that neither reserve ports nor contact hosts. PUT merges only supplied fields, including an explicit enabled choice; omitted enabled preserves existing consent (legacy creation defaults to enabled). Native saving defaults restoration off; creating from a live forward captures its actual local port. Runtime `lastError` is joined into GET but never persisted. Active GET keeps its original array of `up`/`down` records.

Only a successful send of `remote.ready` schedules restoration of that host's enabled inactive mappings. Tasks reread current consent, reuse existing openings and never delay the relay or add hub frames. Startup, login, reads and metadata edits remain passive; discovery is unnecessary. An initial failure records `lastError` and waits for explicit action or another successful readiness event. A live supervisor survives screen detachment and Forget, keeps its selected port through loss and rebuild, reports `down` and retries with growing waits up to 10 seconds. Active/opening capacity is separately bounded at 32 per host.

A per-key asynchronous mutation lane covers start, Stop, Forget, metadata merges and cleanup. Short shared locks locate lanes, claim capacity and atomically merge storage; none spans another key's SSH wait. Hub-owned operation tasks retain their owned lane even when a native caller disappears. Stop saves disabled state before closing, and waits for the supervisor, including a bounded in-flight rebuild and child reap, before releasing the lane. A failed durable Stop leaves the live forward alone. Explicit successful POST enables an existing preference and persists an alternate only after verification; a failed commit closes the new tunnel before later mutations proceed. Forget removes only the rule and runtime error, leaving any live tunnel supervised. Automatic POST rechecks disabled consent and refuses with `forward_paused`; no earlier queued restore or cancelled request can undo an acknowledged Stop or Forget. `Connector::forward` and deterministic setup/stop barriers exercise these paths in-process; dropping the hub cancels its operation tasks and supervisors.

Reverse forwards share a port of this machine with a host (`hub::reverse`, `/api/remote/reverse-forwards`, `/api/remote/reverse-forward-preferences`, excluded from the control socket). They reuse the forwards' shape: per-key lanes, an `Opening` slot, a supervisor that rebuilds with the same ports and reports `down`, 32 active per host counted apart, and opt-in rules restored after `remote.ready`, but in `remote-reverse-forwards.json`, since `remote-forwards.json` refuses unknown fields. `Connector::reverse` runs the install's prepare (Darwin is refused: the owner check needs `/proc`), `remote-relay prepare` over a stdio-only `ssh` for the host's private relay directory (`<run dir>/relay`, 0700), then `tunnel::ReverseTunnel`: one owned `ssh -T -a -x` (`Ssh::relay_only`; no `ClearAllForwardings`, so configured forwards are refused as for `PortTunnel`) with `-R <dir>/<16 hex>.sock:localhost:<local>` and the command `remote-relay serve`. `sshd` binds the remote end as a Unix socket, to which `GatewayPorts` does not apply. `host::relay` checks the socket (inside the directory, a socket of its user, made 0600), binds `127.0.0.1` and `::1` on the port, prints `SPLITTY_RELAY_READY_V1` (or `SPLITTY_RELAY_FAILED_V1 <code>`) and relays a connection only when `pty::ports::connection_owner` finds the client socket's uid in `/proc/net/tcp(6)` equal to its own. `ReverseTunnel` reads that report (4 KiB, 20 s) and then writes a heartbeat line every 10 seconds; the relay ends at end of input or after 45 seconds without one and removes its socket, so a lost connection frees the host's port before a rebuild can take it again. Reverse shares use a per-user reservation registry across configuration directories. Live internal-listener claims written by older processes of the same user stay reserved; this version writes no such claims. The reservation exists durably before a forwarding worker can expose the destination and retains process-generation/network-context identity through cleanup. Ambiguous identity or damaged records fail closed; no timeout or bare PID authorizes release; a verified different immutable process birth proves the original worker ended, while a UID change alone does not. Windows and WSL refuse reverse sharing because they lack a common localhost reservation authority. Linux requires the initial PID/user namespaces and a matching root mount view; unsupported namespace views are explicit errors. The registry uses the OS account home, independently of config/XDG overrides, and rejects symlinks, foreign owners and non-sticky group/world-writable ancestors. Its final directory remains 0700 and files 0600 with one link. Native endpoint support for owned 0775 ancestors does not apply to this global reservation authority. The [registry requirements](remote.md#forwarding-registry-requirements) list paths and restrictions. This does not constrain arbitrary forwarding tools run as the same user.

The hub's private `hub::catalog` persists `remote-catalog.json`. Windows submit `hub.catalog.observe` and read the result through `remote.directory.get`. An observation replaces only the named host with a server timestamp; a screen in it without `panes` keeps its stored count, and one without a stored count is left out until an observation gives one. A mutex serializes atomic owner-only writes; at most 20 hosts with 200 screens each survive. This is separate from the remembered open-screen list: reading the catalog never opens a screen or an SSH connection.

The native client retains explicit remote Screen references, reconnects with bounded backoff and marks absent IDs as missing. Detach and window close preserve host sessions; deliberate Terminate sends `close_screen`. Host metadata and the window's connection demand are described below.

The experimental GPUI workspace presents the same backend-owned update
eligibility through the UI-free host offer and confirmation model described
under Client connections. Confirmation is host-scoped: fresh discovery and the
existing instance-bound stop or verified legacy migration decide whether a
replacement remains safe. There is no receipt reserving the originally observed
daemon. An equal-version build lacking a newer workspace capability remains
ineligible under the strict precedence policy and needs an explicit backend
remedy; the native client does not infer eligibility from capability failure.

The native Hosts destination composes these explicit workflows through
`client-core::ports`, described under Client connections. Its retained Kit forms
own input and focus, while the shared client controller owns admitted operations,
observations and browser consent. The native implementation has no popup-tab
reservation or polling fallback. Initial reads, explicit refresh, supported
`hub.changed` notifications and mutation settlement drive passive reconciliation.
Automatic restoration remains separate from Start. The form separates shared host names and catalog removal from local port, binding and restoration settings. Shared edits require a live host connection and its catalogue capability; local-only settings remain editable without that connection.
The hub owns automatic forwarding.

The host daemon owns a separate durable `port-catalog.json`: port numbers and service names shared by all its clients, including manually entered services outside Splitty terminal process trees. `port-catalog-v1` advertises `ports.save`/`ports.remove`; the authenticated source connection receives `ports.result` only after private atomic persistence. `ports.catalog` carries complete revisioned snapshots after welcome and each change; clients reject older revisions within a connection. These events never feed auto-forward discovery. Invalid files are reported without overwriting them. Legacy local labels are editable as an explicit migration source; merely connecting never replaces a shared name.

Local forwarding preferences additionally keep `bind` (default `127.0.0.1`) and `autoLocal` (default false for legacy saved rules). Explicit numeric local ports remain exact. Automatic rules try the remote port and then an available port; a running tunnel always rebuilds at its established port and binding. A deliberate binding to a LAN/VPN IP or wildcard exposes the raw service on that interface; changing an active binding requires Stop. Open/copy addresses derive from the confirmed binding, not an arbitrary URL in a response. Wildcards use loopback for local browser access; other computers use the forwarding computer's reachable address and local port. On WSL, binding occurs in the backend network namespace; host networking/firewall configuration still governs external access.

A port that appears brings no notice; the host's port lists offer it. `remote.autoForwardPorts` stays off by default and belongs to the hub (`hub::auto_forward`, announced as `hub-auto-forward-v1` on local main connections). Every native host stream hands every text frame from the host to a per-relay watch, which reads only a frame that begins `{"type":"ports.changed"` (serde writes a daemon's tag first) of at most 256 KiB, and its first 256 ports. Each relay's first list, and so the first after a reconnect, is a baseline; lists are followed while the setting is off. With the setting on, a port that appeared asks `Forwards::open_automatically`, which skips a port with a forward (opening, up or being rebuilt), a paused saved rule, or a restore or automatic open already scheduled, and otherwise opens it in its key's lane as an automatic request: fixed saved ports exact and opted-in automatic rules allowed a free alternate, nothing newly remembered. What was forwarded is kept per host, with the process, while one of its relays runs, so a second window forwards nothing again and a stopped port is not forwarded again for the same process. The forwards appear in the active list and `hub.changed` like any other; the clients only show them, and against a local service without the capability nothing is forwarded automatically. With no window connected to a host there is no relay and so no discovery. Opening a forward or clicking a link does not implicitly remember it; a link click is explicit consent to resume a paused rule.

A deliberate click on an HTTP(S) link in a host terminal validates or opens that host's forward for loopback addresses, then constructs the URL from the confirmed local binding and port while preserving scheme, path, query and fragment. Saved-port conflicts open no URL. A newer Stop revokes an old continuation. Other valid HTTP(S) links use the system browser through the native external-link policy.

**Editor handoff.** Native terminal menus generate fixed editor folder URIs in `splitty-client-core` (`native_workspace/editor_uri.rs`, checked against `testdata/`) and dispatch through `splitty-gpui` (`workspace/terminal_menu.rs`). Target and cwd come from the captured pane's source and session and are revalidated on click. Generic openers remain HTTP(S)-only (`external_links.rs`). Editor-specific paths and restrictions are documented in [Files and editors](files-and-editors.md). No server command or terminal output authorizes dispatch; observable failures are reported without claiming successful editor connection.

**CLI editor requests.** `splitty open` resolves an existing file or directory on the originating server and posts its session ID, absolute path and optional editor to `/api/editor/open`. The owner control socket and native `editor.open` expose this operation; restricted grant sockets do not. `client_session::editor` selects exactly one registered (`editor-open-v1`, `editor.register`) connection streaming that session, caps admission to one pending request per client / 32 global / one per two seconds, and waits up to 30 seconds for that same connection's `editor.result`. No recipients or several recipients fail explicitly. Disconnect and detach cancel; pending requests are never stored in workspace state or replayed. Subscription membership comes from the core protocol connection, not pane IDs supplied by the request.

The native editor request handler (`native_workspace/editors.rs`) validates source, path, editor, kind and deadline and offers a bounded prompt. Only deliberate Open dispatches a fixed URI. Local requests require build metadata to distinguish WSL paths. Cancellation, session closure and replacement invalidate pending actions. Optional `kind` defaults to `folder`; files omit the trailing slash and reject ambiguous colon syntax. Local editor preferences select defaults and project rules but cannot waive confirmation or be supplied by a remote request. Remote-role connections cannot register for local editor authority.

## Module Map

The module-level map of the Rust crates is the architecture model in [`architecture/`](../architecture/README.md): every crate and top-level module as a component, with its description, its capability under `openspec/specs/`, and the dependencies allowed between components. `cargo test` checks it against the code in both directions (`architecture/check`), so it stays current; read the `.c4` files or query `likec4 export json` as the README shows. `mux::effect_runner` is a component of its own there, so "within the mux, only the effect runner touches the PTY layer" is a checked rule.

The native app (`splitty-gpui`) names only `splitty-client-core`, which links `splitty-server` on macOS and Linux only, since Windows runs the server in WSL, and `splitty-core`, `splitty-remote` and `splitty-transport` on every platform.

The native app enforces clipboard policy in its terminal engine (`crates/splitty-gpui/src/terminal_engine.rs`): only the terminal library's clipboard write callback is installed, so a clipboard query gets no answer. A write is accepted for the standard clipboard, as nonempty UTF-8 of at most 64 KiB, and only while `terminal.allowOsc52Write` of the window's own settings is on; that one setting decides for local and host panes. The server removes clipboard operations from every replay it sends; the native client also marks the output it receives before `attached`, and the engine drops every effect of marked output (replies to the program and clipboard writes), which covers a replay from an older server.

### Hub-owned host metadata and passive updates

The hub builds `/api/remote/directory` from known hosts, display preferences,
saved host order, cached screens and active/saved forwards in both directions.
It stores the handshake's screen list after readiness. Windows report later
screen observations over their ordered local connection; validation and cache
writes stay in the hub. The host relay remains opaque. Legacy catalog PUTs
remain available for older clients.

Forward supervisors and metadata mutations invalidate a coalescing local watch.
The common connection loop subscribes before welcome and sends `hub.changed`;
the native client refreshes passive reads on this message and reconnect. Refreshes never restore
or create forwards. Host update and retry decisions arrive with hub setup
metadata, using the remote supervisor's shared version comparison.

The experimental native Hosts destination consumes the charged client-core
directory state described above. Targets and cached Screen IDs remain identities;
display sanitization does not rewrite them. Cached Screen names, counts and
observation timestamps describe the last observation, never current reachability.
The view owns navigation, a trapped focus scope and virtualized selected-host
catalog rendering. Search and the unopened filter never contact a host. The window
title retains Screen, host and active terminal identity; terminals have no extra
workspace header. Drawer creation belongs to each host group. Screen previews clip
at their group; complete remote blocks use the same drag motion and persist order
through the local host-order metadata API, with the local group fixed above them. Visiting it remains
passive; Connect captures a known exact target and fixed local owner. A deliberate
host stream supplies readiness and a fresh catalog, and Open adds only the chosen
existing Screen to the shared window. A deliberate Open on a cached identity retains
one owner- and view-scoped continuation: connect, confirm that exact ID in the live
catalog, then open it. Cancellation, owner replacement, another selected host or
leaving Hosts revokes that continuation; missing IDs are never recreated.
Browse demand is independent from opened references, so leaving Hosts preserves
opened work while releasing unused host streams. Detach removes a reference;
explicit Terminate uses the common assessed close and an ordered absence fence.
Close host removes every reference of one host in one step (`CloseHost`) and
releases its stream; nothing is ended on the host. The worker keeps the local
service's list of open remote Screens (`remoteScreens.get` and
`remoteScreens.updateHost`, `hub/remembered.rs`) in step with the window: a host with
open Screens is written as exactly those, a detached or ended last Screen forgets
the host, and a closed host or a quit app leaves its entries as they are. Each
write names one host, and the service replaces only its entries, so entries of
other hosts and clients are kept; a service without `hub-host-state-v1` is only
read. The native client does not put remembered
Screens back into the Drawer at start. The Hosts panel shows their count on the
host, and choosing the host connects it and opens those that its live catalog
still has in one command (`OpenRemoteScreens`), under the same owner- and
view-scoped continuation as a single cached Open. A closed host's entries carry
`closed`. Reconnecting at start is chosen per host and kept as the list
`remote.reconnectHosts` in the local settings. The worker reads it once, and
when the remembered list and the host directory are known it connects the listed
hosts whose entries are not closed; each such host holds its own demand, which a closing
panel does not release, until its catalog is known and its remaining Screens
are opened, or until it fails and leaves the window with one notice.
Port rows expose direct local settings. Unknown TCP services offer Forward or Copy
address; Open as HTTP is explicit, while terminal URLs preserve their HTTP/HTTPS
scheme and path through verified browser activation. Native requests preserve the
saved restore choice on one-off opening; saving restoration does not start or stop
a tunnel. Shared remote service names remain a separate protocol/persistence
follow-up; existing mapping labels remain local to the initiating backend.
The local all-catalog/bootstrap policy remains local-only. The native remote
surface remains experimental while its broader lifecycle and interaction
acceptance suite is completed.

### Close assessment

Whether a close ends something is decided once, on the server of the Screen. `mux::assess_close` (`mux/close_assessment.rs`) is a pure read of the workspace state for a `CloseScope`: panes of one Screen, whole Screens, or the whole workspace. Per pane, and per session of a Screen that no pane shows, it reports a terminal whose program runs (busy, running, not a managed job pane) or is still starting, a session whose scratchpad holds text, and a viewer pane's annotations, resolved ones included; the totals and `confirm` follow from the entries. A scope names at most 512 panes or Screens and only existing ones. The mux handler answers the session message `mux.assess_close` with `mux.close_assessed` under the same request ID and announces `close-assessment-v1` in `mux.welcome`; as a session message it goes over the Screen's own connection, so a hub relays it to a host unchanged. The control API has no close assessment.

The native close guard asks the source connection for its close assessment, with a five-second deadline. Without the capability, or after failure or timeout, it asks a generic confirmation, including for idle terminals.

### Authoritative templates

The `apply_template` workspace intent carries a complete saved template, client pane IDs, a root directory and whether to create a screen. `mux::project_template` performs pure validation and projection: terminal references become layout panes, splits normalize their ratios and floating slots keep their bounds. Reading a template (`template::Template`'s deserialization, for files, requests and a host's answers alike) drops unsupported `webview` panes with their slots, collapsing a split left with one child. It requires complete unique client pane IDs and returns no sessions or effects. Authoritative application fills omitted IDs at its dispatch boundary, reuses that projection and checks workspace-wide uniqueness. Replacement keeps the screen name and order and emits kills followed by spawns; each launch carries command, target-host-resolved cwd, profile ID and restore mode. Layout-only replacement also ends the previous terminals. The effect runner owns PTY creation, and `mux.pane_spawned` reports only successful spawns before the other clients' nudge.

Applying onto an existing Screen that holds panes or sessions needs `replaceConfirmed: true` (`MuxError::ReplaceNotConfirmed` otherwise), which the native window sends only after asking from the close assessment of that Screen. The generic control API refuses application; only a window session connection may send this intent. Command approval belongs to that window: the server does not receive or verify a proof of consent. The native client binds approval to the captured displayed template and consumes the decision once. The pure `Template::retain_commands` helper clears both command and restore mode for omitted native commands.


Extraction runs on the target server. The effect runner reads current process titles/directories and actual profile selection, then pure mux conversion prunes viewers, normalizes remaining splits and relativizes directories within an optional root. Empty or viewer-only tiled layouts return an error. Native windows extract with `mux.extract_template` / `mux.template_extracted` on the Screen's own session connection, local Screens and host Screens alike. The native `template.extract` mapping (`POST /api/templates/extract/screen`) shares the same read (`EffectRunner::extract_screen`) and stays, deprecated, for native windows of earlier releases, which used it for local Screens. The template YAML store format is unchanged.

Saving by name is the store's: `template::file_name` derives the file name from the template's name, and `FileStore::save_named` writes a complete temporary file and makes it appear under that name with a hard link, which fails when the file exists (an exclusive create where the file system has no hard links), or replaces it by rename with `overwrite`. `POST /api/templates` and native `template.create` answer with the file name, or 409 with the existing one, and `template-names-v1` announces them; native windows send only the name. A blank or whitespace-only command is no command, both in the template's launch data and in the PTY's startup arguments.

The native pending view uses the shared Rust projection without speculative session routes. Existing-Screen application captures and revalidates the replaced identities, including hidden/floating terminals, shows the Screen's close assessment in the review, and sends one apply operation without a preliminary close or initial terminal. An ordered catalog observation confirms layout separately from launch success: post-commit effect errors cannot prove that the old terminals survived. Unverified outcomes are reported without replay. Native saving retains one reviewed server extraction; overwrite/delete choices identify their captured file target, but the existing file API provides no versioned compare-and-swap guarantee.

## Settings authority

Each server owns the effective home settings in `config.toml`. The same config layer
provides defaults, field limits and atomic PATCH validation to native UI
operations. Clients read `/api/home/settings/metadata` when available and use its
values for controls and reset actions. The zoom levels are the `values` of
`appearance.zoom` there (`config::ZOOM_LEVELS`); native clients step through
them and offer them, and the native window bounds its display by them. A
native window whose local service lists none takes the levels of its own
build's core within that service's bounds. The native client validates an edit
against those limits only; for a server without metadata it falls back to the
built-in ranges and choices (`splitty_client_core::settings::fallback_limits`)
and edits no field such a server may not know (`SettingsField::needs_metadata`). A successful settings write
broadcasts the complete `settings.changed` object to every connected window on
that server; a failed write leaves both persistent and running values alone. Shell
profile selection and fallback run on the server, with the effective profile and
fallback reason in `mux.pane_spawned`. Shell profiles are configured in `config.toml`.

The native Settings editor retains controlled widget state in its own view and
submits typed single-field intents through the workspace's fixed local owner,
including while a remote Screen is shown. The existing auxiliary API stream
owns a bounded FIFO, one active transaction and per-field exclusion shared with
Drawer and zoom commands. Confirmed values come from authoritative reads;
rejected or uncertain mutations are never replayed. Each field identifies its
appearance or notification observation domain. Admission, acknowledgement
validation, required readback generations and deferred delivery use that domain;
an OS appearance refresh cannot settle or supersede a notification edit. A fresh
successful home projection releases only its domain's uncertain fields. Closing
the workspace discards
unsent work and joins owned cancellation.

Notification observation retains seven effective booleans and at most seven
optional service-default booleans after releasing charged home and metadata
replies. Six are required; `programStatus` reads as on from a server that does
not send it, and so does its default when the metadata's notification group
lacks it. These typed values establish compatible explicit edit choices without
appearance or metadata availability. Reset requires an exact published default;
metadata failure preserves effective switches and disables only unavailable
resets. Failed reads retain same-owner last-good evidence, while owner replacement
starts without edit authority. Notification mutation acknowledgements validate
only the notification projection; controls and unread policy receive effective
values from subsequent authoritative reads, never mutation reply values.
The retained Settings view receives both observations even while closed, keeps
completion generations per domain, and presents seven notification switches with
field-specific pending or failure state. Switch and Reset callbacks retain the
rendered local owner; Reset additionally rechecks that its exact default is still
available. No optimistic control value changes the notification ledger.

The local presentation snapshot carries settings and metadata; it names the UI
theme and the terminal palette and resolves neither. The application resolves
both in its theme catalog: UI themes are theme sets in the component toolkit's
JSON format and terminal palettes are files in Ghostty's theme format, bundled
from `crates/splitty-gpui/themes/` by the build script and read from the themes
directory at start and whenever the settings editor opens. Settings store an
identifier derived from the display name, which is also what pairs a UI theme
with the terminal palette of the same name. The toolkit theme goes to the
components as it is. The workspace chrome takes its own colors and sizes from
the theme's optional `splitty` object and derives every absent token from the
toolkit colors, lifting primary text to a readable contrast. The terminal
palette is the one `terminal.theme` names, else the one named for the UI theme's
mode (`terminal.themeDark`, `terminal.themeLight`), else the one with the UI
theme's identifier (ignoring `-`), else the default of that mode. The first two
steps are the server core's rule (`splitty_core::theme::terminal_palette_name`),
which the application applies with its own UI theme's mode. A name the catalog lacks
shows the next choice with a notice and leaves the setting alone. Palette
changes update retained terminal engines without replacing sessions or history.
The service's theme module (`splitty_core::theme`, `theme.get`, `theme.list`) remains in the native catalog. It applies the palette rule to its answered theme variant, returning the chosen terminal colors and `terminalPalette`. The native app resolves its own catalog and does not call these operations. They are candidates for a separate protocol cleanup; their payloads remain compatible with released clients.

### Native functions added after the remote slice

The native window has a command palette and a shortcut list (`workspace/palette.rs`): every command is a `Shortcut` or one of a few palette-only actions, run through the same `run_shortcut` as its key. Viewer panes (`workspace/viewer.rs`) ask the worker for a document with `ReadViewer`; `native_workspace/viewers.rs` reads it over the pane's own connection through `document.rs`, which now carries `ifVersion` and reports `viewer.unchanged` and `viewer.failed`. The window asks only for shown panes: at first, for a newer record, and every two seconds for a file. Terminal images come from the engine's render state (`Frame::images`, `Frame::image_pixels`) and are kept as textures per image generation; only the in-stream medium is enabled. `remote.autoForwardPorts` is the hub's (`hub::auto_forward`); the window only shows its forwards. `hub/shares.rs` is the client of the local service's reverse forwards; its stream is opened on first use. `NotificationCenter::take_fresh` hands newly admitted, unwatched facts to the window for notices and system notifications. The change `native-feature-gaps` lists what is still missing.

Viewer annotations live in two places. `splitty-client-core::annotations` describes a selected passage as a target, turns the server's anchors into passages of the shown text (`Anchor::from_wire`; documents are decoded with the core's `viewer_source_text`, so the server's byte ranges index the window's text directly) and holds the hand-over policy (`review_delivery`). `workspace/viewer/notes.rs` holds the list beside the document, the field and the marks, and sends `Annotate`, `AnchorViewer`, `ReviewViewer` and `AnswerReview` commands; the worker turns them into screen operations, `viewer.anchor`, `viewer.review` and `viewer.review_answer`, and reports `viewer.anchored` and `viewer.reviewed` as `Update::ViewerAnchors` and `Update::ViewerReview`. The window asks for anchors when the version shown or the targets change and lists the annotations in the answer's order; copying, sending and answering ask for the review and continue with it under a token. On a host without `viewer-reviews-v1` the notes say the host needs an update. A snapshot's `reviewWait` is read from its JSON into `ScreenModel::review_wait`, because the typed record never reads a wait back. A viewer pane has no session, so drags and group actions identify it by `identity_for`, the session or the pane itself.

### Native window chrome

The local service independently stores interface style, window decoration,
client title-bar colors and window-button policy. The startup appearance cache
remembers all four. Desktop typography and button arrangement are local inputs,
never theme-file or remote-host authority. System interface styling selects a
built-in desktop profile without rewriting saved theme/font choices.

`appearance/titlebar.rs` resolves frame geometry, title typography and its color
source. `chrome.rs` consumes the resolved values and delegates close intent to
the workspace. macOS/Windows choose native versus custom decoration at startup;
Linux requests the selected decoration and renders a fallback when the compositor
requires client decoration.

The decoration profile owns control metrics, title weight and inactive styling.
The local SVG icon adapter reads the desktop's icon-theme selection, resolves
four window symbols through theme inheritance and installed fallbacks, and
retains validated bytes for the renderer. Desktop setting changes refresh that
cache off the UI thread. Its file reads and inheritance traversal are bounded;
missing icons use the existing toolkit symbols. Artwork is loaded locally, not
copied into the application, and never supplies title-bar color policy.

The component wrapper owns per-window frame/shadow radii, independent of the
application theme's component radii. A pinned WGPU renderer patch clips content
primitives to the complete client silhouette, while preserving external shadows.
The Linux platform supplies scaled frame bounds and radii every draw, including
tiling and maximize transitions. This keeps terminal, settings and overlay
backgrounds inside the same window shape without per-view corner workarounds.
