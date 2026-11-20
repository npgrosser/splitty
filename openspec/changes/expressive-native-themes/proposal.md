# Expressive native themes

## Why

A UI theme of the native app can set colors, one menu radius, the tab bar's size and a font. Everything
else is fixed: controls keep their own corner radii, every border is one flat line, the title bar derives
its colors from the chrome and always has the same buttons. Themes therefore differ in palette and little
else, and a look that depends on shape (3D edges and square corners, or soft panels with small control
corners and wide caption buttons) cannot be expressed.

No roadmap milestone; this extends the theme catalog of `native-default-desktop`.

## What Changes

- The `splitty` object of a UI theme gains shape tokens: `bevel` (3D edges in place of flat borders),
  `control-radius`, `field` (the background of text fields), `active-text`, `menu-hover` and `title-bar`
  (colors, optional gradient, inactive colors, height, alignment, weight, caption button style).
- `radius` now bounds every chrome corner, not only menus: a theme with a radius of 0 is square
  throughout. Marks that are round by meaning (dots, pills) stay round.
- Text on a selected entry and on a highlighted menu row is readable in every theme, with or without the
  new tokens.
- An application-drawn title bar takes the theme's title-bar colors while title-bar colors follow the
  theme, and the theme's title-bar shape while the decoration is Splitty's. Splitty decoration no longer
  forces a rounded frame.
- The component toolkit's controls (buttons, fields, checkboxes, menus, dialogs) follow the theme's bevel
  and field background, so settings and dialogs match the workspace chrome.
- Three bundled themes use the tokens: Windows 11 Dark, Windows 11 Light and Windows Classic, each with
  a terminal palette of its name.

## Out of scope

- Backdrop materials (translucent window backgrounds) and other platform compositor effects.
- Bundling fonts; a theme names installed families.
- Per-element border languages beyond the fixed 3D edge, textures, and themeable icons.
- The server's theme list and `theme.get`; the catalog stays local to the native app.
- Changing the meaning or defaults of `appearance.windowDecoration`, `appearance.titleBarStyle` and
  `appearance.windowButtons`.

## Capabilities

### Modified Capabilities

- `settings`: Native Theme Catalog (shape tokens and their derivation), Native Decoration Ownership (what
  an application-drawn title bar takes from the theme).

## Impact

- `crates/splitty-gpui`: appearance resolution, the edge painter, title bar, workspace chrome, bundled
  theme files.
- `vendor/gpui-component`: local patch for bevel and field background on its controls.
- `docs/themes.md`: token table and decoration section.
- Existing themes: flat themes keep their borders and colors; themes with a small radius (Splitty
  Classic, Retro, Neon) get that radius on every control.
