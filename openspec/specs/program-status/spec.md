# program-status Specification

## Purpose

Programs in a session report their own state through the Program Status Protocol (OSC 7501, revision 0.4: https://www.superlogical.com/rex/docs/build/program-status). This capability covers how the server reads those reports into records per session, answers the protocol's probe and keeps it out of the output, and how a native window shows the records and notifies about them. The event that carries records to clients is in `mux-protocol`.

## Requirements

### Requirement: Status Reports
The server SHALL read every report `OSC 7501 ; key=value:key=value`, ended by `ESC \` or BEL, in a session's output from the first byte it records, whether or not a connection streams the session. Pairs without `=` SHALL be skipped and unknown keys ignored; of a repeated key the last SHALL count. A report whose `state` is missing or is none of `idle`, `working`, `done`, `blocked`, `error` and `clear` SHALL be ignored.

#### Scenario: Report while nobody streams
- **WHEN** a program in a session that no connection streams writes `ESC ] 7501 ; state=working:app=claude-code BEL`
- **THEN** the session holds a root record with the state `working` and the app `claude-code`

#### Scenario: Report split between reads
- **WHEN** a report reaches the server in two reads of the terminal, split at any byte
- **THEN** it is read once, as if it had come in one read

#### Scenario: Unknown state
- **WHEN** a session holds a `working` root record and its program writes a report with `state=thinking`
- **THEN** the session still holds the `working` record

#### Scenario: Unknown key and malformed pair
- **WHEN** a program writes `ESC ] 7501 ; state=done:color=red:junk ESC \`
- **THEN** the session holds a `done` root record

### Requirement: Report Values
An `id` SHALL be segments of 1 to 32 characters from `A`–`Z`, `a`–`z`, `0`–`9`, `_`, `.`, `+` and `-`, joined by `/`, at most 128 bytes and 8 segments; a report with any other `id` SHALL be ignored, and a report without one addresses the root record. `kind` (`permission`, `question`, `auth`) SHALL count only with `blocked`, `progress` (an integer from 0 to 100) only with `working` and `blocked`, and `app` only as 1 to 32 such characters; any other value of these SHALL count as absent.

#### Scenario: Invalid id
- **WHEN** a program writes a report with `id=a//b`
- **THEN** the report is ignored

#### Scenario: Kind outside a block
- **WHEN** a program writes a report with `state=working:kind=permission`
- **THEN** the record has the state `working` and no kind

#### Scenario: Progress out of range
- **WHEN** a program writes a report with `state=working:progress=150`
- **THEN** the record has the state `working` and no progress

### Requirement: Report Limits
A report SHALL be dropped whole when the sequence from `ESC ]` to its terminator exceeds 4096 bytes, when its encoded `title` exceeds 256 bytes or decodes to more than 192, when its encoded `msg` exceeds 2732 bytes or decodes to more than 2048, when either is not base64 of UTF-8 text, or when either decoded text holds a character from U+0000 to U+001F, U+007F or one from U+0080 to U+009F. Encoded sizes SHALL be checked before decoding. Base64 SHALL be accepted with and without padding.

#### Scenario: Control character in a message
- **WHEN** a session holds a `working` root record and its program writes a `blocked` report whose `msg` decodes to text holding `ESC`
- **THEN** the report is dropped and the session still holds the `working` record

#### Scenario: Oversized report
- **WHEN** a program writes a report of 5000 bytes ended by BEL
- **THEN** the report is dropped and its BEL counts as no bell

#### Scenario: Unpadded base64
- **WHEN** a program writes a report whose `msg` is unpadded base64 of `Allow edit?`
- **THEN** the record's message is `Allow edit?`

### Requirement: Status Text Sanitizing
Before the server keeps a record, it SHALL remove from its `title` and `msg` the bidirectional controls U+061C, U+200E, U+200F, U+202A to U+202E and U+2066 to U+2069, the invisible characters U+200B, U+2060 and U+FEFF, and the separators U+2028 and U+2029. A text that is empty afterwards SHALL count as absent. Clients SHALL show both texts as plain text only.

