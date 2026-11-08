# Proposal

## Why

The native terminal has keyboard clipboard/search commands and captured workspace actions, but no body context menu. A menu must preserve terminal application mouse input and must never redirect a delayed action to another pane.

## What Changes

- Offer Copy, Paste, Find, Split Right/Below, Show Drawer and assessed Close Pane from an admitted local right-click.
- Keep application-reported right-button gestures in the terminal; Shift selects the existing local gesture route.
- Capture the clicked displayed pane, session and owner before opening the menu, focus that pane coherently, and revoke obsolete menus and callbacks.
- Use Kit menu navigation, placement and dismissal, with existing clipboard encoding, search, split and close admission.
- Keep displayed offline Copy/Find usable through worker-owned pure focus intent; exact retained identity grants no transport or structural authority.

Out of scope: bulk close, clear buffer, viewer/editor actions, moving panes, protocol changes, new clipboard/file-upload behavior and changes to web menus. This supports the native UI direction in [the roadmap](../../../roadmap/README.md).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `terminal-sessions`: local context actions respect the admitted mouse route and captured terminal identity.

## Impact

Native terminal pointer dispatch and local commands; workspace menu composition and captured group/close commands; shared client window focus validation and worker admission; focused unit and in-process tests. No new dependency, wire message or service operation.

## Architecture impact

Update existing `terminal` and `gpui_workspace` descriptions and their relationship in `architecture/gpui.c4`, `client_workspace` and `native_workspace` in `architecture/client-core.c4`, plus the native interaction section of `docs/architecture.md`; no new architecture component.

## Decisions

- Terminal emits menu intent after its existing pointer route admits a local right press; Workspace owns the menu and pane authority. The menu is not installed on passive outgoing terminal surfaces.
- The clicked displayed pane becomes the coherent model and keyboard target through existing focus admission. A menu never reselects a hidden pane or falls back to whichever pane later has focus.
- Copy/Paste/Find share the keyboard implementations. Split and close reuse captured workspace commands; Close Pane retains process-impact assessment and confirmation.
- Unsupported split actions are disabled for floating-only targets. Show Drawer reveals existing navigation without changing the stored pin preference.
