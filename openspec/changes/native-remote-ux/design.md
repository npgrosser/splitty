# Native remote workspace design

Implementation design — 2026-11-11. Completion is tracked in tasks.md.

## Current findings

| Concern | Current owner / evidence | Consequence |
|---|---|---|
| Detected listeners | Remote server `client_session/ports.rs`, using `splitty-core/src/pty/ports.rs` | Clients of that server receive the same port, process and session facts. Detection currently covers supported Linux session process trees, not every host service. |
| Live outbound forwards | Initiating backend `hub/forwards.rs`, executing `splitty-remote` tunnel workers | The listener is local to that backend. It is independent of a drawer popup or remote screen. |
| Labels, saved local ports, enabled state | Initiating backend's forward preferences | Names currently belong to local mappings; they are not shared remote service names. |
| UI requests and browser intent | `splitty-client-core/src/ports.rs` and native workspace presentation | Actions capture an owner and exact host/mapping. Browser opening requires a verified endpoint. |

“Server crate” is not the same as “remote machine.” A local backend can use server
code while acting as the SSH client. Native currently creates app-owned backends;
different owners are not automatically one device-wide forwarding manager.
The reservation registry prevents conflicting exposures; it does not make
independent owners share one tunnel. Preserve that distinction in wording and
implementation.

## Design direction

### Window identity and terminal space

Keep the existing native window-title projection: Screen @Host · active terminal
· Splitty, with the host alias when available. Do not add the prototype
workspace-head/breadcrumb row. Detach stays in the drawer and Hosts catalog,
so terminal panes retain their full available height.

### Drawer: opened work, grouped by where it runs

This computer and each opened remote host use the same group hierarchy. A plus
beside the group name creates a screen there; its tooltip includes the location.
There is no global local-only New screen action after the remote groups.

Revised 2026-11-11 after use: connecting through the large browser, and a
permanent list of remembered hosts in the drawer, cost more than they gave.
Three panels beside the drawer now carry the frequent paths, and the browser
is the manager behind them.

| Trigger | Panel | Primary result |
|---|---|---|
| Hosts item | Known hosts with a field that filters them and takes a new `user@host[:port]` | Choosing a host connects it and shows its screens in the same panel |
| Host group name | That host's state and screens, open or not | Open attaches, an open screen is shown, New Screen creates |
| Port pill | That host's ports with a field for any other port | Forwards if needed and opens the system browser |

The drawer lists only opened work. A host without open screens is reached from
the Hosts item, which keeps it out of the drawer until something is opened.
Each panel ends with one entry into the manager. A panel connects a host only
on a deliberate choice and gives up that connection when it closes without an
opened screen. Connection state and forward state are independent: a
reconnecting screen connection does not prove that a separate tunnel is down.

The port pill shows an icon and the count of forwarded ports, else of found
ports, and no count when there are none. A forwarded port's row also copies
its local address and stops the forward; the row's own action stays Open.

All panels and the screen, tab, group and host menus use one surface: the
context menu's panel, 12px rows with a 10px detail, and one keyboard model
(arrows move, Enter chooses, Escape and Tab close). A trigger looks pressed
only while its panel is open. A focus ring appears after Tab and goes with the
next pointer input, so a clicked control keeps no highlight.

Screen menus distinguish Remove from this window from End screen on host. The
first preserves processes and forwards; the second identifies and confirms the
remote processes it will end. Avoid a generic Close connection action whose
effects could include unrelated browser sessions or windows. Close host, below,
is scoped to this window.

### Closing a host and coming back

Added 2026-11-11. Between detaching one screen and quitting the app there was
no way to put a host away. Close host takes the whole group out of this window:
its screens keep running, and its connection ends when nothing else in the
window uses it. It sits where the host is, on the group caption while the
pointer is over it, and for the keyboard in the host's panel; the manager has
it on the host's row.

| Action | Window | Host | Remembered for the host |
|---|---|---|---|
| Detach a screen | That screen leaves | Keeps running | The remaining open screens; none after the last |
| Close host | The group leaves | Everything keeps running | The screens that were open |
| Quit the app | Closes | Everything keeps running | The screens that were open |
| End screen on host | That screen leaves | The screen ends | The remaining open screens |

The local service already keeps a list of open remote screens for the browser
client (`remote-screens.json`). The native client uses the same list and writes
one host at a time, so it does not drop what another client stored for other
hosts. By default it does not connect anything at start and does not put
remembered screens into the drawer: an SSH prompt or a timeout before the first
local terminal is usable costs more than one click later. The Hosts panel shows the
count on the host ("3 Screens"), and choosing the host connects it and opens
those that still exist, showing the first. Screens that ended meanwhile are
left out and forgotten. The host's panel and the manager offer the same as
Reopen. Saved forwards follow their own restore rule on that connection.

