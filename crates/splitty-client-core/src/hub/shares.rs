//! Local ports shared with a host: reverse forwards through the local
//! service (`reverse.*` and `reversePreferences.*`). A share starts only on
//! the user's own action; the service restores enabled saved rules itself
//! when their host connects. Nothing here reacts to a host's messages.
use super::valid_target;
use crate::api_stream::{decode, ApiStream};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use splitty_core::native_api::Operation;
use std::{collections::VecDeque, future::Future, io, pin::Pin, sync::Arc};

const MAX_SAVED: usize = 256;
const MAX_QUEUED: usize = 16;
/// Ports of administrative services, for which sharing warns more strongly.
const ADMINISTRATIVE: [u16; 7] = [22, 2375, 2376, 3389, 5900, 6443, 10250];

/// Whether sharing `port` hands a host an administrative service of this machine.
pub fn is_administrative_port(port: u16) -> bool {
    ADMINISTRATIVE.contains(&port)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareState {
    /// Programs on the host reach the local port.
    Up,
    /// The service is rebuilding the share.
    Rebuilding,
    /// Remembered and enabled, not active now.
    Saved,
    /// Remembered and stopped; only Resume starts it again.
    Paused,
}

/// One share of a host: active, remembered or both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Share {
    pub target: String,
    /// The host's port.
    pub remote: u16,
    /// The port here, always on `localhost`.
    pub local: u16,
    pub state: ShareState,
    pub remembered: bool,
    /// Why the last start or restore failed.
    pub error: Option<String>,
}
impl Share {
    pub fn is_active(&self) -> bool {
        matches!(self.state, ShareState::Up | ShareState::Rebuilding)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareRequest {
    /// Share `localhost:local` as `localhost:remote` on the host.
    Share {
        target: String,
        remote: u16,
        local: u16,
        remember: bool,
    },
    /// Stop a share; its remembered rule stays, paused.
    Stop { target: String, remote: u16 },
    /// Remember an active share.
    Remember {
        target: String,
        remote: u16,
        local: u16,
    },
    /// Stop a share and forget its rule.
    Forget { target: String, remote: u16 },
}
impl ShareRequest {
    fn key(&self) -> (&str, u16) {
        match self {
            Self::Share { target, remote, .. }
            | Self::Stop { target, remote }
            | Self::Remember { target, remote, .. }
            | Self::Forget { target, remote } => (target, *remote),
        }
    }
    pub fn validate(&self) -> io::Result<()> {
        let (target, remote) = self.key();
        valid_target(target)?;
        let local = match self {
            Self::Share { local, .. } | Self::Remember { local, .. } => *local,
            _ => 1,
        };
        if remote == 0 || local == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "A port is between 1 and 65535",
            ));
        }
        Ok(())
    }
}

/// The shares as the local service last returned them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SharesSnapshot {
    shares: Vec<Share>,
    /// Requests that are queued or under way, by host and host port.
    pending: Vec<(String, u16)>,
    /// The failure of the last request per host and host port.
    failures: Vec<(String, u16, String)>,
    read_error: Option<String>,
    /// A read has returned, so an empty list means that nothing is shared.
    loaded: bool,
}
impl SharesSnapshot {
    #[cfg(feature = "ui-test-support")]
    #[doc(hidden)]
    pub fn ui_test_fixture(shares: Vec<Share>) -> Self {
        Self {
            shares,
            ..Self::default()
        }
    }
    /// The shares of `target`, by host port.
    pub fn on<'a>(&'a self, target: &'a str) -> impl Iterator<Item = &'a Share> + 'a {
        self.shares
            .iter()
            .filter(move |share| share.target == target)
    }
    pub fn pending(&self, target: &str, remote: u16) -> bool {
        self.pending
            .iter()
            .any(|(host, port)| host == target && *port == remote)
    }
    /// Why the last request for this share failed.
    pub fn failure(&self, target: &str, remote: u16) -> Option<&str> {
        self.failures
            .iter()
            .find(|(host, port, _)| host == target && *port == remote)
            .map(|(_, _, reason)| reason.as_str())
    }
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }
    pub fn read_error(&self) -> Option<&str> {
        self.read_error.as_deref()
    }
}

