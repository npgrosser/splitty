//! The remote Screens a window has open, kept by the local service across
//! restarts (`remoteScreens.get` and `remoteScreens.updateHost`). Each write
//! names one host, and the service replaces only that host's Screens, so
//! another window's hosts and a host closed here stay remembered. A service
//! without `hub-host-state-v1` is only read.
use super::valid_target;
use crate::api_stream::{decode, ApiStream};
use serde::{Deserialize, Serialize};
use splitty_core::native_api::{
    remembered::{HostScreen, HostScreens},
    Operation,
};
use std::{future::Future, io, pin::Pin, sync::Arc, time::Duration};
use tokio::time::Instant;

const FORMAT: u32 = 1;
/// The service's own limits; a list beyond them is refused as a whole.
const MAX_SCREENS: usize = 200;
const MAX_ID: usize = 256;
const MAX_NAME: usize = 200;
/// A failed read or write is not repeated before this has passed.
const RETRY: Duration = Duration::from_secs(5);
/// How often a write is tried before its edits are given up.
const ATTEMPTS: u8 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RememberedScreen {
    pub target: String,
    pub screen_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Written by clients that mix local and remote Screens in one list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    /// The host was closed in the window: offered again, not reopened at start.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed: bool,
}
impl RememberedScreen {
    pub fn new(target: &str, screen_id: &str, name: Option<&str>) -> Self {
        Self {
            target: target.into(),
            screen_id: screen_id.into(),
            name: name.map(|name| name.chars().take(MAX_NAME).collect()),
            position: None,
            closed: false,
        }
    }
    pub fn closed(mut self) -> Self {
        self.closed = true;
        self
    }
    /// The same Screen under the same name; its place in a mixed list is
    /// another client's concern.
    fn same(&self, other: &Self) -> bool {
        self.target == other.target
            && self.screen_id == other.screen_id
            && self.name == other.name
            && self.closed == other.closed
    }
}

#[derive(Serialize, Deserialize)]
struct Document {
    version: u32,
    screens: Vec<RememberedScreen>,
}

/// The list as the local service last returned it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RememberedScreens {
    screens: Vec<RememberedScreen>,
}
impl RememberedScreens {
    #[cfg(feature = "ui-test-support")]
    #[doc(hidden)]
    pub fn ui_test_fixture(screens: Vec<RememberedScreen>) -> Self {
        Self { screens }
    }
    /// The Screens remembered for `target`, in their order.
    pub fn on<'a>(&'a self, target: &'a str) -> impl Iterator<Item = &'a RememberedScreen> + 'a {
        self.screens
            .iter()
            .filter(move |screen| screen.target == target)
    }
    pub fn count(&self, target: &str) -> usize {
        self.on(target).count()
    }
    /// The hosts that were open when the window went away, each with its
    /// Screens in their order. A host that was closed is not among them.
    pub fn open_hosts(&self) -> Vec<(String, Vec<String>)> {
        let mut hosts: Vec<(String, Vec<String>)> = Vec::new();
        for screen in self.screens.iter().filter(|screen| !screen.closed) {
            match hosts
                .iter_mut()
                .find(|(target, _)| *target == screen.target)
            {
                Some((_, screens)) => screens.push(screen.screen_id.clone()),
                None => hosts.push((screen.target.clone(), vec![screen.screen_id.clone()])),
            }
        }
        hosts
    }
}

type Edit = (String, Vec<RememberedScreen>);
type Job = Pin<Box<dyn Future<Output = (ApiStream, io::Result<Vec<RememberedScreen>>)> + Send>>;

