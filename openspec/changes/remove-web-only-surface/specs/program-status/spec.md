## MODIFIED Requirements

### Requirement: Status Record Lifetime
`working`, `blocked` and `idle` records SHALL be removed at an OSC 133 `A` in the session's output, when the session of a pane stops being busy (see `mux-protocol`, Pane State From Session Snapshots), checked at least every 500 ms, and when the session's output ends; `done` and `error` records SHALL survive these. Changes SHALL apply in the order of the output. A full reset (`ESC c`) SHALL remove every record; a soft reset (`CSI ! p`), a switch between the normal and the alternate screen and a resize SHALL remove none. Closing or removing a session removes its records.

#### Scenario: Agent killed without clearing
- **WHEN** a program started from a pane's shell reports `blocked` and is killed, so the shell is the foreground process again
- **THEN** within a second the session holds no record

#### Scenario: Done survives the prompt
- **WHEN** a program reports `done`, exits, and the shell then marks its prompt with OSC 133 `A`
- **THEN** the session still holds the `done` record

#### Scenario: Report after a prompt mark in one read
- **WHEN** one read of the terminal holds OSC 133 `A` followed by a `working` report
- **THEN** the session holds the `working` record

#### Scenario: Full reset
- **WHEN** a session holds a `done` record and its program writes `ESC c`
- **THEN** the session holds no records

#### Scenario: Alternate screen
- **WHEN** a program reports `working` and then enters the alternate screen with `CSI ? 1049 h`
- **THEN** the session still holds the `working` record