#[derive(Deserialize)]
struct Active {
    target: String,
    remote: u16,
    local: u16,
    #[serde(default)]
    state: String,
}
#[derive(Deserialize)]
struct Saved {
    target: String,
    remote: u16,
    local: u16,
    enabled: bool,
    #[serde(default, rename = "lastError")]
    last_error: Option<Failure>,
}
#[derive(Deserialize)]
struct Failure {
    message: String,
}
#[derive(Deserialize)]
struct Preferences {
    version: u32,
    forwards: Vec<Saved>,
}

type Outcome = (io::Result<Vec<Share>>, Option<String>);
type Job = Pin<Box<dyn Future<Output = (ApiStream, Outcome)> + Send>>;

/// One API stream that carries out share requests one at a time and reads
/// both lists after each. Nothing here connects a host's Screens.
pub(crate) struct SharesClient {
    stream: Option<ApiStream>,
    job: Option<Job>,
    running: Option<ShareRequest>,
    queue: VecDeque<ShareRequest>,
    known: Arc<SharesSnapshot>,
    read_due: bool,
    broken: bool,
}
impl SharesClient {
    pub(crate) fn new(stream: impl Into<ApiStream>) -> Self {
        Self {
            stream: Some(stream.into()),
            job: None,
            running: None,
            queue: VecDeque::new(),
            known: Arc::default(),
            read_due: true,
            broken: false,
        }
    }
    /// Read both lists again, as when the panel that shows them opens.
    pub(crate) fn refresh(&mut self) {
        self.read_due = true;
    }
    /// Queue a request. A share that already has one waiting keeps the newer.
    pub(crate) fn submit(&mut self, request: ShareRequest) -> io::Result<Arc<SharesSnapshot>> {
        request.validate()?;
        if self.broken {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "The local service is not reachable",
            ));
        }
        self.queue.retain(|queued| queued.key() != request.key());
        if self.queue.len() >= MAX_QUEUED {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Too many share requests are waiting",
            ));
        }
        self.queue.push_back(request);
        Ok(self.publish(None, None))
    }
    fn publish(
        &mut self,
        shares: Option<io::Result<Vec<Share>>>,
        failure: Option<(String, u16, Option<String>)>,
    ) -> Arc<SharesSnapshot> {
        let mut next = SharesSnapshot {
            shares: self.known.shares.clone(),
            pending: self
                .running
                .iter()
                .chain(&self.queue)
                .map(|request| {
                    let (target, remote) = request.key();
                    (target.to_owned(), remote)
                })
                .collect(),
            failures: self.known.failures.clone(),
            read_error: self.known.read_error.clone(),
            loaded: self.known.loaded || shares.is_some(),
        };
        match shares {
            Some(Ok(shares)) => {
                next.shares = shares;
                next.read_error = None;
            }
            Some(Err(error)) => next.read_error = Some(error.to_string()),
            None => {}
        }
        if let Some((target, remote, reason)) = failure {
            next.failures
                .retain(|(host, port, _)| !(*host == target && *port == remote));
            if let Some(reason) = reason {
                next.failures.push((target, remote, reason));
            }
        }
        self.known = Arc::new(next);
        self.known.clone()
    }
    /// The shares whenever a request settles or a read returns.
    pub(crate) async fn next_update(&mut self) -> Arc<SharesSnapshot> {
        loop {
            if self.job.is_none() {
                if self.broken || !(self.read_due || !self.queue.is_empty()) {
                    std::future::pending::<()>().await;
                }
                let mut stream = self.stream.take().expect("shares stream retained");
                self.running = self.queue.pop_front();
                self.read_due = false;
                let request = self.running.clone();
                self.job = Some(Box::pin(async move {
                    let failure = match &request {
                        Some(request) => carry_out(&mut stream, request).await.err(),
                        None => None,
                    };
                    let shares = read(&mut stream).await;
                    (stream, (shares, failure.map(|error| error.to_string())))
                }));
            }
            let (stream, (shares, failure)) = self.job.as_mut().expect("job started").await;
            self.job = None;
            self.stream = Some(stream);
            let settled = self.running.take().map(|request| {
                let (target, remote) = request.key();
                (target.to_owned(), remote, failure)
            });
            if matches!(&shares, Err(error) if error.kind() == io::ErrorKind::BrokenPipe) {
                self.broken = true;
                self.queue.clear();
            }
            let before = self.known.clone();
            let next = self.publish(Some(shares), settled);
            if *next != *before {
                return next;
            }
        }
    }
    pub(crate) async fn finish(mut self) -> io::Result<()> {
        if let Some(job) = self.job.take() {
            let (stream, _) = job.await;
            self.stream = Some(stream);
        }
        if let Some(stream) = self.stream.take() {
            stream.finish().await?;
        }
        Ok(())
    }
}

