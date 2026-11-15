## ADDED Requirements

### Requirement: Terminal Clipboard Writes
A terminal of the desktop app SHALL copy the text of a program's OSC 52 clipboard write in live output to the system clipboard while `terminal.allowOsc52Write` is on, which is the default, for writes of at most 64 KiB of UTF-8; a larger write SHALL leave the clipboard unchanged. `terminal.allowOsc52Write=false` SHALL disable writes and survive settings save/reload; until a window has the setting, its terminals SHALL write nothing. A program's OSC 52 clipboard query SHALL never read the clipboard and SHALL not be answered with its content. Output replayed on attach SHALL never write the clipboard.

#### Scenario: Copy from a terminal application
- **WHEN** an application emits a nonempty OSC 52 write within the size limit and writes are enabled
- **THEN** the system clipboard holds the text

#### Scenario: Clipboard opt-out
- **WHEN** a user disables clipboard writes and reloads settings
- **THEN** OSC 52 writes remain disabled

#### Scenario: Clipboard read request
- **WHEN** a terminal program requests the clipboard via OSC 52
- **THEN** the terminal does not read the local clipboard and sends no clipboard content to the program

#### Scenario: Oversized write
- **WHEN** a write exceeds 64 KiB of UTF-8
- **THEN** the clipboard is not changed

#### Scenario: Replayed write
- **WHEN** a window attaches to a session whose buffered output holds an OSC 52 write
- **THEN** the clipboard is not changed

## REMOVED Requirements

### Requirement: Terminal Clipboard Policy
**Reason**: It described the web client's terminal (success and failure notices of the browser clipboard). The desktop app's terminals follow Terminal Clipboard Writes.
**Migration**: None.
