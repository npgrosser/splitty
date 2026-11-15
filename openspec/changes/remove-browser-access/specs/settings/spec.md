## ADDED Requirements

### Requirement: Terminal Palette Rule
Every window SHALL choose the name of its terminal palette by one rule, the server's: the palette `terminal.theme` names; while that is `default`, the palette `terminal.themeDark` or `terminal.themeLight` names for the mode of the shown UI theme; while that is `default` too, the UI theme's own palette. An empty name SHALL count as `default`. `theme.get` and `GET /api/home/theme` SHALL apply this rule to the server's stored terminal settings for the UI theme they answer with, by that theme's mode: the answer's `terminal` colors SHALL be those of the server's theme with the chosen name, and its `terminalPalette` SHALL name the theme they come from; where the rule chooses the UI theme's own palette, or the server has no theme of the chosen name, they SHALL be the answered theme's own colors and `terminalPalette` its identifier. The native workspace SHALL look the chosen name up in its own catalog (see Native Theme Catalog), so a name only the server has SHALL show the UI theme's palette there; stored settings SHALL NOT change for it.

#### Scenario: Palette by the UI theme's mode
- **WHEN** `terminal.theme` is `default`, `terminal.themeDark` is `splitty-retro` and `terminal.themeLight` is `splitty-light`, and the answered UI theme is dark
- **THEN** the answer carries the terminal colors of `splitty-retro`, and for a light UI theme those of `splitty-light`

#### Scenario: Palette the server does not have
- **WHEN** `terminal.theme` names a palette that only the native catalog has
- **THEN** the theme answer carries the UI theme's own terminal colors and its identifier as `terminalPalette`, and the setting keeps its value

## MODIFIED Requirements

### Requirement: Settings Change Propagation
After a successful settings change, the server SHALL send `settings.changed` with the complete effective settings to every connected window, including the writer, over its native connection. A window SHALL apply that event, and SHALL restore its prior value after a failed update.

#### Scenario: Other window updates settings
- **WHEN** one window changes a setting
- **THEN** every connected window receives `settings.changed` with the saved settings

#### Scenario: Rejected optimistic update
- **WHEN** a window's settings update is rejected or its request fails
- **THEN** it restores its previous settings and exposes the failure

### Requirement: Independent Native Terminal Palette
The native workspace SHALL choose the terminal palette by Terminal Palette Rule, with the mode of the UI theme it shows, and SHALL look the chosen name up in its own catalog; for the UI theme's own palette it SHALL take the palette with the UI theme's identifier, ignoring `-`, otherwise the default palette of that mode. The service SHALL store `terminal.themeDark` and `terminal.themeLight` with the default `default`. A named palette SHALL be used while the selected UI theme stays in use for chrome. Palette-only changes SHALL update existing and future terminals without recreating sessions, losing terminal history, or changing native terminal font metrics.

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

### Requirement: Native Theme Catalog
The native workspace SHALL resolve UI themes and terminal palettes in a catalog of its own, made of the themes bundled with the application and the files of the themes directory (`themes/` in the configuration directory of the computer the application runs on). A UI theme SHALL be a theme of a theme set in the component toolkit's JSON format (`*.json`: a `themes` list whose entries have `name`, `mode` and `colors`). A terminal palette SHALL be a file in Ghostty's theme format: `palette = N=#rrggbb` for the ANSI colors 0 to 15, `background`, `foreground`, `cursor-color`, `selection-background` and `selection-foreground`; other keys SHALL be ignored, and a file without `background` and `foreground` is not a palette. In the themes directory, `*.toml` files SHALL be ignored, and every other file that is not a theme set SHALL be read as a terminal palette named by its file name.

Settings SHALL store a theme by its identifier: the display name in lowercase with every run of characters other than ASCII letters and digits replaced by `-`, or the `id` of the theme's `splitty` object. A bundled theme SHALL keep its identifier against a file with the same one. The identifiers of the themes shipped before this catalog (`splitty-dark`, `splitty-light` and the others) SHALL keep selecting the same looks, as a UI theme and as a terminal palette of the same identifier.

A UI theme MAY carry a `splitty` object with the workspace chrome's own tokens: `surface`, `raised`, `overlay`, `text`, `text-secondary`, `text-muted`, `border`, `border-subtle`, `active`, `hover`, `accent`, `danger`, `danger-hover`, `success`, `warning`, `hover-overlay`, `shadow`, `drag-over-border`, `tab-height`, `tab-font-size`, `tab-accent`, `radius`, `font-size` and `font-family`. Every absent token SHALL be derived from the theme's toolkit colors. An unsupported value SHALL keep its derived value and be reported, without rejecting the theme.

Third-party themes bundled with the application SHALL be limited to those whose license permits redistribution without further conditions, and their license notices SHALL ship in the repository next to them.

#### Scenario: Third-party theme without chrome tokens
- **WHEN** the selected UI theme is a toolkit theme without a `splitty` object
- **THEN** the application's components use the theme as it is and the workspace chrome uses colors derived from it

#### Scenario: Stored identifier from an earlier version
- **WHEN** the settings hold `splitty-dark` as the UI theme and `default` as the terminal palette
- **THEN** the chrome and the terminal show the colors that theme had before the catalog

#### Scenario: Files in the themes directory
- **WHEN** the themes directory holds a theme set, a Ghostty theme file, a `*.toml` file and a file that is neither a theme set nor a palette
- **THEN** the theme set's themes and the palette are offered with the bundled ones
- **AND** the `*.toml` file is ignored and the other file is reported as not loaded

#### Scenario: Unsupported chrome token
- **WHEN** a theme's `splitty` object holds a value outside the supported form or range
- **THEN** that token keeps its derived value, the theme is shown, and the problem is reported

## REMOVED Requirements

### Requirement: Terminal Palette Choice
**Reason**: Restated as Terminal Palette Rule without the web window, which is deleted; the server's rule and the theme answers are unchanged.
