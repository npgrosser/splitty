# Design

## Context
See proposal.md. Session process information and OSC state already exist; the missing piece is their authority outside an attached window. Reducer state and effects remain pure.

## Goals / Non-Goals
Provide consistent names, titles, busy state, inheritance, exit behavior and safe replay to all transports. Do not infer agent thinking, add commands, or change job retention and connection authority.

## Decisions
- A service-owned observer samples all registered mux sessions, regardless of clients, updates metadata through a trusted reducer op and handles exit through normal close effects. It follows sessions across pane moves. Per-connection title events retain immediate OSC updates and use the same display function.
- Session snapshots add displayTitle and busy; shell pane titles follow displayTitle, managed job labels stay intact. Busy means a known foreground process other than the original shell; an exited or unknown process is not busy.
- Display titles remove control/bidirectional formatting characters and cap text at 500 characters. CLI listing cells escape such characters, including cwd, while JSON remains structured.
- Titles select a non-base-shell OSC title, otherwise the process name. Known shells use their normalized basename and home-shortened working directory. Home shortening requires a path-component boundary.
- Close policy is read from the owning service settings at exit. Always closes any exit, graceful closes only a known zero result, never retains. Managed jobs retain all outcomes. Allow a three-second grace, beyond the stream drain bound, before removing the pane; screen removal rules are unchanged.
- Ordinary mux dispatch/effects/publication and observer updates share a gate so metadata nudges cannot precede spawn announcements. Job lifecycle keeps its own publication ordering.
- The reducer supplies the first unused Screen N only for omitted names, including pane moves. Explicit names and existing empty screen names remain intact.
- Clients advertise support via serverDerivedPaneState. New clients use server metadata and omit unspecified launch overrides on capable hosts, keeping legacy derivation and close behavior on older hosts.
- Use an in-place parser with the existing byte replay rules before replay framing, retaining its incomplete trailing-sequence behavior and client defense for older hosts. Live bytes and attach ordering remain unchanged.

## Provisional decisions
- PSD-1 (2026-11-06, provisional, tbd): use one additive serverDerivedPaneState capability for this coordinated behavior, with legacy web fallbacks.
- PSD-2 (2026-11-06, provisional, tbd): poll workspace metadata independently of clients and retain a brief output-drain grace before automatic close; no new native requests are needed.

## Architecture impact
Existing core mux, pty and protocol components and server service component change. Add a service submodule for the observer, no new top-level component or dependency edge. Update the component descriptions and the terminal lifecycle concept in docs/architecture.md. docs/protocol.ts documents title events and session snapshot fields; native operation catalog is unchanged because no request is added.

## Risks / Trade-offs
- Metadata races with moves or closure → trusted reducer resolves the current session location and ignores removed sessions.
- Final output races with auto-close → defer close after exit to permit stream drain; preserve the stream closed event's actual result.
- Older hosts lack fields → capability-gated behavior and client replay filtering remain.

## Migration Plan
Additive fields deserialize with defaults and old clients ignore them. No persisted layout migration. Rollback keeps old fields and client fallbacks.