#### Scenario: Bidirectional override in a message
- **WHEN** a program writes a report whose `msg` decodes to `ok` followed by U+202E and `txt.exe`
- **THEN** the record's message is `oktxt.exe`

### Requirement: Status Records
The server SHALL keep each session's records: a root record from reports without `id` and child records by `id`, each with its `state`, `kind`, `progress`, `app`, `title`, `msg` and the time of its last report. A report SHALL replace its record whole. `state=clear` with an `id` SHALL remove that record and every record whose `id` begins with that `id` and `/`; without an `id` it SHALL remove every record. A session SHALL hold at most 64 records; a report that adds one more SHALL first remove the child record reported least recently, never the root.

#### Scenario: A report replaces its record
- **WHEN** a program reports `blocked` with the kind `permission` and a message, and then reports `working`
- **THEN** the root record has the state `working`, no kind and no message

#### Scenario: Clear a subtree
- **WHEN** a session holds child records `a`, `a/b` and `ab` and its program writes `state=clear:id=a`
- **THEN** only `ab` and the root record remain

#### Scenario: Clear everything
- **WHEN** a session holds a root record and child records and its program writes `state=clear` without an `id`
- **THEN** the session holds no records

#### Scenario: Record limit
- **WHEN** a session holds a root record and 63 child records and its program reports a new child
- **THEN** the child reported least recently is removed, the new one is kept and the root record remains

### Requirement: Status Record Lifetime
`working`, `blocked` and `idle` records SHALL be removed at an OSC 133 `A` in the session's output, when the session of a pane stops being busy (see `terminal-sessions`, Session Metadata Updates), checked at least every 500 ms, and when the session's output ends; `done` and `error` records SHALL survive these. Changes SHALL apply in the order of the output. A full reset (`ESC c`) SHALL remove every record; a soft reset (`CSI ! p`), a switch between the normal and the alternate screen and a resize SHALL remove none. Closing or removing a session removes its records.

#### Scenario: Agent killed without clearing
- **WHEN** a program started from a pane's shell reports `blocked` and is killed, so the shell is the foreground process again
- **THEN** within a second the session holds no record

#### Scenario: Done survives the prompt
- **WHEN** a program reports `done`, exits, and the shell then marks its prompt with OSC 133 `A`
- **THEN** the session still holds the `done` record

#### Scenario: Report after a prompt mark in one read
- **WHEN** one read of the terminal holds OSC 133 `A` followed by a `working` report
- **THEN** the session holds the `working` record

#### Scenario: Full reset
- **WHEN** a session holds a `done` record and its program writes `ESC c`
- **THEN** the session holds no records

#### Scenario: Alternate screen
- **WHEN** a program reports `working` and then enters the alternate screen with `CSI ? 1049 h`
- **THEN** the session still holds the `working` record

### Requirement: Probe Answer
When a session's output holds the probe, exactly the bytes `ESC ] 7501 ; ?` followed by BEL or `ESC \`, the server SHALL write the answer `ESC ] 7501 ; ?`, ended by the probe's terminator, to the session's input, once per probe and also when no client is connected. The answer SHALL list no states or kinds, which by the protocol means all of them. At most 16 answers SHALL wait to be written per session; a probe beyond them SHALL get no answer. The answer is the only input the server writes because of the status protocol.

