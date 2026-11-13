# Spec Delta

## MODIFIED Requirements

### Requirement: Status Notifications in the Window
For an event marked `notify` of a pane the user is not watching, a window SHALL add a notification while `notifications.enabled` and `notifications.programStatus` are on, taken from the event's notice (see `mux-protocol`, Classified Notification Events): a warning for `blocked`, a success for `done`, an error for `error`. Its text SHALL name the screen and, on a host, the host, and SHALL show the record's `msg`, or else its `title`, as plain, shortened text. While a pane holds a root record, its OSC notifications and its shell integration's reports of a finished command SHALL add no notification of their own; a bell adds no notification.

#### Scenario: Blocked in an unwatched pane
- **WHEN** an event marked `notify` reports `blocked` with a message for a host pane the user is not watching
- **THEN** the window adds a warning that names the host and the screen and shows the message

#### Scenario: Watched pane
- **WHEN** an event marked `notify` arrives for the pane the user is watching
- **THEN** the window shows no notice for it and the pane gets no unread mark

#### Scenario: Switch off
- **WHEN** `notifications.programStatus` is off and an event marked `notify` arrives for an unwatched pane
- **THEN** the window adds no notification, and a native window's tab for the pane still shows the status

#### Scenario: Bell of a pane with a record
- **WHEN** a pane holds a `working` root record and its program rings the bell
- **THEN** the bell adds no notification

#### Scenario: Finished command of a pane with a record
- **WHEN** a pane holds a `working` root record and its shell integration reports that a command finished
- **THEN** the window adds no notification for it

#### Scenario: Web window
- **WHEN** a browser window shows a screen of a server with `notification-notices-v1` and a pane the user is not watching becomes `blocked`, marked `notify`
- **THEN** the browser window adds a warning notification as the native window does
