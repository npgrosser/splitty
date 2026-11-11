## Native presentation

- [x] Replace the stacked host panel with a themed searchable browser, selected-host detail and Screens/Ports tabs. Keep terminal identity in the existing window title; add no workspace header.
- [x] Show all visible remote screens, filter those not open here, and provide Open/Show/Detach with cached/live distinction and exact captured ownership.
- [x] Implement deliberate connect-and-open for cached screen identities; cancel pending follow when its originating view or owner expires. Never recreate missing screens.
- [x] Place local creation at the local group, retain per-host creation, and clip dragged screen cards to their source group.
- [x] Reorder complete remote groups with existing drag motion and persist the local host order through the existing service operation; keep local screens first.
- [x] Present outgoing ports with compact direct actions and separate local settings; preserve verified browser intent, cancellation, conflicts and Stop semantics.
- [x] Keep shared service metadata as a separate protocol/persistence follow-up, with explicit remote scope and capability/authorization requirements. Existing labels remain private; the implemented settings identify them as personal.

## Drawer panels, menus and files (revision of 2026-11-11)

- [x] Open hosts, a host's screens and a host's ports as panels beside the drawer; remove the remembered-hosts list from the drawer; keep the browser as the manager one step behind each panel.
- [x] Open a listed or typed port in the system browser in one step, with copy and stop on a forwarded port's row.
- [x] Use one panel surface and keyboard model for the screen, tab-group, host and port menus; show a trigger as pressed only while its panel is open; show focus rings only for keyboard traversal.
- [x] Bring the Hosts manager, port settings and SSH prompts to the drawer's text size and spacing.
- [x] Store pasted and dropped images and files on the pane's server and paste their paths, in terminals and scratchpads; save offered files after Save into the download folder.
- [ ] Offer a destination directory for dropped files and accept drops on a scratchpad, as the web client does.

## Close host and reopen (addition of 2026-11-11)

- [x] Close a host group as one worker command that removes its references and releases its stream; offer it on the hovered caption, in the host's panel and on the manager's row.
- [x] Keep the local service's remembered remote screens in step with the window through per-host replacements: open screens as shown, none after the last detach, unchanged after Close host and after quitting.
- [x] Show the remembered count on a host in the Hosts panel and reopen the screens that still exist in one command when the host is chosen; offer Reopen in the host's panel and the manager.
- [x] Offer reconnecting at start per host, off by default: reconnect a chosen host that was open at quit, leave closed hosts closed, keep local screens usable and report a host that does not come up.

## Validation and integration
- [x] Show divider drags and tab or pane moves at once on local and remote screens; the service confirms or replaces them. Enable tab dragging on remote screens.

- [x] Extend unit/in-process client tests for captured cached-screen opening, cancellation, missing screens and persisted host reorder without SSH or selection side effects.
- [x] Extend existing GPUI UI tests for browser layout, captured screen opening, window title, group dragging and port action routing; verify filtering, attachment/detachment and clipping in the real GUI smoke check.
- [x] Update architecture component descriptions and touched protocol documentation alongside the implementing slice.
- [x] Build the optimized native app and check the remote flow in an isolated real GUI/SSH fixture. This smoke check verifies platform focus, drawing and actual SSH behavior that the in-process tests cannot establish.
- [x] Run targeted checks for touched crates and OpenSpec artifacts.
- [ ] Before merge run the full gate once: cargo fmt --all --check; cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings; cargo test --workspace --exclude splitty-gui; openspec validate --specs. If frontend code is touched, also pnpm check, pnpm test and pnpm build there.

## Verification record

- Shared client: 272 tests passed, including exact host-order acknowledgements and independent restore consent.
- Native workspace: 164 UI/unit tests passed, including pointer group reorder, cancellation/missing-screen admission, local group metrics, focus and forwarding actions.
- Forwarding: 43 server unit/in-process tests passed, including HTTP input guards, restore updates without tunnel effects and legacy request behavior.
- Architecture: component conformance tests passed. The change validates strictly; all 19 main OpenSpec capabilities validate.
- Optimized native GUI with isolated SSH fixtures: verified fingerprint acceptance, cached Open, live catalog filtering, Show/Detach, Enter submission, complete-group drag/reorder, local-card clipping at its group boundary, host/screen window title and the compact 560×420 browser layout. Remote screens remain running when detached.
- Revision of 2026-11-11: 241 native and 276 shared-client tests passed, including the panels' passive opening, one-step port opening, typed-host connect, focus-ring rule, a paste and an offered download against the real session protocol, and drop/offer confirmation. In the optimized app against the SSH fixtures: connect from the Hosts panel with host-key and password prompts, open and create screens, open a typed port (the forward came up and the browser request named its local port), paste a clipboard image into a remote terminal and its scratchpad, and save two offered files without replacing the first.
- Close host and reopen: 242 native and 280 shared-client tests passed, including per-host replacement in the stored list against a scripted service, the rule that keeps a closed host and forgets a detached one, opening listed screens in one step, and the caption action and reopening from the Hosts panel. In the optimized app against an SSH fixture: Close host from the hovered caption left both screens running and stored, the Hosts panel showed their count and reopened both, also after the app was restarted; detaching the screens one by one emptied the stored list; the manager's Reopen reconnected and opened the stored screen.
- Reconnect at start: 243 native and 283 shared-client tests passed, including the closed mark, the hosts chosen for a start, a restore that survives a closing panel and reports a host that does not come up, and reopening without taking the view. In the optimized app against the SSH fixtures: choosing the host in its panel saved it in `remote.reconnectHosts` and connected nothing in the running window; the next start brought the host's screen back while the local screen stayed shown; a host closed before quitting stayed closed and was still offered; a host whose SSH prompt was cancelled left the window with a notice that stays until dismissed.
- Full integration gate remains open: workspace formatting reports existing changes outside this UX slice, and strict targeted Clippy is blocked by `ensure_scratchpad_editor` in the separate scratchpad implementation (eight arguments). The changed remote UX and forwarding paths produced no Clippy diagnostics.
- Layout changes at once: 243 native and 285 shared-client tests passed, including a resize that outlives older snapshots and a move that yields to rejection and confirmation. In the optimized app, a divider drag and tab moves into a tab bar and to an edge were checked on a local and an SSH fixture screen; latency was not simulated.
