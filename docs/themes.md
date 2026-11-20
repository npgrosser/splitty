# Themes

The app has two kinds of themes, chosen separately in the settings:

- a **UI theme** for the app itself: drawer, tab bars, menus, dialogs;
- a **terminal palette** for what terminals show: the 16 ANSI colors,
  background, foreground, cursor and selection.

Both come with the app and can be added as files.

## What ships

- The Splitty themes (Classic, Modern, Retro, Aurora, Neon, Cozy, their light
  and gray variants, Adwaita), each as a UI theme and as a terminal palette.
  Splitty Classic is the default, with Splitty Classic Light where the theme
  follows a light system. Splitty Modern is the rounded sans-serif look that
  was called Splitty Dark; settings that name it keep it.
- The Windows themes, which also change the shape of the interface: Windows 11
  Dark and Light (soft panels, small control corners, wide window buttons) and
  Windows Classic (3D edges, square corners, a navy selection, a gradient title
  bar with boxed window buttons, and a black console as its terminal palette).
  Their title-bar shape shows with Splitty window decoration; see below.
- The theme sets of [GPUI Kit](https://github.com/longbridge/gpui-kit)
  (Catppuccin, Gruvbox, Tokyo Night, Solarized, Ayu, Everforest and others).
- About 270 terminal palettes from
  [iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes),
  the collection Ghostty's themes come from. Only palettes whose license permits
  redistribution without further conditions are included; any other palette of
  the collection works as a file.

The files and their license notices are in `crates/splitty-gpui/themes/`;
`scripts/vendor-themes.py` refreshes the third-party ones.

## Choosing

Settings, Appearance: one theme, or a dark and a light one that follow the
system. Settings, Terminal:

- **Terminal colors**: one palette for every mode, or "By interface mode".
- **Terminal colors, dark / light**: the palette used with a dark or a light UI
  theme. "Follow interface theme" takes the palette with the UI theme's name
  (Catppuccin Mocha gets the Catppuccin Mocha palette; spaces and dashes do
  not matter, so Tokyo Night gets TokyoNight) and the Splitty palette of that
  mode where there is none.

## Native appearance and window decoration

Appearance keeps four choices independent:

| Setting | Choices | Owns |
| --- | --- | --- |
| Interface style | System / Splitty theme | Interface colors, typography and density |
| Window decoration | System / Splitty | Frame shape, title-bar height, typography and controls |
| Title bar colors | System / Match app theme | Client title-bar palette |
| Window buttons | System layout / All three | Client minimize, maximize/restore and close controls |

System interface style follows the desktop light/dark preference and typography
through a built-in desktop profile. GNOME uses the bundled Adwaita palette and
roomier spacing. This is GPUI styling; arbitrary GTK CSS and native GTK widgets
are not imported. Saved theme and font choices remain available when returning
to Splitty theme. An explicit terminal palette remains separate.

System decoration uses a native OS frame when system colors are selected and
one is available. Otherwise it draws desktop-style client decoration: on GNOME,
a taller header, small symbolic controls on circular buttons, and desktop font.
On GNOME, symbolic SVG window icons are loaded from the active icon theme and
its parents, then installed fallback themes. Missing or invalid icons use the
existing Splitty symbols. Icon-theme changes reload the four cached symbols;
no Adwaita artwork is bundled. Geometry and bold title styling follow an explicit
Adwaita profile, independently of the icon theme and title-bar palette.
Splitty decoration uses app typography and takes the title bar's shape from the
UI theme: its height, title alignment (centered or at the start) and weight, and
the caption button style. Plain buttons are symbols on rectangular hover
backgrounds; boxed buttons are small raised buttons with the theme's edge,
sunken while pressed; wide buttons are cells as tall as the bar, with a red
close cell under the pointer. Windows uses the same styles for its caption
buttons; macOS keeps its native traffic lights, centered in the bar. A theme
without these values gets a compact header with plain buttons. The client
frame's corner radius is the theme's radius, so a square theme has a square
window. System decoration keeps the desktop's geometry and a rounded frame. The
renderer clips all content, including terminal backgrounds, to the frame's
shape. Tiled edges and maximized windows lose the relevant rounded corners and
shadows.

Match app theme takes colors from the currently shown interface, including the
theme's own title-bar colors (a solid color or a gradient, and separate colors
for a window without focus) where it has them, under either decoration. Changing
it does not change decoration geometry, and system colors ignore the theme's
title-bar colors. All three is the default client button policy and places all
controls on the right. System layout reads the desktop arrangement at startup
and follows changes live, including left/right placement and hiding minimize or
maximize; it can show only Close.
Unavailable platform actions are disabled. Native OS-owned frames retain their
own buttons and may retain system colors.

