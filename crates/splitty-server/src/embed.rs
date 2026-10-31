//! Embeds the compiled frontend SPA assets using `rust_embed` and provides
//! `themed_index_html()` which injects the resolved Splitty theme colors into
//! the `index.html` placeholders before serving.

use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../frontend/dist"]
pub struct FrontendAssets;

/// Returns index.html with Splitty theme colors injected.
pub fn themed_index_html() -> Option<Vec<u8>> {
    let file = FrontendAssets::get("index.html")?;
    let html = String::from_utf8_lossy(&file.data);

    let config = splitty_core::config::load();
    let themes_dir = splitty_core::config::themes_dir();

    // When theme mode is "auto", use the dark theme for the initial HTML render.
    // The frontend will detect the OS preference and correct it immediately.
    let theme_name = if config.appearance.theme_mode == "auto" {
        &config.appearance.theme_dark
    } else {
        &config.appearance.theme
    };
    let theme = splitty_core::theme::resolve_theme(theme_name, &themes_dir);

    let html = html
        .replace("__SPLITTY_BG__", &theme.ui.surface_base)
        .replace("__SPLITTY_FG__", &theme.ui.text_primary);

    Some(html.into_bytes())
}
