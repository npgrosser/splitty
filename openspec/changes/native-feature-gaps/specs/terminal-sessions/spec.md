## ADDED Requirements

### Requirement: Native Images In Terminals
The native window SHALL draw the images a program sends to a terminal with the Kitty graphics protocol, at their placement, clipped to the terminal, below or above the text as their layer says. Image data SHALL arrive in the terminal's own stream only; the file and shared-memory media SHALL stay disabled.

#### Scenario: An image from a program
- **WHEN** a program transmits and displays an image in a terminal
- **THEN** the terminal shows it at the cursor's cells, and it leaves the screen when the program deletes it

### Requirement: Native Command Palette
The native window SHALL offer its commands in a palette opened by a shortcut, filtered by the words typed, each with its key combination, and SHALL list all shortcuts on request. Each destination of the Drawer's navigation (Hosts, Templates, Settings) SHALL have a shortcut that opens it from the Screens and from the other destinations and, while it is open, returns to the Screens; a consumed shortcut SHALL NOT reach a terminal.

#### Scenario: Filter and run
- **WHEN** the user opens the palette, types part of a command's name and presses Enter
- **THEN** that command runs as from its own control

#### Scenario: A destination's shortcut from another destination
- **WHEN** Settings is open and the user presses the shortcut of the Hosts manager, then presses it again
- **THEN** the Hosts manager opens, and the second press returns to the Screens
