## ADDED Requirements

### Requirement: Native Images In Terminals
The native window SHALL draw the images a program sends to a terminal with the Kitty graphics protocol, at their placement, clipped to the terminal, below or above the text as their layer says. Image data SHALL arrive in the terminal's own stream only; the file and shared-memory media SHALL stay disabled.

#### Scenario: An image from a program
- **WHEN** a program transmits and displays an image in a terminal
- **THEN** the terminal shows it at the cursor's cells, and it leaves the screen when the program deletes it

### Requirement: Native Command Palette
The native window SHALL offer its commands in a palette opened by a shortcut, filtered by the words typed, each with its key combination, and SHALL list all shortcuts on request.

#### Scenario: Filter and run
- **WHEN** the user opens the palette, types part of a command's name and presses Enter
- **THEN** that command runs as from its own control
