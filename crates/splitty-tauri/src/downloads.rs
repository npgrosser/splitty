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
    pending: VecDeque<(String, PathBuf)>,
    /// Finished, with where the file is.
    saved: VecDeque<(String, PathBuf)>,
}

impl Downloads {
    /// The webview starts saving `url` to `destination`.
    pub fn requested(&self, url: &str, destination: &Path) {
        let mut inner = self.lock();
        remember(&mut inner.pending, url, destination.to_path_buf());
    }

    /// The webview finished `url`. `path` is where it saved the file; macOS
    /// reports none, and then the requested destination is where it went.
    pub fn finished(&self, url: &str, path: Option<PathBuf>, success: bool) {
        let mut inner = self.lock();
        let destination = inner
            .pending
            .iter()
            .position(|(pending, _)| pending == url)
            .and_then(|index| inner.pending.remove(index))
            .map(|(_, destination)| destination);
        if !success {
            return;
        }
        if let Some(path) = path.or(destination).filter(|path| path.is_absolute()) {
            remember(&mut inner.saved, url, path);
        }
    }

    /// Where the finished download of `url` is.
    pub fn saved(&self, url: &str) -> Option<PathBuf> {
        self.lock()
            .saved
            .iter()
            .find(|(saved, _)| saved == url)
            .map(|(_, path)| path.clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves the lists consistent.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Put `url` at the end of `list`, once, dropping the oldest beyond `KEEP`.
fn remember(list: &mut VecDeque<(String, PathBuf)>, url: &str, path: PathBuf) {
    list.retain(|(known, _)| known != url);
    if list.len() == KEEP {
        list.pop_front();
    }
    list.push_back((url.to_string(), path));
}

/// Show the finished download of `url` in the file manager, the file selected.
#[tauri::command]
pub fn reveal_download(
    app: tauri::AppHandle,
    downloads: tauri::State<'_, Arc<Downloads>>,
    url: String,
) -> Result<(), String> {
    let path = downloads
        .saved(&url)
        .ok_or_else(|| "no finished download of this URL".to_string())?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "blob:http://127.0.0.1:45299/0d1c6f2e-6a4b-4f63-9d8e-2b7a1c3e5f90";

    fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn a_finished_download_is_found_by_its_url() {
        let downloads = Downloads::default();
        downloads.requested(URL, &path("report.pdf"));
        assert_eq!(downloads.saved(URL), None, "not finished yet");
        downloads.finished(URL, Some(path("report (1).pdf")), true);
        assert_eq!(downloads.saved(URL), Some(path("report (1).pdf")));
        assert_eq!(downloads.saved("blob:http://127.0.0.1:45299/other"), None);
    }

    #[test]
    fn without_a_reported_path_the_destination_is_where_it_went() {
        let downloads = Downloads::default();
        downloads.requested(URL, &path("report.pdf"));
        downloads.finished(URL, None, true);
        assert_eq!(downloads.saved(URL), Some(path("report.pdf")));
    }

    #[test]
    fn a_failed_or_unrequested_download_is_not_shown() {
        let downloads = Downloads::default();
        downloads.requested(URL, &path("report.pdf"));
        downloads.finished(URL, Some(path("report.pdf")), false);
        assert_eq!(downloads.saved(URL), None);
        downloads.finished(URL, None, true);
        assert_eq!(downloads.saved(URL), None, "nothing to fall back to");
        downloads.finished(URL, Some(PathBuf::from("relative.pdf")), true);
        assert_eq!(downloads.saved(URL), None, "only an absolute path");
    }

    #[test]
    fn only_the_most_recent_downloads_are_kept() {
        let downloads = Downloads::default();
        let url = |n: usize| format!("blob:http://127.0.0.1:45299/{n}");
        for n in 0..=KEEP {
            downloads.requested(&url(n), &path(&format!("{n}.bin")));
            downloads.finished(&url(n), None, true);
        }
        assert_eq!(downloads.saved(&url(0)), None);
        assert_eq!(downloads.saved(&url(1)), Some(path("1.bin")));
        assert_eq!(
            downloads.saved(&url(KEEP)),
            Some(path(&format!("{KEEP}.bin")))
        );
    }
}