fn lost() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "share reply lost")
}

/// The service's own words for a refusal, else its status.
fn refusal(reply: &splitty_transport::wire::Reply) -> io::Error {
    let message = serde_json::from_str::<Value>(&reply.body)
        .ok()
        .and_then(|body| {
            body.get("message")
                .or_else(|| body.get("error").and_then(|error| error.get("message")))
                .or_else(|| body.get("error"))
                .and_then(Value::as_str)
                .map(|text| text.chars().take(300).collect::<String>())
        })
        .unwrap_or_else(|| format!("The local service answered {}", reply.status));
    io::Error::other(message)
}

async fn call(client: &mut ApiStream, operation: Operation, body: Value) -> io::Result<()> {
    let reply = client
        .request(operation, Map::from_iter([("body".into(), body)]))
        .await
        .map_err(|_| lost())?;
    if reply.uncertain {
        return Err(io::Error::other("The outcome is unknown; check the list"));
    }
    if (200..300).contains(&reply.status) {
        Ok(())
    } else {
        Err(refusal(&reply))
    }
}

async fn carry_out(client: &mut ApiStream, request: &ShareRequest) -> io::Result<()> {
    match request {
        ShareRequest::Share {
            target,
            remote,
            local,
            remember,
        } => {
            call(
                client,
                Operation::ReverseOpen,
                json!({"target": target, "port": remote, "local": local}),
            )
            .await?;
            if *remember {
                call(
                    client,
                    Operation::ReversePreferencesSave,
                    json!({"target": target, "port": remote, "local": local}),
                )
                .await?;
            }
            Ok(())
        }
        ShareRequest::Stop { target, remote } => {
            call(
                client,
                Operation::ReverseStop,
                json!({"target": target, "port": remote}),
            )
            .await
        }
        ShareRequest::Remember {
            target,
            remote,
            local,
        } => {
            call(
                client,
                Operation::ReversePreferencesSave,
                json!({"target": target, "port": remote, "local": local}),
            )
            .await
        }
        ShareRequest::Forget { target, remote } => {
            let key = json!({"target": target, "port": remote});
            // An absent rule or share is what Forget is after.
            let rule = call(client, Operation::ReversePreferencesForget, key.clone()).await;
            let share = call(client, Operation::ReverseStop, key).await;
            match (rule, share) {
                (Err(error), Err(_)) => Err(error),
                _ => Ok(()),
            }
        }
    }
}

