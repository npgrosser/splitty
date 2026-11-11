# Proposal

> Draft captured on 2026-11-12 from `native-default-desktop`. Not planned yet. Specs and tasks are written when this change is next. Related: `multi-attach`.

## Why

Every start of the native app is a process of its own with its own embedded backend. Starting it twice works as a second window, but the instances only share the configuration directory: a settings change in one is not seen by the other until it reads again, stored state (restore snapshots, remembered hosts and Screens, saved forwards) is written by both, and both try to restore the same port forwards. The web app prevented this with a single-instance handoff, at the price of one window.

## What Changes

- One backend per user session serves every window of the native app: a second start opens another window on the running backend instead of starting a second one, on Windows through the owned WSL helper and on Linux and macOS through the embedded service of the first process.
- Windows show different Screens of the same workspace; settings, hosts and forwards have one owner.
- Until then several instances stay allowed and are documented as independent workspaces that share settings files.

## Capabilities

### Modified Capabilities
- `client-connections`: a second start joins the running backend.
- `server-modes`: lifetime of the embedded backend with several windows.

## Impact

- `crates/splitty-gpui` (process startup, a window opened on request of another start), `crates/splitty-client-core` (several windows on one worker or one service), the Windows owner helper.
- Open: the handoff channel per platform, and what closing the first window does to the others.

## Architecture impact

Not determined yet; at least a relationship from the app's startup to a per-user handoff endpoint.
