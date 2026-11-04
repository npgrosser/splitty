# Design

## Context
See proposal.md. Existing uploads use sequential 1 MiB chunks and connection-owned cleanup. Session metadata supplies lastCwd; native Windows shells use the WSL backend's paths.

## Goals / Non-Goals
Provide the agreed drop workflow using the existing connection and store. No directory browser, new dependency, overwrite or resume support.

## Decisions
- Announce upload-control-v1. Require it for the new drop flow so older servers cannot silently ignore destinations. Clipboard images keep save_image compatibility.
- Reuse first-chunk dir and add upload_cancel. Process cancellation on the same ordered socket. An AbortSignal stops reading/sending subsequent chunks and releases client listeners; server cancellation removes partial state.
- Stage durable uploads in a private new directory inside the selected directory, without changing the parent permissions. Publish with a hard link that fails if any destination entry exists; then delete staging. This makes completion atomic without replacement and avoids cross-filesystem moves from the cache.
- Preserve the existing filename sanitizer for both destinations. Quote inserted paths when necessary. Reject invalid/missing directories rather than guessing or making folders.
- A shared Svelte drop component supplies a native modal choice and a nonmodal per-pane progress card. Terminal and scratchpad own its lifecycle and abort pending work when disposed.

## Architecture impact
The core protocol component, server ws component and web frontend container gain behavior within their existing boundaries; no new component or relationship is required. Update component descriptions and docs/architecture.md File Transfers and Pasted Images.

## Risks / Trade-offs
Hard links must be supported by the selected filesystem; failure is reported without replacement. Cancellation racing final completion retains the already completed file. Last known cwd is a suggestion and is shown for review. Completed files in a cancelled batch remain but paths are not inserted.

## Migration Plan
Deploy server and frontend together; older hosts show an update notice for drops while clipboard images still work. Existing clients retain temporary upload behavior.

## Provisional decisions
D1 (2026-10-31, provisional, tbd): reject filename conflicts with a readable error; no overwrite prompt in this scope. D2 (2026-10-31, provisional, tbd): require upload-control-v1 for drops on old hosts to guarantee destination and cancellation semantics.
