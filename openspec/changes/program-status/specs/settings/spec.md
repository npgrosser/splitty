# Spec Delta

## ADDED Requirements

### Requirement: Program Status Notification Switch
A server's settings SHALL hold `notifications.programStatus`, a boolean that defaults to true and is listed in the settings metadata; settings stored without it SHALL read it as true, and a window SHALL read it as true from an older server that does not send it. A window SHALL add program status notifications (see `program-status`) only while both `notifications.enabled` and `notifications.programStatus` are on. The switch SHALL NOT change which status a window shows on tabs, screens and host groups.

#### Scenario: Settings stored before the switch existed
- **WHEN** a server starts with stored settings whose `notifications` lack `programStatus`
- **THEN** `GET /api/home/settings` returns `notifications.programStatus` as true

#### Scenario: Switch turned off
- **WHEN** a user turns off `notifications.programStatus` and a pane the user is not watching becomes `blocked`
- **THEN** the window adds no notification and the pane's tab shows that it is blocked
