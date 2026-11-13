## ADDED Requirements

### Requirement: Review Texts And Anchors
A server that lists `viewer-reviews-v1` in `mux.welcome.capabilities` SHALL answer `viewer.anchor` with `viewer.anchored` and `viewer.review` with `viewer.reviewed`, on the same connection and to its sender only, or with `viewer.failed` under the same `requestId` (shapes in `docs/protocol.ts`). Both read the pane's annotations after the connection's earlier messages and its document as `viewer.read` does, change nothing, and name the document version they used.

#### Scenario: Anchors of a shown file
- **WHEN** a window sends `viewer.anchor` for viewer pane `v1`, which shows a file with a comment on line 3 and a general comment
- **THEN** it receives `viewer.anchored` with the file's version and, in the order of the pane's list, the general comment with the state `general` and the comment with the state `anchored`, its range in the source and lines 3 to 3

#### Scenario: Ranges in the source
- **WHEN** a pane's document starts with a byte order mark and has Windows line breaks
- **THEN** the ranges of `viewer.anchored` count UTF-8 bytes of the document without the byte order mark and with every line break as one line feed

#### Scenario: A review on request
- **WHEN** a window sends `viewer.review` for `v1` with the verdict `changes_requested`
- **THEN** it receives `viewer.reviewed` with the review's text, starting with the heading and `Verdict: changes requested`, the number of annotations it holds and the entries it names

#### Scenario: Passages of a rendered view
- **WHEN** a window sends `viewer.review` with the state and lines it found for an annotation taken from the rendered view, and for an annotation taken from the source
- **THEN** the review names the first with the window's lines and state and the second as the server anchored it

#### Scenario: A pane that is gone
- **WHEN** a window sends `viewer.review` for a pane the server does not have
- **THEN** it receives `viewer.failed` with the code `unknown_pane`, and nothing changes

#### Scenario: A malformed request
- **WHEN** a window sends `viewer.anchor` without a `paneId`, or `viewer.review` with an unknown verdict or a passage with a line of 0
- **THEN** it receives `viewer.failed` with the code `invalid`

#### Scenario: A passage of a removed annotation
- **WHEN** a window sends `viewer.review` with a passage of an annotation that another window removed just before
- **THEN** the server builds the review without that annotation and ignores the passage

#### Scenario: Through the hub
- **WHEN** a window shows a viewer pane of a host through the hub and asks for its review
- **THEN** the host builds the review, and the answer reaches the window over the same connection
