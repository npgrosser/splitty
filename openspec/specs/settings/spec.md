# settings Specification

## Purpose

Settings belong to each server and govern its terminal, workspace, appearance, shell, editor, and notification behavior for all connected windows.

## Requirements

### Requirement: Server Settings and Metadata
The server SHALL return its current settings at `GET /api/home/settings` and defaults and field limits at `GET /api/home/settings/metadata`, including numeric bounds and steps and allowed values. Defaults MUST reflect that server's platform and profile detection. A window SHALL derive controls, reset values and the validation of an edit from this metadata when available, so that a limit the server widens or a choice it adds is offered and taken as it is; only without metadata SHALL it fall back to its own built-in limits.

#### Scenario: Server-specific defaults and limits
- **WHEN** a window loads settings from a server
- **THEN** it uses that server's defaults and limits for the settings controls and reset actions

#### Scenario: Older server without metadata
- **WHEN** the metadata endpoint is unavailable on an older server
- **THEN** the window retains the settings returned by that server and continues to support compatible updates within its built-in limits, without requiring metadata

#### Scenario: A server widens a limit
- **WHEN** a server's metadata allows font sizes up to 32 and lists an editor the window's built-in limits do not know
- **THEN** the window offers and sends a font size of 30 and that editor, and refuses 33

### Requirement: Settings Change Propagation
After a successful settings change, the server SHALL send `settings.changed` with the complete effective settings to every connected window, including the writer, over its native connection. A window SHALL apply that event, and SHALL restore its prior value after a failed update.

#### Scenario: Other window updates settings
- **WHEN** one window changes a setting
- **THEN** every connected window receives `settings.changed` with the saved settings

#### Scenario: Rejected optimistic update
- **WHEN** a window's settings update is rejected or its request fails
- **THEN** it restores its previous settings and exposes the failure

### Requirement: Removed Terminal Fields
`linkTarget` and `webviewMode` SHALL NOT be validated or returned as current terminal settings. Existing stored values SHALL be ignored when loading settings. An update naming them SHALL be refused like any unknown setting.

#### Scenario: Legacy stored fields
- **WHEN** a stored configuration contains `link_target` or `webview_mode`
- **THEN** the effective settings omit them and all other valid fields remain usable

#### Scenario: Legacy field in an update
- **WHEN** a client sends `PATCH /api/home/settings` with `{"terminal":{"linkTarget":"tab"}}`
- **THEN** it receives status 400 with the field `terminal.linkTarget` and no setting changes

### Requirement: Experimental Native Local Appearance
The experimental native workspace SHALL read local appearance settings through `home.get` and resolve the UI theme they name in its own theme catalog, applying appearance font overrides and the theme's colors to workspace chrome without writing settings during reads. It SHALL NOT request `theme.get` or `theme.list`. Terminal colors SHALL follow the selected native terminal palette, and terminal text the terminal font settings (see Native Terminal Font Choice). Unset chrome font overrides SHALL use the UI theme's defaults. Automatic mode SHALL select the saved light or dark UI theme using the local OS preference. A UI theme name the catalog does not have SHALL show the default theme of the OS preference and a notice, and SHALL NOT change the stored setting. Remote settings SHALL NOT select application appearance.

#### Scenario: Startup appearance
- **WHEN** the local appearance read succeeds
- **THEN** workspace chrome uses the supported appearance font overrides or the UI theme's defaults
- **AND** existing and newly created terminals use the selected native terminal palette without recreating sessions or losing terminal history
- **AND** loading appearance does not submit a settings mutation

#### Scenario: Chrome font changes preserve terminal typography
- **WHEN** appearance font size or family changes, including selection of a proportional UI font
- **THEN** workspace chrome uses the supported font choice while terminal text keeps the face and size the terminal font settings choose
- **AND** the change does not reinterpret appearance font fields as terminal font settings or replace the saved terminal palette choice

