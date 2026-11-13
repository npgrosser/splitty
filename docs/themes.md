# Themes

The app has two kinds of themes, chosen separately in the settings:

- a **UI theme** for the app itself: drawer, tab bars, menus, dialogs;
- a **terminal palette** for what terminals show: the 16 ANSI colors,
  background, foreground, cursor and selection.

Both come with the app and can be added as files. The
[browser UI](cli.md#browser-ui) has its own theme files (`*.toml`), which the
app does not read.

## What ships

- The Splitty themes (Splitty Dark, Splitty Light, Classic, Retro, Aurora, Neon,
  Cozy, the gray variants, Adwaita), each as a UI theme and as a terminal
  palette.
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

## Adding themes

Put files into `themes/` in the configuration directory
(`~/.config/splitty/themes/`, macOS
`~/Library/Application Support/splitty/themes/`), on the computer the app runs
on. They are offered the next time the settings open. A file that cannot be
loaded is named there.

| File | Read as |
|---|---|
| `*.json` | UI themes: a GPUI Kit theme set |
| `*.toml` | ignored (browser UI themes) |
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

Themes from `~/.config/ghostty/themes/` can be copied as they are.

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
| `accent` | color | `primary.background` |
| `danger`, `danger-hover`, `success`, `warning` | color | `danger`, a tint of it, `success`, `warning` |
| `hover-overlay`, `shadow` | `#rrggbbaa`: drop marker fill, menu shadow | text at 6 %, black |
| `drag-over-border` | `{ "width": 1, "color": "#…" }` or `"none"` | none |
| `tab-height`, `tab-font-size` | pixels, 16 to 64 and 8 to 24 | 32, 12 |
| `tab-accent` | `{ "color", "thickness" (0 to 8), "position": "top" or "bottom" }` or `"none"` | none |
| `radius` | pixels, 0 to 24: menu corners | the theme's `radius` |
| `font-size` | pixels, 8 to 24 | 12 |
| `font-family` | family list; `system-ui` and `monospace` name the platform's fonts | the system font |

Colors are `#rgb`, `#rrggbb` or `#rrggbbaa`. A value outside these forms or
ranges keeps its derived value, and the app says so in its status line.
