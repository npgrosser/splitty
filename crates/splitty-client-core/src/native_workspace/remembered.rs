//! Keeps the local service's list of open remote Screens in step with the
//! window. A host that is closed keeps its Screens in the list; a Screen
//! that is detached or has ended leaves it.
use super::HubSetup;
use crate::{
    hub::{RememberedClient, RememberedScreen, RememberedScreens},
    workspace::{CatalogState, SourceStatus, WindowWorkspace},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub(super) struct RuntimeRemembered<'a> {
    setup: Option<HubSetup<'a>>,
    setup_context: Option<splitty_transport::queue::Context>,
    client: Option<RememberedClient>,
    memory: OpenScreenMemory,
    /// The service writes one host at a time (`hub-host-state-v1`).
    writes: bool,
}
impl<'a> RuntimeRemembered<'a> {
    pub(super) fn new(service: &'a crate::owned_service::OwnedService, writes: bool) -> Self {
        let context = splitty_transport::queue::Context::default();
        Self {
            setup: Some(Box::pin(service.api_stream_with_context(context.clone()))),
            setup_context: Some(context),
            client: None,
            memory: OpenScreenMemory::default(),
            writes,
        }
    }
    /// The list whenever the service returns a different one.
    pub(super) async fn next_update(&mut self) -> Arc<RememberedScreens> {
        loop {
            if let Some(client) = &mut self.client {
                return client.next_update().await;
            }
            let Some(setup) = &mut self.setup else {
                // Without its stream nothing is remembered; the window works.
                return std::future::pending().await;
            };
            let result = setup.await;
            self.setup = None;
            self.setup_context = None;
            let writes = self.writes;
            self.client = result
                .ok()
                .map(|stream| RememberedClient::new(stream, writes));
        }
    }
    /// The host leaves the window with its Screens still remembered.
    pub(super) fn keep(&mut self, target: &str) {
        self.memory.kept.insert(target.to_owned());
    }
    /// The list as the service last returned it.
    pub(super) fn known(&self) -> Option<&Arc<RememberedScreens>> {
        self.client.as_ref().and_then(RememberedClient::known)
    }
    pub(super) fn sync(&mut self, window: &WindowWorkspace) {
        let Some(client) = &mut self.client else {
            self.memory.kept.clear();
            return;
        };
        for change in self.memory.observe(window) {
            match change {
                Change::Set(target, screens) => client.set(&target, screens),
                Change::Prune(target, live) => {
                    client.retain(&target, |id| live.iter().any(|known| known == id))
                }
            }
        }
    }
    pub(super) async fn finish(mut self) -> std::io::Result<()> {
        if let Some(context) = self.setup_context.take() {
            context.close();
        }
        if let Some(setup) = self.setup.take() {
            let writes = self.writes;
            self.client = setup
                .await
                .ok()
                .map(|stream| RememberedClient::new(stream, writes));
        }
        match self.client.take() {
            Some(client) => client.finish().await,
            None => Ok(()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Change {
    /// Remember exactly these Screens for the host.
    Set(String, Vec<RememberedScreen>),
    /// Forget the host's Screens that are not among these live ones.
    Prune(String, Vec<String>),
}

/// What the window showed on the previous pass, to tell a host that lost
/// its last Screen from one that never had any.
#[derive(Default)]
pub(super) struct OpenScreenMemory {
    attached: HashMap<String, Vec<(String, Option<String>)>>,
    kept: HashSet<String>,
}
impl OpenScreenMemory {
    #[cfg(test)]
    pub(super) fn keep(&mut self, target: &str) {
        self.kept.insert(target.to_owned());
    }
    pub(super) fn observe(&mut self, window: &WindowWorkspace) -> Vec<Change> {
        let mut changes = Vec::new();
        let mut current = HashMap::new();
        for host in window.hosts() {
            let action = host.action_target();
            let Some(source) = window.source(action.source_id()) else {
                continue;
            };
            let open: Vec<_> = source
                .screens()
                .iter()
                .map(|screen| (screen.id.as_str().to_owned(), screen.name.clone()))
                .collect();
            if !open.is_empty() {
                if self.attached.get(action.target()) != Some(&open) {
                    changes.push(Change::Set(
                        action.target().to_owned(),
                        open.iter()
                            .map(|(id, name)| {
                                RememberedScreen::new(action.target(), id, name.as_deref())
                            })
                            .collect(),
                    ));
                }
                current.insert(action.target().to_owned(), open);
            } else if host.status() == SourceStatus::Ready
                && host.catalog_state() == CatalogState::Ready
                && !self.attached.contains_key(action.target())
            {
                // A connected host says which remembered Screens still exist.
                if let Some(live) = source.live_catalog() {
                    changes.push(Change::Prune(
                        action.target().to_owned(),
                        live.iter()
                            .map(|screen| screen.id.as_str().to_owned())
                            .collect(),
                    ));
                }
            }
        }
        for target in self.attached.keys() {
            if current.contains_key(target) {
                continue;
            }
            // A closed host keeps its Screens, marked so that a start does
            // not bring them back unasked.
            let screens = if self.kept.contains(target) {
                self.attached[target]
                    .iter()
                    .map(|(id, name)| RememberedScreen::new(target, id, name.as_deref()).closed())
                    .collect()
            } else {
                Vec::new()
            };
            changes.push(Change::Set(target.clone(), screens));
        }
        self.kept.clear();
        self.attached = current;
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn work() -> Vec<RememberedScreen> {
        vec![RememberedScreen::new("one.test", "work", Some("work"))]
    }

    #[test]
    fn a_closed_host_stays_remembered_and_a_detached_screen_does_not() {
        let mut window = WindowWorkspace::new(Uuid::new_v4());
        let owner = window.lifecycle_token();
        let (key, _) = window.ui_test_remote_terminal_fixture("one.test").unwrap();
        let mut memory = OpenScreenMemory::default();
        assert_eq!(
            memory.observe(&window),
            [Change::Set("one.test".into(), work())]
        );
        assert!(memory.observe(&window).is_empty(), "nothing changed");

        // Closing the host takes its Screens out of the window only.
        memory.keep("one.test");
        window.detach_all(owner, key.source_id()).unwrap();
        assert_eq!(
            memory.observe(&window),
            [Change::Set(
                "one.test".into(),
                work().into_iter().map(RememberedScreen::closed).collect()
            )]
        );
        // While it is still connected, it says which Screens exist.
        assert_eq!(
            memory.observe(&window),
            [Change::Prune("one.test".into(), vec!["work".into()])]
        );

        let token = window.source(key.source_id()).unwrap().source_token();
        window.open_existing(token, &key).unwrap();
        assert_eq!(
            memory.observe(&window),
            [Change::Set("one.test".into(), work())]
        );
        // Detaching the last Screen forgets the host's Screens.
        window.detach(owner, &key).unwrap();
        assert_eq!(
            memory.observe(&window),
            [Change::Set("one.test".into(), Vec::new())]
        );
    }
}
