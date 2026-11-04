# Proposal

## Why
Dropped files immediately upload to temporary storage without a destination choice or cancellation. Users need both temporary attachments and durable copies in a chosen host directory.

## What Changes
- Confirm dropped files before sending bytes; default to temporary storage and path insertion.
- Offer an existing absolute destination directory, suggested from the session's last reported working directory, and optional path insertion.
- Show per-pane progress and cancellation; remove incomplete uploads and retain completed files.
- Keep clipboard image pastes immediate. Never overwrite existing destination files.
- Out of scope: folder uploads, remote directory browsers, overwrite prompts, resumable transfers, CLI upload.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `terminal-sessions`: destination-aware, cancellable uploads and drop confirmation.
- `session-scratchpad`: the same drop choices with optional insertion into scratchpad text.
- `access-control`: explicitly permit a user-selected directory while preserving file-name confinement and no overwrite.

## Impact
Core upload storage and dispatch, WebSocket capability announcement, frontend session client and terminal/scratchpad drop UI, protocol and architecture documentation. No new dependency or roadmap milestone.
