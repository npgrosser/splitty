//! Where the app's windows may navigate, and what they may open.
//!
//! Splitty must never render foreign web content inside its own window
//! (security review SR-01, SR-26, SR-27, see docs/security/findings.md). The
//! window holds the session cookie and Tauri IPC, and terminal output is
//! untrusted: ghostty-web, for one, opens OSC 8 hyperlinks with a bare
//! `window.open` and no scheme check. So the decision is made here, at the
//! native layer, for every navigation and every new-window request, whichever
//! code in the page asked for it.
//!
//! A remote window shows UI code delivered by a host and is stricter still: it
//! stays on its tunnel origin, never shows the app's bundled pages and opens no
//! window at all (`access-control`, Remote Window Isolation).
//!
//! The decisions are pure functions of the URL so they can be tested without a
//! webview; `follow` and `follow_new_window` carry them out.

use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Url, Wry};
use tauri_plugin_opener::OpenerExt;

/// What to do with a navigation or a new-window request.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Load it in Splitty.
    Allow,
    /// Hand it to the system browser instead.
    OpenExternally,
    /// Drop it.
    Block,
}

/// The origins of a Splitty backend listening on `port`.
///
/// Mirrors `splitty_server::http::allowed_origins`, which is not available on
/// Windows: the backend runs inside WSL there and the server crate is not a
/// dependency of this one.
pub fn backend_origins(port: u16) -> Vec<String> {
    vec![
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
    ]
}

/// Decide a top-level navigation of the main window.
///
/// `local_app` admits the bundled pages (the Windows setup screen); only the
/// Windows window ever shows one.
pub fn navigation(url: &Url, backend_origins: &[String], local_app: bool) -> Decision {
    if is_backend(url, backend_origins) || is_blank(url) {
        return Decision::Allow;
    }
    if local_app && is_local_app(url) {
        return Decision::Allow;
    }
    external_or_block(url)
}

/// Decide a `window.open` or `target="_blank"` request.
///
/// Windows of the app itself (dockview popouts) stay in Splitty. Every other web
/// target goes to the system browser, and anything else is dropped.
pub fn new_window(url: &Url, backend_origins: &[String]) -> Decision {
    if is_backend(url, backend_origins) || is_blank(url) {
        return Decision::Allow;
    }
    external_or_block(url)
}

/// The origin of the tunnel a remote window shows, from its session URL.
pub fn tunnel_origin(session_url: &Url) -> String {
    session_url.origin().ascii_serialization()
}

/// Decide a top-level navigation of a remote window showing `tunnel_origin`.
///
/// Unlike the main window, not even an empty document is admitted, and the
/// bundled pages are dropped rather than handed to the system browser: one of
/// them in a window that runs host code would be the app's own page under the
/// host's control.
pub fn remote_navigation(url: &Url, tunnel_origin: &str) -> Decision {
    if url.origin().ascii_serialization() == tunnel_origin {
        return Decision::Allow;
    }
    if is_local_app(url) {
        return Decision::Block;
    }
    external_or_block(url)
}

/// Decide a `window.open` or `target="_blank"` request of a remote window.
///
/// Nothing opens inside Splitty, the UI's own popouts included (D6 of
/// remote-window, provisional): a new window would need the same isolation.
/// Other web targets go to the system browser.
pub fn remote_new_window(url: &Url, tunnel_origin: &str) -> Decision {
    if url.origin().ascii_serialization() == tunnel_origin || is_local_app(url) || is_blank(url) {
        return Decision::Block;
    }
    external_or_block(url)
}

/// Decide a download of the main window: only the page's own `blob:` URLs,
/// which it makes of a file it received (`splitty download`), are saved.
/// Without a handler the webview would save whatever a link starts.
pub fn download(url: &Url, backend_origins: &[String]) -> bool {
    // A `blob:` URL's origin is that of the page that made it.
    url.scheme() == "blob" && is_backend(url, backend_origins)
}

/// Carry out `decision` on a navigation to `url`: whether the webview loads
/// it.
pub fn follow(app: &AppHandle, url: &Url, decision: Decision) -> bool {
    match decision {
        Decision::Allow => true,
        Decision::OpenExternally => {
            open_externally(app, url);
            false
        }
        Decision::Block => {
            // The scheme only: a URL can carry anything, credentials
            // included, and this ends up in logs.
            tracing::warn!("blocked navigation to a {}: URL", url.scheme());
            false
        }
    }
}

