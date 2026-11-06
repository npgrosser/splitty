## MODIFIED Requirements

### Requirement: File Transfers Need The User
A client SHALL send `save_image` and `upload` only for the user's own drop or paste into a pane or a scratchpad, or for a review the user sends to a terminal pane as a file (see `viewer-annotations`), only to the server of that pane's connection, and SHALL paste or insert the returned paths only there. A server SHALL store an upload under its temporary directory by default, or under the existing absolute directory explicitly selected by the user. In both cases the server SHALL sanitize the filename; `name` SHALL NOT select a directory. Durable uploads SHALL never overwrite files or follow an existing destination symlink. A client SHALL read an offered file only after the user chose Save for it, only from the connection that offered it, and SHALL save it only as a download of the window under its sanitized name. File bytes SHALL be relayed by the hub like any other session message and SHALL NOT be passed to a shell.

#### Scenario: Upload name with a path
- **WHEN** an upload names `../../.ssh/authorized_keys`
- **THEN** the server stores it as `authorized_keys` inside a new `upload-` directory of its paste directory

#### Scenario: Offer without Save
- **WHEN** a host offers a file and the user does nothing
- **THEN** the window reads nothing and the offer expires

#### Scenario: A review sent as a file
- **WHEN** the user sends a review to a terminal pane as a file
- **THEN** the window uploads it to the server of that pane's connection under its temporary directory and inserts the returned path only in that pane
