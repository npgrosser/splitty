//! Theme system: palette-based token derivation for UI and terminal colors.
//!
//! Themes are authored as TOML files with a ~15-color palette and a dark/light variant.
//! The resolver derives semantic UI tokens and terminal ANSI colors from the palette,
//! with optional per-token overrides. An `[effects]` section allows themes to enable
//! visual effects (glow, scanlines) that the frontend applies via CSS. Six themes are
//! bundled at compile time (dark, light, classic, retro, aurora, neon); custom themes
//! are loaded from `~/.config/splitty/themes/`.

mod bundled;
mod list;
mod resolve;
mod types;

pub use list::list_themes;
pub use resolve::{resolve_theme, resolve_theme_file};
pub use types::*;