/// Carry out `decision` on a new-window request for `url`.
pub fn follow_new_window(app: &AppHandle, url: &Url, decision: Decision) -> NewWindowResponse<Wry> {
    match decision {
        Decision::Allow => NewWindowResponse::Allow,
        Decision::OpenExternally => {
            open_externally(app, url);
            NewWindowResponse::Deny
        }
        Decision::Block => {
            tracing::warn!("blocked new window for a {}: URL", url.scheme());
            NewWindowResponse::Deny
        }
    }
}

fn open_externally(app: &AppHandle, url: &Url) {
    if let Err(error) = app.opener().open_url(url.as_str(), None::<&str>) {
        tracing::warn!("could not open link in the system browser: {error}");
    }
}

fn external_or_block(url: &Url) -> Decision {
    match url.scheme() {
        "http" | "https" => Decision::OpenExternally,
        // `javascript:`, `data:`, `file:`, `blob:` and every OS protocol handler
        // (`search-ms:`, `ms-msdt:`, ...) end here.
        _ => Decision::Block,
    }
}

fn is_backend(url: &Url, backend_origins: &[String]) -> bool {
    let origin = url.origin().ascii_serialization();
    backend_origins.contains(&origin)
}

/// An empty document. Webviews use it as the initial page of new windows, and
/// it carries no content of its own.
fn is_blank(url: &Url) -> bool {
    url.as_str() == "about:blank"
}

/// The bundled frontend: `tauri://localhost` on macOS and Linux,
/// `http(s)://tauri.localhost` on Windows, and the Vite dev server in debug
/// builds.
fn is_local_app(url: &Url) -> bool {
    let bundled = match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "http" | "https" => url.host_str() == Some("tauri.localhost"),
        _ => false,
    };
    bundled || DEV_ORIGINS.contains(&url.origin().ascii_serialization().as_str())
}

