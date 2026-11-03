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
//! Every role loads packaged executable UI. Remote windows receive their fixed
//! host through native policy, and never gain a main-window command grant.
//!
//! The decisions are pure functions of the URL so they can be tested without a
//! webview; `follow` and `follow_new_window` carry them out.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Url, Wry};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

const MAX_POPOUTS: usize = 32;
#[derive(Default)]
struct PopoutRegistry {
    next: u64,
    entries: HashMap<String, (String, u64, bool)>,
}
impl PopoutRegistry {
    fn reserve(&mut self, parent: &str, document: u64) -> Option<String> {
        if self.entries.len() >= MAX_POPOUTS {
            return None;
        }
        self.next = self.next.checked_add(1)?;
        let label = format!("popout-{}", self.next);
        self.entries
            .insert(label.clone(), (parent.to_owned(), document, false));
        Some(label)
    }
    fn owned(&self, label: &str, parent: &str, document: u64) -> bool {
        self.entries
            .get(label)
            .is_some_and(|owner| owner.0 == parent && owner.1 == document && !owner.2)
    }
    fn revoke(&mut self, parent: &str) -> Vec<String> {
        let labels: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, owner)| owner.0 == parent && !owner.2)
            .map(|(label, _)| label.clone())
            .collect();
        for label in &labels {
            self.entries.get_mut(label).expect("reserved popout").2 = true;
        }
        labels
    }
    fn start_close(&mut self, label: &str, parent: &str, document: u64) -> bool {
        if !self.owned(label, parent, document) {
            return false;
        }
        self.entries.get_mut(label).expect("owned popout").2 = true;
        true
    }
}

/// Parent-owned browser popouts are real native windows, without a broker
/// registration or command capability. Reservations also count pending builds.
#[derive(Default)]
pub(crate) struct Popouts(Mutex<PopoutRegistry>);
impl Popouts {
    pub(crate) fn close_owned(
        &self,
        app: &AppHandle,
        parent: &str,
        document: u64,
        label: &str,
        current: impl FnOnce() -> bool,
    ) -> Result<(), String> {
        let allowed = {
            let mut registry = self.0.lock().expect("native popouts");
            current() && registry.start_close(label, parent, document)
        };
        if !allowed {
            return Err("native popout close denied".into());
        }
        if let Some(window) = app.get_webview_window(label) {
            window.destroy().map_err(|error| error.to_string())?;
        }
        Ok(())
    }
    /// Lock order is popouts -> native revocation. Publication takes the same
    /// lock before checking the native document, so it cannot outlive teardown.
    pub(crate) fn revoke_parent(&self, app: &AppHandle, parent: &str, revoke: impl FnOnce()) {
        let labels = {
            let mut registry = self.0.lock().expect("native popouts");
            revoke();
            registry.revoke(parent)
        };
        for label in labels {
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.destroy();
            }
        }
    }
    pub(crate) fn destroyed(&self, app: &AppHandle, label: &str, revoke: impl FnOnce()) {
        self.revoke_parent(app, label, revoke);
        self.0.lock().expect("native popouts").entries.remove(label);
    }
    pub(crate) fn open(
        &self,
        app: &AppHandle,
        parent: &str,
        document: u64,
        features: tauri::webview::NewWindowFeatures,
        current: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> NewWindowResponse<Wry> {
        let label = {
            let mut registry = self.0.lock().expect("native popouts");
            if !current() {
                return NewWindowResponse::Deny;
            }
            let Some(label) = registry.reserve(parent, document) else {
                return NewWindowResponse::Deny;
            };
            label
        };
        let nav_current = current.clone();
        let nav_app = app.clone();
        let links_app = app.clone();
        let initial_navigation = AtomicBool::new(true);
        let close_script = format!(
            r#"(() => {{
          const fallback = window.close.bind(window);
          Object.defineProperty(window, 'close', {{ value() {{
            try {{
              const parent = window.opener?.__SPLITTY_TAURI__;
              if (parent) {{ void parent.invoke('close_popout', {{ label: {} }}).catch(fallback); return; }}
            }} catch {{}}
            fallback();
          }} }});
        }})();"#,
            serde_json::to_string(&label).expect("native popout label")
        );
        let built = WebviewWindowBuilder::new(
            app,
            &label,
            WebviewUrl::External("about:blank".parse().expect("native blank entry")),
        )
        .window_features(features)
        .initialization_script(close_script)
        .title("Splitty")
        .disable_drag_drop_handler()
        .on_navigation(move |url| {
            let initial = initial_navigation.swap(false, Ordering::Relaxed);
            nav_current()
                && follow(
                    &nav_app,
                    url,
                    if (initial && url.as_str() == "about:blank") || packaged_popout(url) {
                        Decision::Allow
                    } else {
                        external_or_block(url)
                    },
                )
        })
        .on_new_window(move |url, _| follow_new_window(&links_app, &url, external_or_block(&url)))
        .on_download(|_, _| false)
        .build();
        let window = match built {
            Ok(window) => window,
            Err(error) => {
                self.0
                    .lock()
                    .expect("native popouts")
                    .entries
                    .remove(&label);
                tracing::warn!("could not create native popout: {error}");
                return NewWindowResponse::Deny;
            }
        };
        #[cfg(target_os = "linux")]
        {
            let owned = window.clone();
            if window
                .with_webview(move |platform| {
                    use gtk::prelude::WidgetExt;
                    // Wry's default GTK close handler destroys only the WebView,
                    // leaving its native toplevel as an empty white window.
                    platform.inner().connect_destroy(move |_| {
                        let owned = owned.clone();
                        // Wry destroys the widget in its first close handler.
                        // Defer native teardown until that signal/any existing
                        // runtime window-map borrow has finished.
                        webkit2gtk::glib::idle_add_once(move || {
                            let _ = owned.destroy();
                        });
                    });
                })
                .is_err()
            {
                if let Some(owner) = self
                    .0
                    .lock()
                    .expect("native popouts")
                    .entries
                    .get_mut(&label)
                {
                    owner.2 = true;
                }
                let _ = window.destroy();
                return NewWindowResponse::Deny;
            }
        }
        let publish = {
            let mut registry = self.0.lock().expect("native popouts");
            let publish = current() && registry.owned(&label, parent, document);
            if !publish {
                if let Some(owner) = registry.entries.get_mut(&label) {
                    owner.2 = true;
                }
            }
            publish
        };
        if !publish {
            let _ = window.destroy();
            return NewWindowResponse::Deny;
        }
        NewWindowResponse::Create { window }
    }
}

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

