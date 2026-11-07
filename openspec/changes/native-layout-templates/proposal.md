# Proposal

## Why

The native window can arrange real terminals, including floating groups, but cannot save or restore those layouts as templates. The server already owns extraction, validation and application; exposing that workflow natively should reuse those rules and preserve explicit command and replacement consent.

This advances the native workspace evaluation in [the current roadmap](../../../roadmap/README.md#als-nächstes), without choosing between eventual replacement, coexistence or a reference native client.

## What Changes

- Add a local saved-template workflow: list/load, extract/save with selected commands and optional relative root, confirmed overwrite/delete, and apply to a new or existing Screen.
- Reuse one pure shared layout projection for authoritative application and the native pending view. Pending panes never become speculative sessions or attachment routes.
- Capture the exact template, target, root and replacement impact before command/replacement confirmation; send one existing apply operation and verify its authoritative result, including partial launch failures.
- Extend the owned auxiliary API client with bounded template jobs while retaining independent terminal output, fair settings/zoom progress and joined cancellation.
- Make web command consent fail closed without a dialog handler, bind it to an immutable snapshot and consume each decision once.
- Add targeted unit/in-process coverage and private real-window acceptance for persistence, command choices, replacement, pending presentation and retained unrelated sessions.

Out of scope: remote host UI, YAML editing/import/export UI, a general command palette or settings editor, changing the template file format, adding a server consent protocol, generic mutation retries, web-stack removal, WASM migration and full merge CI.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `templates`: immutable fail-closed command consent, local native library management, replacement assessment, pending projection and authoritative application outcomes.

## Impact

Shared template/mux helpers, native client library/API scheduling and workspace lifecycle, GPUI modal/input/pending presentation, the existing web template guard and its one caller. Existing template CRUD/extraction APIs and session `apply_template` retain their wire shapes. Architecture descriptions, template documentation, native runbook and tests change together; no new library dependency is planned.