/// The Vite dev server that `cargo tauri dev` loads the bundled pages from.
/// Compiled out of release builds, like the matching entry in the server's
/// origin allowlist, so a shipped binary cannot be talked into trusting it.
#[cfg(debug_assertions)]
const DEV_ORIGINS: &[&str] = &["http://localhost:1420", "http://127.0.0.1:1420"];
#[cfg(not(debug_assertions))]
const DEV_ORIGINS: &[&str] = &[];

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        raw.parse().expect("valid url")
    }

    fn backend() -> Vec<String> {
        backend_origins(45299)
    }

    /// file-transfer D10: the page's own blob URLs are saved, nothing else.
    #[test]
    fn only_the_pages_own_blob_urls_are_downloaded() {
        let own = url("blob:http://127.0.0.1:45299/550e8400-e29b-41d4-a716-446655440000");
        assert!(download(&own, &backend()));
        let localhost = url("blob:http://localhost:45299/550e8400-e29b-41d4-a716-446655440000");
        assert!(download(&localhost, &backend()));
        for other in [
            "blob:http://127.0.0.1:3000/550e8400-e29b-41d4-a716-446655440000",
            "blob:https://example.com/550e8400-e29b-41d4-a716-446655440000",
            "http://127.0.0.1:45299/api/file",
            "https://example.com/setup.exe",
            "data:application/octet-stream;base64,AAAA",
            "file:///etc/passwd",
        ] {
            assert!(!download(&url(other), &backend()), "{other}");
        }
    }

    #[test]
    fn the_backend_may_load_in_the_window() {
        let target = url("http://127.0.0.1:45299/?bootstrap=abc&desktop=1");
        assert_eq!(navigation(&target, &backend(), false), Decision::Allow);
    }

    /// Every other loopback port is a different origin. A page there could
    /// otherwise reach Tauri IPC from inside the trusted window.
    #[test]
    fn another_loopback_port_is_not_the_backend() {
        let target = url("http://127.0.0.1:5173/");
        assert_eq!(
            navigation(&target, &backend(), false),
            Decision::OpenExternally
        );
        assert_eq!(new_window(&target, &backend()), Decision::OpenExternally);
    }

    #[test]
    fn foreign_sites_go_to_the_system_browser() {
        let target = url("https://example.com/");
        assert_eq!(
            navigation(&target, &backend(), true),
            Decision::OpenExternally
        );
        assert_eq!(new_window(&target, &backend()), Decision::OpenExternally);
    }

    /// What a terminal program can put into an OSC 8 hyperlink.
    #[test]
    fn non_web_schemes_are_dropped() {
        for raw in [
            "file:///C:/Windows/System32/calc.exe",
            "search-ms:query=x&crumb=location:evil.example",
            "ms-msdt:/id",
            "javascript:alert(1)",
            "vscode://vscode-remote/ssh-remote+vm1/srv/",
            "zed://ssh/vm1/srv/",
            "sftp://vm1/srv/",
            "data:text/html,<script>alert(1)</script>",
        ] {
            let target = url(raw);
            assert_eq!(
                navigation(&target, &backend(), true),
                Decision::Block,
                "{raw}"
            );
            assert_eq!(new_window(&target, &backend()), Decision::Block, "{raw}");
        }
    }

    #[test]
    fn app_popouts_stay_in_splitty() {
        let target = url("http://127.0.0.1:45299/popout.html");
        assert_eq!(new_window(&target, &backend()), Decision::Allow);
        assert_eq!(new_window(&url("about:blank"), &backend()), Decision::Allow);
    }

    #[test]
    fn the_setup_page_is_only_allowed_where_requested() {
        let windows = url("http://tauri.localhost/setup.html");
        let unix = url("tauri://localhost/setup.html");

        assert_eq!(navigation(&windows, &[], true), Decision::Allow);
        assert_eq!(navigation(&unix, &[], true), Decision::Allow);
        assert_eq!(navigation(&windows, &[], false), Decision::OpenExternally);
        assert_eq!(navigation(&unix, &[], false), Decision::Block);
    }

    /// Before the WSL backend is up there is no backend origin yet, and the
    /// window must not be able to leave the setup page for a loopback port.
    #[test]
    fn no_backend_origin_means_no_loopback_page() {
        let target = url("http://127.0.0.1:45299/");
        assert_eq!(navigation(&target, &[], true), Decision::OpenExternally);
    }

    #[test]
    fn a_lookalike_host_is_not_the_bundled_app() {
        let target = url("http://tauri.localhost.evil.example/setup.html");
        assert_eq!(navigation(&target, &[], true), Decision::OpenExternally);
    }

    fn tunnel() -> String {
        tunnel_origin(&url("http://127.0.0.1:9800/?bootstrap=abc&host=vm1"))
    }

    #[test]
    fn a_remote_window_stays_on_its_tunnel() {
        assert_eq!(tunnel(), "http://127.0.0.1:9800");
        for raw in [
            "http://127.0.0.1:9800/",
            "http://127.0.0.1:9800/?bootstrap=def&host=vm1",
        ] {
            assert_eq!(
                remote_navigation(&url(raw), &tunnel()),
                Decision::Allow,
                "{raw}"
            );
        }
    }

    /// The main window's backend, another tunnel and `localhost` are other
    /// origins, so the host's page cannot load them in its window.
    #[test]
    fn a_remote_window_hands_other_web_pages_to_the_system_browser() {
        for raw in [
            "https://example.com/",
            "http://127.0.0.1:45299/",
            "http://localhost:9800/",
            "https://127.0.0.1:9800/",
        ] {
            assert_eq!(
                remote_navigation(&url(raw), &tunnel()),
                Decision::OpenExternally,
                "{raw}"
            );
        }
    }

    #[test]
    fn a_remote_window_never_shows_a_bundled_page() {
        for raw in [
            "tauri://localhost/setup.html",
            "http://tauri.localhost/setup.html",
            "https://tauri.localhost/index.html",
        ] {
            assert_eq!(
                remote_navigation(&url(raw), &tunnel()),
                Decision::Block,
                "{raw}"
            );
            assert_eq!(
                remote_new_window(&url(raw), &tunnel()),
                Decision::Block,
                "{raw}"
            );
        }
    }

    #[test]
    fn a_remote_window_drops_other_schemes() {
        for raw in [
            "about:blank",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "search-ms:query=x",
            "vscode://vscode-remote/ssh-remote+vm1/srv/",
            "zed://ssh/vm1/srv/",
            "sftp://vm1/srv/",
        ] {
            assert_eq!(
                remote_navigation(&url(raw), &tunnel()),
                Decision::Block,
                "{raw}"
            );
            assert_eq!(
                remote_new_window(&url(raw), &tunnel()),
                Decision::Block,
                "{raw}"
            );
        }
    }

    #[test]
    fn a_remote_window_opens_no_window_of_its_own() {
        for raw in ["http://127.0.0.1:9800/popout.html", "about:blank"] {
            assert_eq!(
                remote_new_window(&url(raw), &tunnel()),
                Decision::Block,
                "{raw}"
            );
        }
        assert_eq!(
            remote_new_window(&url("https://example.com/"), &tunnel()),
            Decision::OpenExternally
        );
    }
}
