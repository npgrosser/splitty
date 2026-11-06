## MODIFIED Requirements

### Requirement: Review Answers
A client SHALL answer a wait for a review (see `viewer-annotations`, Waiting For A Review) with the message `viewer.review_answer`, carrying the `screenId`, the viewer pane's `paneId`, the wait's `waitId`, the `outcome`, `approved` or `changes_requested`, and the `review`, 1 byte to 1 MiB of UTF-8, which is optional with `approved` and required with `changes_requested`. The server SHALL settle the wait with the first answer whose `waitId` names it, end the wait, and answer every `viewer.review_answer` to its sender only with `viewer.review_answered`, carrying the `paneId`, the `waitId` and `accepted`, true only for the answer that settled the wait. An answer for a wait that has ended, for another pane, or that breaks these rules SHALL change nothing and be answered with `accepted` false.

#### Scenario: The first answer
- **WHEN** client A sends `viewer.review_answer` with outcome `changes_requested` for the wait of viewer pane `v1`, and then client B sends one with outcome `approved` for the same wait
- **THEN** A receives `viewer.review_answered` with `accepted` true, B receives it with `accepted` false, and the waiting process receives A's review

#### Scenario: An approval with notes
- **WHEN** a client answers the wait of `v1` with outcome `approved` and a review
- **THEN** the client receives `accepted` true, and the waiting process receives the approval with the review

#### Scenario: A request for changes without a review
- **WHEN** a client answers a wait with outcome `changes_requested` and no `review`
- **THEN** the client receives `accepted` false, and the wait goes on

#### Scenario: A stale answer
- **WHEN** a client answers a wait whose process has already stopped waiting
- **THEN** the client receives `viewer.review_answered` with `accepted` false, and nothing changes
