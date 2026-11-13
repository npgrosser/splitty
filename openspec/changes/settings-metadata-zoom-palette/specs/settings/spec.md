## ADDED Requirements

### Requirement: Terminal Palette Choice
Every window SHALL choose the name of its terminal palette by one rule, the server's: the palette `terminal.theme` names; while that is `default`, the palette `terminal.themeDark` or `terminal.themeLight` names for the mode of the shown UI theme; while that is `default` too, the UI theme's own palette. An empty name SHALL count as `default`. `theme.get` and `GET /api/home/theme` SHALL apply this rule to the server's stored terminal settings for the UI theme they answer with, by that theme's mode: the answer's `terminal` colors SHALL be those of the server's theme with the chosen name, and its `terminalPalette` SHALL name the theme they come from; where the rule chooses the UI theme's own palette, or the server has no theme of the chosen name, they SHALL be the answered theme's own colors and `terminalPalette` its identifier. The web window SHALL show the `terminal` colors of that answer and SHALL ask for it again when `terminal.theme`, `terminal.themeDark` or `terminal.themeLight` changes. Each client SHALL look the chosen name up in its own themes, so a name only the other client has SHALL show the UI theme's palette; stored settings SHALL NOT change for it.

#### Scenario: Named palette in the web window
- **WHEN** `terminal.theme` is `splitty-light` and the web window shows the UI theme `splitty-dark`
- **THEN** the theme answer for `splitty-dark` carries the chrome colors of `splitty-dark`, the terminal colors of `splitty-light` and `terminalPalette` `splitty-light`, and the web window's terminals show them

#### Scenario: Palette by the UI theme's mode
- **WHEN** `terminal.theme` is `default`, `terminal.themeDark` is `splitty-retro` and `terminal.themeLight` is `splitty-light`, and the answered UI theme is dark
- **THEN** the answer carries the terminal colors of `splitty-retro`, and for a light UI theme those of `splitty-light`

#### Scenario: Palette the server does not have
- **WHEN** `terminal.theme` names a palette that only the native catalog has
- **THEN** the theme answer carries the UI theme's own terminal colors and its identifier as `terminalPalette`, and the setting keeps its value

#### Scenario: Palette change reaches open terminals
- **WHEN** another window changes `terminal.theme`
- **THEN** the web window asks for the theme again and its open terminals take the new colors without recreating sessions

## MODIFIED Requirements

### Requirement: Independent Native Terminal Palette
The native workspace SHALL choose the terminal palette by the rule of Terminal Palette Choice, with the mode of the UI theme it shows, and SHALL look the chosen name up in its own catalog; for the UI theme's own palette it SHALL take the palette with the UI theme's identifier, ignoring `-`, otherwise the default palette of that mode. The service SHALL store `terminal.themeDark` and `terminal.themeLight` with the default `default`. A named palette SHALL be used while the selected UI theme stays in use for chrome. Palette-only changes SHALL update existing and future terminals without recreating sessions, losing terminal history, or changing native terminal font metrics.

#### Scenario: Separate terminal choice
- **WHEN** the user chooses an available terminal palette different from the UI theme
- **THEN** terminal colors change and the workspace chrome retains its UI theme

#### Scenario: Palette pair by mode
- **WHEN** `terminal.theme` is `default`, `terminal.themeDark` and `terminal.themeLight` name palettes, and the shown UI theme changes between a dark and a light one
- **THEN** terminal colors use the palette named for the UI theme's mode

#### Scenario: Inherit current UI theme
- **WHEN** all three terminal palette settings are `default`
- **THEN** terminal colors use the palette with the UI theme's identifier, or the default palette of the UI theme's mode where no such palette exists

#### Scenario: Unavailable saved palette
- **WHEN** a saved terminal palette name is not in the catalog
- **THEN** the native workspace shows the palette it would show for `default`, reports the problem, and does not overwrite the saved setting

#### Scenario: Resolved palette identity
- **WHEN** a terminal palette is selected by name
- **THEN** the shown palette is the catalog entry with exactly that identifier
- **AND** another palette is shown in its place only together with a notice, without rewriting settings

#### Scenario: Service without the palette pair
- **WHEN** the local service does not publish `terminal.themeDark` and `terminal.themeLight`
- **THEN** both count as `default` and the editor does not offer to change them
