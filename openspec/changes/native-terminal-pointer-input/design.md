# Design

## Context

See proposal.md. GPUI currently starts selection for every left press and always substitutes cursor keys for alternate-screen wheel input. The pinned Ghostty API contains state-aware mouse encoders, but native Command::Input and Client::input use String. Core Request.data and PTY writers already accept bytes. Serial session dispatch and the PTY write mutex preserve each request's ordering. Session authority intentionally does not require attach; the native client independently gates input on completed attach and its current pane/session route.

## Goals / Non-Goals

Keep terminal semantics in the terminal adapter, surface geometry in GPUI, transport compatibility in the shared client, and PTY authority in the existing service. Do not create a second input writer or mouse-specific backend API. Search, link opening, input remapping, remote UI and CI are separate work.

## Decisions

- NP1: Add `input_bytes {id,data}` and `terminal-input-bytes-v1`. Decode strict standard padded base64 in the core dispatcher after an encoded-size bound (87,384), reserve decoded allocation through the existing transport budget before allocating, enforce 65,536 decoded bytes and retain the lease across the existing PTY write. Share session lookup/error handling with text input. The shared service allowlist admits it on the same authorized session stream, including the browser gateway. Existing `input` is unchanged. Binary transport frames are unsuitable here because carriers currently treat them as output and the additive text message already has bounded ordered routing.
- NP2: Carry Vec<u8> from the terminal through the existing generation-checked callback and bounded native worker command. Client records capability only from its own ordered welcome; UTF-8 data can use legacy `input`, non-UTF-8 requires the capability. Never retry an uncertain input, split a paste or change terminal modes to fit the transport.
- NP3: Extend the private Ghostty ABI checks and owning Engine wrapper with a mouse encoder and plain input values. Ghostty derives tracking/format from terminal state. Engine owns protocol encoding and motion deduplication; GPUI translates surface-relative positions, button/modifier events and lifecycle cancellation, retaining the local/application gesture latch and only actually admitted pressed buttons. This state belongs with queue admission and view lifetime. The pinned API has no reliable tracking-mode getter when overlapping mode resets disagree with bit flags: lazily probe the state-synchronized native encoder with a constant set of zero-coordinate events and dedup disabled, then restore it, rather than add a second escape parser. Fractional font metrics must retain correct cell mapping; SGR pixels use actual surface pixels. No raw C pointer escapes the adapter.
- NP4: Shift at press selects locally for that gesture; changing Shift while an application drag is held does not orphan its release. Wheel chooses application reports for wheel-capable modes, otherwise existing local scrollback/alternate-arrow behavior; fractional wheel accumulation is bounded and reset when routing changes. Focus loss, hidden/replaced view, terminal reset and window deactivation settle an admitted application gesture on its captured route where possible, then clear it. Do not synthesize input to a new session or retry queue rejection.

## Architecture impact

`architecture/core.c4` protocol keeps the shared dispatcher and PTY relationship; describe byte input. `architecture/client-core.c4` connection records capability and imports core protocol constants; native_workspace carries bytes with existing identity/attachment validation. `architecture/gpui.c4` terminal owns geometry and routes bytes; terminal_engine owns protocol encoding/gesture state. Update those component descriptions and the connection-to-core-protocol relationship. Update docs/architecture.md native client/terminal boundary explanation. No new crate, top-level module or PTY dependency from UI.

## Risks / Trade-offs

- Legacy encodings can produce non-UTF-8 → explicit byte path and high-coordinate tests; never lossy conversion.
- Missed release outside an element or focus loss → explicit capture/bubble lifecycle handling, gesture latch and real-window tests.
- Decoder allocation or malformed input → preallocation length checks, charged storage retained through write and no partial effects on decode failure.
- High-frequency all-motion → encoder deduplication in cell modes and existing bounded admission; no unbounded queue or retry loop.
- Platform differences → deterministic engine tests plus real private X11 input and Wayland/X11 hardware smoke; native Windows/macOS pointer behavior remains unmeasured on this host.

## Migration Plan

Additive capability; current web clients continue using text input. Roll back the native feature commits together if necessary; no state migration. Validate locally on the feature branch and preserve main. Full CI/workspace/platform checks remain deferred during the native experiment; run targeted touched-crate, architecture, protocol and real-window checks.

## Provisional decisions

NP1–NP4 above are provisional, tbd, 2026-11-07. They preserve the current feature branch experiment and can be revised after interactive review.