/// One API stream that reads the list once and then writes coalesced
/// per-host replacements. Nothing here connects a host.
pub(crate) struct RememberedClient {
    stream: Option<ApiStream>,
    /// The service writes one host at a time (`hub-host-state-v1`); without
    /// it the list is only read.
    writes: bool,
    job: Option<Job>,
    known: Option<Arc<RememberedScreens>>,
    /// Per host, the Screens to remember instead of the stored ones.
    edits: Vec<Edit>,
    /// The edits of the write that is under way.
    writing: Vec<Edit>,
    read_due: bool,
    retry_at: Option<Instant>,
    failures: u8,
    broken: bool,
}
impl RememberedClient {
    pub(crate) fn new(stream: impl Into<ApiStream>, writes: bool) -> Self {
        Self {
            stream: Some(stream.into()),
            writes,
            job: None,
            known: None,
            edits: Vec::new(),
            writing: Vec::new(),
            read_due: true,
            retry_at: None,
            failures: 0,
            broken: false,
        }
    }
    pub(crate) fn known(&self) -> Option<&Arc<RememberedScreens>> {
        self.known.as_ref()
    }
    /// Whether `screens` is what the service holds for `target` once the
    /// queued edits are written. Unknown before the first read.
    fn settled(&self, target: &str, screens: &[RememberedScreen]) -> bool {
        let same = |held: &[&RememberedScreen]| {
            held.len() == screens.len() && held.iter().zip(screens).all(|(a, b)| a.same(b))
        };
        for edits in [&self.edits, &self.writing] {
            if let Some((_, edit)) = edits.iter().find(|(host, _)| host == target) {
                return same(&edit.iter().collect::<Vec<_>>());
            }
        }
        self.known
            .as_ref()
            .is_some_and(|known| same(&known.on(target).collect::<Vec<_>>()))
    }
    /// Remember exactly `screens` for `target`; an empty list forgets it.
    pub(crate) fn set(&mut self, target: &str, screens: Vec<RememberedScreen>) {
        if !self.writes || self.broken || self.settled(target, &screens) {
            return;
        }
        self.edits.retain(|(host, _)| host != target);
        self.edits.push((target.to_owned(), screens));
    }
    /// Forget the Screens of `target` that `keep` rejects.
    pub(crate) fn retain(&mut self, target: &str, keep: impl Fn(&str) -> bool) {
        let Some(known) = self.known.clone() else {
            return;
        };
        if self
            .edits
            .iter()
            .chain(&self.writing)
            .any(|(host, _)| host == target)
        {
            return;
        }
        if known.on(target).any(|screen| !keep(&screen.screen_id)) {
            let kept = known
                .on(target)
                .filter(|screen| keep(&screen.screen_id))
                .cloned()
                .collect();
            self.set(target, kept);
        }
    }
    /// The list whenever the service returns a different one.
    pub(crate) async fn next_update(&mut self) -> Arc<RememberedScreens> {
        loop {
            if self.job.is_none() {
                if self.broken || !(self.read_due || !self.edits.is_empty()) {
                    std::future::pending::<()>().await;
                }
                if let Some(at) = self.retry_at {
                    tokio::time::sleep_until(at).await;
                    self.retry_at = None;
                }
                let mut stream = self.stream.take().expect("remembered stream retained");
                self.writing = std::mem::take(&mut self.edits);
                self.read_due = false;
                let edits = self.writing.clone();
                self.job = Some(Box::pin(async move {
                    let result = if edits.is_empty() {
                        read(&mut stream).await
                    } else {
                        write(&mut stream, &edits).await
                    };
                    (stream, result)
                }));
            }
            let (stream, result) = self.job.as_mut().expect("job started").await;
            self.job = None;
            self.stream = Some(stream);
            let written = std::mem::take(&mut self.writing);
            match result {
                Ok(screens) => {
                    self.failures = 0;
                    let next = RememberedScreens { screens };
                    if self.known.as_deref() != Some(&next) {
                        let next = Arc::new(next);
                        self.known = Some(next.clone());
                        return next;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => self.broken = true,
                Err(_) => {
                    self.failures += 1;
                    if self.failures < ATTEMPTS {
                        // Newer edits for a host win over the failed ones.
                        for (target, screens) in written {
                            if !self.edits.iter().any(|(host, _)| *host == target) {
                                self.edits.push((target, screens));
                            }
                        }
                    } else {
                        self.failures = 0;
                    }
                    self.read_due = self.known.is_none();
                    self.retry_at = Some(Instant::now() + RETRY);
                }
            }
        }
    }
    pub(crate) async fn finish(mut self) -> io::Result<()> {
        if let Some(job) = self.job.take() {
            let (stream, _) = job.await;
            self.stream = Some(stream);
        }
        if let Some(mut stream) = self.stream.take() {
            // A change made just before closing is still written.
            if !self.broken && !self.edits.is_empty() {
                let _ = write(&mut stream, &self.edits).await;
            }
            stream.finish().await?;
        }
        Ok(())
    }
}

fn lost() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "remembered Screens reply lost")
}