Reconnecting at start is chosen per host, in the host's panel and in the
manager's host menu (Reconnect at start, off). A global switch was tried first
and dropped: what speaks against connecting unasked, a password prompt, a VPN
or a production host, belongs to the host. With the choice, a start connects
that host if its screens were open when the app was closed and reopens them,
while the local screens are already usable. The screens appear in the drawer
without taking the view from the one in use. A host put away with Close host
stays away: its entries carry `closed` in the list. The Hosts item says
Connecting… while a host comes up without a group yet. A host that does not
come up leaves the window with a notice that stays until dismissed, and stays
offered in the Hosts panel; nothing retries in the background.

The chosen hosts are a list in the local settings (`remote.reconnectHosts`).
The host preferences file would be the closer place, but its reader rejects
unknown fields, so an older Splitty on the same configuration would drop a
host's display name with it. The list is read once per start, so choosing a
host connects nothing in the running window.

### Dragging: two levels, two clipping boundaries

Screens already reorder within their own host; preserve that interaction.
The dragged screen's painting boundary must be that group's screen-list region,
including for local screens when remote groups follow it. Moving beyond the
group slides the preview under the boundary until it disappears. The global
drawer boundary and the position of New screen must not control this clipping.
Keep the clipping ancestor on both the lifted preview and its moving siblings;
do not draw the preview in an unclipped window-level overlay.

Remote host groups reorder as whole blocks: caption plus all their screen cards.
Dragging a caption/grip uses the remote-groups region as its boundary. This
computer remains above that region. The target gap follows the block's height;
sibling groups move aside with the same visual timing as screen rows. Dragging
a group does not select a screen or connect a host. Caption click still opens
the browser; a movement threshold separates click from drag.

For implementation, reuse the current native drag motion rather than adding a
second animation vocabulary. Capture owner, group identity and order revision;
cancel on stale owner/source, removal, Escape or loss of the gesture. Existing
screen/group order contracts and local persistence remain authoritative. Do not
change remote ownership, cross-host screen placement, or another device's order.
Preserve autoscroll within the appropriate drawer region and honor reduced motion.

The prototype demonstrates screen clipping, animated sibling displacement and
whole-block host moves. It uses a short CSS ease and supports Alt+Up/Down as a
review aid. It does not implement native spring timing, full autoscroll or durable
reorder commits; those remain implementation/validation work.

### Hosts: inspect, open, create

A wider browser has a searchable host list and one selected host's detail.
Name, canonical SSH target including a nondefault port, and connection status
appear once. Screens and Ports are separate tabs. Screens are first: Open
attaches to a specific existing screen; an already open screen offers Show.
New screen is explicit and creates on the selected host.

The screen catalog includes every visible screen on the selected remote
workspace, including those not opened here. Each row distinguishes the remote
observation (Running / Last seen) from this window's reference (Open in this
window / Not open here). A Not open here filter finds available screens without
searching through already opened ones. This is not a claim that no other client
is attached: cross-client presence is a separate capability and is not invented
from local attachment state.

Open attaches this window to the existing screen; Show selects its existing
reference. Detach removes only that window's reference. It keeps the screen,
processes, other clients and explicit forwards intact; a Reopen action offers
recovery. The prototype initially included Detach in an extra remote screen header.
The native implementation keeps Detach in the drawer and opened catalog rows. Detaching the last screen removes its drawer group,
while the known host and full catalog remain available through Hosts. An offline
remembered reference can also be removed without contacting the host.

Screen existence and attachment have separate owners: the remote workspace owns
the screen, its name/layout and sessions; each window owns its opened references
and selection. End screen on host is a distinct destructive operation requiring
confirmation and affecting every attached client. It is not the Detach action.
The prototype simulates Open, Show, Detach and Reopen; remote termination remains
a described flow, not an executable prototype control.

Opening the browser, filtering, or selecting a cached host never connects it.
Connect or Open on a cached screen is deliberate network intent. Cached rows
show Last seen. A missing screen stays missing with refresh/retry guidance; it
is never silently recreated. On an empty connected host, New screen is primary.
For many screens, add catalog filtering inside the selected host instead of a
nested menu. No terminal keyboard shortcut is assigned by this study.

