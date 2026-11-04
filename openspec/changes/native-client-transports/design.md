# Design

## Context

See [proposal.md](proposal.md) for scope. The relevant implementation currently has four different concerns intertwined:

- `splitty-tauri/src/lib.rs::start_embedded_server` embeds the service on a thread, binds `127.0.0.1:0`, and navigates the WebView to its HTTP origin. Browser credentials, the bound port and business state share `http::AppState`.
- `ws::handler`, `WsState` and `WsSender` own useful connection/session behavior but depend on Axum WebSocket types. `WsSender`, session subscriptions and the frontend's reconnect queue are unbounded. Merely adding another queue would make this worse.
- `remote::bridge` already carries framed text/binary messages through SSH stdio. `host::bridge` converts them back into a ticket-authenticated loopback WebSocket. Separate remote windows and `splitty connect` instead forward the daemon's TCP port and load its UI.
- `control` and the keeper layer already have Unix sockets with filesystem and peer-user checks. The owner control socket intentionally omits settings, host-management and credential endpoints; restricted grant sockets are narrower again.

Frontend `ConnSocket` and connection-scoped routing are useful boundaries. `apiFetch` is another integration point, but several remote modules call `fetch` directly. Desktop capability and navigation policies are tied to a backend port; downloads validate its blob origin. Electron presently packages the server executable but obtains the app assets from it. Windows startup parses text lines and waits for WSL localhost forwarding.

Planning accounts for the reverse-forward implementation on `origin/main` and the concurrent upload cancellation change. Apply on the combined baseline; do not replace either one's work with the older checkout.

## Goals / Non-Goals

**Goals:** remove implicit terminal-server listeners; give every connection a documented principal and lifetime; retain identical service semantics across transports; make every additional reachable endpoint justified and testable.

**Non-goals:** redesign mux operations, PTY ownership, keeper framing or agent grants; make native IPC a sandbox against processes of the same OS user; replace OpenSSH; tunnel arbitrary URLs or filesystem paths; introduce a second implementation of the application's API.

## Security boundaries

The security improvement is the removal of ambient terminal-service TCP listeners and host-supplied desktop code. It does not reduce the legitimate terminal owner's ability to execute commands on the assigned service. The following boundaries define what the implementation must prove:

| Threat / trust domain | Required boundary | Acceptance evidence |
| --- | --- | --- |
| Foreign web pages and unrelated loopback services | No native bridge; explicit browser gateways retain exact-origin, Fetch Metadata and credential checks | Browser guard ordering and native-origin negative tests |
| Other local OS users | Private endpoint paths plus bidirectional peer checks; no socket-to-TCP proxy | Filesystem/peer-identity tests on Linux/macOS |
| A hostile SSH peer or malformed host data | Fixed host/generation, bounded directional protocol, no local native actions, no executable host UI; no trust in remote cleanup claims | Host-ID injection, framing/admission and orphan-forward smoke tests |
| Separate remote window | Assigned remote service only; independent ephemeral storage and no opener, named-window or shared-JavaScript path to another trust domain | Actual WebView/Electron isolation smoke tests |
| Main window and its Dockview popouts | One trusted UI domain; parent owns connections and native actions, child gets no independent direct grant | Direct-call denial plus tests that acknowledge shared parent script authority |
| Same-UID processes or a compromised main UI domain | Not sandboxed from the owner's terminal/service authority | Documentation must not claim OS isolation or popout compromise containment |

Explicit browser access still uses bearer credentials over loopback. Gateway-specific cookie names prevent overwrites; they do not make cookies port-scoped or isolate them from every other HTTP service on the same hostname. Existing process-argument/bootstrap URL limitations remain. Resource quotas bound transport consumption; they are not a guarantee of uninterrupted availability against arbitrary owner processes.

## Decisions

### NCT-1: Separate the service from its adapters

Extract a server-side service/composition layer from the current HTTP/WS roots. The service owns workspace, dispatcher, jobs, settings, templates, notifications and the host hub. Browser state (cookies, tokens, tickets, origin and listener address) belongs to a browser adapter, not to the service. The real-time client loop accepts a bounded message sender/receiver and a transport-established caller context; it does not inspect TCP addresses to decide authority.

Keep the pure reducer and EffectRunner boundary unchanged. HTTP handlers and native requests call the same service operations. Session text and binary payloads retain their existing encoding, including the session-id-prefixed output frames. Scratchpad access, file/editor registrations, sizing, job ownership, spawn-before-nudge ordering and disconnect cleanup remain in one shared implementation.

Use a small platform-neutral `splitty-transport` crate for native envelope/framing, negotiation and bounded channel primitives, shared by server, remote and desktop crates. It depends on neither PTYs nor the server. This avoids making the Windows remote-only app link the Unix server, or placing app-service authority in the SSH library. Domain validation stays server-side.

Alternative: duplicate HTTP handlers as desktop commands. Rejected because validation, permissions and error behavior would drift. Alternative: a native HTTP proxy with arbitrary URLs. Rejected because it would expose unnecessary outbound and local-network authority.

### NCT-2: One native connection contract, appropriate carriers

