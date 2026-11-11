# Native remote workspace UX

Implementation — 2026-11-11. Native remote workspace redesign based on the reviewed interaction study.

## Why

Remote connections now support everyday work, but their presentation makes the
user reconstruct the hierarchy. The local New screen action sits below remote
groups. The narrow Hosts panel repeats targets and interleaves connection state,
screen catalogs, port mappings and management actions. A saved port's name,
local address and reconnect policy appear to describe one thing even though
they have different owners and lifetimes.

Native can evolve independently of the Web UI. Both clients should preserve
the same underlying meanings, without requiring identical layouts.

## What changes

- Keep host, screen and active terminal identity in the window title. Do not add a
  workspace header above terminal panes.
- Give every workspace group its own New screen action, including This computer.
- Reorder whole remote host groups by dragging their captions, with the existing
  screen-drag feel. Clip a dragged screen to its own group's screen region.
- Connect, open a host's screens and open its ports from small panels beside the
  drawer. The drawer's Hosts item lists the hosts and takes a new SSH target; a
  host group's name lists that host's screens, its port pill lists its ports.
  The drawer shows only what is open; it lists no remembered hosts.
- Keep Hosts as the manager behind those panels: a searchable browser with the
  complete screen catalog, port settings, display names and updates.
- Show the complete remote screen catalog with a Not open here filter. Make
  attachment, selection and detachment explicit; Detach preserves remote work.
- Open a port in the system browser in one step from the drawer: choosing a
  listed port, or typing one, forwards it if needed and opens it. Put detailed
  management in the host browser.
- Give every menu and panel of the native workspace one look and one keyboard
  model, and show a focus ring only for keyboard traversal.
- Store pasted and dropped images and files on the server of their pane and
  paste the paths; save a file a pane offers with `splitty download` after Save.
- Separate the remote service from its forwarding on this device. Propose shared
  service names on the remote workspace, with local endpoints and reconnect
  preferences owned by the initiating local backend.
- Make closing a view, stopping a forward and ending a remote screen visibly
  different operations. Show recovery and conflicts where the action happened.
- Close a host as a whole without ending anything on it, and reopen the screens
  it had open here when the host is chosen again, also after the app was quit.
- Use the native theme and existing component vocabulary with less repeated
  information, consistent spacing and explicit action placement.

Review the [clickable mockup](mockup.html) and [design](design.md). All network,
terminal and browser actions in the mockup are simulated. The Ownership review
page explains the proposed boundaries; it is not an application settings page.

## Scope

This change covers native navigation, known-host access, connection presentation,
drawer groups, outgoing ports and the ownership model that makes those flows
coherent. It follows the native workspace work in
[roadmap item 120](../../../roadmap/README.md#als-nächstes).

Out of scope: moving crates, selecting
a Web/WASM sharing mechanism, changing SSH trust or authentication policy,
redesigning terminal panes, or implementing reverse sharing in Native. Reverse
sharing still needs a distinct direction and consent flow in any later UI.

Navigation and forwarding presentation use the existing client controllers.
Shared service metadata remains a distinct protocol/persistence slice; existing
private mapping labels must remain private throughout implementation.

## Carried-over design notes

- 2026-11-11, from `native-drawer-host-ports`: keep exact host/source capture,
  verified endpoints, cancellation and bounded menus. A popup's lifetime is not
  the forwarding lifetime. Replace the presentation without weakening these rules.
- 2026-11-11, from `native-remote-screens` and `native-remote-screen-create`:
  browsing, opening an existing screen and creating a screen are distinct intents;
  detaching does not end processes. Hosts with no open screens must remain usable.
- 2026-11-11, from the existing native screen reorder and `host-group-order`:
  keep screen moves within their host. A host move changes the local presentation
  order, never remote screen ownership or order on another device.
- 2026-11-11, from `frontend-client-core`: responsibility boundaries matter more
  than crate names. Native independence concerns presentation; shared policies
  should still live outside renderer code. This change does not promise that Web
  already consumes the Rust client core.
