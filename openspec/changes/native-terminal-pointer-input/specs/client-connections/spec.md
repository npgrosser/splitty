## ADDED Requirements

### Requirement: Native Terminal Application Pointer Input
The native terminal SHALL honor application-requested mouse tracking and coordinate encoding for button presses, releases, motion and supported wheel reports. A gesture SHALL remain assigned to its original terminal. Without applicable mouse reporting, normal scrollback and alternate-screen cursor-key scrolling SHALL remain available.

#### Scenario: Requested mouse formats
- **WHEN** an application enables press-only, normal, button-motion or all-motion tracking and selects legacy, UTF-8, SGR, urxvt or SGR-pixel encoding
- **THEN** supported pointer events produce reports in that requested format with positions relative to the terminal content
- **AND** cell-based motion reports are deduplicated while pixel reporting retains pixel changes

#### Scenario: Wheel routing
- **WHEN** an application requests wheel-capable mouse tracking
- **THEN** wheel input produces mouse reports rather than substituted arrow keys
- **WHEN** wheel reporting is absent, including press-only tracking
- **THEN** wheel input retains local scrollback or alternate-screen cursor-key behavior

### Requirement: Native Pointer Local Override And Lifetime
Holding Shift at gesture start SHALL retain local selection despite application mouse tracking. That choice SHALL remain stable until release. Lifecycle changes SHALL clear local gestures; an active application gesture SHALL be settled on its original live terminal where possible, without retries after failed input admission or forwarding to a replacement terminal.

#### Scenario: Local selection under application tracking
- **WHEN** a Shift-started selection continues after Shift is released
- **THEN** it remains local, can be copied and sends no mouse reports
- **WHEN** Shift is pressed during an application-started gesture
- **THEN** its release still settles that application gesture

#### Scenario: Leaving or replacing a terminal
- **WHEN** a drag leaves the terminal, focus is lost, the terminal is hidden, reset or replaced, or the window deactivates
- **THEN** no pressed gesture remains stuck and subsequent input cannot leak to another pane
- **AND** a failed input admission is surfaced without replaying the report
