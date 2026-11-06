# Proposal

> Draft created on 2026-11-05. Not planned yet. Specs and tasks are written when this change is next. Related: `remote-microphone` (it keeps audio output out of scope; a bell is the audio output terminal tools actually use), `terminal-intelligence` (other semantic events delivered through the same notifications).

## Why

Terminal tools signal "done" or "waiting for input" with the terminal bell (BEL, 0x07). Claude Code offers it as a notification channel (`terminal_bell`), next to the OSC notifications of iTerm2, kitty and Ghostty. Splitty's server already detects a bell in a session's output and sends a `bell` event to the connections streaming the session, local and remote alike. The window only logs it (`session_client.ts`: "the UI indicator is not designed yet"), so a bell is neither heard nor seen in Splitty. Handling it in the window makes it work the same for local panes and every host, with no audio sent over the connection.

## What Changes

- **A bell marks its pane**: the tab of a pane whose session rang shows a mark until the user looks at the pane, the way unread notifications already work (a pane the user is watching does not get one). A screen with such a pane shows it in the screen list and the host group.
- **Optional sound**: a setting plays a short sound in the window on a bell, at most once per pane within a short interval, and not for the pane the user is typing in.
- **Optional notification**: a setting turns a bell in a pane the user is not watching into a regular Splitty notification (drawer, toast, desktop notification), with the server's name for a host, through the existing notification path.

Out of scope:
- Audio from programs on a host (playback, speech output); see `remote-microphone`, Out of scope.
- The visual bell of the emulator (screen flash).

Related, to decide when this is next: OSC desktop notifications (`OSC 9` of iTerm2, `OSC 99` of kitty, `OSC 777` of rxvt/Ghostty), the other channels Claude Code can use. The server's OSC scanner ignores them today. Mapping them to `notification` events would carry a title and a message. `OSC 9;4` is ConEmu's progress report, which Claude Code also sends, and must not become a notification.

## Capabilities

### New Capabilities

### Modified Capabilities
- `terminal-sessions`: the `bell` session event (when the server sends it, coalescing of bursts), which no spec states yet.

## Impact

- Frontend: `session_client.ts` (the `bell` case), the tab and screen marks, the notifications module, settings (sound, notification).
- Server: none for the bell itself. Only for the OSC notifications, if included: the OSC scanner in `pty/osc.rs` and the notification events.

## Architecture impact

none

## Decisions

- **In the window, not on the host**: the sound and the mark come from the window that shows the pane, so hosts need no sound device and no audio crosses the connection.