| Use | UI assets | Connection | New listening endpoint |
| --- | --- | --- | --- |
| Tauri embedded, Linux/macOS | Packaged | WebView IPC -> native broker -> bounded in-process service connection | None for UI |
| Electron embedded, Linux | Packaged | Restricted preload IPC -> main -> native helper stdin/stdout -> embedded service | None for UI |
| Windows local, WSL 2 | Packaged | WebView IPC -> native broker -> `wsl.exe` stdin/stdout -> embedded service | None for UI |
| Local `splitty attach` | Terminal | Daemon client socket; direct keeper path when daemon absent | None additional |
| Main-window remote screens | Packaged | Native local service/hub -> system SSH exec -> remote bridge -> daemon client socket | None beyond host's private socket |
| Separate desktop remote window | Packaged | Host-bound IPC -> system SSH exec -> remote bridge -> daemon client socket | No local service or TCP listener |
| Browser, embedded | Local binary's assets | Explicit `splitty serve` HTTP/WS adapter -> embedded service | One loopback listener |
| Browser, existing daemon | Local binary's assets | Explicit `splitty serve --daemon` HTTP/WS adapter -> client socket | One loopback listener owned by command |
| Browser, remote | Local binary's assets | Explicit `splitty connect` HTTP/WS adapter -> SSH exec -> remote client socket | One local loopback listener, none on host |
| CLI control, grants, keepers | None | Existing sockets and restricted operation sets | Unchanged |
| Requested development forwards | External service | Existing SSH TCP forwarding and reverse-relay paths | Only the deliberately requested ports |

The native protocol is independent of mux protocol version. A framing/handshake version change does not automatically change session semantics. Native transport v1 carries a bounded handshake, request/reply messages with request IDs, real-time text/binary messages, cancellation, liveness and delivery credits. Host targets, principals and OS actions are not embedded in opaque terminal bytes.

Initial wire decision: retain a kind byte plus big-endian u32 payload length for stream carriers; add distinct transport-control and API-request/reply kinds rather than reinterpret terminal binary frames. A versioned magic/hello precedes them. The server announces its service instance, transport version and limits, and the client accepts a supported version. The first direct-service application event is still `mux.welcome`; an explicitly opened hub host stream retains its `remote.progress`/`remote.ready` prelude before the remote `mux.welcome`. A host stream is a distinct connection with a fixed target, not a way to retarget the existing local connection. Negotiation times out after 5 seconds, separately from the existing SSH installation/connect deadlines. Unexpected framing/version ends the connection before application dispatch. The precise enum and field declarations are part of the first implementation task in `docs/protocol.ts`, with golden cross-language fixtures.

A private owner endpoint also accepts a bounded Hello `openHost` intent containing the selected target, explicit update flag and expected local `ownerInstance`. The endpoint checks its immutable local instance before host preparation; direct and SSH remote carriers reject this intent before accepting the transport. This opens a separate fixed-host stream through the owning service's hub, preserving its forwarding state and quotas. It does not change the role or target of an existing stream. The resulting Ready identifies the verified remote instance and fixed host; host setup can resolve a supported previous mux version, whereas direct discovery continues to verify its exact reported version. In-process main brokers call the same hub with a validated target without an endpoint.


A launched helper establishes its framing handshake immediately, then may report bounded preparation/progress frames while WSL or SSH starts. This is not service admission. Once the backend is ready, the helper reports the verified service instance/limits and opens a fresh application generation. Each framing/service handshake has the 5-second deadline; the existing bounded backend preparation deadlines remain separate. API and session input cannot run during preparation. A helper reconnect revokes the previous service generation even when the inherited pipes stay open.

Transport v1's exact declarations and shared byte examples live in `docs/protocol.ts` and `docs/native-transport-v1.json`. Its eight-byte magic is `SPLITTY` followed by byte 1. Data frames prepend a nonzero u32 generation and append a random 16-byte delivery receipt after the application payload; controls have a separate 4096-byte ceiling. `ReadyAccept` confirms the advertised instance/generation before service admission. Credits consume a single unpredictable receipt, so guessed or duplicate acknowledgements cannot free buffers which have not reached a renderer. The receipt is last, preventing a renderer from acknowledging later IPC chunks after seeing only the first chunk. Request IDs increase within a generation and are never reused there. Reserve 64 KiB of each 32 MiB connection allowance and 8 MiB of the 128 MiB process allowance for controls; these reservations are included in the stated ceilings. IPC chunks are at most 64 KiB.

Native API calls name an enumerated operation with validated arguments, not an arbitrary URL, path, method or headers. Map the existing API once. Browser routes translate to these operations; native IPC has no generic `fetch`, `connect(path)`, `spawn(command)` or `open(url)` escape hatch. Intended terminal/job launches, input and file operations remain authority over the assigned backend and retain their existing validation; this is distinct from executing an arbitrary command in the local native broker. Operation identifiers and transport connection IDs are not proof of authorization.

Alternative: WebSocket-over-Unix-socket everywhere. It could work for native consumers, but retains HTTP upgrade/auth assumptions and would still require an IPC adapter for the WebView. Reusing application semantics without that extra protocol hop gives the embedded path no socket at all.

### NCT-3: Keep authority separate from transport

| Entry point | Authority | Forbidden additions |
| --- | --- | --- |
| Main packaged window | Its assigned local UI service; explicit host operations through that service | Raw network/filesystem/process access; grant administration |
| Separate remote window | Session stream and UI API of exactly its assigned host | Local service, another host, local editor/reveal commands, host management on the PC, reverse-forward requests |
| Dockview popout | Parent-owned pane view in the main window's shared trust domain | Independent direct native grant or service connection; no claim to contain compromised same-origin popout code |
| Setup window | Existing setup/known-host/open-host commands | Terminal-service access before native assignment |
| Native daemon client socket | Owner's UI API and session stream | Browser credentials; owner-only grant administration |
| Owner control socket | Existing control and agent-administration route set | Settings, host hub, UI stream or browser credentials |
| Agent socket | Live grant's own job operations | Owner/client/keeper fallback, raw sessions, UI or network credentials |
| Browser adapter | Authenticated UI/API/session calls to its fixed backend | Owner-only grant administration, arbitrary endpoint proxying |
| Keeper socket | Existing per-session keeper protocol | UI APIs, browser credentials, network listeners |

The daemon client socket is a separate endpoint, `client.sock` beside `daemon.sock`. Reusing `daemon.sock` would silently expand a deliberately narrow interface. Full UI access already belongs to the OS owner, who can currently read the token file; adding a private UI socket does not establish a new user-level sandbox. No client-socket path or credential is added to pane environments.

