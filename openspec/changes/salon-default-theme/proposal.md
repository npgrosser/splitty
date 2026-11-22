# Proposal

## Why

The application icon is four panes in muted earth tones, while the default theme is a neutral gray with a blue accent: the app and its icon do not look like one product. A default theme that takes the icon's tones as quiet accents ties them together.

## What Changes

- Two bundled themes, Splitty Salon (`splitty-salon`, dark) and Splitty Salon Light (`splitty-salon-light`), each as a UI theme and as a terminal palette. They have the shape of Splitty Modern; the grays lean slightly to teal (dark) or to paper (light), the accent is ochre (dark) or slate teal (light), danger is terracotta and success is sage.
- Their terminal palettes keep the ANSI colors of Splitty Modern and Splitty Modern Light and change only background, foreground, selection and cursor.
- They are the default: new settings start in a fixed theme mode with `splitty-salon` and follow the system with `splitty-salon` and `splitty-salon-light`; a theme the catalog does not have shows them, and they are the default terminal palettes of their modes.
- Settings that name a theme keep it, `splitty-dark` included. Splitty Modern and its variants stay bundled under their identifiers.

## Capabilities

### New Capabilities

### Modified Capabilities
- `settings`: Native Theme Catalog names `splitty-salon` and `splitty-salon-light` as the default UI themes.

## Impact

- `crates/splitty-gpui/themes/` (UI theme set, two palettes), `crates/splitty-core/src/theme/bundled/` (two theme files), the default identifiers in `splitty-core` settings and the app's theme catalog, the core's fallback theme.
- `docs/themes.md`.
- Themes without a palette of their own (Windows 11 Light, third-party themes) take the Salon palette of their mode.

## Architecture impact

none

## Decisions

- The ANSI colors of the terminal palettes stay those of Splitty Modern: muted earth tones would make red, yellow and green hard to tell apart in program output.
- **SDT-1 (provisional, tbd):** Windows 11 Light keeps taking the default light palette, so its terminals get the paper background of Splitty Salon Light in place of white. Alternative: give Windows 11 Light a palette of its own with the white background.
- **SDT-2 (provisional, tbd):** the themes are named Splitty Salon. Alternative: another name; the identifiers would change with it.
