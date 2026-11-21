# Modern as the default theme

## Why

Since `classic-default-theme`, new settings start in Splitty Classic and an unknown theme falls back to it.
The default look of a new installation is to be Splitty Modern, the rounded sans-serif theme stored as
`splitty-dark`, with Splitty Modern Light (`splitty-light`) as the light half of the pair for following the
system. Splitty Classic stays a theme to choose. A first start shows one theme, whatever the system's
preference, so the default theme mode stays fixed.

## What Changes

- The default UI themes are `splitty-dark` (dark, and for a fixed theme) and `splitty-light` (light), on the
  server's settings and in the native catalog; they are also what is shown in place of a theme that cannot
  be found, and the default terminal palettes of their modes.
- New settings keep the fixed theme mode with `splitty-dark`, so a first start shows Splitty Modern on a
  light system preference as on a dark one; the pair for following the system defaults to `splitty-dark`
  and `splitty-light`.
- Windows 11 Light, which has no terminal palette of its own, takes the default light palette, now Splitty
  Modern Light. Windows 11 Dark keeps its own palette (Splitty Classic's colors on a darker background).
- The display names (Splitty Modern, Splitty Modern Light, Splitty Modern Light Gray) and every identifier
  stay as they are.

Out of scope: changing the theme of settings that already name one (an installation started while
Splitty Classic was the default keeps it), the built-in appearance a window shows before any setting or
remembered appearance exists, and the themes' looks.

## Capabilities

### Modified Capabilities

- `settings`: Native Theme Catalog (the default themes and the default theme mode of new settings).

## Impact

`crates/splitty-core` (default settings, the fallback theme), `crates/splitty-gpui` (catalog defaults),
tests in `splitty-core`, `splitty-client-core`, `splitty-gpui` and the server's conformance tests,
`docs/themes.md`, `openspec/open-decisions.md`.

## Architecture impact

none (the theme module keeps falling back to the default theme of the settings)

## Decisions

- **CDT-2 (provisional, tbd), carried over from `classic-default-theme`:** settings that store a theme keep
  it when the default changes, including installations that never chose one, since the settings file holds
  the default of its release by name; an installation started while Splitty Classic was the default keeps
  `splitty-classic`. Alternative: move settings that still hold an earlier default to the current one.
- CDT-1 (Splitty Classic Light as the light default) and CDT-4 (the server's fallback is
  `splitty-classic`) no longer apply: the light default is `splitty-light`, and the fallback, which follows
  the default, is `splitty-dark` again.