#### Scenario: Automatic local theme
- **WHEN** automatic theme mode is configured and the local OS color preference changes
- **THEN** the matching configured light or dark theme is shown
- **AND** a remote service's settings cannot override that choice

#### Scenario: Unavailable UI theme
- **WHEN** the settings name a UI theme the catalog does not have
- **THEN** the default theme of the OS preference is shown with a notice
- **AND** the stored setting is unchanged

### Requirement: Appearance Refresh and Failure Isolation
Native appearance refresh SHALL coalesce same-service `settings.changed` events, discard superseded results, preserve terminal event delivery and finish owned API work during shutdown. Read failures SHALL retain working defaults or the last valid appearance and expose a status while terminals remain usable. Unsupported presentation values SHALL use bounded native fallbacks.

#### Scenario: New settings during a refresh
- **WHEN** local settings change while an earlier appearance read is in flight
- **THEN** the superseded result is not applied and the latest refresh is eventually used
- **AND** terminal output continues through its existing stream

#### Scenario: Failed or unsupported appearance
- **WHEN** an appearance read fails or a requested font or color form is unsupported
- **THEN** terminals remain usable with the last valid appearance or native defaults and the failure is visible
- **AND** no corrective settings write is made

#### Scenario: Close during a read
- **WHEN** the owned workspace closes during an appearance request
- **THEN** outstanding appearance work is cancelled and its stream cleanup is awaited without leaking delivery allocations or altering another service

### Requirement: Experimental Native Local Shell Choice
The experimental native workspace SHALL offer the local service's enabled shell profiles from `shellProfiles.get` for explicit new-tab creation, disabling unavailable choices. Selection SHALL submit only `shellProfileId`; shell commands, arguments, working-directory inheritance and fallback remain service-owned. Ordinary tab and split actions SHALL retain service inheritance without explicit launch overrides.

#### Scenario: Explicit profile with inherited directory
- **WHEN** a user chooses an available profile for a new tab
- **THEN** `new_tab` carries that profile identifier and no explicit working directory
- **AND** the service uses its current known source directory and the selected profile's configured program and arguments

#### Scenario: Ordinary launch with stale client metadata
- **WHEN** the client has an older source directory or profile than the service and creates an ordinary tab or split
- **THEN** the service's current known launch defaults determine the new session

#### Scenario: Profile becomes unavailable before launch
- **WHEN** a selected profile becomes missing, disabled or unavailable before the service resolves the launch and the service uses its fallback
- **THEN** the native workspace visibly reports the effective profile and `shellProfileFallbackReason` for the admitted new pane
- **AND** the client neither reconstructs a command nor retries the launch

### Requirement: Native Profile Read Ownership and Isolation
Native profile reads SHALL belong to the local workspace connection, retain the latest valid result and refresh on same-service `settings.changed`. Superseded or foreign results MUST NOT replace current choices. Appearance and profile failures SHALL remain independent and visible while terminals remain usable. Refreshes SHALL coalesce without starving either read, and owned pending reads SHALL finish cleanup on shutdown without settings writes.

#### Scenario: Concurrent refresh and local preference change
- **WHEN** same-service settings change while a profile read is pending and the OS color preference changes repeatedly
- **THEN** superseded profiles are discarded and both requested settings reads eventually progress
- **AND** color preference alone does not reload profiles or consume terminal output

#### Scenario: Independent failure and recovery
- **WHEN** a profile read fails while appearance remains readable, or appearance fails while profiles remain readable
- **THEN** the successful feature updates independently, the failed feature retains its last valid state with a visible failure, and ordinary terminal actions remain usable
- **AND** a later same-service refresh can recover without an automatic retry loop or settings mutation

#### Scenario: Foreign result and shutdown
- **WHEN** a result belongs to another workspace connection, or the owned workspace closes during a read
- **THEN** foreign data is not applied and shutdown cancels pending reads and awaits owned stream cleanup without retaining delivery allocations

