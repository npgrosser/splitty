## MODIFIED Requirements

### Requirement: Native Theme Catalog
The native workspace SHALL resolve UI themes and terminal palettes in a catalog of its own, made of the themes bundled with the application and the files of the themes directory (`themes/` in the configuration directory of the computer the application runs on). A UI theme SHALL be a theme of a theme set in the component toolkit's JSON format (`*.json`: a `themes` list whose entries have `name`, `mode` and `colors`). A terminal palette SHALL be a file in Ghostty's theme format: `palette = N=#rrggbb` for the ANSI colors 0 to 15, `background`, `foreground`, `cursor-color`, `selection-background` and `selection-foreground`; other keys SHALL be ignored, and a file without `background` and `foreground` is not a palette. In the themes directory, `*.toml` files SHALL be ignored, and every other file that is not a theme set SHALL be read as a terminal palette named by its file name.

Settings SHALL store a theme by its identifier: the display name in lowercase with every run of characters other than ASCII letters and digits replaced by `-`, or the `id` of the theme's `splitty` object. A bundled theme SHALL keep its identifier against a file with the same one. The identifiers of the themes shipped before this catalog (`splitty-dark`, `splitty-light` and the others) SHALL keep selecting the same looks, as a UI theme and as a terminal palette of the same identifier.

A UI theme MAY carry a `splitty` object with the workspace chrome's own tokens: `surface`, `raised`, `overlay`, `text`, `text-secondary`, `text-muted`, `border`, `border-subtle`, `active`, `active-text`, `hover`, `menu-hover`, `accent`, `danger`, `danger-hover`, `success`, `warning`, `hover-overlay`, `shadow`, `drag-over-border`, `tab-height`, `tab-font-size`, `tab-accent`, `radius`, `control-radius`, `field`, `bevel`, `title-bar`, `font-size` and `font-family`. Every absent token SHALL be derived from the theme's toolkit colors. An unsupported value SHALL keep its derived value and be reported, without rejecting the theme or the other values of the same object.

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

### Requirement: Native Decoration Ownership
`appearance.windowDecoration` SHALL select `system` (default) or `splitty`, independently of title-bar colors. System decoration with system colors SHALL use native decoration where available and desktop-style client decoration otherwise. Splitty SHALL use application-owned geometry, which SHALL take the title bar's height, alignment, weight and window buttons and the frame's corner from the shown UI theme where the theme names them. While title-bar colors follow the theme, an application-drawn title bar SHALL take the theme's title-bar colors under either decoration, including those of an inactive window. Compositor-owned frames MAY retain their colors. All close controls SHALL follow the existing close policy.

#### Scenario: Client and server decoration
- **WHEN** the desktop supplies a title bar or requires the application to draw one
- **THEN** the native app shows only the supplied title bar or its own resolved title bar, respectively

#### Scenario: Close control
- **WHEN** the user activates a client title bar's close control
- **THEN** the window follows its existing workspace close policy

#### Scenario: Theme title bar under each decoration
- **WHEN** the shown UI theme names title-bar colors and a title-bar shape, and title-bar colors follow the theme
- **THEN** under system decoration the title bar shows the theme's colors in the desktop's geometry
- **AND** under Splitty decoration it shows the theme's colors, height, alignment and window buttons
