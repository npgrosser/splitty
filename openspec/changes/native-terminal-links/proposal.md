# Proposal

## Why

The native terminal can run local workspaces but cannot open links printed by their processes. Supporting explicit OSC 8 targets and ordinary HTTP(S) output removes a common daily-use gap without changing workspace authority or choosing a final web/native product direction.

## What Changes

- Resolve explicit terminal hyperlinks and detected absolute HTTP(S) text at a pointer cell, including soft-wrapped retained output.
- Preview the actual allowed target and open it only after a deliberate Ctrl/Cmd-click completes on the same target.
- Preserve terminal selection, application mouse reports and lifecycle cancellation; bound extraction and reject incomplete targets.
- Keep parsed URL policy separate from terminal interpretation and platform browser effects.

Out of scope: remote port forwarding, file/editor URI dispatch, automatic opening, link history, web-client replacement and CI/cutover work. This is an increment of the native UI evaluation, not a separate roadmap milestone.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `terminal-sessions`: deliberate native terminal link activation, safe target resolution and bounded current-target validation.

## Impact

The native terminal adapter gains bounded owned link hits; GPUI gains local gesture handling and system-browser dispatch. The UI-free Rust client layer gains a parsed HTTP(S) target policy, reusing the already-locked URL parser. Plain text detection uses a small dedicated matcher. There are no wire, persisted-data, service-lifecycle or web-client changes. Existing generic-opener restrictions in `access-control` remain intact.
