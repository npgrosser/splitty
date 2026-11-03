## MODIFIED Requirements

### Requirement: Chunked File Uploads
The server SHALL accept `{"type":"upload","requestId":"<r>","offset":<n>,"data":"<base64>"}` with at most 1 MiB of decoded data per message, the first message of an upload (`offset` 0) also carrying `name` and `size`, the file's length in bytes, at most 512 MiB. It SHALL answer every message with `upload_progress` (`requestId`, `received`) while bytes are missing, with `upload_saved` (`requestId`, `path`, an absolute path) once `received` equals `size`, or with `upload_failed` (`requestId`, `error`). By default, each upload SHALL be stored as a new file with mode 0600, under the name of D4 of the design (the last component of `name`, characters other than letters, digits, `.`, `-` and `_` replaced by `_`, no leading dot, at most 120 bytes, else `file`), in a new directory `upload-<UTC date and time>-<random hex>` of the image store's directory (`splitty/paste/` of the cache directory, mode 0700). A message whose `offset` differs from the bytes received, that would exceed `size`, that is not a first message for an unknown `requestId`, or that would start a fifth concurrent upload of the connection SHALL fail the upload; a failed upload and every unfinished upload of a connection that closes SHALL be removed. Before it starts an upload, the server SHALL remove the `upload-` directories of that directory that are older than 24 hours, and then all but the newest 100.

Servers SHALL announce `upload-control-v1` for destination selection and cancellation. The first `upload` chunk MAY carry `dir`, an existing absolute directory. Such uploads SHALL retain the sanitized filename, use mode 0600, appear at the destination only when complete, and never replace an existing entry. The directory itself SHALL NOT be created or have its permissions changed. `upload_cancel` with `requestId` SHALL discard that connection's unfinished upload; already completed files SHALL remain. Cancellation SHALL be idempotent.

#### Scenario: Upload in two chunks
- **WHEN** a client uploads `notes.txt` of 1.5 MiB in two messages
- **THEN** the server answers the first with `upload_progress` and `received` 1 MiB, the second with `upload_saved` whose `path` ends in `/notes.txt` inside a new `upload-` directory, and that file holds the bytes and has mode 0600

#### Scenario: Name with spaces and a path
- **WHEN** the first message names `../My Report (final).pdf`
- **THEN** the file is stored as `My_Report__final_.pdf`

#### Scenario: Chunk out of order
- **WHEN** after a first chunk of 1 MiB the client sends a chunk with `offset` 0
- **THEN** the server answers `upload_failed`, and nothing of the upload remains

#### Scenario: Connection lost during an upload
- **WHEN** a connection closes while one of its uploads has received half of its file
- **THEN** that upload's directory is removed

#### Scenario: Durable upload
- **WHEN** a client uploads a file with an existing absolute `dir`
- **THEN** the completed file appears directly in that directory, outside temporary retention, and no partial destination file is visible

#### Scenario: Existing destination
- **WHEN** an entry already exists at the chosen destination filename
- **THEN** the upload fails without replacing that entry or leaving incomplete data

#### Scenario: Cancel unfinished upload
- **WHEN** a client sends `upload_cancel` for its unfinished upload
- **THEN** that upload's incomplete data is removed and other uploads and completed files remain

### Requirement: Files Into A Pane
When the user pastes into a pane and the clipboard data holds files and no text, the client SHALL send each image (PNG, JPEG, GIF, WebP) of at most 10 MiB with `save_image` and every other file, larger images included, with chunked `upload` messages to the server of that pane's connection, sending the next chunk only after the answer to the last, and SHALL paste the returned paths into that pane as one bracketed paste, in the order of the files and separated by spaces. Files over 512 MiB SHALL be refused before anything is sent. When the server answers `upload_failed`, or does not know `upload`, the client SHALL say so in a notice and paste nothing; for a server that does not know it, the notice SHALL say that it needs an update.

Dropped files SHALL wait for the user's choice before any bytes are sent. Temporary storage with path insertion SHALL be the default; an existing absolute directory and optional path insertion SHALL be the alternative. The client SHALL show the target server, files, total size and destination; it SHALL suggest only a known absolute session working directory. All confirmed dropped files, including images, SHALL use chunked uploads. The pane SHALL show aggregate byte and file progress with cancellation. Cancellation SHALL stop further chunks and files and request removal of the unfinished upload; completed files remain. Failed or cancelled batches SHALL insert no paths. Inserted paths SHALL be quoted when necessary to remain literal shell arguments. Older hosts without `upload-control-v1` SHALL receive no dropped file bytes and the user SHALL be told to update.

#### Scenario: PDF into Claude Code on a host
- **WHEN** the user drops `spec.pdf` and confirms temporary storage onto a pane of a remote screen of `vm1`
- **THEN** the file is stored on `vm1`, its path is pasted into that pane, and nothing is sent to any other server

#### Scenario: Host with an older Splitty
- **WHEN** the user confirms a dropped text file in a pane of a host whose server answers `upload` with "unknown message type"
- **THEN** a notice says the host needs an update, and nothing is pasted

#### Scenario: Cancel destination selection
- **WHEN** the user drops files and dismisses the choice
- **THEN** no upload or image message is sent and nothing is inserted

#### Scenario: Optional insertion
- **WHEN** the user uploads to a directory with insertion disabled
- **THEN** the file is stored there and no path is inserted

#### Scenario: Cancel a batch
- **WHEN** the user cancels while the second file is transferring
- **THEN** no third file starts, the second file's incomplete data is discarded, the first file remains, and nothing is inserted

### Requirement: Image Paste In A Pane
When the user pastes into a pane and the clipboard data holds an image file and no text, the client SHALL send each image with `save_image` to the server of that pane's connection and SHALL paste the returned path into that pane as a bracketed paste, several paths separated by spaces. Ctrl+V without other modifiers SHALL be pasted this way where the browser pastes with it (not on macOS); when the clipboard then holds neither an image nor another file, the client SHALL send `^V` to the terminal as before. Other files and all dropped images SHALL follow Files Into A Pane. A paste or drop without a file SHALL behave as before. When the server answers `image_failed`, or does not know `save_image`, the client SHALL say so in a notice and paste nothing; for a server that does not know it, the notice SHALL say that it needs an update.

#### Scenario: Screenshot into Claude Code on a host
- **WHEN** the clipboard holds a screenshot and the user presses Ctrl+V in a pane of a remote screen of `vm1`
- **THEN** the image is stored on `vm1`, its path is pasted into that pane as a bracketed paste, and nothing is sent to any other server

#### Scenario: Ctrl+V with text
- **WHEN** the clipboard holds text and the user presses Ctrl+V in a pane
- **THEN** the terminal receives `^V` and no `save_image` is sent

#### Scenario: Host with an older Splitty
- **WHEN** the user pastes an image into a pane of a host whose server answers `save_image` with "unknown message type"
- **THEN** a notice says the host needs an update, and nothing is pasted
