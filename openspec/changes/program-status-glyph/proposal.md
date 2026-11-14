# Proposal

## Why

A program that learns from the probe that the terminal reads status reports stops putting its state into the window title, so the title's leading symbol goes. The native window then shows the state only as a small dot behind the tab title, nothing for an idle program, and nothing on the pane's row in the drawer. The drawer's pane list, where the title symbol used to tell the panes' states apart, no longer shows which program works, waits or is done.

## What Changes

- The native window shows a pane's program status as a glyph in the place of the title's leading icon, on the tab and on the pane's row in the drawer: animated while the program works, in the warning color while it is blocked, in the success or error color for an unseen `done` or `error` record.
- An idle record shows a calm, muted glyph there, but only when the title carries no leading icon of its own.
- The dot behind the tab title goes; the glyph replaces it.
- The glyph is monochrome and follows the record's state, not the program's name.

## Capabilities

### New Capabilities

### Modified Capabilities
- `program-status`: Status in the Window also covers the pane's row in the drawer and an idle record.

## Impact

`crates/splitty-gpui` (`workspace/program_status.rs`, `workspace/drawer.rs`, `workspace/screen_motion.rs`, the tab strip in `workspace.rs`) and its view tests. No protocol, server or web client change.

## Architecture impact

none

## Decisions

- A `working`, `blocked`, unseen `done` or `error` record takes the icon place also when the title has a leading icon of its own; the title's icon returns with an idle record (provisional, tbd).
- The web client keeps its present display; it only proves the protocol.