### Requirement: Native Local Drawer Preferences
The native local workspace SHALL restore Drawer pinning and supported width from the assigned local service's `drawer.pinned` and `drawer.sidebarWidth` settings through `home.get`. An explicit pin change SHALL update only `drawer.pinned` through `home.update`. Temporary edge reveal or dismissal SHALL NOT mutate settings. Loading preferences SHALL NOT write settings, and foreign or superseded reads SHALL NOT replace current presentation.

#### Scenario: Restore saved preferences
- **WHEN** the native workspace starts or receives a same-service settings change
- **THEN** the latest valid local Drawer pinning and bounded width are applied
- **AND** loading or applying those values performs no settings mutation

#### Scenario: Explicit visibility update
- **WHEN** the user changes Drawer pinning and the local service confirms the update
- **THEN** the effective value is retained by the service and can be restored by a later workspace
- **AND** unrelated settings remain unchanged

#### Scenario: Temporary unpinned reveal
- **WHEN** an unpinned Drawer is revealed from the window edge or dismissed
- **THEN** only transient window presentation changes, without a settings write
- **AND** revealing it preserves terminal sessions and the terminal workspace geometry

### Requirement: Native Drawer Mutation Outcomes
Native Drawer updates SHALL retain connection and request identity, admit at most one pending visibility update and never automatically repeat a mutation. Explicit rejection SHALL retain confirmed presentation and expose failure. An unavailable, uncertain or invalid mutation reply SHALL expose uncertainty and refresh current settings without claiming a persisted outcome. Shutdown SHALL join owned API cleanup.

#### Scenario: Rejection and uncertainty
- **WHEN** a Drawer update is rejected or its outcome cannot be confirmed
- **THEN** the window keeps its last confirmed presentation, reports rejection or uncertainty as appropriate, and resynchronizes settings
- **AND** it does not replay the write or apply a result from another connection or request

#### Scenario: Repeated toggle and close
- **WHEN** another toggle arrives while a Drawer update is pending or the window closes during that update
- **THEN** no overlapping or repeated mutation is submitted
- **AND** shutdown retains ownership until pending API cleanup is joined

### Requirement: Experimental Native Settings Editor
The experimental native workspace SHALL offer an editor for the local service's supported Appearance, Terminal, and Workspace preferences. It SHALL read effective values through `home.get` and defaults and limits through `home.metadata`, and SHALL offer the UI themes and terminal palettes of its theme catalog by display name, storing their identifiers. The catalog SHALL be read again when the editor opens. It SHALL submit only explicit supported changes through `home.update`; selecting a remote Screen SHALL NOT change the settings source.

#### Scenario: Local source while viewing remote content
- **WHEN** a remote Screen is selected and the native Settings editor opens
- **THEN** it shows the assigned local service's effective values and changes only that service's settings

#### Scenario: Supported controls and reset
- **WHEN** a user changes or resets UI theme mode or choice, UI font size or family, display zoom or scope, terminal palette, terminal font or font size, or close-on-exit policy, or Drawer visibility or width
- **THEN** the native client submits a bounded patch for that setting and displays the confirmed effective value
- **AND** reset uses the local service's published default when available

#### Scenario: Unavailable metadata or theme catalog
- **WHEN** settings metadata cannot be read, or a file of the themes directory cannot be loaded
- **THEN** already loaded effective settings remain visible, terminal use continues, the remaining themes stay offered and the file is reported
- **AND** controls that cannot establish valid choices or reset values do not submit guessed values

#### Scenario: Theme file added while the application runs
- **WHEN** a theme set or palette is placed in the themes directory and the Settings editor is opened afterwards
- **THEN** the editor offers it without a restart

### Requirement: Native Settings Mutation Reconciliation
Native settings mutations SHALL be identified with their local connection and request, admitted within a bounded queue, submitted at most once, and reconciled with the service's complete effective settings and `settings.changed`. A rejected, uncertain, or stale result SHALL NOT be shown as saved. The editor SHALL retain a visible error or uncertain state and refresh from the local service without replaying the write.

