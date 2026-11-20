# Design: expressive native themes

## Context

`NativeAppearance` is the resolved presentation of the selected UI theme: the toolkit theme as it is, plus
the chrome's own tokens from the theme's `splitty` object, each derived from the toolkit colors where the
theme leaves it out. Views read colors and a few sizes from it; shapes are constants in the views.

## Goals

- A theme can state its edges, corners, selection text and title bar, and every view follows.
- A theme without the new tokens renders as before, apart from corners following its radius.
- One place draws 3D edges, so a flat and a beveled theme share every call site.

## Decisions

### Tokens

| Token | Value | Derived |
|---|---|---|
| `bevel` | `{ highlight, light, shadow, dark-shadow }` or `"none"` | none: flat borders |
| `control-radius` | pixels, 0 to 24 | `radius` |
| `field` | color | each field's present background |
| `active-text` | color | `text` where it reads on `active`, else black or white |
| `menu-hover` | color | `hover` |
| `title-bar` | object, see below | the chrome's `raised` and `text`, centered, the decoration's buttons |

`title-bar`: `background` and `inactive-background` (a color, or `{ from, to }` for a gradient along the
bar), `foreground`, `inactive-foreground`, `height` (20 to 64), `align` (`center`, `start`), `weight`
(`normal`, `medium`, `semibold`, `bold`), `buttons` (`plain`, `boxed`, `wide`). Each value is validated
alone; an unsupported one keeps its derived value and is reported, as for the existing tokens.

### Corners

A control's corner is the smaller of its own radius and the theme's control radius; a floating surface
uses `radius` as it is. No minimums remain, also for the frame of a client-decorated window under Splitty
decoration.

### Edges

The edge painter knows four reliefs (raised, frame, sunken and etched) and
which sides carry the edge. With a bevel it reserves the relief's width as border space and paints the
two rings of lines there, lit from the top left, as an overlay child that takes no input; without a bevel
it is a 1 px border in the caller's color. Views name what an element is (a control, a field, a floating
surface, a separator), not how it looks. A bevel is always square.

### Text on highlights

Selected entries use `active-text`, and a highlighted menu row uses `menu-hover` with text that reads on
it. Where the theme names no color and its text cannot be read on the highlight (a contrast below 3), the
text is black or white, whichever reads better, not the first readable grey; text that is merely soft
there is kept. Secondary and muted text of a highlighted entry is lifted toward the highlight's text only
where the highlight has a text color of its own and the subdued color does not stand out; in every other
theme subdued text keeps its value there, as on every other surface.

The theme's corners also bound the toolkit's: its control radius is at most `control-radius` and its
large radius at most `radius`, so a theme that names a radius only in the `splitty` object squares or
rounds settings and dialogs with the chrome.

### Title bar

Colors come from the theme while `appearance.titleBarStyle` is `theme`, under either decoration. Shape
(height, alignment, weight, caption buttons, frame corner) comes from the theme only while
`appearance.windowDecoration` is `splitty`; system decoration keeps the desktop's geometry. Caption
buttons are `plain` (symbols with a hover background), `boxed` (small raised buttons with drawn glyphs)
or `wide` (cells as tall as the bar, the close cell red under the pointer). On macOS the native traffic
lights stay.

### Toolkit

The vendored toolkit's theme carries the bevel and the field background; its buttons, fields, checkboxes,
menus, popovers and dialogs draw the same 3D edges. The toolkit has its own copy of the painter, since it
cannot depend on the application.

### Bundled themes

`ui/windows.json` holds Windows 11 Dark, Windows 11 Light and Windows Classic, with terminal palettes of
the same names. Windows Classic is a light UI theme whose own terminal palette is the black console.

## Architecture impact

None in `architecture/*.c4`: the edge painter is part of `splitty_gpui::appearance`, which the chrome and
the workspace already depend on. No concept of `docs/architecture.md` changes.

## Provisional decisions

Taken 2026-11-20 without the maintainer's confirmation; each is also in `openspec/open-decisions.md`.

- **ENT-1:** theme title-bar colors follow `titleBarStyle: theme`; theme title-bar shape follows
  `windowDecoration: splitty`. Keeps NWC-1. Alternative: a theme that names a title-bar shape shows it
  under system decoration too.
- **ENT-2:** the theme's radius bounds every control corner, so existing square themes become square
  throughout. Alternative: keep fixed control radii and make following the theme opt-in per theme.
- **ENT-3:** one fixed 3D edge described by four colors. Alternative: a general border language (widths,
  styles, per-side colors) per element class.
- **ENT-4:** the vendored toolkit is patched to draw the bevel and field background. Alternative: leave
  toolkit components flat in a beveled theme.
- **ENT-5:** Windows Classic keeps a dark terminal palette under a light interface. Alternative: a light
  palette, so every bundled theme's palette has its mode.
- **ENT-6:** highlight text falls back to black or white when the theme's text cannot be read there
  (contrast below 3); subdued text is lifted only on a highlight with a text color of its own.
  Alternative: require `active-text` and keep the theme's text otherwise.
- **ENT-7:** no backdrop material; themes stay opaque. Alternative: a platform backdrop token.
- **ENT-8:** themes name installed fonts; none is bundled. Alternative: bundle a face per theme.
