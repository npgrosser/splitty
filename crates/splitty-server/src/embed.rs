//! The compiled frontend SPA assets, embedded with `rust_embed` when the
//! `web-assets` feature is on (the default), and `themed_index_html()`, which
//! injects the resolved Splitty theme colors into the `index.html`
//! placeholders before serving. Without the feature there are no assets and
//! every page request finds none, as with an empty `frontend/dist`.

use std::borrow::Cow;

#[cfg(feature = "web-assets")]
#[derive(rust_embed::Embed)]
#[folder = "../../frontend/dist"]
struct FrontendAssets;

/// A file of the frontend build.
pub struct Asset {
    pub data: Cow<'static, [u8]>,
    pub content_type: String,
}

/// The file at `path` of the frontend build, if it was embedded.
#[cfg(feature = "web-assets")]
pub fn asset(path: &str) -> Option<Asset> {
    let file = FrontendAssets::get(path)?;
    let content_type = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    Some(Asset {
        data: file.data,
        content_type,
    })
}

/// The file at `path` of the frontend build: none, since this build embeds
/// no frontend.
#[cfg(not(feature = "web-assets"))]
pub fn asset(_path: &str) -> Option<Asset> {
    None
}

/// Returns index.html with Splitty theme colors injected.
pub fn themed_index_html() -> Option<Vec<u8>> {
    let file = asset("index.html")?;
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
