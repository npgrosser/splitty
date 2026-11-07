## ADDED Requirements

### Requirement: Byte-Preserving Terminal Input
A server announcing `terminal-input-bytes-v1` SHALL accept `input_bytes` with a session `id` and standard padded base64 `data`, decode at most 64 KiB and write those exact bytes using the same session authority, order and write-failure behavior as `input`. Existing `input` remains unchanged. Payload shapes are in `docs/protocol.ts`.

#### Scenario: Binary mouse report and text remain ordered
- **WHEN** an authorized client sends `input`, then `input_bytes` containing bytes invalid as UTF-8, then another `input` for an existing session
- **THEN** the process receives their exact bytes in that order without text conversion or interleaving within a request
- **AND** the connection need not stream that session

#### Scenario: Invalid or excessive encoded input
- **WHEN** `input_bytes` has invalid base64 or decodes to more than 64 KiB
- **THEN** the server sends an error for that session and writes no part of the request
- **AND** encoded length is bounded before decoding and decoded storage remains charged through writing

#### Scenario: Byte input for an unknown session
- **WHEN** valid `input_bytes` names no session
- **THEN** the server sends an `error` with that `sessionId` and `session not found`

### Requirement: Compatible Native Byte Input
A native client SHALL preserve terminal input bytes in one bounded request and send `input_bytes` only after its current connection announces `terminal-input-bytes-v1`. With an older server it SHALL send valid UTF-8 through `input`, reject non-UTF-8 input visibly and never perform lossy conversion. Attachment, generation, replay and no-retry rules SHALL apply equally to both forms.

#### Scenario: Older server
- **WHEN** the current connection lacks the capability
- **THEN** UTF-8 keyboard input remains usable and non-UTF-8 reports are refused without sending corrupt bytes

#### Scenario: Unattached or obsolete terminal
- **WHEN** raw input belongs to a terminal still attaching or to an obsolete pane/session generation
- **THEN** it is rejected rather than delivered to another terminal or replayed later