The native broker assigns opaque connection handles tied to window identity, current document generation, fixed backend, role and permitted operations. Verify these on each call and before delivering responses. Renderer-provided IDs, origin strings, `desktop=1`, `host=` and labels cannot grant authority. Navigation, reload, destruction, WSL distribution change or backend replacement revokes handles, pending requests and subscriptions; late results cannot reach a new document or backend. IPC transport commands and local OS actions remain separate allowlists.

A remote-window operation is evaluated against the assigned remote service, never against the local hub. Reverse-forward and local host-management operations are rejected for that role even if the remote response claims to be local. Separate remote/setup windows must not receive opener or shared-JavaScript references into the main/popout domain or other remote windows, including named-window reuse. Their native events and ports stay recipient-scoped. Existing connection-derived editor/file confirmations and host-ID isolation remain mandatory in the trusted main UI.

### NCT-4: Packaged desktop UI and bounded IPC

All desktop windows load the shipped frontend, including remote windows. A host supplies protocol data only. Tauri uses its bundled application origin and narrowly scoped commands plus per-connection bounded native output callbacks; Electron uses a dedicated secure application protocol, sandbox/context isolation, a small contextBridge surface and a MessagePort established only after validating the top-level sender. Package frontend assets into Electron instead of loading them from the child server. Never enable `file://` access or Node integration as a shortcut.

Tauri permissions are scoped to native-assigned roles and packaged origins, with command-side handle checks. Electron checks the actual sender frame and document lifecycle, not arguments supplied by JavaScript. A native challenge bound to the committed document accompanies private preload requests: main-frame navigation blocks new opens until commit and invalidates the previous challenge, including already-queued IPC when Electron reuses the frame object. Only the current document receives the new challenge. The actual MessagePort stays inside the isolated preload; application code can address only its assigned handles through a bounded byte API. A packaged document alone is insufficient: setup and remote contexts are isolated; Dockview popouts have no independent direct grants but share the main UI's trust domain. Avoid global app events for terminal data. No capability pattern or production CSP permits arbitrary loopback ports. Tauri's platform IPC scheme is an internal mechanism, not authorization to call network HTTP endpoints.

Preserve Dockview's parent-owned popout connection and reinitialization in `terminal_pane.ts`. Same-origin popouts with parent references share script authority: a call through the parent's bridge is seen natively as a parent call. Parent generation/backend replacement revokes its popout views and subscriptions along with the parent connection. Do not claim that denying a popout's direct IPC contains compromised popout code. Keep this shared domain explicit rather than redesign Dockview around cross-origin messaging. Isolated remote/setup windows must have no such references. This follows the [HTML origin model](https://html.spec.whatwg.org/multipage/browsers.html#origins).

Serve packaged assets from a build-produced allowlist of exact asset paths and intended app entry documents. The Electron custom-protocol handler must reject unexpected authorities, decoded traversal/separators, malformed encodings and unknown paths before resolving a resource; no generic filesystem fallback, remote fetch, directory listing or private package resource is served. Denying `file:` navigation alone is insufficient. Validate styles, fonts, WASM, zoom and clipboard behavior under each platform's actual application origin. Blob downloads remain same-window and user initiated; update origin checks and saved-download ownership without introducing renderer-chosen filesystem paths. Remote windows keep ephemeral storage and no local native side effects beyond their fixed transport.

Tauri's public `eval_with_callback` delivers bounded encoded chunks to a private document-scoped receiver. Each callback owns its source chunk, attempt and encoding reservation until actual evaluation completion or callback destruction; a separate consumption receipt releases writer credit. Start only after confirmed finished-page bootstrap, since Wry can queue scripts without their callbacks before the first commit. A renderer-side close retains its slot while normal or cleanup invokes are pending. This replaces Tauri Channels: the private unfetched Channel cache retained buffers across navigation during real reload tests. Electron uses [MessagePorts](https://www.electronjs.org/docs/latest/tutorial/message-ports). Electron explicitly calls for [sender checks and application protocols](https://www.electronjs.org/docs/latest/tutorial/security#17-validate-the-sender-of-all-ipc-messages). These mechanisms do not supply our application-level scopes or flow control; those remain broker responsibilities.

### NCT-5: Private endpoints and owned child pipes

A daemon must successfully bind its private client endpoint before detaching/reporting ready. The directory is 0700, sockets 0600; Linux and macOS verify peer identity in both directions and reject unreadable credentials. Validate the parent directory and endpoint with no symlink traversal, correct owner and filesystem support. Hold a daemon-instance lock; never unlink an endpoint of a live owner. Refuse non-sockets, unsafe owners, unsupported mount locations and overlong socket paths with a clear error. Do not fall back to TCP or a public temporary directory. Existing optional control-socket failure behavior remains separate from the mandatory daemon client endpoint.

`daemon.json` with `format:2` retains `pid`, `socket` (the control socket), `startedAt`, `version`, `protocol` and `keepsSessions`; adds `clientSocket`, `transportVersion` and an instance identifier; no required `port` and no credential. Match endpoint hello to the advertised instance before admitting requests. Read older metadata for diagnostics/migration, never treat a port or stale path as authority. The shared status operation returns `listenAddr:null`, `instance` and `transportVersion`; gateway addresses belong to gateway startup output, so status through different adapters still describes the same service. Keeper directories/protocol and stable CLI links stay unchanged.

`daemon status --json` labels a running legacy record with `legacy:true` and its known diagnostic fields; it does not fabricate native fields or interpret the old port as an attach instruction. A live daemon with damaged discovery is not silently replaced: the instance lock/endpoint remains authoritative for refusing a second start.

### Existing group-writable ancestors

