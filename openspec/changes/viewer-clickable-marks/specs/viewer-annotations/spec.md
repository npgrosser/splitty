## MODIFIED Requirements

### Requirement: Annotations In The Pane
A window SHALL mark the anchored passage of each open annotation in the view the annotation was taken from, and its lines in the other view. It SHALL list a pane's annotations inside the pane with their numbers and threads: the open ones, the anchored ones in document order, then those not found, then the general comments in creation order; and after them the resolved ones, folded. It SHALL show which entries were handed over to an agent, and show each reply of an agent as plain text with the title of the pane it came from. A mark SHALL lead to its thread: choosing a marked passage, or the mark beside its lines, SHALL open the list when it is closed and choose that annotation in it, without moving the document; where marks overlap, the shortest passage SHALL come first, and choosing the same marks again SHALL go on to the others. Choosing a mark SHALL NOT take the place of selecting text or following a link. The user SHALL be able to hide the marks in the document, and to show the marks of resolved annotations, without changing the annotations; while the marks are hidden, the window SHALL mark only the passage of the annotation the user chose since. The window SHALL keep these choices per pane for its own lifetime. While a pane holds open annotations, its tab SHALL show a marker with their number.

#### Scenario: From the rendered view to the source
- **WHEN** the user annotates a passage in the rendered view of a Markdown file and switches to its source
- **THEN** the lines of that passage are marked in the source

#### Scenario: Marks hidden
- **WHEN** the user hides the marks of a pane
- **THEN** the document shows no marks, the list keeps the annotations, and other windows showing the pane keep their marks

#### Scenario: An annotation chosen while the marks are hidden
- **WHEN** the marks of a pane are hidden and the user chooses annotation #2 in the list
- **THEN** the passage of #2 is marked, and no other passage is

#### Scenario: A mark chosen
- **WHEN** the list of a pane is closed and the user clicks a marked passage of annotation #1
- **THEN** the list opens with #1 chosen, and the document stays where it is

#### Scenario: Overlapping passages
- **WHEN** the user clicks where the passage of #1 lies inside the longer passage of #2, and clicks there again
- **THEN** the first click chooses #1 and the second #2