/// A packaged document role is assigned by the native window builder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DocumentRole {
    Main,
    Remote,
    Setup,
}

/// Exact packaged authority and entry path. Queries never confer authority.
/// Deliberate development servers are excluded from production native grants.
pub(crate) fn packaged_entry(url: &Url, role: DocumentRole) -> bool {
    #[cfg(target_os = "windows")]
    let authority = url.scheme() == "http" && url.host_str() == Some("tauri.localhost");
    #[cfg(not(target_os = "windows"))]
    let authority = url.scheme() == "tauri" && url.host_str() == Some("localhost");
    authority
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && match role {
            DocumentRole::Main | DocumentRole::Remote => {
                matches!(url.path(), "" | "/" | "/index.html")
            }
            DocumentRole::Setup => url.path() == "/setup.html",
        }
}

pub(crate) fn packaged_navigation(url: &Url, role: DocumentRole) -> Decision {
    if packaged_entry(url, role) {
        Decision::Allow
    } else {
        external_or_block(url)
    }
}

/// Only the parent-owned popout entry may acquire a shared script reference.
/// It receives no independent native registration or command grant.
pub(crate) fn packaged_new_window(url: &Url, role: DocumentRole) -> Decision {
    if role == DocumentRole::Main && packaged_popout(url) {
        Decision::Allow
    } else {
        external_or_block(url)
    }
}

fn packaged_popout(url: &Url) -> bool {
    let mut entry = url.clone();
    entry.set_path("/index.html");
    url.path() == "/popout.html" && packaged_entry(&entry, DocumentRole::Main)
}