Ordinary owned `0775` ancestors are supported without changing their permissions or consulting group membership databases. The final runtime/config directory remains owned by the effective user with mode `0700`; socket/lock/discovery/log ownership and modes, no-symlink traversal, local filesystem support and peer/instance authentication remain mandatory. Non-sticky world-writable ancestors remain refused. This replaces the blanket group-write rejection with directory-identity enforcement.

Walk directories using `openat` with no-follow directory handles and retain the final private directory. Lock opens, discovery publication/rename, log opens, socket metadata, permission changes and cleanup use that handle. A namespace change cannot redirect those mutations into a substituted directory. Revalidate the configured path against the retained directory at admission/publication boundaries; if it changed, fail instead of accepting a replacement. Group writers may still disrupt path availability; protecting authority does not promise availability against a party allowed to rename ancestors.

Unix socket operations must resolve relative to the retained directory too. Linux can use a held `/proc/self/fd` directory reference; macOS requires a thread-local directory operation isolated to a dedicated thread, without changing the process working directory. The launcher passes only its held instance-lock descriptor; the daemon checks its exact configured inode and binds its own listener before reporting readiness over the existing stdout pipe. This avoids relying on platform-specific bound-address spellings or consuming queued application clients during listener verification. Client peer credentials and the expected native instance remain independent checks. The existing run directory, keeper paths, discovery format and app-owned desktop lifetime do not change. Exercise actual legacy keepers and fresh setup with existing group-writable data and config ancestors. No deployed host is a mutation fixture.

### Confirmed legacy migration

An incompatible legacy host may advertise a bounded `legacyMigration` outcome in handshake format 2: `ready` with a conditional proof token and keeper-session count, or `blocked` with a reason. This is read-only discovery, not permission to replace the daemon. Ordinary connect/retry never migrates a legacy daemon. The trusted main/setup launcher or the existing hub update action consumes explicit confirmation once; remote documents keep their fixed backend authority and cannot select an executable, PID or migration proof. Renderer-visible failures carry an actionable offer or blocked reason, not the conditional token.

The installed replacement's `remote-migrate --expected <token>` revalidates older-version, matching payload, legacy status, ownership, unchanged discovery and keeper compatibility. The initial hardened implementation uses Linux pidfds plus the retained private owner socket's peer credentials and a fresh bounded daemon-status response, then signals only the pinned process generation. Read-only live session/keeper probes reject unsupported or unmatched state. Bounded descriptor inspection also refuses daemon-owned PTY masters absent from the workspace snapshot. These checks are preflight, not a freeze of concurrent clients; preservation is promised for the checked keeper-backed sessions. A different daemon publication observed after the pinned process exits aborts before normal startup; old clients that ignore the native instance lock cannot be serialized by this check. The proof is conditional on that observed daemon generation and record; it is not a new bearer credential or saved grant. Unsupported kernels/platforms, including legacy macOS without an equivalent process handle, return a specific blocked reason. Existing native instance-bound updates remain supported on their current platforms. No PID-only fallback, TCP connection, session-ending flag or forced-kill escalation is permitted.

Migration intent is native-owned and consumed before the possibly mutating SSH command, including direct remote-window helper/provider creation. Concurrent ordinary opens cannot absorb a confirmed migration into a focus operation. After migration a fresh handshake must identify the intended native replacement. A lost response, canceled request or discovery retry cannot replay migration; a later ordinary connection can discover the outcome or start a missing native daemon through the existing path. Keeper-managed PTYs remain independent; the UI does not promise preservation of an unsupported in-daemon fallback session. Tests use isolated hosts/processes and preserve an unrelated process; deployed hosts are not fixtures.

An embedded Tauri service needs no UI listener. Electron and WSL use a dedicated internal `splitty desktop-backend --transport native-v1` entry point over pipes inherited from the launcher. Its explicit `--backend embedded|ssh` selects an owned embedded service or the existing SSH connector (with a validated `--target` only for `ssh`). Electron and WSL use `embedded` for local work; Electron uses one `ssh` helper per remote window. Neither discovers or starts a local daemon. Thus Rust retains peer checks, installation, SSH and reconnect behavior instead of duplicating them in Electron or weakening Unix peer verification for Node. Native lifecycle/progress/error frames replace the old text bootstrap/JSON-connect output. Stdout contains framed protocol only; diagnostics go to bounded stderr. No shell, remote PTY, line decoder or UTF-16 conversion is applied to protocol bytes. WSL provisioning output may keep its existing text handling before the backend starts. EOF or launcher termination closes admission and shuts down owned embedded sessions; a helper attached to an SSH host only detaches. Bound shutdown waits and terminate only the child/bridge that this launcher owns, never the WSL distribution.

The embedded helper's service lifetime is the app lifetime, not a document's connection lifetime. Desktop launchers use `--owner` with `--backend embedded` to negotiate the distinct `owner` purpose over the inherited pipes. This private carrier opens up to 32 independent native substreams with increasing, never-reused IDs. Local streams share the owned runtime; host streams go through that runtime's Hub. Each inner stream negotiates its own direct/host identity and retains its own client registrations and credit ledger. Owner commands are at most 4096 UTF-8 bytes; binary chunks contain a four-byte stream ID and at most 64 KiB of inner bytes. The existing framed peer supplies bounded queues, byte accounting, receipts and liveness. Each substream waits for consumption of its outgoing chunk, and separately counted bounded duplex buffers prevent a stalled stream from blocking the owner reader. Substream cancellation uses reserved controls, including bounded cancellation-before-open tombstones; it cannot depend on free data slots. Closing/reloading a document closes its substream, while owner-pipe EOF ends the embedded runtime. Daemon/SSH endpoints and renderer ports never accept this owner purpose or its open commands.

### NCT-6: Explicit browser gateways

