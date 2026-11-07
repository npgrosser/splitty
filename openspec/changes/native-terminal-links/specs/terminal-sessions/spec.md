## ADDED Requirements

### Requirement: Deliberate Native Terminal Links
The native terminal SHALL open an absolute HTTP(S) target in the system browser only after a deliberate Ctrl/Cmd-left-click without Shift. Explicit OSC 8 targets SHALL take precedence over detected text, including when the explicit target is rejected. Output, hover and ordinary selection SHALL NOT open a target. Generic link activation SHALL NOT dispatch file, editor or other URI schemes.

#### Scenario: Explicit target differs from visible text
- **WHEN** a terminal cell has an allowed OSC 8 target different from its visible label
- **THEN** link feedback and deliberate activation use the actual target
- **AND** an invalid or disallowed explicit target does not fall back to a URL in its label

#### Scenario: Detected text in retained output
- **WHEN** a complete absolute HTTP(S) URL occurs in retained terminal text across a soft wrap, including Unicode or wide characters
- **THEN** deliberate activation opens that complete detected target
- **AND** hard line boundaries and adjacent punctuation do not become part of the target

### Requirement: Current And Bounded Native Link Activation
Native link resolution SHALL bound inspected output and target storage, reject malformed or incomplete targets and never open a truncated prefix. Activation SHALL validate the current target again at gesture completion. Dragging, release outside the captured cell, changed target, changed view geometry or session, and lifecycle cancellation SHALL prevent opening. An admitted link gesture SHALL remain local to the UI throughout its lifetime.

#### Scenario: Target changes during a gesture
- **WHEN** output changes the pressed target before release, or the view scrolls, resizes, hides or loses focus
- **THEN** release opens nothing and no part of the captured link gesture reaches the session

#### Scenario: Excessive or malformed target
- **WHEN** an explicit target is malformed or exceeds limits, or a detected logical line cannot be completely inspected within its limits
- **THEN** activation opens nothing, without substituting a shortened target
- **AND** the terminal remains usable

#### Scenario: Existing mouse and selection behavior
- **WHEN** the user performs an ordinary click or Shift selection while a terminal application requests mouse input
- **THEN** existing application mouse and local selection routes remain available
- **AND** deliberate link activation sends neither mouse bytes nor link text to the session
