# Design

> Draft design notes from 2026-11-05; revisit against the specs and the then-current Claude Code before implementation.

## Context

See proposal.md for motivation and scope. Current state that shapes the approach:

- **How Claude Code records on Linux** (observed in version 2.1.318):
  - *Availability:* its native module if `/proc/asound/cards` lists a card; otherwise `arecord` if a 150 ms probe recording to `/dev/null` succeeds; otherwise `sox --version` and `rec --version` both succeeding.
  - *Recording:* the native module, `arecord -f S16_LE -r 16000 -c 1 -t raw -q -`, or `rec -q --buffer 1024 -t raw -r 16000 -e signed -b 16 -c 1 -` followed by `silence 1 0.1 3% 1 2.0 3%` unless silence detection is off. It reads stdout and stops the process with SIGTERM.
  - *On a host without a sound card,* the native module and `arecord` drop out, so a `rec` on `PATH` is what it uses.
  - *Settings and modes:* voice is a user setting (`voiceEnabled`), so it holds for all Claude Code sessions of the user on the host. The modes are hold (record while the key is held), tap, and a focus mode that records while the terminal has focus (DEC mode 1004).
  - *Possible own path:* the same module names `~/.cache/coder-audio/port` and `token` without using them in that version. If Claude Code gets its own way of receiving forwarded audio, this change should feed that instead of a `rec` on `PATH`.
- **Focus events reach host panes.** Both renderers send `ESC [ I` and `ESC [ O` when an application enables mode 1004: xterm.js always did, ghostty-web since a fix on 2026-11-05. Pane input goes to the session unfiltered, for host panes through the hub.
- **Precedents for a pane process asking the window.**
  - `POST /api/editor/open` goes to exactly one capable attached client, is bounded and expires, and detaching cancels it.
  - `POST /api/file/offer` with `file-offer-v1` goes to registered attached connections. The first `file.accept` takes it, the others get `file.closed`, the file is read in chunks, and the outcome comes back as the HTTP status.
  - Audio runs opposite to a file offer: the window writes and the pane process reads.
- **Upstream data.** Uploads send base64 chunks of at most 1 MiB in JSON messages; terminal output is the only binary frame type. 16 kHz mono S16LE is 32 KB/s.
- **Session environment.** The server sets `SPLITTY_SESSION_ID` and `SPLITTY_SOCK` for each session, and `SPLITTY_AGENT_SOCK` for scoped agent access. Shell integration has a directory of its own per shell.
- **Webview permissions.** Electron's permission handler allows only `notifications` and `clipboard-sanitized-write`, and only for the app's origin. Tauri has no microphone handling.

## Direction

1. **SoX's command line as the interface, not a sound server.** The recorder implements the narrow part of `rec` that tools use for raw capture to stdout: rate, encoding, bits, channels, `-q`, `--buffer`, and the `silence` effect for leading and trailing silence. On anything else it fails clearly, unless a real `sox` exists to hand the call to. Other tools that record through `rec` work as well.
2. **Nothing installed beyond Splitty itself.** The recorder is the `splitty` binary started under the name `rec` (or `sox`), through links in the version directory that the payload already brings. It is reached only through the `PATH` of Splitty's sessions on enabled hosts. Claude Code starts `rec` by name without a shell, so an alias or a shell function cannot stand in for it. If Claude Code's own forwarded-audio path (Context) becomes usable, it replaces the links.
3. **Consent per host, capture per request.** Enabling is a deliberate action in the trusted UI and is stored per host, like saved forwards. Each capture is one request bound to the session that asked; there is no standing audio stream.
4. **Only the focused pane of a focused window.** The window decides whether it takes a request: the session must be shown in its focused pane, and the window must have focus. Two windows on different machines may both qualify; then the first to take it wins, as with file offers, and the others hear nothing. Losing focus, of the pane or of the window, ends the capture.
5. **Takeover instead of busy.** One capture per window. A request from another session in the same window ends the running capture (its recorder sees the end of the stream and exits) and starts the new one.
6. **Visible for the whole capture.** The mark on the pane appears when the window opens the microphone and disappears when it closes it. The OS's own indicator comes on top of that.
7. **Nothing stored.** Audio passes through the server, hub and bridge and is never written to disk. Detach, disconnect and the end of the session end a capture.

## Considered alternatives

- **Forward the local sound server over Splitty's SSH.** A `-R` Unix-socket forward of the local PulseAudio or PipeWire socket, plus `PULSE_SERVER` for the host's sessions. Rejected, for four reasons:
  - It works only where a local PulseAudio exists (Linux, WSLg), not from macOS, Windows without WSL or a browser.
  - It needs SoX with its PulseAudio backend on the host.
  - It exposes the whole sound server (every source and playback) to every process of the user on the host for the connection's lifetime.
  - It goes against the hub's stance of opening SSH connections without forwards (`ClearAllForwardings`).
- **Splitty transcribes in the window and types the text.** That works for any tool and needs nothing on the host. But it replaces the tool's own transcription (vocabulary, language settings, push-to-talk) and moves speech-to-text into Splitty. OS dictation already covers that.
- **Provide `arecord` instead of `rec`.** Rejected: Claude Code probes `arecord` with a 150 ms recording on every availability check, which would open the microphone without the user recording. Only `rec` and `sox --version` are provided.

## Architecture impact

To be settled when the change is next. Likely:
- **splitty binary:** `server.cli` (the recorder command), `server.http` and `server.control` (the endpoint), `server.client_session` and `server.ws` (registration, request, end and data messages), `server.hub` and `server.host` (relaying to a host's windows).
- **splitty-core:** `core.pty` and `core.keeper` (the `PATH` entry in the session environment).
- **Desktop and Electron shell:** `desktop.navigation` or a new desktop component (the webview's permission request), and the Electron shell's permission policy.
- **Web frontend:** a capture module.
- **docs/architecture.md:** a section on audio capture next to file offers.

## Open Questions

- **Where the `PATH` entry comes from**: the server's session environment (every program sees it) or shell integration (only integrated shells). Sessions started before the host was enabled keep their `PATH`.
- **A real `rec` on the host**: if the host has a working microphone or a real SoX, which one wins. Either Splitty's entry only when the host has no capture device, or always once enabled.
- **Transport of audio**: base64 JSON chunks like uploads (simplest, about a third overhead on 32 KB/s) or a binary frame type from client to server.
- **Agent socket**: whether a process with only `SPLITTY_AGENT_SOCK` may request the microphone. The recorder normally runs inside the agent itself.
- **Bounds**: maximum duration of a capture, requests per second, and what the recorder does with a format it cannot produce from the window's audio.
- **Remote windows**: they have no local service, so the request has to reach them through the native bridge like other host events. Check the bridge's message set.
- **Platform details**:
  - WebView2's permission event, and whether the app may grant it without a prompt for its own origin.
  - WKWebView needs `NSMicrophoneUsageDescription` and the media-capture delegate.
  - WebKitGTK has its own permission request.
  - Browser clients need a secure context (localhost is one).
- **macOS hosts**: how Claude Code's native module behaves in an SSH session there, and whether a second path is worth it.