Keep `serve`'s foreground embedded behavior. Add `serve --daemon` to attach a browser adapter to an already running native daemon; it fails clearly if none exists and never creates a second workspace. `connect` owns a browser adapter to exactly its SSH target. In all three modes the adapter owns one explicitly requested loopback HTTP listener, application assets, process token, bootstrap/session/ticket store and the existing browser guards.

Browser authentication ends at the gateway. Host/Origin/Fetch Metadata checks run before authentication; every WebSocket still needs a one-use ticket. No native endpoint mints browser credentials and no browser cookie/token/ticket is forwarded to a daemon or host. The gateway chooses its backend from its command invocation, not a browser query, redirect or response. It translates only known API operations and binds WebSocket streams to their authenticated owner. Static assets are local, even for `connect`.

Keep an independent service client identity for every browser WebSocket. Each owns its attachments, resize contributions, file/editor registration and cleanup. A gateway must not collapse tabs into one upstream client or broadcast one tab's private response to the others. The fixed-backend connector opens one native service connection per WebSocket (and one SSH exec bridge for each remote connection); authenticated stateless HTTP operations use a bounded gateway-owned API connection with the existing trusted-user job actor. They do not inherit a WebSocket's attachments. Preserve cancellation/request ownership within that connection. Connection establishment is bounded and supervised; closing one tab does not close another tab or the gateway. There is no new upstream multiplexing protocol or transparent shared raw stream.

Use a gateway-instance-specific HttpOnly cookie name, `splitty-session-<instance>`, so simultaneous gateways on different ports cannot overwrite each other's cookie. Each accepts only its own sessions, bootstrap codes and tickets and keeps the current TTLs. `connect` uses an ephemeral process token and never reads a token file; `serve` retains its existing explicit/file/one-shot token options. Two `serve` gateways intentionally configured with the same owner token can accept that bearer token; this does not merge their browser session stores. Cookies are not port-scoped: default gateway URLs and `connect` use `127.0.0.1`, while forwarded development URLs use `localhost`. Supported browser listeners are canonical `127.0.0.1` or `::1`, including a validated localhost name; other resolved loopback addresses get explicit unsupported-address guidance. Origin/CSP checks derive only the chosen canonical address and localhost at the actual bound port, correctly bracketing IPv6. This closes the current mismatch between accepting an IPv6 bind and rejecting its browser origin, without a configurable or wildcard allowlist. An explicit alternate hostname must not be advertised as isolating cookies from other services on that hostname. Session and bootstrap URLs remain absent from logs.

Gateway port and session lifetime are independent of SSH/daemon reconnection. Keep the actual listening socket bound throughout retry; stop accepting application operations while its target is unavailable. A daemon restart produces a fresh backend connection and resync, not a new browser origin or an automatically replayed mutation. Closing `serve --daemon` or `connect` removes its listener and detaches, leaving daemon sessions alive. Closing ordinary `serve` also shuts down its owned embedded service.

`issue-bootstrap` targets an explicitly supplied browser address with the existing token mechanism. With no browser endpoint it gives actionable guidance to `serve --daemon`; it cannot start a listener or turn a native socket into a credential issuer. `--addr`, `--token` and `SPLITTY_ADDR` have no role in native daemon/desktop startup. Old daemon-start flags fail with migration guidance rather than enabling HTTP silently.

Alternative: forward a privileged Unix socket to localhost with SSH. Technically possible, but it would expose native owner authority to any local caller unless a complete authentication gateway were added anyway. The gateway is that boundary, not a transparent socket proxy.

### NCT-7: SSH stays an exec stream; forwarding stays explicit

Remote handshake format 2 reports a daemon's transport capability/instance instead of requiring a TCP port. `--ensure-daemon` starts a private-endpoint daemon without a client-selected port. A local client never connects to a socket path returned by a remote host: the host-side `remote-bridge --transport native-v1` resolves its own configuration and endpoint. Continue using the installed, checksum-verified server path and validated, correctly quoted constant commands.

The bridge is a native connection relay over SSH stdin/stdout; no HTTP, WebSocket ticket or token file is involved on the host. Preserve `Ssh::stdio_only` and its `ClearAllForwardings=yes`, `-a`, `-x` and `-T` options for every command/stream path, including installation and handshake: user-configured local/remote/dynamic forwards, agent/X11 forwarding or PTYs must not appear incidentally. System OpenSSH, host-key checks, noninteractive authentication and applicable user configuration remain. The main-window hub and separate remote clients share negotiation/framing, but retain their existing ownership and reconnect policies. Windows remote-only uses native system SSH with no WSL or local server dependency.

Do not request OpenSSH stream-local forwarding for terminal traffic: the exec bridge already works and avoids a second local endpoint and server forwarding-policy dependency. Explicit dev forwards still need normal forwarding permissions. `connect --forward` uses a separately supervised forward-only SSH process, since its daemon traffic no longer uses `-L`; preserve selected-port reservation, output and reconnect semantics. These deliberate forwarding processes retain configured-forward rejection and disable multiplex reuse, persistence and backgrounding so they own exactly the requested forwarding lifetime.

Reverse forwards keep the Linux host relay, UID checks, heartbeat cleanup and owner-only remote socket already merged on main. Extend the own-port exclusion into a per-user registry of active Splitty browser listeners and admitted reverse shares, shared across configurations. Acquire a private lock when reserving a gateway listener or admitting a reverse forward so a bind/share race cannot expose a newly started gateway. A gateway cannot take a port already shared by Splitty.

