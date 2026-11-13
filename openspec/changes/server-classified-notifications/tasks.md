# Tasks

## 1. Server

- [ ] 1.1 `splitty-core` protocol: a `Notice` type (kind, severity, title, setting, suppressed) with the builders for each event; `process.finished`, `process.idle`, `session.exited`, `notification` and `program_status.changed` carry it; `mux.welcome` lists `notification-notices-v1`. Unit tests of the builders and the wire shape in `protocol/messages.rs`.
- [ ] 1.2 PTY session: remember an end the server asked for while the program ran (`SessionHandle::closed_on_request`); `stream_output` reports the exit user-initiated from it; the engine's mark for a `close` request goes. In-process test (dispatcher with a real PTY): a session closed through the backend while it runs reports `session.exited` user-initiated and suppressed, one that exits by itself does not.
- [ ] 1.3 `process.idle` suppressed `program-status` while the session holds a root record (`watch_semantic`). Unit test in `protocol/notify.rs`.
- [ ] 1.4 `splitty-server` program status: a marked event carries the notice, titled with the screen's label from the server's catalog. Unit test in `client_session/program_status/tests.rs`; in-process: extend `http/tests/program_status.rs` so a marked event over both transports carries the notice.

## 2. Native client

- [ ] 2.1 `splitty-client-core/src/notifications.rs`: read `notice` (severity, title, setting, suppressed), fall back to the generic notice without it, apply the window's switches to arriving events only, record a watched pane's notification as read without a notice for every kind; remove the client-side classification, the program status and bell rules and the kind table. Unit tests in `notifications/tests.rs`.

## 3. Web client

- [ ] 3.1 `frontend/src/lib/notifications.ts`: build notifications from `notice` and the generic fallback, follow `program_status.changed` with a notice, drop suppressed ones, no toast for the watched pane (toasts and system notifications gated in one place); `NotificationSettings.programStatus` and its switch in the settings section; the duplicate gating in `App.svelte` goes. Vitest in `notifications.test.ts`.

## 4. Docs and checks

- [ ] 4.1 `docs/protocol.ts` (`Notice`, the events, the capability), `docs/architecture.md` (notification engine and windows), `docs/native-ui-handoff.md` where it describes notification rules.
- [ ] 4.2 Open decisions SCN-1 to SCN-7 in `openspec/open-decisions.md`.
- [ ] 4.3 Full checks, archive with spec sync.
