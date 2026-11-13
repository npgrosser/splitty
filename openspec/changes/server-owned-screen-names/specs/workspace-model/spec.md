# Spec Delta

## ADDED Requirements

### Requirement: First Screen of an Empty Workspace
A `create_screen` op with `onlyIfEmpty: true` SHALL be applied only to a workspace without screens, as any other `create_screen`. Applied to a workspace that holds at least one screen, it SHALL be rejected with a "workspace already has screens" error, leaving the workspace unchanged and starting no session. Ops are applied one after another, so of several such ops exactly one creates a screen.

#### Scenario: Empty workspace
- **WHEN** `create_screen` with `onlyIfEmpty: true`, `screenId` `s1` and no name is applied to a workspace without screens
- **THEN** the workspace holds screen `s1` named "Screen 1" with one pane and one session, and it is the active screen

#### Scenario: Two first screens
- **WHEN** two `create_screen` ops with `onlyIfEmpty: true` for `s1` and `s2` are applied to an empty workspace one after the other
- **THEN** the workspace holds only `s1`
- **AND** the op for `s2` is rejected and starts no session

#### Scenario: Without the flag
- **WHEN** `create_screen` without `onlyIfEmpty` is applied to a workspace that holds a screen
- **THEN** a second screen is created as before
