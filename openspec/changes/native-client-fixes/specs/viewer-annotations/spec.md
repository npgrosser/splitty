## MODIFIED Requirements

### Requirement: Handing A Review To A Terminal
A window SHALL offer to send a pane's review to a terminal pane of the same workspace: by default the viewer's origin while it exists as a terminal pane, otherwise one the user chooses among the terminal panes of the screen. Sending SHALL paste the review into the terminal as the user's own paste would, without submitting it, and focus that terminal. When the terminal's program has not enabled bracketed paste and the review has more than one line, the window SHALL instead store the review as a file and insert its path, and say so. The window SHALL also offer to store the review as a file on that terminal's server with `upload` and insert its quoted path, as a drop with path insertion does, and, when that server announced `scratchpad-v1` and the result stays within the scratchpad's limit, to append it to the terminal's scratchpad. Sending in any of these forms SHALL, once the review went out, mark the annotations and the user's replies the review held as handed over with `mark_sent`, naming each with the content the review held, and SHALL keep them; an entry added or changed in the meantime, in this window or another, SHALL stay pending. A review stored as a file SHALL go out only once the server stored it and its path was inserted; a store that fails, or a terminal that is gone by then, SHALL mark nothing, and the window SHALL say so; a store the user cancels SHALL mark nothing. Copying the review to the clipboard and saving it as a download of the window SHALL mark nothing.

#### Scenario: Send to the agent
- **WHEN** the user sends the review of a plan whose viewer was opened from the agent's terminal, and the agent's program has enabled bracketed paste
- **THEN** the review appears at the agent's prompt as a paste, nothing is submitted, the terminal has the focus, and the pane keeps its annotations, marked as handed over

#### Scenario: Edited meanwhile in another window
- **WHEN** window A sends a review with annotation #2, and window B saves a new remark for #2 before A's `mark_sent` arrives
- **THEN** #2 stays pending with B's remark, and the next review holds it

#### Scenario: Undo after sending
- **WHEN** the user has sent a review
- **THEN** the window offers no undo, since the pane keeps the annotations it sent

#### Scenario: Sent again
- **WHEN** the user sends the review of a pane whose open annotations have all been handed over
- **THEN** the review holds all open annotations

#### Scenario: The origin is gone
- **WHEN** the viewer's origin pane has been closed and the user sends the review
- **THEN** the window asks for a terminal pane of the screen and sends nothing before the user chooses one

#### Scenario: A program without bracketed paste
- **WHEN** the user sends a review of several lines to a terminal whose program has not enabled bracketed paste
- **THEN** the review is stored as a file on that terminal's server, its path is inserted at the prompt, and a notice says so

#### Scenario: Into the scratchpad
- **WHEN** the user sends the review to the agent's scratchpad
- **THEN** the review is appended to the scratchpad's text, its area opens, and the terminal receives nothing

#### Scenario: Copied
- **WHEN** the user copies the review as Markdown
- **THEN** the clipboard holds the review, and no annotation is marked as handed over

#### Scenario: Added while sending
- **WHEN** window B adds an annotation to the pane while window A sends the review without it
- **THEN** B's annotation stays pending

#### Scenario: The file is not stored
- **WHEN** the user sends a review as a file and the terminal's server answers `upload_failed`
- **THEN** no path is inserted, the annotations stay pending, and a notice says that the review was not sent