The mockup interprets “browser” as the native host browser. External browser
opening belongs to verified web services; a separate launch of a host's Splitty
Web UI would be another feature and needs gateway/credential lifecycle design.

### Ports: service first, local access second

The drawer popover answers: which services can I use, and are they reachable
here? A web service offers Open or Forward & open; a non-web listener offers
Forward or Copy address. Do not infer HTTP just from an arbitrary TCP port.
The existing source URL retains its scheme and path. Unknown manual services
default to TCP until the user specifies HTTP/HTTPS. The prototype's HTTP service
classification is sample data, not an implemented detector.

The host's Ports tab lists source port/process, local address and an explicit
state. Each row has one primary action and a direct settings button. No cascade
of submenus is required for normal use. Active count counts verified active
tunnels, not discovered services or saved rules. Unavailable detection is not
an empty host; manual Add port remains available.

Settings separate two scopes:

| Information | Proposed owner | UI meaning |
|---|---|---|
| Listening port, process, originating session | Remote server | What is actually running; stale observations are marked. |
| Explicit service name and optional service protocol | Remote server, scoped to its account/workspace | Consistent names for clients of that same workspace, not all users on a machine. |
| Host nickname, optional personal port label | Local preferences | Personal naming; existing saved labels stay here until explicitly published. |
| Local bind address/port, live forward, restore preference | Initiating local backend | How this client environment reaches a service. Another device chooses independently. |
| Which screens are open/selected | Window/client model | Navigation and attachment, not ownership of remote processes. |

Use a named local environment in details (“This computer” for the native local
case). In Web, if the backend runs elsewhere, the bind is on that backend, not
on the browser device. Never construct a browser `localhost` URL for a different
machine. A reachable endpoint or an explicit unsupported message is required.
On Windows, name the WSL environment when relevant and use verified reachability.

The shared name must bind to stable server/workspace identity and its network
namespace, not a mutable host nickname. Keep the port and current process visible
to expose reuse of a port by another program. This study does not invent a stable
project identity that the current server lacks. Project-scoped names can be added
when such an identity exists. A stale named service can remain configured but
must not be shown as listening. Aliases resolving to the same verified workspace
may share service metadata; SSH targets remain exact forwarding destinations.

Saving a name neither starts a forward nor changes restoration. Saving local
settings does not start one either. “Restore when I connect” is explicit, local
and initially off. Persisting a preferred port and enabling restoration are
separate choices. Stop pauses a saved rule durably, including across reconnects;
it visibly clears automatic restoration. Forward/Resume is deliberate. Removing
local settings leaves a live tunnel running and says so; Stop is separate.
Turning restoration off alone does not stop the current connection.

Existing local labels must not be silently copied to the remote server. Older
servers fall back to process names and local labels. Remote metadata editing
requires a supported capability and the same authorization boundary as other
workspace metadata. These are future protocol and persistence changes.

### Lifetimes and failure feedback

| Event | Recommended result |
|---|---|
| Close popover/browser or detach a screen | Keep explicit forwards; keep remote processes. |
| Close a window | End window attachments; retain a forward while its owning backend lives. No promise of cross-owner tunnel reuse. |
| Quit the owning native app/backend | Stop its tunnels. Remote screens survive. Saved local settings survive; restore only after a subsequent explicit host connection. |
| Reload/close a Web tab | Do not imply the independent backend exited. Backend-owned tunnels can remain until stopped or that backend exits. |
| Connection loss | Mark each tunnel according to its own supervisor; preserve chosen ports. Never open an unverified/stale endpoint. |
| Local port conflict | Keep the saved choice, explain which local port is busy, offer Retry or a user-selected alternative. Commit the alternative after success. |
| Shared name edited elsewhere | Update presentation; do not start/stop or redirect any local forward. |
| Reverse share | Distinct “Share with host” direction with consent and source environment. Never mix it into outbound active counts or auto-start from discovery. |

The browser stays in place while a connection is pending; errors appear there
with Retry. New-host fingerprint and password dialogs use the same spacing and
target identity. A changed key is distinct from an unknown key and retains the
existing verification policy. Cancel ends the initiating attempt; it does not
immediately reprompt. Passwords and key material are not persisted by this mockup.

### Files between a pane and this computer

A pane's program takes a file as a path, and its own clipboard on a host is
empty. Native follows the web client's rule (see `terminal-sessions`, Files Into A
Pane and Saving An Offered File): a paste without text stores its images and
files on the server of that pane and pastes the returned paths; Ctrl+V does the
same when the clipboard holds only an image or files. A local pane gets the
paths of local files directly. Files dropped on a host's pane wait for Send in
a notice that names the host and the total size. An offered file is read only
after Save and is written under its name in the download folder, never over an
existing file; an unfinished download leaves nothing behind.

