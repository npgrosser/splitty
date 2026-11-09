# Proposal

> Draft created on 2026-11-09. Not planned yet. Specs and tasks are written when this change is next. Related: `native-ui-gpui` (the native client reads terminal colors through the same theme resolution); the `settings` capability's `terminal.theme`.

## Why

Terminal palettes are a matter of taste, and many users already have one they like from Ghostty or iTerm2. Ghostty's bundled themes come from the iTerm2-Color-Schemes collection, which is MIT-licensed and contains several hundred palettes. Splitty ships a dozen of its own themes. `terminal.theme` can already select the terminal colors of any available theme while the chrome keeps the selected UI theme, which fits terminal-only palettes well. Today each such palette needs a hand-written Splitty theme file, including a `[palette]` from which the server derives UI colors that a terminal palette does not define.

## What Changes

- **Terminal palettes in Ghostty's format become available as terminal themes.** A palette's `palette = N=#rrggbb` entries, `background`, `foreground`, `cursor-color`, `cursor-text`, `selection-background` and `selection-foreground` map to a theme's terminal tokens.
- **Both clients list them** wherever a terminal palette is chosen: the web settings and the native client's settings once it has a theme picker.
- **The license notice ships with them.** That is the collection's MIT notice and its credits; any palette under a license of its own is reviewed before it is bundled.

To decide when this is next:
- **How palettes arrive:**
  - Ghostty-format files placed in the themes directory are read directly;
  - a selection is converted and bundled;
  - or the whole collection is converted at build time and bundled.

  Today each bundled theme is a separate `include_str!` entry.
- **Terminal-only or full themes:**
  - The palettes are offered only for `terminal.theme`, as a terminal-only category, because they carry no UI colors.
  - Or a palette is derived from their colors (a gray ramp between background and foreground; accent, danger, warning and success from the ANSI colors) so they also work as UI themes.
- **How a list of several hundred palettes stays usable:** search, a preview and grouping by light and dark.

Out of scope: Ghostty's other configuration (fonts, cursor style, padding, keybindings).

## Capabilities

### New Capabilities

### Modified Capabilities
- `settings`: which themes `terminal.theme` may name, including terminal-only themes, and how theme lists present them.

## Impact

- Core: theme types and resolution (`crates/splitty-core/src/theme/`), the bundled catalog or a build step, the theme list.
- Frontend: the terminal palette picker in settings.
- Native client: its theme picker, once it has one; the resolution path is unchanged.
- Repository: a third-party license notice for bundled palettes.

## Architecture impact

none expected; a build-time generator would be added to the model if one is chosen.