#### Scenario: Confirmed edit or same-service event
- **WHEN** this window's settings change is confirmed or `settings.changed` arrives on its assigned local service connection
- **THEN** the native editor and current workspace display the latest effective settings
- **AND** terminal output continues on its separate stream

#### Scenario: Rejection or uncertain completion
- **WHEN** `home.update` rejects a change or its outcome is uncertain
- **THEN** the editor retains the last confirmed value, identifies the field and failure, and requests a fresh local settings read
- **AND** the mutation is not repeated automatically

#### Scenario: Old result and close
- **WHEN** a result belongs to an older local connection or an older settings generation, or the window closes during an update
- **THEN** the result cannot replace current presentation
- **AND** owned pending work is cancelled and joined without changing another service

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
The native workspace SHALL resolve UI themes and terminal palettes in a catalog of its own, made of the themes bundled with the application and the files of the themes directory (`themes/` in the configuration directory of the computer the application runs on). A UI theme SHALL be a theme of a theme set in the component toolkit's JSON format (`*.json`: a `themes` list whose entries have `name`, `mode` and `colors`). A terminal palette SHALL be a file in Ghostty's theme format: `palette = N=#rrggbb` for the ANSI colors 0 to 15, `background`, `foreground`, `cursor-color`, `selection-background` and `selection-foreground`; other keys SHALL be ignored, and a file without `background` and `foreground` is not a palette. In the themes directory, `*.toml` files SHALL be ignored, and every other file that is not a theme set SHALL be read as a terminal palette named by its file name. A comment line `# name: <text>` in a palette file SHALL give the palette that display name, while its identifier stays the one of its file name.

Settings SHALL store a theme by its identifier: the display name in lowercase with every run of characters other than ASCII letters and digits replaced by `-`, or the `id` of the theme's `splitty` object. A bundled theme SHALL keep its identifier against a file with the same one. The identifiers of the themes shipped before this catalog (`splitty-dark`, `splitty-light` and the others) SHALL keep selecting the same looks, as a UI theme and as a terminal palette of the same identifier. A bundled theme whose display name changes SHALL keep its identifier: `splitty-dark`, `splitty-light` and `splitty-light-gray` are shown as Splitty Modern, Splitty Modern Light and Splitty Modern Light Gray.

The default UI themes SHALL be `splitty-classic` for a dark and for a fixed theme and `splitty-classic-light` for a light one: settings created without a choice name them, and they are shown in place of a stored theme the catalog does not have. Settings that already name a theme SHALL keep it.

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
- **WHEN** the app starts with no stored settings
- **THEN** the UI theme is `splitty-classic`, and `splitty-classic-light` where the theme follows a light system preference

#### Scenario: Settings from before the default changed
- **WHEN** the stored settings name `splitty-dark`
- **THEN** that theme is shown, listed as Splitty Modern, with the terminal palette of the same identifier

#### Scenario: Palette with a display name
- **WHEN** a palette file named `Night` holds the line `# name: Night Owl`
- **THEN** the palette is offered as Night Owl and stored as `night`

#### Scenario: Windows 11 themes propose Cascadia
- **WHEN** Windows 11 Dark or Windows 11 Light is shown, `terminal.fontFamily` is empty and Cascadia Mono is not installed but Cascadia Code is
- **THEN** terminals use Cascadia Code, and the bundled font where neither is installed

### Requirement: Program Status Notification Switch
A server's settings SHALL hold `notifications.programStatus`, a boolean that defaults to true and is listed in the settings metadata; settings stored without it SHALL read it as true, and a window SHALL read it as true from an older server that does not send it. A window SHALL add program status notifications (see `program-status`) only while both `notifications.enabled` and `notifications.programStatus` are on. The switch SHALL NOT change which status a window shows on tabs, screens and host groups.