pub(crate) fn packaged_download(url: &Url, role: DocumentRole) -> bool {
    if role != DocumentRole::Main || url.scheme() != "blob" {
        return false;
    }
    let Ok(mut source) = Url::parse(url.path()) else {
        return false;
    };
    source.set_path("/index.html");
    packaged_entry(&source, DocumentRole::Main)
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
        // Allowed popouts must use the explicitly owned Create path above.
        Decision::Allow => NewWindowResponse::Deny,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popout_reservations_bound_pending_builds_and_never_reuse_labels() {
        let mut registry = PopoutRegistry::default();
        let labels: Vec<_> = (0..MAX_POPOUTS)
            .map(|_| registry.reserve("main", 7).expect("available popout"))
            .collect();
        assert!(registry.reserve("main", 7).is_none());
        registry.entries.remove(&labels[0]);
        let replacement = registry.reserve("main", 7).unwrap();
        assert!(!labels.contains(&replacement));
        assert_eq!(registry.entries.len(), MAX_POPOUTS);
    }

    #[test]
    fn parent_revocation_cancels_pending_popouts_and_only_its_own_children() {
        let mut registry = PopoutRegistry::default();
        let pending = registry.reserve("main", 7).unwrap();
        let sibling = registry.reserve("other-native-parent", 9).unwrap();
        assert!(!registry.owned(&pending, "main", 8));
        assert_eq!(registry.revoke("main"), vec![pending.clone()]);
        assert!(
            !registry.owned(&pending, "main", 7),
            "a late build cannot publish"
        );
        assert!(registry.owned(&sibling, "other-native-parent", 9));
        assert!(registry.revoke("main").is_empty());
    }

    #[test]
    fn parent_close_denies_other_documents_and_keeps_capacity_until_destruction() {
        let mut registry = PopoutRegistry::default();
        let own = registry.reserve("main", 7).unwrap();
        let foreign = registry.reserve("other-native-parent", 9).unwrap();
        for label in ["main", "remote-1", foreign.as_str(), "unknown"] {
            assert!(!registry.start_close(label, "main", 7));
        }
        assert!(!registry.start_close(&own, "main", 8));
        assert!(registry.owned(&own, "main", 7));
        assert!(registry.start_close(&own, "main", 7));
        assert!(!registry.start_close(&own, "main", 7));
        assert_eq!(
            registry.entries.len(),
            2,
            "close does not establish native destruction"
        );
        registry.entries.remove(&own); // actual Destroyed callback
        assert_eq!(registry.entries.len(), 1);
        assert!(registry.owned(&foreign, "other-native-parent", 9));
    }

    fn url(raw: &str) -> Url {
        raw.parse().expect("valid url")
    }

    fn packaged(path: &str) -> Url {
        #[cfg(target_os = "windows")]
        let origin = "http://tauri.localhost";
        #[cfg(not(target_os = "windows"))]
        let origin = "tauri://localhost";
        url(&format!("{origin}{path}"))
    }

    #[test]
    fn packaged_roles_require_exact_authorities_and_entry_paths() {
        for path in ["", "/", "/index.html", "/index.html?role=setup"] {
            assert!(
                packaged_entry(&packaged(path), DocumentRole::Main),
                "{path}"
            );
            assert!(
                packaged_entry(&packaged(path), DocumentRole::Remote),
                "{path}"
            );
            assert!(
                !packaged_entry(&packaged(path), DocumentRole::Setup),
                "{path}"
            );
        }
        for raw in [
            "tauri://localhost.evil/index.html",
            "tauri://user@localhost/index.html",
            "http://tauri.localhost:1420/",
            "https://tauri.localhost/",
            "http://localhost:1420/",
            "about:blank",
        ] {
            assert!(!packaged_entry(&url(raw), DocumentRole::Main), "{raw}");
        }
        #[cfg(not(target_os = "windows"))]
        assert!(!packaged_entry(
            &url("http://tauri.localhost/"),
            DocumentRole::Main
        ));
        #[cfg(target_os = "windows")]
        assert!(!packaged_entry(
            &url("tauri://localhost/"),
            DocumentRole::Main
        ));
        for path in ["/popout.html", "/setup.html", "/other.html"] {
            assert!(!packaged_entry(&packaged(path), DocumentRole::Main));
        }
        assert!(packaged_entry(
            &packaged("/setup.html"),
            DocumentRole::Setup
        ));
        assert_eq!(
            packaged_new_window(&packaged("/popout.html"), DocumentRole::Main),
            Decision::Allow
        );
        assert_ne!(
            packaged_new_window(&packaged("/popout.html"), DocumentRole::Remote),
            Decision::Allow
        );
        let blob = url(&format!("blob:{}", packaged("/id")));
        assert!(packaged_download(&blob, DocumentRole::Main));
        assert!(!packaged_download(&blob, DocumentRole::Remote));
        assert!(!packaged_download(
            &url("blob:http://127.0.0.1:1420/id"),
            DocumentRole::Main
        ));
    }

    #[test]
    fn external_navigation_never_loads_foreign_executable_content() {
        for role in [
            DocumentRole::Main,
            DocumentRole::Remote,
            DocumentRole::Setup,
        ] {
            assert_eq!(
                packaged_navigation(&url("https://example.com/"), role),
                Decision::OpenExternally
            );
            for raw in [
                "javascript:alert(1)",
                "data:text/html,test",
                "file:///etc/passwd",
                "about:blank",
                "vscode://file/tmp/",
            ] {
                assert_eq!(
                    packaged_navigation(&url(raw), role),
                    Decision::Block,
                    "{raw}"
                );
                assert_eq!(
                    packaged_new_window(&url(raw), role),
                    Decision::Block,
                    "{raw}"
                );
            }
        }
    }
}