fn checked(
    client: &ApiStream,
    reply: &splitty_transport::wire::Reply,
) -> io::Result<Vec<RememberedScreen>> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid remembered Screens");
    if reply.uncertain {
        return Err(invalid());
    }
    let document = decode::<Document>(client, reply)?;
    if document.version != FORMAT || document.screens.len() > MAX_SCREENS {
        return Err(invalid());
    }
    for screen in &document.screens {
        valid_target(&screen.target)?;
        let id = screen.screen_id.chars().count();
        if id == 0
            || id > MAX_ID
            || screen
                .name
                .as_ref()
                .is_some_and(|name| name.chars().count() > MAX_NAME)
        {
            return Err(invalid());
        }
    }
    Ok(document.screens.clone())
}

async fn read(client: &mut ApiStream) -> io::Result<Vec<RememberedScreen>> {
    let reply = client
        .request(Operation::RemoteScreensGet, serde_json::Map::new())
        .await
        .map_err(|_| lost())?;
    checked(client, &reply)
}

/// Have the service replace the Screens of each edited host, one host per
/// request; the list it returns last is the one it holds.
async fn write(client: &mut ApiStream, edits: &[Edit]) -> io::Result<Vec<RememberedScreen>> {
    let mut stored = None;
    for (target, screens) in edits {
        let body = serde_json::to_value(host_screens(target, screens))?;
        let reply = client
            .request(
                Operation::RemoteScreensUpdateHost,
                serde_json::Map::from_iter([("body".into(), body)]),
            )
            .await
            .map_err(|_| lost())?;
        stored = Some(checked(client, &reply)?);
    }
    match stored {
        Some(stored) => Ok(stored),
        None => read(client).await,
    }
}