#### Scenario: Settings stored before the switch existed
- **WHEN** a server starts with stored settings whose `notifications` lack `programStatus`
- **THEN** `GET /api/home/settings` returns `notifications.programStatus` as true

#### Scenario: Switch turned off
- **WHEN** a user turns off `notifications.programStatus` and a pane the user is not watching becomes `blocked`
- **THEN** the window adds no notification and the pane's tab shows that it is blocked

### Requirement: Native Appearance Before The Local Service Answers
The native app SHALL keep the appearance selection it last applied from its local service (UI theme mode and names, interface font, zoom, terminal palettes, terminal font and size) in `last-appearance.json` in the configuration directory of the computer it runs on. A new window SHALL show that selection, resolved in its catalog for the current OS preference, until the local service's appearance replaces it. The file SHALL NOT be a source of settings; a missing or unreadable file SHALL leave the built-in appearance.

#### Scenario: Start while the local service is not ready
- **WHEN** the app starts on Windows, its WSL backend is still being started, and the last window had applied a light UI theme with a larger zoom
- **THEN** the window shows the startup phases in that theme, font and zoom

#### Scenario: Settings changed since the last window
- **WHEN** the local service's appearance differs from the remembered selection
- **THEN** the window shows the service's appearance once it arrives, and the file holds that selection afterwards
- **AND** no settings mutation is submitted

#### Scenario: Automatic mode after the OS preference changed
- **WHEN** the remembered selection is in automatic mode and the OS preference changed since it was stored
- **THEN** the window starts with the remembered UI theme of the current preference

#### Scenario: No usable file
- **WHEN** the file is missing, oversized or not a remembered selection
- **THEN** the window starts with the built-in appearance and reports nothing

#### Scenario: Isolated starts
- **WHEN** the app runs its workspace smoke check or is started with an explicit WSL target
- **THEN** it neither reads nor writes the file

#### Scenario: File from before the terminal font was remembered
- **WHEN** the file holds a selection without the terminal font and size
- **THEN** the window starts with that selection and the default terminal font and size

### Requirement: Terminal Palette Rule
Every window SHALL choose the name of its terminal palette by one rule, the server's: the palette `terminal.theme` names; while that is `default`, the palette `terminal.themeDark` or `terminal.themeLight` names for the mode of the shown UI theme; while that is `default` too, the UI theme's own palette. An empty name SHALL count as `default`. `theme.get` and `GET /api/home/theme` SHALL apply this rule to the server's stored terminal settings for the UI theme they answer with, by that theme's mode: the answer's `terminal` colors SHALL be those of the server's theme with the chosen name, and its `terminalPalette` SHALL name the theme they come from; where the rule chooses the UI theme's own palette, or the server has no theme of the chosen name, they SHALL be the answered theme's own colors and `terminalPalette` its identifier. The native workspace SHALL look the chosen name up in its own catalog (see Native Theme Catalog), so a name only the server has SHALL show the UI theme's palette there; stored settings SHALL NOT change for it.

#### Scenario: Palette by the UI theme's mode
- **WHEN** `terminal.theme` is `default`, `terminal.themeDark` is `splitty-retro` and `terminal.themeLight` is `splitty-light`, and the answered UI theme is dark
- **THEN** the answer carries the terminal colors of `splitty-retro`, and for a light UI theme those of `splitty-light`

#### Scenario: Palette the server does not have
- **WHEN** `terminal.theme` names a palette that only the native catalog has
- **THEN** the theme answer carries the UI theme's own terminal colors and its identifier as `terminalPalette`, and the setting keeps its value

### Requirement: Independent Native Window Chrome
The native desktop app SHALL support `appearance.titleBarStyle` with values `system` and `theme`, defaulting to `theme` when absent. This setting SHALL be independent of application and terminal themes, and editable only when local service metadata advertises it.

