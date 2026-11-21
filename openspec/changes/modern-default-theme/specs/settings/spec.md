## MODIFIED Requirements

### Requirement: Native Theme Catalog
The native workspace SHALL resolve UI themes and terminal palettes in a catalog of its own, made of the themes bundled with the application and the files of the themes directory (`themes/` in the configuration directory of the computer the application runs on). A UI theme SHALL be a theme of a theme set in the component toolkit's JSON format (`*.json`: a `themes` list whose entries have `name`, `mode` and `colors`). A terminal palette SHALL be a file in Ghostty's theme format: `palette = N=#rrggbb` for the ANSI colors 0 to 15, `background`, `foreground`, `cursor-color`, `selection-background` and `selection-foreground`; other keys SHALL be ignored, and a file without `background` and `foreground` is not a palette. In the themes directory, `*.toml` files SHALL be ignored, and every other file that is not a theme set SHALL be read as a terminal palette named by its file name. A comment line `# name: <text>` in a palette file SHALL give the palette that display name, while its identifier stays the one of its file name.

Settings SHALL store a theme by its identifier: the display name in lowercase with every run of characters other than ASCII letters and digits replaced by `-`, or the `id` of the theme's `splitty` object. A bundled theme SHALL keep its identifier against a file with the same one. The identifiers of the themes shipped before this catalog (`splitty-dark`, `splitty-light` and the others) SHALL keep selecting the same looks, as a UI theme and as a terminal palette of the same identifier. A bundled theme whose display name changes SHALL keep its identifier: `splitty-dark`, `splitty-light` and `splitty-light-gray` are shown as Splitty Modern, Splitty Modern Light and Splitty Modern Light Gray.

The default UI themes SHALL be `splitty-dark` (Splitty Modern) for a dark and for a fixed theme and `splitty-light` (Splitty Modern Light) for a light one: settings created without a choice name them, and they are shown in place of a stored theme the catalog does not have. Settings created without a choice SHALL select the fixed theme mode with `splitty-dark`, so that a first start shows Splitty Modern whatever the system's preference; `splitty-dark` and `splitty-light` SHALL be the pair they follow the system with once the mode is switched. Settings that already name a theme SHALL keep it.

A UI theme MAY carry a `splitty` object with the workspace chrome's own tokens: `surface`, `raised`, `overlay`, `text`, `text-secondary`, `text-muted`, `border`, `border-subtle`, `active`, `active-text`, `hover`, `menu-hover`, `accent`, `danger`, `danger-hover`, `success`, `warning`, `hover-overlay`, `shadow`, `drag-over-border`, `tab-height`, `tab-font-size`, `tab-accent`, `radius`, `control-radius`, `field`, `bevel`, `title-bar`, `font-size`, `font-family` and `terminal-font-family`. Every absent token SHALL be derived from the theme's toolkit colors. An unsupported value SHALL keep its derived value and be reported, without rejecting the theme or the other values of the same object. `terminal-font-family` SHALL be a family list the theme proposes for terminal text (see Native Terminal Font Choice), in which `monospace` and `ui-monospace` name the bundled terminal font.

The shape tokens SHALL apply to the whole interface, the component toolkit's controls included. `radius` SHALL be the corner of floating surfaces and SHALL bound the corner of every control, unless `control-radius` names that bound; marks that are round by meaning stay round. A theme with a `bevel` (`highlight`, `light`, `shadow`, `dark-shadow`) SHALL have its controls, fields, floating surfaces and separators drawn with 3D edges in place of one-color borders; a theme without one SHALL keep its one-color borders. `field` SHALL be the background of fields that hold text. Text on a selected entry and on a highlighted menu row SHALL be readable whether or not the theme names `active-text`. `title-bar` SHALL name the colors (`background` and `inactive-background`, each a color or a `from`/`to` gradient, `foreground`, `inactive-foreground`) and the shape (`height`, `align`, `weight`, `buttons`: `plain`, `boxed` or `wide`) of an application-drawn title bar (see Native Decoration Ownership).

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

#### Scenario: Theme with 3D edges and square corners
- **WHEN** the selected UI theme names a `bevel` and a `radius` of 0
- **THEN** controls, fields and floating surfaces of the workspace and of the toolkit's components show 3D edges and no rounded corner

#### Scenario: Selection color the theme's text does not read on
- **WHEN** a theme's `active` color has too little contrast to its text and the theme names no `active-text`
- **THEN** text on selected entries is shown in black or white, whichever reads better there

#### Scenario: Partly unsupported title bar
- **WHEN** a theme's `title-bar` holds a supported `foreground` and an unsupported `align`
- **THEN** the title bar takes the foreground, keeps its derived alignment, and the problem is reported

#### Scenario: First start
- **WHEN** the app starts with no stored settings, on a dark or on a light system preference
- **THEN** the theme mode is fixed and the UI theme is `splitty-dark`, listed as Splitty Modern, with the terminal palette of the same identifier

#### Scenario: Following the system without a chosen pair
- **WHEN** settings created without a choice switch the theme mode to follow the system
- **THEN** the UI theme is `splitty-dark` on a dark system preference and `splitty-light` on a light one

#### Scenario: Settings from before the default changed
- **WHEN** the stored settings name `splitty-classic`, the default of an earlier release
- **THEN** that theme is shown, with the terminal palette of the same identifier, and the current default does not replace it

#### Scenario: Palette with a display name
- **WHEN** a palette file named `Night` holds the line `# name: Night Owl`
- **THEN** the palette is offered as Night Owl and stored as `night`

#### Scenario: Windows 11 themes propose Cascadia
- **WHEN** Windows 11 Dark or Windows 11 Light is shown, `terminal.fontFamily` is empty and Cascadia Mono is not installed but Cascadia Code is
- **THEN** terminals use Cascadia Code, and the bundled font where neither is installed