The settings are `appearance.interfaceStyle` (`system` / `theme`, default
`theme`), `appearance.windowDecoration` (`system` / `splitty`, default `system`),
`appearance.titleBarStyle` (`system` / `theme`, default `theme`; colors only), and
`appearance.windowButtons` (`system` / `all`, default `all`). Existing theme
preferences keep their meanings.

Linux client decoration updates live. macOS and Windows select native versus
custom decoration at startup, so changing decoration or title-bar colors requires
restarting Splitty. macOS retains native traffic lights in custom headers.

## Adding themes

Put files into `themes/` in the configuration directory
(`~/.config/splitty/themes/`, macOS
`~/Library/Application Support/splitty/themes/`, Windows
`%APPDATA%\splitty\themes\`), on the computer the app runs on. On Windows
that is the Windows side, not the WSL distribution the settings live in. They
are offered the next time the settings open. A file that cannot be loaded is
named there.

| File | Read as |
|---|---|
| `*.json` | UI themes: a GPUI Kit theme set |
| `*.toml` | ignored |
| anything else | terminal palette: a Ghostty theme file, named by its file name |

Settings store a theme by an identifier made from its name: lowercase, with
`-` for everything that is not a letter or digit (`Tokyo Night` is
`tokyo-night`). A file cannot replace a bundled theme of the same identifier.

### Terminal palette

A [Ghostty theme](https://ghostty.org/docs/features/theme) file. These keys are
read; `background` and `foreground` are required:

```
palette = 0=#45475a
palette = 1=#f38ba8
# ... up to 15
background = #1e1e2e
foreground = #cdd6f4
cursor-color = #f5e0dc
selection-background = #f5e0dc
selection-foreground = #1e1e2e
```

Themes from `~/.config/ghostty/themes/` can be copied as they are. A comment
line `# name: Night Owl` shows the palette under that name; settings keep
storing the identifier of the file name.

### UI theme

A theme set in GPUI Kit's format:

```json
{
  "name": "Mine",
  "themes": [
    {
      "name": "Mine Dark",
      "mode": "dark",
      "colors": {
        "background": "#101014",
        "foreground": "#d8d8e0",
        "primary.background": "#7aa2f7",
        "border": "#26262e"
      }
    }
  ]
}
```

Colors that are left out keep GPUI Kit's defaults for the mode. The files in
`crates/splitty-gpui/themes/ui/` show the available keys.

### The `splitty` block

The workspace has a few tokens of its own that GPUI Kit's format does not
define. A theme can set them in a `splitty` object; every token left out is
derived from the theme's colors.

```json
{
  "name": "Mine Dark",
  "mode": "dark",
  "colors": { "background": "#101014" },
  "splitty": {
    "tab-height": 28,
    "tab-accent": { "color": "#7aa2f7", "thickness": 2, "position": "bottom" },
    "text-muted": "#5a5a66"
  }
}
```

| Token | Value | Derived from |
|---|---|---|
| `id` | identifier, where the name's should not be used | the name |
| `surface`, `raised`, `overlay` | color | `background`, `title_bar.background`, `popover.background` |
| `text`, `text-secondary`, `text-muted` | color | `foreground`, `muted.foreground`, a dimmer step of it |
| `border`, `border-subtle` | color | `border`, `title_bar.border` |
| `active`, `hover` | color: selected and hovered entries | `accent.background`, halfway to it |
| `active-text` | color: text on selected entries | `text` where it reads on `active`, else black or white |
| `menu-hover` | color: the highlighted row of a menu | `hover` |
| `field` | color: the background of text fields | each field's own background |
| `accent` | color | `primary.background` |
| `danger`, `danger-hover`, `success`, `warning` | color | `danger`, a tint of it, `success`, `warning` |
| `hover-overlay`, `shadow` | `#rrggbbaa`: drop marker fill, menu shadow | text at 6 %, black |
| `drag-over-border` | `{ "width": 1, "color": "#…" }` or `"none"` | none |
| `tab-height`, `tab-font-size` | pixels, 16 to 64 and 8 to 24 | 32, 12 |
| `tab-accent` | `{ "color", "thickness" (0 to 8), "position": "top" or "bottom" }` or `"none"` | none |
| `radius` | pixels, 0 to 24: corners of menus, panels and dialogs; the largest corner of a control | the theme's `radius` |
| `control-radius` | pixels, 0 to 24: the largest corner of a control, where it differs | `radius` |
| `bevel` | `{ "highlight", "light", "shadow", "dark-shadow" }` or `"none"`: 3D edges in place of flat borders | none |
| `title-bar` | object, see below | the chrome's colors, a centered title |
| `font-size` | pixels, 8 to 24 | 12 |
| `font-family` | family list; `system-ui` and `monospace` name the platform's fonts | the system font |

Colors are `#rgb`, `#rrggbb` or `#rrggbbaa`. A value outside these forms or
ranges keeps its derived value, and the app says so in its status line.

### Shape

A theme shapes the interface with four tokens.

**Corners.** Every control (card, row, button, field) has a corner of its own,
at most `control-radius`; menus, panels and dialogs use `radius`. With
`"radius": 0` nothing is rounded except marks that are round by meaning, such as
status dots.

**Edges.** With a `bevel`, controls are drawn raised, text fields sunken, and
menus, panels and dialogs framed, each with two lines lit from the top left:
`highlight` and `light` on the lit sides, `shadow` and `dark-shadow` on the
others. Separators become grooves. A bevel is square, so set `radius` to 0 with
it, and make `shadow` transparent if floating surfaces should cast none. The
components of settings and dialogs follow the same edges and `field`.

```json
"splitty": {
  "radius": 0,
  "field": "#ffffff",
  "shadow": "#00000000",
  "bevel": { "highlight": "#ffffff", "light": "#dfdfdf", "shadow": "#808080", "dark-shadow": "#000000" }
}
```

**Highlights.** `active-text` is the text of selected entries and `menu-hover`
the highlighted row of a menu. Text on either stays readable without them.

**Title bar.** `title-bar` describes the title bar the app draws itself:

```json
"title-bar": {
  "background": { "from": "#000080", "to": "#1084d0" },
  "foreground": "#ffffff",
  "inactive-background": "#808080",
  "inactive-foreground": "#c0c0c0",
  "height": 22,
  "align": "start",
  "weight": "bold",
  "buttons": "boxed"
}
```

| Key | Value | Applies |
|---|---|---|
| `background`, `inactive-background` | color, or `{ "from", "to" }` for a gradient along the bar | with title bar colors set to Match app theme |
| `foreground`, `inactive-foreground` | color | with title bar colors set to Match app theme |
| `height` | pixels, 20 to 64 | with Splitty window decoration |
| `align` | `center` or `start` | with Splitty window decoration |
| `weight` | `normal`, `medium`, `semibold` or `bold` | with Splitty window decoration |
| `buttons` | `plain` (symbols with a hover background), `boxed` (small raised buttons) or `wide` (cells as tall as the bar) | with Splitty window decoration |

Each key is read alone: an unsupported one keeps its derived value without
costing the others.