#### Scenario: Independent colors
- **WHEN** a user selects system window chrome while named application and terminal themes are active
- **THEN** both theme selections remain effective and unchanged, and the title bar follows the system appearance

#### Scenario: Unsupported selection or older service
- **WHEN** an update names an unsupported title-bar selection or the local service does not advertise the setting
- **THEN** the server rejects the unsupported update atomically, or the native editor leaves the unadvertised setting uneditable

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

### Requirement: Native Decoration Startup Selection
The native app SHALL remember its last confirmed interface, decoration, color and button selections with the startup appearance and restore them for window creation. Absent selections SHALL use their declared defaults. A change requiring restart SHALL disclose that requirement and take effect at the next normal start. Reading settings SHALL NOT write them or recreate terminal sessions.

#### Scenario: Persisted selection and old settings
- **WHEN** the app starts with a previously confirmed title-bar selection, or reads a startup cache from before the selection existed
- **THEN** it restores the confirmed selection for window creation, or defaults to theme mode when absent

### Requirement: Independent Native Interface Style
The native app SHALL support `appearance.interfaceStyle` (`system` or `theme`, default `theme`). System style SHALL follow the desktop light/dark preference and its typography and interface conventions through a local desktop profile. Selecting it SHALL preserve saved named-theme and font preferences so returning to theme style restores them. Terminal palette selection and window decoration SHALL remain independent.

#### Scenario: System interface and named theme restoration
- **WHEN** a user switches to system interface style and then back to theme style
- **THEN** the system profile is shown first, and the saved named-theme and font choices become effective again without rewriting them

### Requirement: Native Window Button Policy
The native app SHALL support `appearance.windowButtons` (`system` or `all`, default `all`) for client decorations. System policy SHALL respect desktop button arrangement; all SHALL expose minimize, maximize/restore and close, disabling actions unavailable on the platform. Native OS-owned decorations MAY retain their own controls.

#### Scenario: Complete client controls
- **WHEN** the user selects all window buttons on a client-decorated window
- **THEN** minimize, maximize/restore and close are available independently of the desktop's button arrangement, subject to platform capabilities

### Requirement: Settings Update Validation
`PATCH /api/home/settings` SHALL validate all submitted fields, including `editors`, before changing persisted or running settings. Invalid input MUST receive status 400 with `error` and a dotted `field` path. A valid update SHALL return the complete effective settings. Fractional values SHALL be accepted where the metadata publishes them, such as the listed zoom levels.

#### Scenario: Invalid editor rule
- **WHEN** a client submits an invalid `editors.extensions` or `editors.projects` rule
- **THEN** it receives status 400 with the failing field and no settings change is persisted or applied

#### Scenario: Invalid numeric or enum field
- **WHEN** a client submits an out-of-range number or unsupported allowed value
- **THEN** it receives status 400 with the failing field and the prior settings remain effective

#### Scenario: Fractional setting
- **WHEN** a client submits `appearance.zoom` 1.25, a level the metadata lists
- **THEN** the server persists and returns that fractional value

### Requirement: Removed Setting Groups And Fields
The settings SHALL NOT contain, validate, describe in their metadata or return `terminal.renderer`, `terminal.ligatures`, `terminal.windowPaddingBalance`, the group `terminal.xterm`, the groups `tabs`, `window` and `experimental`, or `drawer.resizable`. A stored `config.toml` holding them SHALL load without error, with every other valid value kept; the removed keys SHALL be ignored and SHALL be absent from the file the server writes next. An update naming one of them SHALL be refused with status 400 and that field as an unknown setting.

#### Scenario: Older configuration file
- **WHEN** `config.toml` holds `[terminal] renderer`, `[terminal.xterm]`, `[tabs]`, `[window]`, `[experimental]` and `[drawer] resizable` next to a valid `[appearance] font_size = 15`
- **THEN** the server starts with font size 15, the effective settings contain none of the removed keys, and the file the server writes next holds none of them