A reverse-share reservation must exist durably **before** any local worker can expose the destination. Record the local worker identity with process generation and network context before allowing forwarding; an incomplete spawn/publication transition is conservatively reserved. Stop and crash recovery release the port only after local forwarding workers are positively known to have terminated. A dead coordinator PID, an expired remote heartbeat, remote EOF or a host's assertion of cleanup is not proof: a hostile host can keep the SSH stream and its direct Unix forward alive. Unknown worker identity or failed cleanup keeps the reservation; a positively different immutable birth identity within the same verified machine/boot/PID view proves the saved generation ended, while UID changes alone do not; an ephemeral gateway chooses another provably unreserved port, while an explicitly requested port fails with a clear reason. An unreadable/corrupt registry fails admission closed rather than being treated as an empty registry. Recovery must never kill an unrelated process based only on a reused PID. No fixed timeout alone clears an ambiguous lease.

Test crashes before spawn, between spawn and worker publication, and after publication, with a hostile peer keeping the forward alive. If a platform cannot atomically publish a complete worker identity before spawn, persist the pending reservation first and retain it through ambiguous recovery. This deliberately favors refusing a port over exposing a gateway; normal Stop awaits local worker termination and releases capacity.

Scope reservations to the actual loopback/network context, not just a port number in one config directory. Windows/WSL tests must cover the selected runtime's reachable localhost listeners; where loopback is shared across runtimes, their reservation authority must also be shared through the existing native broker, or sharing must fail with a precise unsupported-environment error. Do not open a registry HTTP service to solve this. No client/control/keeper socket is forwardable through the UI API. The registry is not a defense against arbitrary forwarding tools run by the same OS user.

### NCT-8: Bound application-owned transport and define failure semantics

Initial byte limits (provisional, tested before cutover): 16 MiB maximum native frame, 1 MiB API request body unless an existing endpoint has a smaller cap, 64 pending API requests per connection, 32 MiB queued/in-flight bytes per connection and 128 MiB aggregate transport budget per process. Count frame reassembly and IPC delivery buffers; a frame cannot bypass the budget by being fragmented. Preserve existing image/file limits and chunked transfer protocols. Use bounded IPC chunks and delivery credits to keep application-produced output and its retained delivery reservations bounded. The contract covers application-owned transport buffers throughout the path, including raw chunks, encoding copies and native output reservations through evaluation completion or callback destruction.

Wry allocates inbound invoke bodies before the application handler, and generic framework replies and unrelated engine storage have separate ownership. These allocations and total process/renderer RSS are excluded from the numeric contract; no Wry fork is part of this change. This exclusion does not release any application-owned reservation early or relax frame/body/request/admission limits. A compromised packaged renderer may exhaust framework storage before validation; role isolation prevents authority transfer but does not guarantee availability of other windows sharing the native process. Remote protocol data alone does not grant renderer script execution.

Byte budgets do not bound idle descriptors, task state or subprocesses. Add admission ceilings, also provisional, acquired **before** allocating application-owned per-client state, dialing or spawning. They do not precede framework ingress allocation outside the handler:

| Resource | Initial ceiling |
| --- | --- |
| Accepted transport connections per process, including unauthenticated gateway HTTP and handshakes | 256 |
| Live application streams/native handles per process | 128 |
| Simultaneous connection-establishment/handshake attempts per process | 16 |
| Live streams per assigned backend | 64 |
| Live streams per native window/trust domain or authenticated browser session | 32 |
| Pending opens per native window/trust domain or authenticated browser session | 4 |
| Owned SSH and transport-helper workers per process, including setup and deliberate forwarding workers | 32 |

Parent-owned popouts share the parent's quota. Anonymous callers consume the process quotas before authentication; endpoint/role/backend identities come from trusted assignment, not caller-supplied quota keys. Reject excess authenticated work with an equivalent busy/503 result; refuse excess unauthenticated connections without upstream work. There is no unbounded waiter queue. Quota saturation must not starve already admitted clients' liveness, cancellation or shutdown. Release slots exactly once after cancellation, handshake timeout or stream cleanup; a worker slot remains held until exit is observed, not merely until a kill is requested.

Keep the 5-second framing/service handshake limits. Bound incomplete native frame assembly to 60 seconds and unauthenticated HTTP header/idle reads to 15 seconds; slow partial input cannot retain a slot indefinitely. Backend preparation uses its existing bounded phase deadlines and holds its admission slot while pending. Setup/diagnostic capture stays bounded as well as established streams. Counts and high-water bytes need test-visible diagnostics to prove actual resource bounds; these transport quotas do not claim to cap PTY/application state or sandbox owner-created jobs.

Give each connection a single ordered writer. Credits/control responses have bounded reserved capacity so a full data window cannot deadlock cancellation or liveness, without reordering application messages. A stalled or over-budget client is disconnected independently, with a resync/replay outcome; no partial terminal frame is silently dropped. PTY reading and keeper retention continue. Replace the existing unbounded session subscriber/sender path and per-event spawned queueing where necessary; bounding only the final IPC adapter is insufficient.

Native liveness uses connection control ping/pong with the existing 15 s / 30 s policy: check elapsed time since the last matched pong (or connection start) at 15-second ticks, rather than granting a fresh 30-second allowance to each ping; pipe EOF/process exit can report failure sooner. API requests have endpoint-appropriate deadlines (including existing long-poll and file-offer waits), request IDs scoped to a connection generation and explicit cancellation. Browser request aborts and native window closure cancel pending waits. A cancellation or lost response after a mutation began does not prove it never happened. Preserve existing job idempotency keys; do not invent automatic retry for terminal input, pane mutations, editor requests or uploads. Reconnect clears unsent input and stale RPCs, renegotiates the endpoint, then resyncs state and reattaches sessions.

Alternative: unlimited queues to hide latency. Rejected because a frozen WebView or disconnected host would exhaust memory. Arbitrary payload dropping is also rejected because it corrupts terminal state.

### NCT-9: Complete API and feature coverage before cutover

