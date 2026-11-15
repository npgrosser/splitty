## MODIFIED Requirements

### Requirement: Anchoring By Quote
Each annotation's passage SHALL be found in the document by the annotation's quote. The server SHALL find it in the document's source, for every annotation, on a window's request (see `mux-protocol`, Review Texts And Anchors); a window SHALL find an annotation taken from its rendered view in the text of that view while it shows the document rendered, and every other annotation where the server found it. Where the quote occurs more than once, the occurrence whose surrounding text matches the context best SHALL win, and among equal ones the one nearest the recorded lines. Where it does not occur, the search SHALL run again with runs of whitespace treated as one space. Where that fails as well, the passage the quote most likely became SHALL be looked for near the recorded lines, and the annotation anchored there, marked as changed, when that passage is similar enough to the quote; otherwise the annotation SHALL be marked as not found in the current version. An annotation whose passage changed or was not found SHALL keep its recorded quote and lines, and SHALL be anchored without a mark again when a later version contains the quote. The window SHALL anchor again on every new version of the document and when the user switches between the views. The user SHALL be able to move an annotation to the passage currently selected in the pane, which becomes its target. The server SHALL store annotations without interpreting their text.

#### Scenario: Lines inserted above the passage
- **WHEN** an agent inserts ten lines above an annotated passage of a shown file
- **THEN** the passage stays marked at its new place, and the review names its new lines

#### Scenario: The passage reworded
- **WHEN** an agent rewrites one sentence of an annotated passage and keeps most of its words
- **THEN** the annotation is anchored at the reworded passage, marked as changed, and keeps its recorded quote

#### Scenario: The passage removed
- **WHEN** an agent rewrites the file without the annotated passage or anything similar near it
- **THEN** the annotation stays in the list, marked as not found in the current version, with its quote and recorded lines

#### Scenario: Moved to another passage
- **WHEN** the user selects a passage and moves an annotation whose passage was not found to it
- **THEN** the annotation's target is the selected passage, and it is no longer marked as not found

#### Scenario: The same words twice
- **WHEN** the quote of an annotation occurs twice in the document and only one occurrence has the recorded context
- **THEN** the annotation is anchored at that occurrence

#### Scenario: A reflowed paragraph
- **WHEN** an agent rewraps the lines of an annotated paragraph without changing its words
- **THEN** the annotation is anchored at the rewrapped words, without a mark of a change

#### Scenario: The same result in every window
- **WHEN** two clients of one service show the same file with an annotation on a passage that an agent reworded
- **THEN** both mark the passage as changed at the same lines, and a review from either names the same lines and note

#### Scenario: A script without spaces
- **WHEN** a passage of Lao, Khmer, Myanmar, Thai, Chinese or Japanese text is reworded
- **THEN** the changed passage may start and end at any of its characters, not only at spaces
