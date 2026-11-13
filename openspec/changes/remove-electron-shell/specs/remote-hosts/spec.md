## MODIFIED Requirements

### Requirement: Payload Sources
The client SHALL take the server it installs from the first available of these sources:
1. the archive named by the environment variable `SPLITTY_REMOTE_PAYLOAD`, a gzip-compressed tar archive with the server binary `splitty` and its `shell-integration` directory at the top level;
2. an archive of that layout built into the client for the host's kernel and architecture, with its checksum computed when the client was built. The Linux x86_64 and aarch64 archives and the macOS x86_64 and aarch64 archives are named at build time by `SPLITTY_REMOTE_PAYLOAD_X86_64`, `SPLITTY_REMOTE_PAYLOAD_AARCH64`, `SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64` and `SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64`. The released static server for x86_64 carries the other three, and the released Tauri apps and Linux packages carry all four;
3. the client itself, when it is a static Linux build for the host's architecture and the host runs Linux, together with the `shell-integration` directory next to it if there is one.

The checksum the host verifies SHALL be computed by the client over the exact bytes it sends. When no source fits the host, the client SHALL install nothing and run `splitty` from the host's `PATH` instead. When that is missing too, its error SHALL name both ways out: installing `splitty` on the host, or a client that carries a server for the host's architecture.

#### Scenario: Development payload
- **WHEN** `SPLITTY_REMOTE_PAYLOAD` names an archive built by `packaging/windows/build-wsl-payload.sh` and the host has no Splitty
- **THEN** `splitty connect` installs the server from that archive

#### Scenario: No payload and no splitty on the host
- **WHEN** the client has no payload for the host and the host has no `splitty` in its `PATH`
- **THEN** the client reports that `splitty` is not installed on the host and that a client with a server for the host's architecture would install it, and exits with status 1

#### Scenario: Desktop app on an aarch64 host
- **WHEN** a released Tauri app opens a remote window for an aarch64 host without Splitty
- **THEN** it installs the aarch64 server it carries

#### Scenario: A Linux server is not offered to a Mac
- **WHEN** a client carries only the Linux payloads and connects to a macOS arm64 host without Splitty
- **THEN** it uploads nothing and looks for `splitty` in the host's `PATH`
