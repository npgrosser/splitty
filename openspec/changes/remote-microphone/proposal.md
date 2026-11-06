# Proposal

> Draft created on 2026-11-05. Not planned yet. Specs and tasks are written when this change is next. Builds on the done remote milestones (7 and 8). Related: `multi-attach` (several windows on one session: which one takes a request), `frontend-client-core` (microphone capture is a platform port of the client). Before this is next: check whether the then-current Claude Code records through something other than a `rec` on `PATH` on Linux (see design.md, Context).

## Why

Voice input in terminal tools records on the machine the tool runs on. Claude Code's `/voice` records through a local recorder (its own native module, `arecord` or SoX `rec`) and sends the audio to its own transcription service. For a pane on a host, that machine is the host: usually a server or VM without a microphone, so `/voice` fails there. In a local pane it works, on Windows through WSLg's PulseAudio.

The workaround today is to forward a local PulseAudio socket over a separate SSH connection and point the host's sessions at it. That needs a local PulseAudio (so not macOS, not Windows without WSL), SoX with its PulseAudio backend on the host and a second SSH connection kept open by hand. It also leaves the whole sound server open to every process of the user on the host for as long as the forward runs. Splitty already carries a host's sessions to the window. It can carry the window's microphone to the host as well: for the one pane that asks, and only while that pane records.

## What Changes

- **Microphone per host, opt-in**: a host setting, off by default, enables the microphone for that host's sessions. It holds for all screens, panes and connections to the host; nothing is set up per pane.
- **A recorder on the host, nothing installed**: Splitty's install directory on the host (`~/.local/share/splitty/remote/<version>/`) holds `rec` and `sox` as links to the installed `splitty` binary, shipped in the payload like the shell integration. On enabled hosts, that directory comes first on the `PATH` of Splitty's sessions. No package is installed and nothing is written outside the install directory; other shells and SSH logins on the host do not see it. Started as `rec`, the binary answers `rec --version` and `sox --version` without touching a microphone. Started for raw output to stdout (the form Claude Code uses: `rec -q --buffer 1024 -t raw -r 16000 -e signed -b 16 -c 1 -`, optionally with a `silence` effect), it asks the host's server for audio through the control socket. It writes the PCM it receives to stdout until it is stopped (SIGTERM) or the window ends the capture. Other invocations go to a real `sox` if one is installed and fail otherwise.
- **The focused pane gets the microphone**: the server sends the request to the windows attached to the session that announced microphone support. Only a window that has focus and shows that session in its focused pane takes it. Otherwise the recorder fails at once, and the tool reports that no audio is available. Switching between two agent panes needs no step, because the pane the user holds the key in is the one that records. A request from the newly focused pane takes the microphone over from the previous one instead of failing as busy, since Claude Code's focus mode stops the old pane's recorder and starts the new one's almost at the same moment.
- **Capture in the window**: the window opens the microphone only between the request and its end and converts to the requested format. It streams the audio over the existing connection (local server, hub, bridge). The pane shows a recording mark for exactly that time, and losing focus ends the capture. The input device is an app setting.
- **Desktop shells**: Electron allows audio capture for the app's own origin (today only notifications and sanitized clipboard writes). Tauri answers the webview's microphone permission request for the app on each platform.

Out of scope:
- Local panes: Claude Code records locally there already (WSLg on Windows, its native module on macOS and Linux).
- macOS hosts in the first stage: Claude Code records there through its native module first, which a `rec` on `PATH` does not reach.
- Audio output (playback), video and screen capture.
- Dictation or transcription by Splitty itself; the tool in the pane transcribes as before. OS dictation into the terminal (Windows voice typing, macOS dictation) works independently of this.
- Forwarding a sound server (PulseAudio, PipeWire) over SSH (see design.md, Considered alternatives).

## Capabilities

To be decided when the change is next. Likely candidates:

### New Capabilities
- `audio-capture`: the recorder's command line and behavior, the life of a request (request, take, stream, end, takeover), which window may take it, bounds.

### Modified Capabilities
- `control-api`: the endpoint the recorder streams from, its validation and outcomes; the route set of the owner control socket, and whether the agent socket gets it.
- `terminal-sessions` / `mux-protocol`: the capability string in `mux.welcome.capabilities`, the window's registration, the request, end and audio data messages.
- `remote-screens` / `desktop-remote`: the per-host setting and its persistence; relaying through the hub and to remote windows.
- `access-control`: a process on a host can make the local window open the microphone. The invariants bounding that: opt-in per host, only the focused pane of a focused window, a mark for the whole capture, nothing stored, bounded duration and rate.

## Impact

- **Host**: the recorder command (part of the `splitty` binary), a directory for it on the sessions' `PATH` on enabled hosts, the control endpoint that streams audio.
- **Server and hub**: routing a request to one window and the audio back, through the hub and the native bridge.
- **Frontend**: microphone capture and resampling, the recording mark, the host setting.
- **Desktop shells**: microphone permission in Electron and in Tauri on Windows (WebView2), macOS (WKWebView, usage description) and Linux (WebKitGTK).
- **Security**: a new direction of trust, since a process on the host triggers a capture device on the user's machine. That is equivalent to a local process with microphone access, bounded to the focused pane on an enabled host.
