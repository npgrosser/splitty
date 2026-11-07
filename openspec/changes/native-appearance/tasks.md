# Tasks

## 1. Shared appearance reader

- [x] 1.1 Add the API-only embedded connection factory, deserializable shared theme types and bounded charged appearance reader with coalesced generation handling and cancellation-safe polling; verify unit tests for theme choice, parsing bounds and stale generation handling.
- [x] 1.2 Add in-process reader tests using explicit in-memory service settings for read-only snapshots, separate session-event delivery, superseded refresh and cancellation/cleanup; verify no production settings loader or update operation is used by the fixture.
- [x] 1.3 Record the reader's actual module dependencies in architecture/client-core.c4 and its ownership contract in module documentation; validate the architecture model against the owned source changes.

## 2. Native runtime and presentation integration

- [x] 2.1 Integrate the reader with the native worker after navigation changes, poll alongside terminal delivery, coalesce same-service settings events and OS preference changes, retain defaults/last valid appearance on error and await reader cleanup; verify focused in-process runtime tests for failure isolation and foreign-connection rejection.
- [x] 2.2 Add the GPUI appearance adapter with chrome font overrides/resolved UI defaults and semantic chrome/terminal colors, retaining native terminal typography; apply colors to retained, reset and new terminals with cache invalidation and visible-only size reports when chrome geometry changes. Verify unit tests for supported colors, UI font lists/default sentinels and engine color/reset retention; measure unchanged terminal font metrics after proportional chrome font changes in the actual-window check.
- [x] 2.3 Update architecture/gpui.c4 and docs/architecture.md for the completed native presentation path and supported subset; verify actual-window startup, light/dark appearance, chrome font sizing with unchanged terminal typography, retained history and usable fallback with isolated fixtures. This window smoke covers native font/layout and renderer behavior that in-process protocol tests cannot show.

## 3. Focused integration validation

- [x] 3.1 Run targeted client-core, core-theme and GPUI tests plus touched-target formatting/Clippy, architecture validation and strict OpenSpec validation; record exact results and remaining subset limitations. Broad CI, release-cutover and platform certification gates are outside this focused native MVP increment.
