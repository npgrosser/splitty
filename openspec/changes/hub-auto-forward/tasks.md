# Tasks

## 1. Hub

- [x] 1.1 `hub::auto_forward`: per-relay watch with a baseline list, per-host record of automatically forwarded ports and processes while a relay of the host runs, reading only a daemon's `ports.changed` (prefix, 256 KiB, 256 ports)
- [x] 1.2 `Forwards::open_automatically`: the one skip rule (forward present, paused rule, open scheduled), an automatic open in the key's lane
- [x] 1.3 Both relays (`/ws/remote`, native host streams) hand host frames to the watch; the service hands its settings to the hub; `hub-auto-forward-v1` in the welcome of local main connections
- [x] 1.4 Tests (unit, `hub::auto_forward`): baseline, once per process, setting off, two relays of one host, refused ports asked again, frame filtering and limits
- [x] 1.5 Tests (in-process, native host streams): two streams of one host forward an appearing port once, a paused rule stays paused, nothing while off; the scripted bridge emits daemon events
- [x] 1.6 Tests (in-process, `/ws/remote`): a port that appears on a browser relay is forwarded; the welcome capability on local main connections only (conformance)

## 2. Clients

- [x] 2.1 Native: remove `native_workspace/auto_forward.rs` and `ForwardRequest::automatic`
- [x] 2.2 Web: remove the automatic path of `PortForwarding` and `appearedPorts`; vitest: a port that appears is listed, not forwarded
- [x] 2.3 Web: `remote.reconnectHosts` in the settings type; `hostsToReconnect` connects the listed hosts of the restored screens at start; vitest

## 3. Docs

- [x] 3.1 `docs/protocol.ts`, `docs/architecture.md`, `docs/remote.md`, `docs/native-ui-handoff.md`, `architecture/server.c4`
- [x] 3.2 `openspec/open-decisions.md`: HAF-1 to HAF-3

## 4. Checks

- [ ] 4.1 Targeted tests of the touched modules
- [ ] 4.2 Full check as in CLAUDE.md, including `cargo test -p splitty-gpui --features ui-test-support` and `pnpm check`, `pnpm test`, `pnpm build` in `frontend/`
