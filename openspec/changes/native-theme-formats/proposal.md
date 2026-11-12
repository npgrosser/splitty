# Proposal

> Draft created on 2026-11-09 as a terminal palette import, widened on 2026-11-12 to both theme formats. Not planned yet. Specs, design and tasks are written when this change is next. Related: `native-ui-gpui`; the `settings` capability's `ui.theme` and `terminal.theme`; open decisions NWC-3, NS-2, NS-3 and NS-4, which this change settles.

## Why

Splitty's theme format was built for the web client: a palette of gray steps and accents from which the service derives CSS tokens, lengths such as `32px`, colors with alpha, and effects as CSS custom properties. The native app, now the default desktop app, reads those resolved tokens through `theme.get` and translates them back: it parses CSS lengths and colors, bounds them, and projects a subset onto the component toolkit's colors and the terminal's palette. The result is a third format between two that already exist:

- the toolkit has a theme format of its own (JSON theme sets, a registry, a watched directory) and a collection of ready themes in its repository;
- terminal palettes have a de facto format in Ghostty's theme files, and the iTerm2-Color-Schemes collection (MIT) holds several hundred of them.

Users bring palettes they like from other tools, and each one needs a hand-written Splitty theme file today.

## What Changes

- **UI themes in the native app are toolkit themes.** The app loads theme sets in the toolkit's JSON format: bundled ones and files in a user themes directory. `ui.theme` names one of them.
- **Splitty's chrome options stay, as an optional extension.** A theme file may carry a Splitty block for the tokens the toolkit does not define (tab height, the selected tab's accent line and its position, the drop marker). Themes without the block get defaults derived from their toolkit colors.
- **Terminal themes are Ghostty-format palette files.** `palette = N=#rrggbb`, `background`, `foreground`, `cursor-color`, `cursor-text`, `selection-background` and `selection-foreground` map to the terminal's colors. Bundled palettes come from the iTerm2-Color-Schemes collection; files in the user themes directory are read too. The app parses the format itself; the terminal engine has no theme loader.
- **The bundled Splitty themes are ported.** Each of today's themes becomes a toolkit theme with the Splitty block plus a terminal palette of the same name, generated from the current resolution so the look stays the same.
- **The default terminal palette follows the UI mode** (provisional, tbd): `terminal.theme` holds a light and a dark palette, as Ghostty's `light:…,dark:…` does, each with a default. A UI theme with a terminal palette of the same name uses that one while the setting is `default`.
- **Theme resolution moves into the native app.** Settings stay with the local service and keep the theme names; the native app no longer calls `theme.get` and `theme.list`.
- **The license notices ship with the bundled themes.** That is the palette collection's MIT notice and its credits, and the notices of the toolkit's themes; any theme under a license of its own is reviewed before it is bundled.

To decide when this is next:
- **How the collections arrive:** converted and embedded at build time, or a selection checked in.
- **How a list of several hundred palettes stays usable:** search, a preview and grouping by light and dark.
- **Theme names in settings shared with the web shells:** the same keys with names each client resolves in its own catalog, or separate keys for the native app.

Out of scope:
- The web shells. They are legacy and keep the existing theme module and `theme.get`/`theme.list` until they are removed.
- CSS effects (glow, scanlines), which the native app does not render today.
- Ghostty's other configuration (fonts, cursor style, padding, keybindings).

## Capabilities

### New Capabilities

### Modified Capabilities
- `settings`: where the native workspace reads UI and terminal themes from, which names `ui.theme` and `terminal.theme` may hold, how the default terminal palette is chosen, and how theme lists present large catalogs.

## Impact

- Native client: `appearance.rs` shrinks to loading toolkit themes and the Splitty block; a palette parser and catalog; the theme pickers in settings.
- Core and server: the theme module is unchanged and serves only the web shells; settings validation accepts theme names it cannot resolve itself.
- Repository: bundled theme files or a build step, third-party license notices.

## Architecture impact

The native client gains a theme catalog module and loses its dependency on the service's theme resolution; a build-time generator is added to the model if one is chosen.
