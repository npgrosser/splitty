## ADDED Requirements

### Requirement: Native Viewer Panes
The native window SHALL show a viewer pane's document as "Showing A Document" says, with the source view scrolling sideways to long lines, and SHALL keep a shown file current as "Keeping A Shown Document Current" says. It SHALL offer "Open Selection in Viewer" for a terminal selection on a service that announced `viewer-v1`.

#### Scenario: A Markdown plan
- **WHEN** a viewer pane shows `PLAN.md`
- **THEN** the window shows the rendered Markdown and offers its source

#### Scenario: A failed read
- **WHEN** a read of a shown document fails
- **THEN** the pane says why in place of the content and offers to read again

### Requirement: Native Viewer Annotations
The native window SHALL list a viewer pane's annotations with their threads beside the document and let the user comment, suggest changes, reply, resolve, reopen, delete and move them, anchoring and marking passages as `viewer-annotations` says. It SHALL ask before a close ends annotations, hand the review to a terminal of the Screen pasted, as a file or into its scratchpad with `mark_sent`, and answer a waiting review with either verdict.

#### Scenario: Comment and hand over
- **WHEN** the user comments on a passage and sends the review to the terminal
- **THEN** the review is pasted there without being submitted, and the comment is shown as handed over

#### Scenario: A waiting review
- **WHEN** a process waits for a pane's review and the user requests changes
- **THEN** the process receives the review with `Verdict: changes requested`

#### Scenario: A reworded passage
- **WHEN** a commented passage is reworded in the file
- **THEN** the thread is anchored at the new wording and marked as changed
