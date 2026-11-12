# settings Specification

## Purpose

Settings belong to each server and govern its terminal, workspace, appearance, shell, editor, and notification behavior for all connected windows.

## Requirements

### Requirement: Server Settings and Metadata
The server SHALL return its current settings at `GET /api/home/settings` and defaults and field limits at `GET /api/home/settings/metadata`, including numeric bounds and steps and allowed values. Defaults MUST reflect that server's platform and profile detection. A window SHALL derive controls and reset values from this metadata when available.

#### Scenario: Server-specific defaults and limits
- **WHEN** a window loads settings from a server
- **THEN** it uses that server's defaults and limits for the settings controls and reset actions

#### Scenario: Older server without metadata
- **WHEN** the metadata endpoint is unavailable on an older server
- **THEN** the window retains the settings returned by that server and continues to support compatible updates without requiring metadata

### Requirement: Atomic Settings Validation
`PATCH /api/home/settings` SHALL validate all submitted fields, including `editors`, before changing persisted or running settings. Invalid input MUST receive status 400 with `error` and a dotted `field` path. A valid update SHALL return the complete effective settings. Fractional `terminal.xterm.scrollSensitivity` and `fastScrollSensitivity` SHALL be accepted within published bounds.

#### Scenario: Invalid editor rule
- **WHEN** a client submits an invalid `editors.extensions` or `editors.projects` rule
- **THEN** it receives status 400 with the failing field and no settings change is persisted or applied

#### Scenario: Invalid numeric or enum field
- **WHEN** a client submits an out-of-range number or unsupported allowed value
- **THEN** it receives status 400 with the failing field and the prior settings remain effective

#### Scenario: Fractional scroll setting
- **WHEN** a client submits a fractional scroll sensitivity on a published step
- **THEN** the server persists and returns that fractional value

### Requirement: Settings Change Propagation
After a successful settings change, the server SHALL send `settings.changed` with the complete effective settings to every connected window, including the writer, over browser and native connections. A window SHALL apply that event, and SHALL restore its prior value after a failed update.

#### Scenario: Other window updates settings
- **WHEN** one window changes a setting
- **THEN** every connected window receives `settings.changed` with the saved settings

#### Scenario: Rejected optimistic update
- **WHEN** a window's settings update is rejected or its request fails
- **THEN** it restores its previous settings and exposes the failure

### Requirement: Removed Terminal Fields
`linkTarget` and `webviewMode` SHALL NOT be validated or returned as current terminal settings. Existing stored values SHALL be ignored when loading settings.

#### Scenario: Legacy stored fields
- **WHEN** a stored configuration contains `link_target` or `webview_mode`
- **THEN** the effective settings omit them and all other valid fields remain usable

### Requirement: Experimental Native Local Appearance
The experimental native workspace SHALL read local appearance settings through `home.get` and resolve the UI theme they name in its own theme catalog, applying appearance font overrides and the theme's colors to workspace chrome without writing settings during reads. It SHALL NOT request `theme.get` or `theme.list`. Terminal colors SHALL follow the selected native terminal palette. Unset chrome font overrides SHALL use the UI theme's defaults. Automatic mode SHALL select the saved light or dark UI theme using the local OS preference. A UI theme name the catalog does not have SHALL show the default theme of the OS preference and a notice, and SHALL NOT change the stored setting. Remote settings SHALL NOT select application appearance.

#### Scenario: Startup appearance
- **WHEN** the local appearance read succeeds
- **THEN** workspace chrome uses the supported appearance font overrides or the UI theme's defaults
- **AND** existing and newly created terminals use the selected native terminal palette without recreating sessions or losing terminal history
- **AND** loading appearance does not submit a settings mutation

#### Scenario: Chrome font changes preserve terminal typography
- **WHEN** appearance font size or family changes, including selection of a proportional UI font
- **THEN** workspace chrome uses the supported font choice while terminal typography retains its native defaults
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
- **WHEN** a user changes or resets UI theme mode or choice, UI font size or family, display zoom or scope, terminal palette or close-on-exit policy, or Drawer visibility or width
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
The native workspace SHALL choose the terminal palette in this order: the palette `terminal.theme` names; while that is `default`, the palette `terminal.themeDark` or `terminal.themeLight` names for the mode of the shown UI theme; while that is `default` too, the palette with the UI theme's identifier, ignoring `-`; otherwise the default palette of that mode. The service SHALL store `terminal.themeDark` and `terminal.themeLight` with the default `default`. A named palette SHALL be used while the selected UI theme stays in use for chrome. Palette-only changes SHALL update existing and future terminals without recreating sessions, losing terminal history, or changing native terminal font metrics.

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
The native workspace SHALL resolve UI themes and terminal palettes in a catalog of its own, made of the themes bundled with the application and the files of the themes directory (`themes/` in the configuration directory of the computer the application runs on). A UI theme SHALL be a theme of a theme set in the component toolkit's JSON format (`*.json`: a `themes` list whose entries have `name`, `mode` and `colors`). A terminal palette SHALL be a file in Ghostty's theme format: `palette = N=#rrggbb` for the ANSI colors 0 to 15, `background`, `foreground`, `cursor-color`, `selection-background` and `selection-foreground`; other keys SHALL be ignored, and a file without `background` and `foreground` is not a palette. In the themes directory, `*.toml` files SHALL be left to the web client, and every other file that is not a theme set SHALL be read as a terminal palette named by its file name.

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