#### Scenario: Update naming a removed setting
- **WHEN** a client sends `PATCH /api/home/settings` with `{"terminal":{"xterm":{"cursorBlink":true}}}`
- **THEN** it receives status 400 with the field `terminal.xterm` and no setting changes

### Requirement: Terminal Font Settings
A server's settings SHALL hold `terminal.fontFamily`, a font family name that defaults to empty (the default face), and `terminal.fontSize`, the terminal text size in pixels before zoom, default 14. The metadata SHALL name both defaults and list the sizes 8 to 32 as the `values` of `terminal.fontSize`. Settings stored without them SHALL read the defaults. An update with an unlisted size, or a family with control characters or longer than 4096 bytes, SHALL be refused with status 400 and that field.

#### Scenario: Settings stored before the font settings existed
- **WHEN** a server starts with stored settings whose `terminal` lacks `font_family` and `font_size`
- **THEN** `GET /api/home/settings` returns `terminal.fontFamily` as empty and `terminal.fontSize` as 14, and a window draws its terminals as before

#### Scenario: Font settings persist
- **WHEN** a client sends `PATCH /api/home/settings` with `{"terminal":{"fontFamily":"Fira Code","fontSize":16}}`
- **THEN** the response and `settings.changed` carry both values, and `config.toml` holds `font_family` and `font_size` in `[terminal]`

#### Scenario: Size outside the listed values
- **WHEN** a client sends `terminal.fontSize` 7, 33 or 14.5
- **THEN** it receives status 400 with the field `terminal.fontSize` and the stored size stays

### Requirement: Native Terminal Font Choice
The native workspace SHALL draw terminals in the first installed family of, in this order, `terminal.fontFamily`, the shown UI theme's `terminal-font-family` (see Native Theme Catalog), and else the bundled terminal font. A `terminal.fontFamily` the computer does not have SHALL show the bundled font with a notice and SHALL NOT change the setting; a theme's proposal that is not installed SHALL be skipped silently. The editor SHALL offer the default and the installed families.

#### Scenario: Setting before the theme's proposal
- **WHEN** the shown UI theme proposes Cascadia Mono, which is installed, and `terminal.fontFamily` names another installed family
- **THEN** terminals use the family of the setting, and with an empty setting Cascadia Mono

#### Scenario: Family not installed
- **WHEN** `terminal.fontFamily` names a family the computer does not have
- **THEN** terminals use the bundled font, the window reports that the terminal font is not installed, and the stored setting is unchanged

#### Scenario: Theme's proposal not installed
- **WHEN** the setting is empty and none of the families the theme proposes is installed
- **THEN** terminals use the bundled font without a notice

#### Scenario: Older local service
- **WHEN** the local service's metadata names no default for `terminal.fontFamily` or `terminal.fontSize`, or it publishes no metadata
- **THEN** both count as their defaults and the editor does not offer to change them

### Requirement: Native Terminal Font Application
Every existing and new terminal of a native window SHALL take a changed terminal font or size without recreating its session or losing history. The size SHALL compose with display zoom as the configured size of Zoom Settings (`display-zoom`). Cell metrics SHALL be measured again with the new face, and the PTY SHALL learn a changed grid size. A character the chosen family has no glyph for SHALL be drawn with the bundled terminal font where that font has it.

#### Scenario: Live change
- **WHEN** a user sets the terminal font size to 18 while terminals are open and zoom is 100 %
- **THEN** every terminal draws 18 px text, keeps its process and output, and reports the grid size that fits its pane

#### Scenario: Size with terminal zoom
- **WHEN** `terminal.fontSize` is 18, `appearance.zoom_scope` is `terminals` and the level is 110 %
- **THEN** the terminals use font size 20

#### Scenario: Glyph the chosen family lacks
- **WHEN** the terminal font is a family without the Nerd Font symbols and a program prints one of them
- **THEN** the symbol is drawn with the bundled terminal font, not as an empty box