#### Scenario: Probe without a client
- **WHEN** a program in a session that no connection streams writes `ESC ] 7501 ; ? ESC \` followed by `CSI c`
- **THEN** it reads `ESC ] 7501 ; ? ESC \` from its terminal

#### Scenario: Probe ended by BEL
- **WHEN** a program writes `ESC ] 7501 ; ? BEL`
- **THEN** it reads `ESC ] 7501 ; ? BEL` from its terminal

#### Scenario: Two clients stream the session
- **WHEN** two connections stream a session and its program writes one probe
- **THEN** the program reads exactly one answer

#### Scenario: Probe split between reads
- **WHEN** a probe reaches the server in two reads of the terminal, split at any byte
- **THEN** the program reads exactly one answer

### Requirement: Probe Answer Order
The server SHALL queue a probe's answer before the output that carried the probe reaches any client, and SHALL write it ahead of any input a client sends after that, so a program reads the answer before a client's reply to the `CSI c` that follows the probe. An answer SHALL be written between two writes of input and never inside one (see `terminal-sessions`, Terminal Input).

#### Scenario: Client answers the device attributes request
- **WHEN** a program writes the probe and `CSI c` while a client streams its session, and the client sends its reply to `CSI c` as soon as it receives that output
- **THEN** the program reads the probe's answer before the client's reply

#### Scenario: Large input
- **WHEN** a client sends 64 KiB of input while the server answers a probe in the same session
- **THEN** the program reads the answer before or after those 64 KiB, not between two of their bytes

### Requirement: Probe Removed from the Output
The server SHALL remove every probe from a session's output before it records or forwards it: the scrollback buffer, the replay, the title, prompt, terminal mode and status tracking and every stream SHALL get the output without it. A probe split between reads SHALL be removed whole. Bytes that may begin a probe SHALL be held back at most until the next read or the end of the output, and bytes that turn out to be no probe SHALL pass on unchanged and in order.

#### Scenario: Probe not forwarded
- **WHEN** a client streams a session and its program writes `a`, the probe and `b`
- **THEN** the client receives `ab`, in one or more frames, and a later replay holds `ab`

#### Scenario: A beginning that is no probe
- **WHEN** a program writes `ESC ] 7501 ; state=done BEL` in two reads split after `ESC ] 7501 ;`
- **THEN** clients receive the whole sequence unchanged and the session holds a `done` record

#### Scenario: splitty attach
- **WHEN** a user's terminal shows a session through `splitty attach` and the session's program writes the probe
- **THEN** the user's terminal does not receive the probe and the program reads one answer, the server's

### Requirement: Output Read Again After Adoption
Output that a daemon receives from a session keeper and that the keeper had buffered before the daemon connected to it (see `session-keepers`) SHALL have its probes removed and SHALL rebuild the session's records, but SHALL get no probe answer and SHALL NOT mark an event to notify. Output the keeper buffers after the daemon connected SHALL be handled as live output.

#### Scenario: Restart after an answered probe
- **WHEN** a program wrote a probe and read its answer, and the daemon is restarted and adopts the session's keeper
- **THEN** the program reads no second answer

#### Scenario: Record rebuilt after a restart
- **WHEN** a program reported `blocked`, and the daemon is restarted and adopts the session's keeper
- **THEN** the adopted session holds the `blocked` record and a client that connects receives it after `mux.welcome`, not marked to notify

### Requirement: OSC Sequences Read to Their End
The server's output tracking SHALL read every OSC sequence, interpreted or not, from `ESC ]` to its end: BEL, `ESC \`, CAN, SUB, or an ESC that begins another sequence. A BEL that ends an OSC SHALL NOT count as a bell, also when the OSC's number or payload exceeds what the server reads. The byte 0x9d SHALL NOT begin an OSC, since in UTF-8 output it is part of a character.

#### Scenario: Unknown OSC ended by BEL
- **WHEN** a client streams a session and its program writes `ESC ] 21337 ; status=working BEL`
- **THEN** the client receives no `bell` event

#### Scenario: Notification OSC ended by BEL
- **WHEN** a client streams a session and its program writes `ESC ] 9 ; done BEL`
- **THEN** the client receives no `bell` event

#### Scenario: Bell after an OSC
- **WHEN** a client streams a session and its program writes `ESC ] 8 ; ; https://example.com BEL` and then BEL
- **THEN** the client receives a `bell` event

#### Scenario: UTF-8 text before a bell
- **WHEN** a client streams a session and its program writes `Ý` (the bytes C3 9D) followed by BEL
- **THEN** the client receives a `bell` event

### Requirement: Status Notifications
The server SHALL mark an event for a session's root record `notify` when the record is `blocked`, `done` or `error` and differs in state, kind or message from the record in the session's previous event, when it came from live output, and when the session's last event marked `notify` with that state is at least 10 seconds old. Events sent right after `mux.welcome` or again for a moved pane SHALL NOT be marked. A window decides whether a marked event notifies.

