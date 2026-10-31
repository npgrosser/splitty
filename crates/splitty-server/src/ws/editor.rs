//! Ephemeral requests to a single attached client. This never authorizes a
//! desktop launch: the trusted client independently validates and confirms it.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};
use tokio::time::Instant;

use super::sender::WsSender;

pub const CAPABILITY: &str = "editor-open-v1";
const LIFETIME: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Editor {
    Vscode,
    Zed,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenRequest {
    pub session_id: String,
    pub path: String,
    pub editor: Option<Editor>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Dispatched,
    Denied,
    Failed,
    Expired,
    Disconnected,
}

pub type Failure = (u16, &'static str, &'static str);

pub fn validate_path(path: &str) -> Result<(), Failure> {
    if !path.starts_with('/') || path.starts_with("//") || path.len() > 4096
        || path.contains('\\') || path.chars().any(|c| c.is_control() || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}'))
        || path.split('/').any(|p| p == "." || p == "..") {
        return Err((400, "invalid_path", "A supported absolute path without control characters or dot segments is required"));
    }
    Ok(())
}

struct Client {
    sender: Arc<WsSender>,
    sessions: HashSet<String>,
    last_request: Option<Instant>,
}
struct Pending {
    client: String,
    session: String,
    deadline: Instant,
    answer: oneshot::Sender<Outcome>,
}
#[derive(Default)]
struct Registry {
    clients: HashMap<String, Client>,
    pending: HashMap<String, Pending>,
}
#[derive(Default)]
pub struct EditorRequests {
    registry: Mutex<Registry>,
}

impl EditorRequests {
    pub(super) async fn register(
        &self,
        id: &str,
        sender: Arc<WsSender>,
        sessions: HashSet<String>,
    ) {
        self.registry
            .lock()
            .await
            .clients
            .entry(id.into())
            .or_insert(Client {
                sender,
                sessions,
                last_request: None,
            });
    }

    pub(super) async fn sessions(&self, id: &str, sessions: HashSet<String>) {
        let mut registry = self.registry.lock().await;
        if let Some(client) = registry.clients.get_mut(id) {
            client.sessions = sessions.clone();
        }
        let cancelled: Vec<_> = registry
            .pending
            .iter()
            .filter(|(_, p)| p.client == id && !sessions.contains(&p.session))
            .map(|(key, _)| key.clone())
            .collect();
        for key in cancelled {
            if let Some(p) = registry.pending.remove(&key) {
                if let Some(client) = registry.clients.get(id) {
                    let _ = client
                        .sender
                        .send_text(&json!({"type":"editor.cancelled","requestId":key}).to_string())
                        .await;
                }
                let _ = p.answer.send(Outcome::Disconnected);
            }
        }
    }

    pub(super) async fn disconnect(&self, id: &str) {
        self.sessions(id, HashSet::new()).await;
        self.registry.lock().await.clients.remove(id);
    }

    pub(super) async fn reply(&self, client: &str, value: &Value) {
        let Some(id) = value["requestId"].as_str() else {
            return;
        };
        let Ok(outcome) = serde_json::from_value::<Outcome>(value["outcome"].clone()) else {
            return;
        };
        let mut registry = self.registry.lock().await;
        if !registry.pending.get(id).is_some_and(|p| p.client == client) {
            return;
        }
        if let Some(p) = registry.pending.remove(id) {
            let _ = p.answer.send(if Instant::now() >= p.deadline {
                Outcome::Expired
            } else {
                outcome
            });
        }
    }

    pub async fn request(self: &Arc<Self>, input: OpenRequest, kind: &str) -> Result<(), Failure> {
        let (id, answer) = self.admit(&input, kind).await?;
        let expiry = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(LIFETIME).await;
            expiry.expire().await;
        });
        // Separate task owns expiry even when an HTTP caller goes away.
        let outcome = answer.await.unwrap_or(Outcome::Disconnected);
        tracing::debug!(request_id = id, ?outcome, "editor request completed");
        match outcome {
            Outcome::Dispatched => Ok(()),
            Outcome::Denied => Err((409, "denied", "Editor request declined in Splitty")),
            Outcome::Failed => Err((
                502,
                "dispatch_failed",
                "Client could not hand off to the editor; see Splitty for details",
            )),
            Outcome::Expired => Err((
                408,
                "expired",
                "Editor request expired; confirm it in Splitty within 30 seconds",
            )),
            Outcome::Disconnected => Err((
                409,
                "disconnected",
                "The receiving Splitty client or terminal disconnected",
            )),
        }
    }

    async fn admit(
        &self,
        input: &OpenRequest,
        kind: &str,
    ) -> Result<(String, oneshot::Receiver<Outcome>), Failure> {
        let mut registry = self.registry.lock().await;
        let recipients: Vec<_> = registry
            .clients
            .iter()
            .filter(|(_, c)| c.sessions.contains(&input.session_id))
            .map(|(id, _)| id.clone())
            .collect();
        let client_id = match recipients.as_slice() {
            [id] => id.clone(),
            [] => return Err((409, "no_client", "Open this terminal in an updated, trusted Splitty hub to receive editor requests")),
            _ => return Err((409, "ambiguous_client", "This terminal is attached in multiple Splitty clients; detach the others and retry")),
        };
        if registry.pending.len() >= 32 || registry.pending.values().any(|p| p.client == client_id)
        {
            return Err((
                429,
                "busy",
                "An editor request is already pending; finish it before retrying",
            ));
        }
        let client = registry
            .clients
            .get_mut(&client_id)
            .expect("selected registered client");
        if client
            .last_request
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return Err((
                429,
                "busy",
                "Please wait two seconds between editor requests",
            ));
        }
        client.last_request = Some(Instant::now());
        let id = hex::encode(rand::random::<[u8; 16]>());
        let (answer, rx) = oneshot::channel();
        let deadline = Instant::now() + LIFETIME;
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            + LIFETIME.as_millis();
        let event = json!({"type":"editor.request","requestId":id,"sessionId":input.session_id,"path":input.path,"kind":kind,"editor":input.editor,"expiresAt":expires_at});
        client
            .sender
            .send_text(&event.to_string())
            .await
            .map_err(|_| (409, "disconnected", "The receiving client disconnected"))?;
        registry.pending.insert(
            id.clone(),
            Pending {
                client: client_id,
                session: input.session_id.clone(),
                deadline,
                answer,
            },
        );
        Ok((id, rx))
    }

    /// Called by the connection heartbeat, and by a request's independent timer.
    pub async fn expire(&self) {
        let mut registry = self.registry.lock().await;
        let expired: Vec<_> = registry
            .pending
            .iter()
            .filter(|(_, p)| Instant::now() >= p.deadline)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some(p) = registry.pending.remove(&id) {
                if let Some(c) = registry.clients.get(&p.client) {
                    let _ = c
                        .sender
                        .send_text(&json!({"type":"editor.cancelled","requestId":id}).to_string())
                        .await;
                }
                let _ = p.answer.send(Outcome::Expired);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::ws::Message;
    use tokio::sync::mpsc::UnboundedReceiver;

    async fn client(
        requests: &EditorRequests,
        id: &str,
        sessions: &[&str],
    ) -> UnboundedReceiver<Message> {
        let (sender, rx) = WsSender::channel();
        requests
            .register(
                id,
                Arc::new(sender),
                sessions.iter().map(|s| (*s).into()).collect(),
            )
            .await;
        rx
    }
    fn input() -> OpenRequest {
        OpenRequest {
            session_id: "s1".into(),
            path: "/srv/app/file.rs".into(),
            editor: Some(Editor::Zed),
        }
    }
    async fn event(rx: &mut UnboundedReceiver<Message>) -> Value {
        let Message::Text(text) = rx.recv().await.expect("event") else {
            panic!("text frame")
        };
        serde_json::from_str(&text).expect("JSON")
    }

    #[tokio::test]
    async fn routes_only_to_sole_attached_client_and_checks_reply_identity() {
        let requests = EditorRequests::default();
        assert_eq!(
            requests.admit(&input(), "file").await.expect_err("error").1,
            "no_client"
        );
        let mut a = client(&requests, "a", &["s1"]).await;
        let mut b = client(&requests, "b", &["other"]).await;
        let (id, mut answer) = requests.admit(&input(), "file").await.expect("admit");
        let message = event(&mut a).await;
        assert_eq!(message["path"], input().path);
        assert_eq!(message["kind"], "file");
        assert!(message.get("target").is_none());
        assert!(b.try_recv().is_err());
        requests
            .reply("b", &json!({"requestId":id,"outcome":"dispatched"}))
            .await;
        assert!(matches!(
            answer.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        requests
            .reply("a", &json!({"requestId":id,"outcome":"denied"}))
            .await;
        assert_eq!(answer.await.expect("answer"), Outcome::Denied);
        requests
            .reply("a", &json!({"requestId":id,"outcome":"dispatched"}))
            .await;
        assert!(requests.registry.lock().await.pending.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn ambiguity_rate_pending_and_expiry_are_bounded() {
        let requests = Arc::new(EditorRequests::default());
        let mut a = client(&requests, "a", &["s1"]).await;
        let _b = client(&requests, "b", &["s1"]).await;
        assert_eq!(
            requests
                .admit(&input(), "file")
                .await
                .expect_err("ambiguous")
                .1,
            "ambiguous_client"
        );
        assert!(a.try_recv().is_err());
        requests.disconnect("b").await;
        let worker = {
            let r = requests.clone();
            tokio::spawn(async move { r.request(input(), "file").await })
        };
        let message = event(&mut a).await;
        assert_eq!(
            requests.admit(&input(), "file").await.expect_err("busy").0,
            429
        );
        requests
            .reply(
                "a",
                &json!({"requestId":message["requestId"],"outcome":"dispatched"}),
            )
            .await;
        assert!(worker.await.expect("worker").is_ok());
        assert_eq!(
            requests.admit(&input(), "file").await.expect_err("rate").0,
            429
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        let worker = {
            let r = requests.clone();
            tokio::spawn(async move { r.request(input(), "file").await })
        };
        event(&mut a).await;
        tokio::time::advance(LIFETIME).await;
        assert_eq!(
            worker.await.expect("worker").expect_err("expired").1,
            "expired"
        );
        assert_eq!(event(&mut a).await["type"], "editor.cancelled");
    }

    #[tokio::test]
    async fn detach_and_disconnect_cancel_without_replay() {
        for disconnect in [false, true] {
            let requests = EditorRequests::default();
            let mut a = client(&requests, "a", &["s1"]).await;
            let (_, answer) = requests.admit(&input(), "folder").await.expect("admit");
            event(&mut a).await;
            if disconnect {
                requests.disconnect("a").await;
            } else {
                requests.sessions("a", HashSet::new()).await;
            }
            assert_eq!(answer.await.expect("answer"), Outcome::Disconnected);
            let mut replacement = client(&requests, "new", &["s1"]).await;
            assert!(replacement.try_recv().is_err());
        }
    }

    #[test]
    fn rejects_remote_control_fields_and_unsafe_paths() {
        for field in ["target", "uri", "command", "kind"] {
            let mut value = json!({"sessionId":"s1","path":"/srv/app"});
            value[field] = json!("evil");
            assert!(serde_json::from_value::<OpenRequest>(value).is_err());
        }
        assert!(serde_json::from_value::<OpenRequest>(
            json!({"sessionId":"s1","path":"/srv/app","editor":"sh"})
        )
        .is_err());
        for path in [
            "relative",
            "//server/share",
            "/srv/../etc",
            "/a\\b",
            "/a\n",
            "/a\u{202e}b",
        ] {
            assert!(validate_path(path).is_err(), "{path:?}");
        }
        assert!(validate_path("/srv/a #?%界").is_ok());
    }
    #[tokio::test(start_paused = true)]
    async fn global_capacity_and_late_replies_fail_closed() {
        let requests = EditorRequests::default();
        let mut receivers = Vec::new();
        let mut answers = Vec::new();
        for n in 0..33 {
            let id = format!("client-{n}");
            let session = format!("session-{n}");
            receivers.push(client(&requests, &id, &[&session]).await);
            let mut request = input();
            request.session_id = session;
            let result = requests.admit(&request, "file").await;
            if n < 32 {
                answers.push(result.expect("admitted"));
            } else {
                assert_eq!(result.expect_err("capacity").0, 429);
            }
        }
        tokio::time::advance(LIFETIME).await;
        let (id, answer) = answers.remove(0);
        requests
            .reply("client-0", &json!({"requestId":id,"outcome":"dispatched"}))
            .await;
        assert_eq!(answer.await.expect("answer"), Outcome::Expired);
        requests.expire().await;
        assert!(requests.registry.lock().await.pending.is_empty());
    }
}