/// The body for one host: a closed host's Screens are all closed.
fn host_screens(target: &str, screens: &[RememberedScreen]) -> HostScreens {
    let closed = !screens.is_empty() && screens.iter().all(|screen| screen.closed);
    HostScreens::new(
        target,
        screens
            .iter()
            .take(MAX_SCREENS)
            .map(|screen| {
                let mut entry = HostScreen::new(screen.screen_id.clone(), screen.name.clone());
                entry.position = screen.position;
                entry
            })
            .collect(),
        closed,
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::hub::tests::fake_peer::scripted_operations;
    use serde_json::json;

    fn document(screens: &[(&str, &str)]) -> String {
        json!({"version": 1, "screens": screens.iter().map(|(target, id)| json!({"target": target, "screenId": id})).collect::<Vec<_>>()})
            .to_string()
    }
    async fn next(client: &mut RememberedClient) -> Arc<RememberedScreens> {
        tokio::time::timeout(Duration::from_secs(5), client.next_update())
            .await
            .expect("the list arrives")
    }

    #[test]
    fn a_closed_host_is_remembered_but_not_among_the_hosts_to_reconnect() {
        let list: Document = serde_json::from_value(json!({"version": 1, "screens": [
            {"target": "vm1", "screenId": "a"},
            {"target": "vm2", "screenId": "x", "closed": true},
            {"target": "vm1", "screenId": "b", "name": "logs"},
        ]}))
        .unwrap();
        let list = RememberedScreens {
            screens: list.screens,
        };
        assert_eq!(
            list.open_hosts(),
            [("vm1".to_owned(), vec!["a".to_owned(), "b".to_owned()])]
        );
        assert_eq!(list.count("vm2"), 1, "it is still offered in Hosts");
        let closed = RememberedScreen::new("vm2", "x", None).closed();
        assert_eq!(
            serde_json::to_value(&closed).unwrap(),
            json!({"target": "vm2", "screenId": "x", "closed": true})
        );
        assert!(!closed.same(&RememberedScreen::new("vm2", "x", None)));
        assert_eq!(
            serde_json::to_value(RememberedScreen::new("vm1", "a", None)).unwrap(),
            json!({"target": "vm1", "screenId": "a"}),
            "an open Screen is written as before"
        );
    }

    #[tokio::test]
    async fn a_host_is_replaced_in_the_list_the_service_holds_and_the_others_are_kept() {
        let stored = [("vm1", "s1"), ("vm2", "x"), ("vm1", "s2")];
        let written = [("vm1", "s2"), ("vm2", "x")];
        let (api, task, dir) = scripted_operations(
            vec![
                (200, document(&stored), false),
                (200, document(&written), false),
            ],
            None,
            None,
            Some(vec![
                ("remoteScreens.get", json!({})),
                (
                    "remoteScreens.updateHost",
                    json!({"body": {"target": "vm1", "screens": [
                        {"screenId": "s2", "name": "api"},
                    ]}}),
                ),
            ]),
        )
        .await;
        let mut client = RememberedClient::new(api, true);
        let list = next(&mut client).await;
        assert_eq!(list.count("vm1"), 2);
        assert_eq!(list.count("vm2"), 1);

        // What the service already holds is not written again.
        client.set("vm2", vec![RememberedScreen::new("vm2", "x", None)]);
        client.retain("vm2", |id| id == "x");
        assert!(client.edits.is_empty());

        client.set("vm1", vec![RememberedScreen::new("vm1", "s2", Some("api"))]);
        client.set("vm1", vec![RememberedScreen::new("vm1", "s2", Some("api"))]);
        assert_eq!(client.edits.len(), 1);
        let list = next(&mut client).await;
        assert_eq!(
            list.on("vm1")
                .map(|screen| screen.screen_id.as_str())
                .collect::<Vec<_>>(),
            ["s2"]
        );
        assert_eq!(list.count("vm2"), 1, "another host's Screens are kept");

        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_refused_list_is_not_trusted_and_ended_screens_are_forgotten() {
        let (api, task, dir) = scripted_operations(
            vec![
                (200, document(&[("vm1", "s1"), ("vm1", "s2")]), false),
                (200, r#"{"version":2,"screens":[]}"#.into(), false),
            ],
            None,
            None,
            Some(vec![
                ("remoteScreens.get", json!({})),
                (
                    "remoteScreens.updateHost",
                    json!({"body": {"target": "vm1", "screens": [{"screenId": "s2"}]}}),
                ),
            ]),
        )
        .await;
        let mut client = RememberedClient::new(api, true);
        assert_eq!(next(&mut client).await.count("vm1"), 2);
        client.retain("vm1", |id| id == "s2");
        assert_eq!(client.edits.len(), 1);
        let mut pending = Box::pin(client.next_update());
        assert!(
            tokio::time::timeout(Duration::from_millis(300), &mut pending)
                .await
                .is_err(),
            "an answer in another format changes nothing"
        );
        drop(pending);
        assert_eq!(client.known.as_ref().unwrap().count("vm1"), 2);
        assert!(client.retry_at.is_some(), "the write is tried again later");
        assert_eq!(client.edits.len(), 1);
        client.edits.clear();
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_closed_host_is_written_as_closed() {
        let closed = [RememberedScreen::new("vm1", "a", Some("api")).closed()];
        assert_eq!(
            serde_json::to_value(host_screens("vm1", &closed)).unwrap(),
            json!({"target": "vm1", "screens": [{"screenId": "a", "name": "api"}], "closed": true})
        );
        assert_eq!(
            serde_json::to_value(host_screens("vm1", &[])).unwrap(),
            json!({"target": "vm1", "screens": []}),
            "an empty list forgets the host"
        );
    }

    #[tokio::test]
    async fn a_service_without_host_writes_is_only_read() {
        let (api, task, dir) = scripted_operations(
            vec![(200, document(&[("vm1", "s1")]), false)],
            None,
            None,
            Some(vec![("remoteScreens.get", json!({}))]),
        )
        .await;
        let mut client = RememberedClient::new(api, false);
        assert_eq!(next(&mut client).await.count("vm1"), 1);
        client.set("vm1", Vec::new());
        client.set("vm2", vec![RememberedScreen::new("vm2", "x", None)]);
        client.retain("vm1", |_| false);
        assert!(client.edits.is_empty(), "nothing is written");
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
