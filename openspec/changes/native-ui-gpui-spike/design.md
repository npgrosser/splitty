# Design

## Context

The existing native connector provides ordered charged frames, negotiated roles and awaited stream cleanup. Service runtime construction is already reusable. Windows intentionally has no dependency on the Unix core/server crates and uses an owned WSL/native stream instead. The full migration remains a draft; this change produces evidence for its phase 0.

## Goals / Non-Goals

Build a real native terminal/document probe and measure its boundaries. Do not build a second PTY manager, invent a transport, port the complete frontend, expose native commands to documents, or turn the spike into a release switch. Host UI and full feature parity stay in the parent change.

## Decisions

- Use exact `gpui-kit =0.9.0`, whose facade pins GPUI snapshot `0.3.11`, without the default component/assets bundle. Use `alacritty_terminal =0.26.0` for terminal state, parsing, selection and scrollback. These Apache-2.0 dependencies avoid copying Zed's GPL application code. Ghostty remains a candidate if image protocols justify its additional C/Zig integration.
- `splitty-client-core` adapts the existing service client on non-Windows and owned native connections on all platforms. A separate embedded composition owns service/control cleanup. Keep this adapter small: existing bounded JSON messages are sufficient for the spike; do not extract a new public protocol crate yet. Preserve charged storage through decode/queue/document assembly and scope IDs and pending reads to one connection.
- `splitty-gpui` owns native rendering and a bounded worker-to-UI channel. The service owns screen/session IDs, PTYs, document content and annotations. The terminal consumes replay in order without generating replay replies, paints native cells and routes committed IME text, keys, selections and explicit clipboard actions. Resize is sent after attachment. Runtime shutdown must complete before its executor exits.
- Use GPUI Base's native Markdown text layout, rendered/source byte-range mapping and range highlights. Convert selection ranges only at UTF-8 boundaries; retain quote/context/source lines for annotations. Disable implicit image resolution and link opening. Server snapshots remain the annotation authority.
- Attempt optional CEF CPU offscreen rendering of a fixed scripted fixture. Bound frame dimensions and retain only the latest complete frame, then display it as native image content beneath overlays. Keep engine code optional, with no release packaging, debug listener, disabled web-security flag or service bridge. An engine/build failure is a documented spike result, not permission to claim success or silently switch to child-window embedding.
- Run Linux Wayland and X11 separately on the available WSLg environment, record renderer identity and actual screenshots when available. Software rendering establishes composition correctness only. macOS/Windows and manual IME/dead-key/clipboard checks remain explicitly unverified unless actually exercised; implementation completion does not imply the full migration go-ahead.

## Architecture impact

Add `splitty.client_core` and `splitty.gpui` containers and their top-level modules in the LikeC4 model. The client adapter depends on `transport`, `remote` and conditionally existing service/core components; the GPUI app depends on the adapter, never the PTY implementation. Optional offscreen rendering belongs to the GUI container. Update the client-connection/development sections of `docs/architecture.md` and add a reproducible spike report; no existing component is removed.

## Provisional decisions

- **GSP-1 (2026-11-07, provisional, tbd):** exact GPUI Kit 0.9.0 / snapshot 0.3.11 and Alacritty 0.26.0 for this probe; no renderer commitment for the final app.
- **GSP-2 (2026-11-07, provisional, tbd):** a small platform-aware client adapter with bounded existing message views precedes a full Rust client core; protocol extraction is deferred until actual consumers require it.
- **GSP-3 (2026-11-07, provisional, tbd):** optional CEF CPU offscreen fixture is an evidence gate. If it fails or platform validation is unavailable, retain that limitation and keep scripted viewer support and release cutover undecided.

## Risks / Trade-offs

- GPU/platform prerequisites may be absent: use isolated development dependencies where possible and distinguish compilation, software rendering and hardware validation.
- UI queues can retain uncharged copies: retain connection leases or reserve replacement storage before decoding and queueing; test saturation and closure.
- Markdown source and rendered offsets differ: test multibyte source mapping and verify saved targets against authoritative snapshots.
- CEF adds substantial binaries and platform lifecycle complexity: optional build only; no engine is shipped by this change.
- A desktop spike cannot prove cross-platform parity: maintain explicit tested/failed/unverified results and keep the parent migration open.

## Migration Plan

No data migration or release change. Run the experimental binary explicitly. Reverting the new crates and development documentation restores the previous tree without modifying saved workspaces. The existing desktop packages remain the supported applications.
