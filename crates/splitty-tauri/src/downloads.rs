//! The main window's downloads (`splitty download`, `terminal-sessions`, Saving
//! An Offered File) and showing a saved one in the file manager.
//!
//! The page names a download only by its `blob:` URL, which it made for exactly
//! that download. The shell shows only a file it saved itself and finished, and
//! never tells the page where (`access-control`, Showing A Saved Download).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri_plugin_opener::OpenerExt;

/// Downloads remembered at once; an older one can no longer be shown.
const KEEP: usize = 32;

/// The downloads of the main window, by URL.
#[derive(Default)]
pub struct Downloads {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Requested and not finished yet, with the destination the webview got.
    pending: VecDeque<(Owner, String, PathBuf)>,
    /// Finished, with where the file is.
    saved: VecDeque<(Owner, String, PathBuf)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Owner {
    pub(crate) window: String,
    pub(crate) document: u64,
}

impl Downloads {
    /// The webview starts saving `url` to `destination`.
    pub(crate) fn requested(&self, owner: Owner, url: &str, destination: &Path) {
        let mut inner = self.lock();
        remember(&mut inner.pending, owner, url, destination.to_path_buf());
    }

    /// The webview finished `url`. `path` is where it saved the file; macOS
    /// reports none, and then the requested destination is where it went.
    pub(crate) fn finished(&self, owner: &Owner, url: &str, path: Option<PathBuf>, success: bool) {
        let mut inner = self.lock();
        let destination = inner
            .pending
            .iter()
            .position(|(pending_owner, pending, _)| pending_owner == owner && pending == url)
            .and_then(|index| inner.pending.remove(index))
            .map(|(_, _, destination)| destination);
        if !success || destination.is_none() {
            return;
        }
        if let Some(path) = path.or(destination).filter(|path| path.is_absolute()) {
            remember(&mut inner.saved, owner.clone(), url, path);
        }
    }

    /// Where the finished download of `url` is.
    pub(crate) fn saved(&self, owner: &Owner, url: &str) -> Option<PathBuf> {
        self.lock()
            .saved
            .iter()
            .find(|(saved_owner, saved, _)| saved_owner == owner && saved == url)
            .map(|(_, _, path)| path.clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves the lists consistent.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Put `url` at the end of `list`, once, dropping the oldest beyond `KEEP`.
fn remember(list: &mut VecDeque<(Owner, String, PathBuf)>, owner: Owner, url: &str, path: PathBuf) {
    list.retain(|(known_owner, known, _)| known_owner != &owner || known != url);
    if list.len() == KEEP {
        list.pop_front();
    }
    list.push_back((owner, url.to_string(), path));
}

/// Show the finished download of `url` in the file manager, the file selected.
#[tauri::command]
pub fn reveal_download(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    native: tauri::State<'_, Arc<crate::client_ipc::carrier::Native>>,
    auth: crate::client_ipc::carrier::Auth,
    downloads: tauri::State<'_, Arc<Downloads>>,
    url: String,
) -> Result<(), String> {
    native.main_action(&webview, &auth)?;
    let proof = auth.proof();
    let owner = Owner {
        window: webview.label().to_owned(),
        document: proof.document,
    };
    let path = downloads
        .saved(&owner, &url)
        .ok_or_else(|| "no finished download of this URL".to_string())?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "blob:http://127.0.0.1:45299/0d1c6f2e-6a4b-4f63-9d8e-2b7a1c3e5f90";

    fn owner() -> Owner {
        Owner {
            window: "main".into(),
            document: 1,
        }
    }

    fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn a_finished_download_is_found_by_its_url() {
        let downloads = Downloads::default();
        downloads.requested(owner(), URL, &path("report.pdf"));
        assert_eq!(downloads.saved(&owner(), URL), None, "not finished yet");
        downloads.finished(&owner(), URL, Some(path("report (1).pdf")), true);
        assert_eq!(downloads.saved(&owner(), URL), Some(path("report (1).pdf")));
        assert_eq!(
            downloads.saved(&owner(), "blob:http://127.0.0.1:45299/other"),
            None
        );
    }

    #[test]
    fn without_a_reported_path_the_destination_is_where_it_went() {
        let downloads = Downloads::default();
        downloads.requested(owner(), URL, &path("report.pdf"));
        downloads.finished(&owner(), URL, None, true);
        assert_eq!(downloads.saved(&owner(), URL), Some(path("report.pdf")));
    }

    #[test]
    fn a_failed_or_unrequested_download_is_not_shown() {
        let downloads = Downloads::default();
        downloads.requested(owner(), URL, &path("report.pdf"));
        downloads.finished(&owner(), URL, Some(path("report.pdf")), false);
        assert_eq!(downloads.saved(&owner(), URL), None);
        downloads.finished(&owner(), URL, None, true);
        assert_eq!(
            downloads.saved(&owner(), URL),
            None,
            "nothing to fall back to"
        );
        downloads.finished(&owner(), URL, Some(PathBuf::from("relative.pdf")), true);
        assert_eq!(
            downloads.saved(&owner(), URL),
            None,
            "only an absolute path"
        );
    }

    #[test]
    fn finished_paths_require_a_matching_request_and_document_owner() {
        let downloads = Downloads::default();
        let other = Owner {
            window: "remote-1".into(),
            document: 1,
        };
        let next = Owner {
            window: "main".into(),
            document: 2,
        };
        downloads.finished(&owner(), URL, Some(path("unrequested")), true);
        assert_eq!(downloads.saved(&owner(), URL), None);
        downloads.requested(owner(), URL, &path("owned"));
        downloads.finished(&other, URL, Some(path("forged")), true);
        assert_eq!(downloads.saved(&other, URL), None);
        downloads.finished(&owner(), URL, None, true);
        assert_eq!(downloads.saved(&owner(), URL), Some(path("owned")));
        assert_eq!(downloads.saved(&next, URL), None);
        assert_eq!(downloads.saved(&other, URL), None);
    }

    #[test]
    fn only_the_most_recent_downloads_are_kept() {
        let downloads = Downloads::default();
        let url = |n: usize| format!("blob:http://127.0.0.1:45299/{n}");
        for n in 0..=KEEP {
            downloads.requested(owner(), &url(n), &path(&format!("{n}.bin")));
            downloads.finished(&owner(), &url(n), None, true);
        }
        assert_eq!(downloads.saved(&owner(), &url(0)), None);
        assert_eq!(downloads.saved(&owner(), &url(1)), Some(path("1.bin")));
        assert_eq!(
            downloads.saved(&owner(), &url(KEEP)),
            Some(path(&format!("{KEEP}.bin")))
        );
    }
}
