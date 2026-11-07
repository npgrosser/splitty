# Native appearance

## Why

The experimental native workspace uses fixed fonts and colors despite the local service already owning appearance settings and theme resolution. Loading that existing authority makes the native client useful without creating another settings store or theme resolver. This continues [roadmap 120](../../../roadmap/README.md) and the [native UI evaluation](../native-ui-gpui/proposal.md).

## What Changes

- Read local appearance settings and the selected resolved theme through existing `home.get` and `theme.get` operations on one owned API-only stream, separate from terminal delivery.
- Apply appearance font family/size to workspace chrome, using resolved UI theme defaults for unset overrides. Apply resolved terminal colors and a bounded semantic chrome palette. Follow the local OS light/dark preference for automatic theme selection.
- Coalesce same-service `settings.changed` refreshes and discard superseded results. Preserve working defaults or the last valid appearance on failures and expose a status without blocking terminals.
- Preserve session identity, terminal history, delivery budgets and owned shutdown while presentation changes.
- Out of scope: terminal font configuration (native monospace remains 15 px text / 21 px rows), activation of the currently unused `terminal.theme` setting, settings controls or writes, file watchers, cross-service synchronization, remote appearance authority, CSS layout/effects, display zoom, full renderer-setting parity, new wire operations, WASM and platform migration.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `settings`: read-only local appearance behavior for the experimental native workspace.

## Impact

The UI-free client gains a bounded appearance reader and the embedded adapter gains an API-only connection factory. Existing core theme types become deserializable. Native runtime integration and GPUI presentation follow after the shared reader. No dependencies, persistence format or wire messages are added. Architecture models and the native-workspace architecture documentation describe the additional owned stream.
