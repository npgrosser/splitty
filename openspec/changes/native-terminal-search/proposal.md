# Proposal

## Why

The native terminal supports local Screens, tabs, splits and pointer input, but finding earlier output still requires manual scrolling. Add terminal-local search without making dense matches or inactive terminals stall interactive use.

## What Changes

- Find literal text in the active terminal's entire retained screen and history, with ASCII case-insensitive matching, result navigation and independent highlights.
- Keep query text per terminal, while only the focused visible pane maintains an active search index.
- Distinguish indexing, completed results and an explicit resource-limit failure. Never present a truncated search as complete.
- Keep search input and selection separate from PTY input and clipboard selection.

Out of scope: regex/case/whole-word modes, server-side or cross-pane search, web refactoring, shared WASM infrastructure, terminal replacement, platform packaging and CI. This extends the native evaluation branch; there is no dedicated roadmap milestone for this increment.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `terminal-sessions`: native local search scope, matching, resource failures and isolation from session input.

## Impact

The existing native terminal adapter owns search handles and returns owned results; the native view owns editing, scheduling and highlighting. Workspace focus controls the active index. No protocol, service authority, dependencies or web behavior change.
