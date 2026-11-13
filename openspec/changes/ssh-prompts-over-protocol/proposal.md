# Proposal

## Why

OpenSSH asks for passwords, key passphrases and host key confirmations through an askpass program. The local service collects these questions and offers them as `ssh.prompts.get` and `ssh.prompts.answer` (HTTP: `/api/remote/ssh-prompts`), which the web client polls. The native client does not use these operations: on Linux and macOS it reads the process-global question list of the service it embeds every 250 ms and answers into it directly, which works only because that service runs in the same process. On Windows the app gives the WSL backend its own executable as askpass program, so every question starts a separate app process with a window of its own. Two prompt routes, one of them past the protocol.

## What Changes

- A local service with a hub announces `ssh-prompts-v1` in `mux.welcome` on local main connections and sends `ssh.prompts.changed` on them when a question appears or goes, and once after the greeting while questions are pending. The notification carries neither question nor answer; the client reads the list with `ssh.prompts.get`. Host relays and remote-window connections get neither.
- The native client reads and answers the questions of its local service over the service's API stream, the same on every platform: embedded service on Linux and macOS, WSL backend on Windows. It reads on notification and does not poll; without the capability it polls `ssh.prompts.get` once a second and stops when the read is refused.
- The Windows app no longer gives the WSL backend an askpass program of its own: the backend asks through its own question broker, like every other service, and the app shows the question. The per-question app process and its window, `SPLITTY_ASKPASS_PROGRAM` and the WSL environment forwarding for it go.
- The native client core no longer re-exports the service's prompt functions; the only part of the SSH question broker it names is the helper entry of the embedded service, which the app executable runs when the embedded service starts it as OpenSSH's askpass program. The architecture check enforces that.

Out of scope: the question broker itself (credential reuse within a host connection, limits, timeouts), the web client's dialog, which keeps polling, and SSH questions of a remote window's own connection (the web shells).

## Capabilities

### New Capabilities

### Modified Capabilities
- `remote-hosts`: a new requirement for the SSH questions of the local service, their notification and how clients answer them.

## Impact

`splitty-remote` (question broker change feed; the direct askpass program route removed from the SSH command), `splitty-server` (session connection: capability and notification), `splitty-client-core` (question reader in the native workspace, re-exports removed, WSL launch without askpass path), `splitty-gpui` (question dialogs fed by the workspace, standalone askpass mode removed, WSL inspection without askpass path), `architecture/` (rule and relationships), `docs/protocol.ts`, `docs/native-ui-handoff.md`.

## Architecture impact

`client_session -> splitty.remote.ssh_askpass` added (change feed); `client_root -> splitty.remote.ssh_askpass` and `client_root -> splitty.remote.ssh` removed, `embedded -> splitty.remote.ssh_askpass` added (helper entry); `gpui_askpass -> client_root` and `gpui_main -> client_root` (helper registration) replaced by the use of the native workspace's question updates and the embedded service's helper entry; a new boundary rule: production code of `splitty_client_core` names `splitty_remote::askpass` only from `embedded`. No concept section of `docs/architecture.md` changes.

## Decisions

- **SPP-1 (provisional, tbd):** the Windows app shows the WSL backend's questions in its own window, read over the backend's connection, instead of handing the backend its executable as askpass program. The Linux side of that route is the embedded service's route and is tested; the app itself has not been run on Windows with it. Alternative: keep the per-question app process for WSL.
- **SPP-2 (provisional, tbd):** `ssh.prompts.changed` is a bare signal and the list is read with `ssh.prompts.get`, as `hub.changed` is followed by passive reads; questions stay out of the session stream. Alternative: the notification carries the list.
- **SPP-3 (provisional, tbd):** a native client whose local service lacks `ssh-prompts-v1` polls `ssh.prompts.get` once a second and stops at the first refused read; it does not hand the service an askpass program of its own. Alternative: show no questions there.
- **SPP-4 (provisional, tbd):** the web client keeps polling `GET /api/remote/ssh-prompts`; it may switch to the notification later.