The native operation catalog covers the existing UI surfaces: home/settings and resolved theme/themes, shell profiles, templates, build/statistics, mux queries/ops, supported job APIs, session commands, and the hub's hosts/preferences/order/catalog/remembered screens, forward preferences/runtime and reverse-forward preferences/runtime. Enumerate every current method/path, role and body limit in a checked fixture; a new route is not automatically exposed by a wildcard. Authentication/bootstrap/ticket endpoints stay browser-only; agent administration stays owner-control-only. File offers and confirmed editor requests retain their existing admission rules.

Frontend transport selection is native-provided state, not URL parameters or global-variable presence alone. Keep `ConnSocket` as the message boundary and route every backend request through a connection-aware API adapter. Convert direct `fetch` calls in remote helpers. Return the same domain errors/status semantics through native replies so UI behavior does not fork. Bundle and resolve static assets locally; UI service responses cannot select script URLs, stylesheets, arbitrary file reads or native commands.

Acceptance covers shell profiles, settings save/reload, themes/fonts/keybindings, templates, notifications, titles/CWD, clipboard policy, file upload/cancel/paste/download/reveal, editor confirmations, pane moves/popouts, zoom, port discovery and forwarding. None may trigger a hidden fallback HTTP server.

### NCT-10: Local desktop sessions belong to the app

All desktop shells always start an owned embedded local service: in-process on Linux/macOS with Tauri, in an owned helper for Electron, and in the selected WSL distribution on Windows. Its sessions and jobs end when the app exits. Starting or closing the app must not discover, adopt, restart, stop, rewrite metadata for, or otherwise change an independent local daemon or its keepers. A live, stale, incompatible or corrupt daemon record does not change normal desktop startup. Each app instance owns only its own embedded service; popouts share that instance's service lifetime.

Persistent local sessions remain available explicitly through `splitty daemon`, `splitty attach` and `splitty serve --daemon`. No automatic desktop-daemon selector is introduced. Remote sessions retain the existing daemon/keeper lifetime. This deliberately replaces automatic daemon reuse on Linux/macOS and makes local behavior uniform across platforms. The prohibition on silently creating a replacement workspace after a daemon connection failure applies to explicit daemon/remote clients; an app-owned local workspace is the selected normal mode, not such a fallback.

Alternative: prefer a detected daemon. Rejected because an unrelated command or old keepers would change whether closing the same app ends its local sessions.

## Architecture impact

- Add `splitty-transport` in the Cargo workspace and `architecture/landscape.c4`, with a corresponding component map for its framing and bounded channel modules. It has no dependency on `splitty-core` or the server. `server`, `remote` and `desktop` depend on it.
- `architecture/server.c4`: introduce `service`, `client_session`, `native_client` and `browser_gateway` components. Move connection registry, mux/session dispatch and fan-out out of `ws`; retain `ws` as a WebSocket adapter. `http`/`control` call shared operations with distinct caller contexts; native client handling never imports credential-minting handlers. Hub relaying accepts the shared connection abstraction. CLI/host bridge use native transport; browser gateway alone owns browser authentication and a TCP listener. Add the shared internal-listener registry to the appropriate server/local-runtime component.
- `architecture/desktop.c4`: add `client_ipc`/broker with relationships to service (Unix builds), transport, remote connector and WSL bridge. Update `app`, `capability`, `navigation`, `remote`, `downloads` and `windows_wsl` away from loopback-origin authority. Windows remote-only retains no server/core dependency.
- `architecture/remote.c4`: `handshake` reports native endpoints; `bridge` uses shared framing; separate connection supervision from browser handoff/forward-only supervision currently combined in `tunnel`. Preserve `ssh`, `install`, reverse forwarding and host preferences. Prefer this refactoring over another connector implementation.
- `architecture/core.c4`: keep mux/effect-runner relationships; update daemon metadata and bounded session subscription dependencies. Keeper v1 stays unchanged. No transport or native-shell dependency enters the reducer.
- `architecture/landscape.c4` and `views.c4`: distinguish private service connections, explicit browser gateways and deliberate application-port forwarding; update Electron's asset and child-pipe relationships.
- `docs/architecture.md`: revise Overview, Two Protocol Paths, Connection Lifecycle, Client connections, CLI Flow, Session Output, Server Modes, Remote Hosts/Screens and desktop security. Add transport/authority tables and failure/backpressure semantics. Update `docs/protocol.ts`, README, CLI/remote/security docs and packaging/build instructions together at implementation time.

## Migration Plan

1. Land service extraction and conformance tests behind existing behavior. Prove that protocol and business behavior do not change before changing startup defaults.
2. Add native framing/client endpoint, private discovery and child stdio; then IPC/frontend adapters and packaged desktop assets on all supported platforms. Keep provisional adapters internal during development, not user-visible fallback endpoints.
3. Move SSH and explicit browser gateways, including CLI outputs and forward coordination. Complete the threat/feature/transport test matrix.
4. Perform one coordinated cutover. New native clients require transport v1 and handshake format 2. Read legacy metadata only to diagnose/update. Preserve the existing `connect` automatic update rule only for an older daemon with keepers and the matching installed payload; otherwise fail with explicit update guidance. Never stop a newer daemon, end old non-kept sessions, start a parallel workspace or use legacy HTTP to conceal incompatibility. Main-window updates keep their existing explicit intent path.
5. Remove old native-to-loopback paths, port-derived capability grants, text desktop bootstrap and token-file reads from native clients. Scan for remaining direct `fetch`, `new WebSocket`, `TcpListener::bind` and broad native permissions; justify every remaining occurrence.

`daemon.json` and the handshake have versioned formats. Workspace/state, settings, grants' semantics and keeper protocol stay readable. Default native startup ignores a stale `port` only after identifying legacy metadata; it never silently attaches to that port. `daemon restart` preserves the private endpoint and kept sessions instead of a TCP port. Existing explicit TCP CLI targeting continues against an explicitly started browser gateway.

