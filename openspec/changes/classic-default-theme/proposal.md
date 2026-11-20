# Classic as the default theme

## Why

A new installation starts in Splitty Dark, a rounded sans-serif look much like any dark application. Splitty
Classic (square, with the terminal's own monospace face in the interface and a higher contrast) reads better
and is the look the documentation shows. The name "Splitty Dark" also says nothing about what sets the
theme apart from the other dark ones.

## What Changes

- The default UI themes are `splitty-classic` (dark, and for a fixed theme) and `splitty-classic-light`
  (light), on the server's settings and in the native catalog; they are also what is shown in place of a
  theme that cannot be found.
- Splitty Dark, Splitty Light and Splitty Light Gray are shown as Splitty Modern, Splitty Modern Light and
  Splitty Modern Light Gray, as UI themes, as the server's themes and as terminal palettes. Their
  identifiers (`splitty-dark`, `splitty-light`, `splitty-light-gray`) stay, so stored settings keep their
  looks.
- A terminal palette file can carry a display name in a comment line `# name: …`; its identifier stays the
  file name's.

Out of scope: changing the theme of settings that already name one, the built-in appearance a window
shows before any setting or remembered appearance exists, and new themes.

## Capabilities

### Modified Capabilities

- `settings`: Native Theme Catalog (default themes, display names beside identifiers, palette display
  names).

## Impact

`crates/splitty-core` (default settings, the fallback theme, theme names), `crates/splitty-gpui` (catalog
defaults, theme set and palette files, palette parsing), `docs/themes.md`.

## Architecture impact

none

## Decisions

- **CDT-1 (provisional, tbd):** the light default becomes `splitty-classic-light`, so the pair of a system
  that follows light and dark is one family. Alternative: keep `splitty-light` as the light default.
- **CDT-2 (provisional, tbd):** settings that already store a theme keep it, including installations that
  never chose one, since the settings file holds the former default by name. Alternative: move unchanged
  installations to the new default.
- **CDT-3 (provisional, tbd):** the renamed themes keep their identifiers, and palettes get a display name
  through a `# name:` comment. Alternative: new identifiers with aliases for the old ones.
- **CDT-4 (provisional, tbd):** the server's fallback for a theme it cannot find follows the default
  (`splitty-classic`). Alternative: keep `splitty-dark` as the fallback.
