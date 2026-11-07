## ADDED Requirements

### Requirement: Native Local Terminal Search
The native client SHALL search literal text across the active terminal's retained screen and history, folding ASCII case and matching other bytes exactly. Soft line wraps SHALL NOT split searchable text. Search navigation SHALL wrap through matches without changing clipboard selection or sending query text, search keys or search-control pointer gestures to the session.

#### Scenario: Retained output and Unicode
- **WHEN** a query matches retained output across a soft wrap, including a wide or combining character
- **THEN** matching and highlights use the corresponding terminal cells, and navigation can reveal matches outside the current viewport
- **AND** ASCII case differences match while non-ASCII case differences are not folded

#### Scenario: Search input remains local
- **WHEN** the user edits, navigates or closes native terminal search while a terminal application requests mouse input
- **THEN** those interactions do not reach the session and existing clipboard selection remains independent

### Requirement: Native Search Progress And Limits
Native search SHALL distinguish incomplete indexing from completed results. Exceeding its query, match or storage limit SHALL discard results and report a visible limit state instead of presenting a partial search as complete. Only the focused visible pane SHALL retain an active index; query text SHALL remain local to its terminal across deactivation. Terminal reset SHALL invalidate old results.

#### Scenario: Broad query exceeds a limit
- **WHEN** a search exceeds a resource limit
- **THEN** it stops with a visible refine-query state, leaving the terminal usable
- **AND** output alone does not retry the failing query; changing the query can start a new full-history search

#### Scenario: Pane activation and new output
- **WHEN** a pane with an open search becomes inactive and later active again
- **THEN** its query remains available and results are rebuilt from current retained output
- **AND** counts and highlights from an old index or terminal generation are not reused

#### Scenario: Output and resize invalidate results
- **WHEN** output, a screen-buffer switch or a resize changes a searched terminal
- **THEN** search refreshes against the current terminal content before presenting updated results, retaining a valid navigation anchor where supported
- **AND** indexing progress is not reported as completed prematurely