The worker owns every transfer per source connection, one answer at a time,
and gives up after 30 seconds without one. Notices sit in the window's lower
right corner and carry the one action that applies: Save, Cancel, Send or Show
in folder. Not in this slice: choosing a destination directory for a drop, and
files dropped onto a scratchpad.

### Layout changes lead the service

Divider drags and tab or pane moves waited for the service's snapshot, which
made them lag on local screens and stall on remote ones; tab dragging was off
for remote screens. As in the web client, the window now changes its layout
first and the service confirms or rejects afterwards. The divider follows the
pointer directly; a dropped tab is in place at once and focused. A rejection or
a different layout from the service replaces the preview. Tab dragging works
on remote screens through the same path.

## Mockup review

Open [mockup.html](mockup.html). The review toolbar switches between Workspace,
Hosts, Ports, Port conflict, Reconnect, Connect and Ownership. It is outside the
product frame. Click host captions, group plus buttons, screen Open/Show,
forwarding controls and settings. Search filters known hosts. Escape dismisses
overlays; dialogs have a keyboard focus boundary. Reset restores sample data.
All actions are simulated and perform no SSH, clipboard or external browser I/O.
Scenario buttons start from sample data; in-scenario actions keep their state.
Drag screen cards inside their group or host grips inside the remote section;
Alt+Up/Down on a focused card/grip is also available in this study.

The sample separates two API endpoints: remote 8080 is available here as 8081.
It also includes a paused database, a detected non-web listener, an offline host
with cached screens, an empty host, and a local-port conflict. Shared service
names are explicitly a proposed feature. Full settings, reverse
sharing, destructive screen operations and changed-key recovery are not modeled.

Prototype checks (Linux Firefox, 2026-11-11): real pointer drags verified screen
clipping and host-block reorder without selection/connection side effects.
Browser interactions checked cached-host opening, port settings versus starting,
conflict resolution, connection cancellation, and layout at 1440 and 1024 px.
The screen follow-up also checked the unopened filter, attach/detach/reopen,
last-reference removal with the catalog and forwards intact, and offline detach
without connecting. These checks validate the mockup only, not a native implementation.

## Reviewed decisions

- **NRUX-1 (2026-11-11, accepted):** grouped creation, a searchable
  host browser and direct screen opening; native presentation can diverge from
  Web. Alternative: retain the narrow stacked panel and global creation menu.
- **NRUX-2 (2026-11-11, accepted):** service facts/names belong to the
  remote account/workspace; forwarding and restoration belong to the initiating
  environment. Keep existing labels local. Alternative: all labels stay local.
- **NRUX-3 (2026-11-11, accepted):** explicit forwards outlive views and
  attachments, bounded by the owning backend. No new background/device-wide
  daemon is implied. Alternative: reference-count by windows or always persist
  through app exit using a separate service.
- **NRUX-4 (2026-11-11, accepted):** quick service popover plus direct
  detail settings, separate save/restore/start, and protocol-aware Open actions.
  Alternative: keep Remember and management nested under the existing port menu.

## Architecture impact

The static prototype adds no runtime dependencies. Implementation touches `gpui.gpui_workspace` for presentation and
`client_core.client_ports` / `client_core.hub` for shared policy/projection,
reusing `server.hub` and `remote.tunnel` for local execution. Shared service
metadata would extend `server.client_session` and durable remote workspace
storage, and require a reviewed wire capability and authorization semantics.
Confirm component IDs and storage placement at implementation; update
`architecture/*.c4`, the Client layers / remote networking concepts in
`docs/architecture.md`, and touched `docs/protocol.ts` contracts then.
The existing `frontend-client-core` draft tracks broader client reuse; this
study does not require moving `splitty-remote` or consuming Rust from Web first.

## Implementation boundaries

The initial native implementation keeps source service metadata unchanged. Unknown
TCP listeners offer Forward/Copy address; local settings expose an explicit Open
as HTTP command. Terminal URLs retain their existing validated scheme and path.
Shared names and stored protocol classification require the separate remote
capability/persistence slice described above; the settings label is explicitly
personal. Outbound local preferences add optional `enabled` writes and
`preserveRestore` opening, preserving omitted-field behavior for existing clients.

The browser uses Kit controls, a retained focus trap and a virtualized per-host
catalog. At narrow widths the searchable host collection moves above the detail.
Screen and host dragging reuse the existing motion and reduced-motion policy.
