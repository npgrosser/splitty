# Design

## Context

See proposal.md for motivation. The terminal already owns admitted pointer routes, clipboard encoding and local search. Workspace owns captured layout and close commands. Retained offline terminal entities exist, but native Focus currently enters live-source command routing even though it changes only client presentation state.

## Goals / Non-Goals

Keep one menu owner and one implementation of each action. Preserve the clicked terminal through asynchronous model updates and popup callbacks. Reuse retained views and existing focus intent; do not add offline input queues, reconnect behavior, a second layout owner or new service operations.

## Decisions

1. Terminal emits context intent only after the existing pointer route admits a local right-button press. Workspace captures source, Screen, pane/session, entity and emulation generation. Installing a generic context listener would bypass application mouse reporting and Shift gesture lifetime, so it is not used.
2. Workspace owns one Kit PopupMenu and its dismissal subscription. The menu uses the Kit popup placement and deferred overlay pattern. Actions capture identity, revalidate at activation, and never infer the target from current global focus. The clicked canonical group is captured independently of focus acknowledgement.
3. Retained identity and live authority are separate. Exact shown Screen, displayed pane, owner and session admit local presentation actions in Ready or Offline state. Paste, split and close additionally require a PaneActionTarget captured while live and revalidated when used. A menu opened offline cannot gain live authority after reconnect.
4. Existing Focus carries TerminalToken and is handled by the native worker before transport admission. WindowWorkspace validates and updates its own focus without dispatching a mux command. The UI first queues that exact intent, then uses the same model method optimistically; queue failure cannot install model focus. A later worker publication retains the authoritative focus. Classifying remote focus as a local-owner command or keeping a UI-only override would violate source ownership.
5. Copy/Paste/Find share terminal keyboard implementations. Split reuses captured GroupTarget admission. Close retains process-impact assessment and confirmation. Show Drawer reveals navigation without changing the pin preference. Popup dismissal restores only its still-current clicked terminal when the popup still owns focus.

## Architecture impact

Update `terminal`, `gpui_workspace`, and their relationship in `architecture/gpui.c4` for context intent and captured action ownership. Update `client_workspace` and `native_workspace` descriptions in `architecture/client-core.c4` for retained focus validation and worker-owned pure model intent. Update the native pointer/input/search section in `docs/architecture.md`. No new architecture component, dependency or wire type is introduced.

## Risks / Trade-offs

- Delayed popup callbacks can outlive a source or pane → exact owner, Screen, session, entity and generation checks revoke them, including before ordinary reconcile dismisses the popup.
- A popup steals focus from a split → admit clicked model focus before opening, block ordinary workspace publications from stealing popup focus, and restore only validated identity on dismissal.
- Offline local actions could accidentally authorize writes → optional captured live authority remains separate and all existing write paths still enforce live admission.
- Test-only popup inspection may differ from production backend → tests open the real terminal menu and activate Kit items with real keyboard events, asserting actual command bytes and focus.

## Migration Plan

No persisted data or protocol migration. Integrate shared focus admission with its UI caller and menu in one change; rollback reverts that slice. Targeted tests run during development. The repository-wide merge checkpoint remains deferred.
