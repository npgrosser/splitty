//! Terminal palettes in Ghostty's theme file format: `key = value` lines with
//! `palette = N=#rrggbb`, `background`, `foreground`, `cursor-color`,
//! `selection-background` and `selection-foreground`. Other keys of a Ghostty
//! configuration are ignored.
use crate::terminal::TerminalColors;

/// A file larger than this is not a palette.
pub const MAX_PALETTE_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalPalette {
    pub id: String,
    /// The file name, which is how the format names a palette.
    pub name: String,
    pub colors: TerminalColors,
    pub selection_background: Option<u32>,
    pub selection_foreground: Option<u32>,
}

impl TerminalPalette {
    /// `None` unless the text sets a background and a foreground. Unset ANSI
    /// entries keep the terminal's defaults and an unset cursor takes the
    /// foreground.
    pub fn parse(name: &str, text: &str) -> Option<Self> {
        if text.len() > MAX_PALETTE_BYTES {
            return None;
        }
        let mut colors = TerminalColors::default();
        let (mut background, mut foreground, mut cursor) = (None, None, None);
        let (mut selection_background, mut selection_foreground) = (None, None);
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "palette" => {
                    let Some((index, color)) = value.split_once('=') else {
                        continue;
                    };
                    if let (Ok(index), Some(color)) = (index.trim().parse::<usize>(), hex(color)) {
                        if let Some(entry) = colors.ansi.get_mut(index) {
                            *entry = color;
                        }
                    }
                }
                "background" => background = hex(value).or(background),
                "foreground" => foreground = hex(value).or(foreground),
                "cursor-color" => cursor = hex(value).or(cursor),
                "selection-background" => {
                    selection_background = hex(value).or(selection_background)
                }
                "selection-foreground" => {
                    selection_foreground = hex(value).or(selection_foreground)
                }
                _ => {}
            }
        }
        colors.background = background?;
        colors.foreground = foreground?;
        colors.cursor = cursor.unwrap_or(colors.foreground);
        Some(Self {
            id: slug(name),
            name: name.to_owned(),
            colors,
            selection_background,
            selection_foreground,
        })
    }

    /// Whether the background is darker than the foreground.
    pub fn is_dark(&self) -> bool {
        luma(self.colors.background) < luma(self.colors.foreground)
    }
}

/// `#rrggbb` or `rrggbb`. Named colors and Ghostty's `cell-foreground`
/// keywords are not colors here.
fn hex(value: &str) -> Option<u32> {
    let value = value.trim();
    let digits = value.strip_prefix('#').unwrap_or(value);
    (digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(digits, 16).ok())
        .flatten()
}

fn luma(color: u32) -> u32 {
    let [_, r, g, b] = color.to_be_bytes();
    u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114
}

/// The identifier settings store for a theme or palette name: lowercase ASCII
/// letters and digits, every other run of characters a single `-`.
pub fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ghostty_theme_file_gives_the_terminal_colors() {
        let text = "# comment\npalette = 0=#45475a\npalette = 15=a6adc8\npalette = 99=#ffffff\n\
                    background = #1e1e2e\nforeground = #cdd6f4\ncursor-color = #f5e0dc\n\
                    cursor-text = #1e1e2e\nselection-background = #f5e0dc\n\
                    selection-foreground = cell-background\nfont-size = 12\n";
        let palette = TerminalPalette::parse("Catppuccin Mocha", text).unwrap();
        assert_eq!(palette.id, "catppuccin-mocha");
        assert_eq!(palette.colors.ansi[0], 0x45475a);
        assert_eq!(palette.colors.ansi[15], 0xa6adc8);
        assert_eq!(palette.colors.ansi[1], TerminalColors::default().ansi[1]);
        assert_eq!(palette.colors.background, 0x1e1e2e);
        assert_eq!(palette.colors.cursor, 0xf5e0dc);
        assert_eq!(palette.selection_background, Some(0xf5e0dc));
        assert_eq!(palette.selection_foreground, None);
        assert!(palette.is_dark());
    }

    #[test]
    fn text_without_both_base_colors_or_over_the_size_limit_is_no_palette() {
        assert!(TerminalPalette::parse("x", "background = #000000\n").is_none());
        assert!(TerminalPalette::parse("x", "name = \"A theme\"\nvariant = \"dark\"\n").is_none());
        let long = format!(
            "background = #000000\nforeground = #ffffff\n{}",
            "#".repeat(MAX_PALETTE_BYTES)
        );
        assert!(TerminalPalette::parse("x", &long).is_none());
        let palette =
            TerminalPalette::parse("Day", "background = #ffffff\nforeground = #000000\n").unwrap();
        assert_eq!(palette.colors.cursor, 0x000000);
        assert!(!palette.is_dark());
    }

    #[test]
    fn names_become_identifiers() {
        assert_eq!(slug("Splitty Dark"), "splitty-dark");
        assert_eq!(
            slug("Analog Heartbeat: Phosphor"),
            "analog-heartbeat-phosphor"
        );
        assert_eq!(slug("  macOS Classic (Light) "), "macos-classic-light");
        assert_eq!(slug("12-bit Rainbow"), "12-bit-rainbow");
    }
}