async fn read(client: &mut ApiStream) -> io::Result<Vec<Share>> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid share list");
    let reply = client
        .request(Operation::ReverseList, Map::new())
        .await
        .map_err(|_| lost())?;
    let active = decode::<Vec<Active>>(client, &reply)?;
    let reply = client
        .request(Operation::ReversePreferencesGet, Map::new())
        .await
        .map_err(|_| lost())?;
    let saved = decode::<Preferences>(client, &reply)?;
    if saved.version != 1 || saved.forwards.len() > MAX_SAVED || active.len() > MAX_SAVED {
        return Err(invalid());
    }
    let mut shares: Vec<Share> = Vec::new();
    for rule in &saved.forwards {
        valid_target(&rule.target)?;
        if rule.remote == 0 || rule.local == 0 {
            return Err(invalid());
        }
        shares.push(Share {
            target: rule.target.clone(),
            remote: rule.remote,
            local: rule.local,
            state: if rule.enabled {
                ShareState::Saved
            } else {
                ShareState::Paused
            },
            remembered: true,
            error: rule
                .last_error
                .as_ref()
                .map(|error| error.message.chars().take(300).collect()),
        });
    }
    for forward in active.iter() {
        valid_target(&forward.target)?;
        if forward.remote == 0 || forward.local == 0 {
            return Err(invalid());
        }
        let state = if forward.state == "down" {
            ShareState::Rebuilding
        } else {
            ShareState::Up
        };
        match shares
            .iter_mut()
            .find(|share| share.target == forward.target && share.remote == forward.remote)
        {
            Some(share) => {
                share.local = forward.local;
                share.state = state;
                share.error = None;
            }
            None => shares.push(Share {
                target: forward.target.clone(),
                remote: forward.remote,
                local: forward.local,
                state,
                remembered: false,
                error: None,
            }),
        }
    }
    shares.sort_by(|a, b| (&a.target, a.remote).cmp(&(&b.target, b.remote)));
    Ok(shares)
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::hub::tests::fake_peer::scripted_operations;
    use std::time::Duration;

    async fn next(client: &mut SharesClient) -> Arc<SharesSnapshot> {
        tokio::time::timeout(Duration::from_secs(5), client.next_update())
            .await
            .expect("the shares arrive")
    }

    #[tokio::test]
    async fn a_share_is_opened_remembered_and_listed_and_a_refusal_keeps_its_words() {
        let rules = |enabled: bool| {
            json!({"version": 1, "forwards": [
                {"target": "vm1", "remote": 11434, "local": 11434, "enabled": enabled}
            ]})
            .to_string()
        };
        let up =
            json!([{"target": "vm1", "remote": 11434, "local": 11434, "state": "up"}]).to_string();
        let (api, task, dir) = scripted_operations(
            vec![
                (200, "[]".into(), false),
                (200, r#"{"version":1,"forwards":[]}"#.into(), false),
                (200, up.clone(), false),
                (200, rules(true), false),
                (200, up.clone(), false),
                (200, rules(true), false),
                (
                    409,
                    r#"{"error":"host_port_conflict","message":"Port 8080 is in use on vm1"}"#
                        .into(),
                    false,
                ),
                (200, up, false),
                (200, rules(true), false),
            ],
            None,
            None,
            Some(vec![
                ("reverse.list", json!({})),
                ("reversePreferences.get", json!({})),
                (
                    "reverse.open",
                    json!({"body": {"target": "vm1", "port": 11434, "local": 11434}}),
                ),
                (
                    "reversePreferences.save",
                    json!({"body": {"target": "vm1", "port": 11434, "local": 11434}}),
                ),
                ("reverse.list", json!({})),
                ("reversePreferences.get", json!({})),
                (
                    "reverse.open",
                    json!({"body": {"target": "vm1", "port": 8080, "local": 3000}}),
                ),
                ("reverse.list", json!({})),
                ("reversePreferences.get", json!({})),
            ]),
        )
        .await;
        let mut client = SharesClient::new(api);
        assert_eq!(next(&mut client).await.on("vm1").count(), 0);
        let pending = client
            .submit(ShareRequest::Share {
                target: "vm1".into(),
                remote: 11434,
                local: 11434,
                remember: true,
            })
            .unwrap();
        assert!(pending.pending("vm1", 11434));
        let shares = next(&mut client).await;
        let share = shares.on("vm1").next().unwrap();
        assert_eq!(
            (share.state.clone(), share.remembered),
            (ShareState::Up, true)
        );
        assert!(!shares.pending("vm1", 11434));

        client
            .submit(ShareRequest::Share {
                target: "vm1".into(),
                remote: 8080,
                local: 3000,
                remember: true,
            })
            .unwrap();
        let shares = next(&mut client).await;
        assert_eq!(
            shares.failure("vm1", 8080),
            Some("Port 8080 is in use on vm1")
        );
        assert_eq!(shares.on("vm1").count(), 1, "a refused share is not listed");
        assert!(client
            .submit(ShareRequest::Stop {
                target: "-oProxyCommand=x".into(),
                remote: 1
            })
            .is_err());
        assert!(is_administrative_port(2375) && !is_administrative_port(11434));
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
