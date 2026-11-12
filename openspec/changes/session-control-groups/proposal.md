# Proposal

> Draft created on 2026-11-12, as the fourth part of a package that otherwise became `replay-terminal-modes`, `version-skew-smoke-test` and `program-status`. Not planned yet; whether it goes ahead is to be decided. Specs, design and tasks are written when this change is next. Related: `session-keepers` and `server-modes` (where the daemon and the keepers run).

## Why

**On Linux, everything shares one control group.** The daemon, the keepers and every program they start are in the control group of the login or the application that started the daemon. Memory pressure is resolved per group, and a service such as systemd-oomd ends a whole group, so one runaway program can end every session and the daemon with it. `setsid`, which the daemon and the keepers use, does not change the group.

Keepers exist so that sessions outlive the process that shows them. Inside one control group they share that process's fate at the hand of the service manager.

## What Changes

- **A scope per session**: on a Linux system with systemd, the daemon places itself and each keeper with its program in a scope of its own, so an out-of-memory kill is confined to one session. A memory limit per session is a possible setting on top and not part of a first version.
- **The desktop first**: on a desktop with systemd, applications usually start in scopes of the user's service manager already, and distributions that enable systemd-oomd let it end such a scope under memory pressure. There the precondition below holds and the app's local backend, its keepers and every local session share the app's scope. To be checked per distribution.
- **Hosts only where the service manager outlives logins**: scopes belong to the user's service manager, which stops at the user's last logout unless lingering is enabled, and takes its scopes with it. Today a host's sessions survive a logout inside the login's own scope, as long as logind's `KillUserProcesses` is off, its default on most distributions; with it on, they end at logout already. Scopes are used only where the user's service manager outlives logins; elsewhere placement stays as it is, and the daemon reports which case holds.
- Not on macOS, and under WSL only with systemd enabled. Hazards to design against: races with short-lived sessions, and scopes left behind.

Out of scope:
- Memory or CPU limits per session in a first version.
- Enabling lingering for the user, or changing logind's settings.
- Other platforms' equivalents (job objects, process groups on macOS).

## Capabilities

To be decided when the change is next. Likely modified: `session-keepers` or `server-modes`, for where the daemon and the keepers are placed and what the daemon reports about it.

## Impact

- Server: the daemon's start and the keeper launcher; a status line for where sessions are placed.
- Platforms: Linux with systemd only; nothing changes on macOS, on Windows outside WSL, or on systems without a user service manager.
- Tests: an in-process test with a real PTY that a keeper and its program land in a scope of their own, skipped without a user service manager.

## Open questions for the design

- How lingering is detected, and how a desktop session's service manager is told apart from one that stops at logout.
- How scopes are requested: through the service manager's bus interface or by a helper program, and what a failed request leaves behind.
- What the daemon does on a host without a user service manager, and how it reports the case.
- Whether the daemon moves itself into a scope as well, and what that means for its own start under a login that ends.