Rollback is an explicit downgrade/restart using the previous release after clients are stopped, never a runtime transport fallback. Keepers survive where the existing version compatibility permits; do not promise old clients can parse new metadata. Recreate discovery metadata on restart. Browser URLs/cookies are ephemeral and renewed. No workspace migration is required by this change.

## Risks / Trade-offs

- IPC adds shell-specific code -> isolate it behind one frontend connection API and run the same request/session transcripts through every adapter.
- New packaged origins affect WASM, blob downloads and popouts -> exact asset-manifest serving and native-platform smoke checks; preserve Dockview's shared main trust domain and prove separate remote windows have no parent-reference path.
- WSL/stdout framing or launcher shutdown behaves differently on Windows -> byte-exact native Windows/WSL roundtrip, forced bridge death and distribution-switch tests, beyond Linux mocks.
- Trusted UI connected to hostile host data could become a confused deputy -> fixed backend handles, role checks, existing untrusted-data validation and explicit local-action confirmations.
- Backpressure/admission changes can stall process output or exhaust descriptors/workers -> bound the full subscription chain and admission before spawning, test control fairness under idle/partial-connection floods and measure queued bytes plus actual worker/FD counts.
- Browser gateway takes over lifecycle/authentication from the target -> independent session stores and cookie names, unchanged guard ordering, fixed-origin reconnect and cross-gateway denial tests.
- A reverse worker can outlive its coordinator and a hostile host can keep it alive -> durable pre-exposure reservations and locally verified worker death; ambiguous leases remain reserved instead of relying on remote heartbeat cleanup.
- Legacy and concurrent changes overlap -> reconcile reverse-forward and upload work first; preserve MCP's restricted surface.

## Provisional decisions

All dated 2026-11-01 and open to review:

- **NCT-1** (provisional, tbd): extract one shared service and a platform-neutral transport crate rather than duplicate endpoints.
- **NCT-2** (provisional, tbd): native framed connection contract with IPC/in-process, pipes or Unix socket carriers; HTTP/WS remains the explicit browser adapter.
- **NCT-3** (provisional, tbd): separate `client.sock` from unchanged control/grant/keeper sockets; window/backend-bound broker roles, with Dockview popouts explicitly sharing the main trust domain and remote/setup windows isolated from it.
- **NCT-4** (provisional, tbd): packaged UI for all desktop windows; bounded Tauri output callbacks and Electron MessagePorts; remote windows receive target-only transport authority.
- **NCT-5** (provisional, tbd): mandatory private daemon client endpoint; inherited pipes for Electron/WSL; discovery format 2 and explicit unsafe-path failures.
- **NCT-6** (provisional, tbd): `serve --daemon` and a local `connect` browser gateway; per-gateway auth/cookie identity; no implicit daemon HTTP or credential minting on sockets.
- **NCT-7** (provisional, tbd): retain hardened SSH exec bridging, split intentional forwarding supervision, and protect internal listeners with durable reservations that outlive coordinator crashes until local forwarder termination is verified.
- **NCT-8** (scope clarified 2026-11-03): bound application-owned transport bytes and connection/handshake/worker admission using the initial ceilings above; framework-owned ingress/generic replies, unrelated engine storage and total RSS are excluded. Retained IPC output stays charged through platform completion/disposal. No automatic mutation/input replay.
- **NCT-9** (provisional, tbd): full API/feature parity and one coordinated default cutover; explicit incompatibility instead of TCP fallback.

- **NCT-10** (provisional, tbd): local desktop backends are always app-owned embedded services on all platforms; independent local daemons and keepers are left untouched, with persistent access explicit through CLI/browser.

## Post-review implementation decisions

Final review corrections preserve strict native endpoint opens while permitting
explicit config preparation to tighten only an owned final directory with no
group/world write bits. The operation pins the directory before chmod, checks
its identity and does not change ancestors. Authorized daemon start and remote
ensure prepare it before discovery; a read-only probe does not repair it.

An install upload gets at most eight minutes with a sixty-second write-idle
deadline refreshed only by bytes accepted by the owned SSH stdin writer. After
stdin completes, setup output/process completion gets the ordinary two minutes.
The whole host preparation is bounded by ten minutes across native, browser and
Electron wrappers; progress messages cannot extend it. Framing/service handshake
deadlines remain five seconds. Cancellation still kills/reaps only owned workers
and retains admission until exit is observed.

The exposure registry retains its stricter ancestor rules. Descriptor-relative
operations alone cannot prove that a newly starting process sees an existing
registry after an ancestor rename, so group-writable registry paths remain a
documented unsupported condition rather than an unregistered gateway fallback.

The accepted corrections retain the original authority and memory boundaries.
Orderly native EOF/Closed drains accepted frames; revocation/protocol errors still
abort them immediately. Owner substream cleanup waits five seconds for its
acknowledgement before closing the physical carrier; the supervisor retains
capacity until helper exit is observed. Successful cleanup leaves other streams
and the owned runtime running.

Browser API calls share one bounded revision-bound carrier, with monotonically
increasing u32 IDs and at most 64 pending operations; mutations are never replayed.
Tauri bootstrap begin only solicits a current native navigation challenge. Its
caller-supplied nonce cannot reserve or replace a newer document challenge.
The token is delivered only by evaluation into the current packaged document,
never in the begin response. Confirmation must match its loaded native generation
and token. Navigation invalidates both; duplicate/older evaluated generations
cannot poison the new bootstrap, which retains its bounded deadline.

Exposure support remains deliberately restricted to verified common registry
authority. No unsupported-to-unregistered-gateway fallback, namespace shortcut or
filesystem allowlist expansion is introduced. Ambiguous spawn/publication leases
remain reserved. Cleanup overflow remains fail-closed rather than dropping claims.
The optional EOF evaluation remains bounded/accounted; request IDs remain consumed
on Busy, and negotiated frame minima protect header subtraction. No broad security
helper deduplication or unconditional Electron build dependency is introduced.