#### Scenario: Permission request
- **WHEN** a session's root record changes from `working` to `blocked` with the kind `permission`
- **THEN** the event for that change is marked `notify`

#### Scenario: Flapping state
- **WHEN** a session's root record changes between `working` and `blocked` every second for ten seconds
- **THEN** only the first change to `blocked` in those ten seconds is marked `notify`

#### Scenario: Progress while blocked
- **WHEN** a `blocked` root record is replaced by one that differs only in `progress`
- **THEN** the event for it is not marked `notify`

### Requirement: Status in the Window
A native window SHALL keep the root records each connection reports, scoped to that connection, and SHALL show on a pane's tab and on the pane's row in the drawer whether its program works, is blocked (with the kind), is done or failed. For an idle record both places SHALL show that the pane holds a program that reports its status, unless the pane's title starts with an icon of its own, which then stays. A screen and a host group SHALL show that a pane in them is blocked, also for a host's screens the window has not opened. A `done` or `error` mark SHALL go once the user has seen the pane and return only for a newer record; the pane then shows as an idle one does. A host that is not connected or does not announce `program-status-v1` shows no status.

#### Scenario: Blocked pane on a screen not opened
- **WHEN** a window is connected to a host, has not opened one of its screens, and a pane on that screen becomes `blocked`
- **THEN** the host group in the drawer shows that a pane is blocked, and the host's list of screens marks that screen

#### Scenario: Done seen
- **WHEN** a pane on another screen shows `done` and the user switches to that screen, where that pane has the focus
- **THEN** the mark goes and does not return when the connection is replaced and reports the same record again

#### Scenario: The same session id on two connections
- **WHEN** a host and the local server each report a record for a session with the same id
- **THEN** each record marks only the pane of its own connection

#### Scenario: Older host
- **WHEN** a host's `mux.welcome` does not list `program-status-v1`
- **THEN** its panes, screens and host group show no status

#### Scenario: Working pane in the drawer
- **WHEN** a pane holds a `working` root record
- **THEN** its tab and its row in the drawer both show that the program works

#### Scenario: Idle pane with a title icon
- **WHEN** a pane holds an `idle` root record and its title starts with an icon
- **THEN** its tab and its row in the drawer show the title's icon and no status mark

### Requirement: Status Notifications in the Window
For an event marked `notify` of a pane the user is not watching, a window SHALL add a notification while `notifications.enabled` and `notifications.programStatus` are on, taken from the event's notice (see `mux-protocol`, Classified Notification Events): a warning for `blocked`, a success for `done`, an error for `error`. Its text SHALL name the screen and, on a host, the host, and SHALL show the record's `msg`, or else its `title`, as plain, shortened text. While a pane holds a root record, its OSC notifications and its shell integration's reports of a finished command SHALL add no notification of their own; a bell adds no notification.

#### Scenario: Blocked in an unwatched pane
- **WHEN** an event marked `notify` reports `blocked` with a message for a host pane the user is not watching
- **THEN** the window adds a warning that names the host and the screen and shows the message

#### Scenario: Watched pane
- **WHEN** an event marked `notify` arrives for the pane the user is watching
- **THEN** the window shows no notice for it and the pane gets no unread mark

#### Scenario: Switch off
- **WHEN** `notifications.programStatus` is off and an event marked `notify` arrives for an unwatched pane
- **THEN** the window adds no notification, and a native window's tab for the pane still shows the status

#### Scenario: Bell of a pane with a record
- **WHEN** a pane holds a `working` root record and its program rings the bell
- **THEN** the bell adds no notification

#### Scenario: Finished command of a pane with a record
- **WHEN** a pane holds a `working` root record and its shell integration reports that a command finished
- **THEN** the window adds no notification for it

#### Scenario: Web window
- **WHEN** a browser window shows a screen of a server with `notification-notices-v1` and a pane the user is not watching becomes `blocked`, marked `notify`
- **THEN** the browser window adds a warning notification as the native window does
