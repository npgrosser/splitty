//! Keeps the local service's last observed Screens of each host in step with
//! the host's live catalog. The service stores a host's Screens when a
//! connection to it becomes ready. While the host stays ready, its live
//! catalog is reported here whenever it differs from what the service holds:
//! a new Screen, a rename, or a Screen that left the host, terminated or
//! closed with its last pane, so that the host's next connection does not
//! list it again while it is not ready yet.
//!
//! A live catalog has no pane counts. The report sends none, and the service
//! keeps the counts it has stored (`hub-host-state-v1`); a service without
//! that rule is sent nothing.
use crate::workspace::{CatalogState, SourceStatus, WindowWorkspace};
use splitty_core::native_api::directory::{Directory, ObservedScreen};
use std::{collections::HashMap, io};
use uuid::Uuid;

/// The service's limit for a stored Screen name, in characters.
const MAX_NAME: usize = 200;

/// What was reported per host, for the connection whose catalog it reflects.
#[derive(Default)]
pub(super) struct HostCatalogReports {
    /// The service keeps stored pane counts for a report without them.
    enabled: bool,
    sent: HashMap<String, (Uuid, Vec<ObservedScreen>)>,
}
impl HostCatalogReports {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            sent: HashMap::new(),
        }
    }
    /// Report, through `send`, each ready host whose live catalog differs
    /// from the Screens `directory` keeps for it. A report is not repeated
    /// for the same connection while the directory has not caught up.
    pub(super) fn sync(
        &mut self,
        window: &WindowWorkspace,
        directory: Option<&Directory>,
        mut send: impl FnMut(&str, &[ObservedScreen]) -> io::Result<()>,
    ) {
        self.sent.retain(|target, _| {
            window
                .hosts()
                .any(|host| host.action_target().target() == target)
        });
        let Some(directory) = directory.filter(|_| self.enabled) else {
            return;
        };
        for host in window.hosts() {
            // Only a current, complete readback says which Screens there are.
            if host.status() != SourceStatus::Ready || host.catalog_state() != CatalogState::Ready {
                continue;
            }
            let action = host.action_target();
            let Some(source) = window.source(action.source_id()) else {
                continue;
            };
            let Some(live) = source.live_catalog() else {
                continue;
            };
            // A host the service keeps nothing for is hidden or not stored yet.
            let Some(stored) = directory
                .hosts
                .iter()
                .find(|stored| stored.target == action.target())
            else {
                continue;
            };
            // The service refuses a whole report with a longer name.
            let screens: Vec<_> = live
                .iter()
                .map(|screen| {
                    ObservedScreen::new(
                        screen.id.as_str(),
                        screen
                            .name
                            .as_ref()
                            .map(|name| name.chars().take(MAX_NAME).collect()),
                        None,
                    )
                })
                .collect();
            if stored.screens.len() == screens.len()
                && stored
                    .screens
                    .iter()
                    .zip(&screens)
                    .all(|(stored, live)| stored.id == live.id && stored.name == live.name)
            {
                continue;
            }
            let report = (source.connection_id(), screens);
            if self.sent.get(action.target()) == Some(&report) {
                continue;
            }
            if send(action.target(), &report.1).is_ok() {
                self.sent.insert(action.target().to_owned(), report);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_core::native_api::directory::{DirectoryHost, DirectoryScreen};

    const HOST: &str = "one.test";

    fn stored(screens: &[(&str, u64)]) -> Directory {
        let mut host = DirectoryHost::new(HOST);
        host.screens = screens
            .iter()
            .map(|(id, panes)| DirectoryScreen::new(*id, Some((*id).to_owned()), *panes))
            .collect();
        Directory::new(Vec::new(), vec![host])
    }

    fn reports(
        memory: &mut HostCatalogReports,
        window: &WindowWorkspace,
        directory: &Directory,
    ) -> Vec<(String, Vec<String>)> {
        let mut sent = Vec::new();
        memory.sync(window, Some(directory), |target, screens| {
            sent.push((
                target.to_owned(),
                screens.iter().map(|screen| screen.id.clone()).collect(),
            ));
            Ok(())
        });
        sent
    }

    /// The window with `screens` open on a ready host, as the service stored
    /// them when the host's connection became ready.
    fn opened(screens: &[&str]) -> (WindowWorkspace, crate::workspace::SourceId, Directory) {
        let mut window = WindowWorkspace::new(Uuid::new_v4());
        let named: Vec<_> = screens.iter().map(|id| (*id, *id)).collect();
        let id = window
            .ui_test_screen_order_fixture(&named, Some(HOST))
            .unwrap()[0]
            .source_id();
        let directory = stored(&screens.iter().map(|id| (*id, 2)).collect::<Vec<_>>());
        (window, id, directory)
    }

    #[test]
    fn a_screen_that_left_the_host_leaves_its_last_observation() {
        let (mut window, id, directory) = opened(&["work", "build"]);
        let mut memory = HostCatalogReports::new(true);
        assert!(
            reports(&mut memory, &window, &directory).is_empty(),
            "the stored Screens are the live ones"
        );

        // The host's readback after a confirmed terminate or a last pane's exit.
        window.ui_test_remote_catalog(id, &["build"]).unwrap();
        assert_eq!(
            reports(&mut memory, &window, &directory),
            [(HOST.to_owned(), vec!["build".to_owned()])]
        );
        let mut sent = None;
        memory.sync(&window, Some(&directory), |_, screens| {
            sent = Some(screens.to_vec());
            Ok(())
        });
        assert_eq!(
            sent, None,
            "a report is not repeated before the service has it"
        );

        // The service stored it; the reconnect lists only the remaining Screen.
        let directory = stored(&[("build", 2)]);
        assert!(reports(&mut memory, &window, &directory).is_empty());
        window.disconnect(id).unwrap();
        window.finish_attempt(id).unwrap();
        window.retry(id).unwrap();
        assert_ne!(window.host(id).unwrap().status(), SourceStatus::Ready);
        assert!(reports(&mut memory, &window, &directory).is_empty());
    }

    #[test]
    fn the_report_is_the_live_catalog_without_pane_counts() {
        let (mut window, id, _) = opened(&["work", "build"]);
        let directory = stored(&[("work", 3), ("build", 1), ("gone", 4)]);
        let mut memory = HostCatalogReports::new(true);
        let mut sent = Vec::new();
        window
            .ui_test_remote_catalog(id, &["work", "build", "new"])
            .unwrap();
        memory.sync(&window, Some(&directory), |target, screens| {
            sent.push((target.to_owned(), screens.to_vec()));
            Ok(())
        });
        assert_eq!(
            sent,
            [(
                HOST.to_owned(),
                vec![
                    ObservedScreen::new("work", Some("work".into()), None),
                    ObservedScreen::new("build", Some("build".into()), None),
                    ObservedScreen::new("new", Some("new".into()), None),
                ]
            )],
            "a removal, a new Screen; the service keeps the counts"
        );
    }

    #[test]
    fn a_rename_on_the_host_is_reported() {
        let (window, _, _) = opened(&["work"]);
        let mut renamed = DirectoryHost::new(HOST);
        renamed.screens = vec![DirectoryScreen::new("work", Some("old".into()), 2)];
        let directory = Directory::new(Vec::new(), vec![renamed]);
        let mut memory = HostCatalogReports::new(true);
        assert_eq!(
            reports(&mut memory, &window, &directory),
            [(HOST.to_owned(), vec!["work".to_owned()])]
        );
    }

    #[test]
    fn a_service_without_the_rule_is_sent_nothing() {
        let (mut window, id, directory) = opened(&["work", "build"]);
        window.ui_test_remote_catalog(id, &["build"]).unwrap();
        let mut memory = HostCatalogReports::default();
        assert!(reports(&mut memory, &window, &directory).is_empty());
    }

    #[test]
    fn an_unconfirmed_terminate_and_a_detach_keep_the_screen() {
        let (mut window, id, directory) = opened(&["work", "build"]);
        let mut memory = HostCatalogReports::new(true);
        // A terminate the host has not confirmed, or one it refused, leaves
        // the Screen in the live catalog.
        window.source_mut(id).unwrap().mark_catalog_dirty();
        assert!(reports(&mut memory, &window, &directory).is_empty());
        window
            .ui_test_remote_catalog(id, &["work", "build"])
            .unwrap();
        assert!(reports(&mut memory, &window, &directory).is_empty());

        // A detached Screen keeps running on the host.
        let key = window.source(id).unwrap().screen_key("work");
        window.detach(window.lifecycle_token(), &key).unwrap();
        window
            .ui_test_remote_catalog(id, &["work", "build"])
            .unwrap();
        assert!(reports(&mut memory, &window, &directory).is_empty());
    }

    #[test]
    fn nothing_is_reported_without_a_ready_catalog_or_a_stored_directory() {
        let (mut window, id, directory) = opened(&["work"]);
        let mut memory = HostCatalogReports::new(true);
        window.ui_test_remote_catalog(id, &[]).unwrap();
        let mut called = false;
        memory.sync(&window, None, |_, _| {
            called = true;
            Ok(())
        });
        assert!(!called, "the stored directory is not known yet");
        assert!(
            reports(
                &mut memory,
                &window,
                &Directory::new(Vec::new(), Vec::new())
            )
            .is_empty(),
            "the service keeps nothing for the host"
        );

        // A failed send is tried again on the next pass.
        memory.sync(&window, Some(&directory), |_, _| {
            Err(io::Error::other("closing"))
        });
        assert_eq!(
            reports(&mut memory, &window, &directory),
            [(HOST.to_owned(), Vec::new())]
        );
    }
}
