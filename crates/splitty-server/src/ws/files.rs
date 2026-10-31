//! Files a process in a pane offers to the windows showing its session
//! (`splitty download`, `POST /api/file/offer`). An offer goes to every client
//! that registered for offers and is attached to the session; nothing is read
//! before one of them accepts, which its user does with Save. The first to
//! accept takes the offer and reads the file, as it was opened at offer time,
//! in chunks at the next offset only.
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};
use tokio::time::Instant;

use super::editor::Failure;
use super::sender::WsSender;

pub const CAPABILITY: &str = "file-offer-v1";
/// The largest file that can be offered, as for uploads.
pub const MAX_FILE_BYTES: u64 = splitty_core::protocol::upload::MAX_FILE_BYTES;
/// The most bytes of the file one `file.chunk` carries, decoded.
pub const CHUNK_BYTES: u64 = 1024 * 1024;
/// How long an offer waits for a window to accept it.
const ACCEPT_WITHIN: Duration = Duration::from_secs(60);
/// How long an accepted offer waits for its reader's next `file.read`.
const READ_IDLE: Duration = Duration::from_secs(60);
/// Offers pending on the server, and for one session.
const MAX_PENDING: usize = 8;
const MAX_PER_SESSION: usize = 2;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfferRequest {
    pub session_id: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Saved,
    Declined,
    Expired,
    Disconnected,
    Failed,
}

impl Outcome {
    /// The `reason` of the `file.closed` that tells clients to drop the offer.
    fn reason(self) -> &'static str {
        match self {
            Outcome::Saved => "saved",
            Outcome::Declined => "declined",
            Outcome::Expired => "expired",
            Outcome::Disconnected => "disconnected",
            Outcome::Failed => "failed",
        }
    }
}

/// A file opened for an offer.
#[derive(Debug)]
pub struct OfferedFile {
    file: std::fs::File,
    name: String,
    size: u64,
}

/// Open the regular file at `path` for an offer, following links, without
/// blocking on a FIFO. Blocking: call it off the async runtime's workers.
pub fn open(path: &Path) -> Result<OfferedFile, Failure> {
    const NOT_A_FILE: Failure = (400, "invalid_path", "Only regular files can be offered");
    let metadata = std::fs::metadata(path).map_err(|_| {
        (
            400,
            "invalid_path",
            "The requested path does not exist or is inaccessible",
        )
    })?;
    if !metadata.is_file() {
        return Err(NOT_A_FILE);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Swapped for a FIFO since the check above, it must not hang here.
        options.custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOCTTY);
    }
    let file = options.open(path).map_err(|_| {
        (
            400,
            "invalid_path",
            "The requested path does not exist or is inaccessible",
        )
    })?;
    let metadata = file.metadata().map_err(|_| NOT_A_FILE)?;
    if !metadata.is_file() {
        return Err(NOT_A_FILE);
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err((413, "too_large", "Files over 512 MiB cannot be offered"));
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    Ok(OfferedFile {
        file,
        name,
        size: metadata.len(),
    })
}

struct Client {
    sender: Arc<WsSender>,
    sessions: HashSet<String>,
}

struct Offer {
    session: String,
    file: Arc<std::fs::File>,
    size: u64,
    /// Who still shows the offer: every recipient until one accepts, then
    /// only the reader.
    recipients: HashSet<String>,
    declined: bool,
    reader: Option<String>,
    /// The offset of the next chunk.
    next: u64,
    /// Until it is accepted, then until the next read.
    deadline: Instant,
    answer: Option<oneshot::Sender<Outcome>>,
}

#[derive(Default)]
struct Registry {
    clients: HashMap<String, Client>,
    offers: HashMap<String, Offer>,
}

impl Registry {
    /// End offer `id` with `outcome`, telling the clients that still show it.
    async fn close(&mut self, id: &str, outcome: Outcome) {
        let Some(mut offer) = self.offers.remove(id) else {
            return;
        };
        if outcome != Outcome::Saved {
            let closed =
                json!({"type":"file.closed","offerId":id,"reason":outcome.reason()}).to_string();
            for client in &offer.recipients {
                if let Some(c) = self.clients.get(client) {
                    let _ = c.sender.send_text(&closed).await;
                }
            }
        }
        if let Some(answer) = offer.answer.take() {
            let _ = answer.send(outcome);
        }
    }

    /// `client` no longer shows offer `id`: it went away or declined.
    async fn leave(&mut self, id: &str, client: &str, declined: bool) {
        let Some(offer) = self.offers.get_mut(id) else {
            return;
        };
        if !offer.recipients.remove(client) {
            return;
        }
        offer.declined |= declined;
        let reader = offer.reader.as_deref() == Some(client);
        if reader || offer.recipients.is_empty() {
            let outcome = if offer.declined {
                Outcome::Declined
            } else {
                Outcome::Disconnected
            };
            self.close(id, outcome).await;
        }
    }
}

#[derive(Default)]
pub struct FileOffers {
    registry: Mutex<Registry>,
}

fn offer_id(value: &Value) -> Option<&str> {
    value["offerId"].as_str()
}

impl FileOffers {
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
            .or_insert(Client { sender, sessions });
    }

    /// The sessions `id` is attached to changed; it leaves the offers of the
    /// others.
    pub(super) async fn sessions(&self, id: &str, sessions: HashSet<String>) {
        let mut registry = self.registry.lock().await;
        if let Some(client) = registry.clients.get_mut(id) {
            client.sessions = sessions.clone();
        }
        let left: Vec<_> = registry
            .offers
            .iter()
            .filter(|(_, o)| o.recipients.contains(id) && !sessions.contains(&o.session))
            .map(|(key, _)| key.clone())
            .collect();
        for key in left {
            registry.leave(&key, id, false).await;
        }
    }

    pub(super) async fn disconnect(&self, id: &str) {
        self.sessions(id, HashSet::new()).await;
        self.registry.lock().await.clients.remove(id);
    }

    /// `file.accept`: the first recipient to accept takes the offer.
    pub(super) async fn accept(&self, client: &str, value: &Value) {
        let Some(id) = offer_id(value) else {
            return;
        };
        let mut registry = self.registry.lock().await;
        let Some(offer) = registry.offers.get_mut(id) else {
            return;
        };
        if offer.reader.is_some() || !offer.recipients.contains(client) {
            return;
        }
        offer.reader = Some(client.into());
        offer.deadline = Instant::now() + READ_IDLE;
        let others: Vec<_> = offer.recipients.drain().filter(|c| c != client).collect();
        offer.recipients.insert(client.into());
        let taken = json!({"type":"file.closed","offerId":id,"reason":"taken"}).to_string();
        for other in others {
            if let Some(c) = registry.clients.get(&other) {
                let _ = c.sender.send_text(&taken).await;
            }
        }
    }

    /// `file.decline`: the offer is declined once every recipient declined or
    /// went away; its reader declining stops the transfer.
    pub(super) async fn decline(&self, client: &str, value: &Value) {
        let Some(id) = offer_id(value) else {
            return;
        };
        self.registry.lock().await.leave(id, client, true).await;
    }

    /// `file.read`: the reader's next chunk, answered with `file.chunk`.
    pub(super) async fn read(&self, client: &str, value: &Value) {
        let (Some(id), Some(offset)) = (offer_id(value), value["offset"].as_u64()) else {
            return;
        };
        let (file, len) = {
            let registry = self.registry.lock().await;
            let Some(offer) = registry.offers.get(id) else {
                return;
            };
            if offer.reader.as_deref() != Some(client) || offset != offer.next {
                return;
            }
            (offer.file.clone(), (offer.size - offset).min(CHUNK_BYTES))
        };
        let read = tokio::task::spawn_blocking(move || read_at(&file, offset, len))
            .await
            .unwrap_or_else(|error| Err(io::Error::other(error)));
        let mut registry = self.registry.lock().await;
        let Some(offer) = registry.offers.get_mut(id) else {
            return;
        };
        if offer.reader.as_deref() != Some(client) || offset != offer.next {
            return;
        }
        let bytes = match read {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(offer = id, %error, "an offered file could not be read");
                registry.close(id, Outcome::Failed).await;
                return;
            }
        };
        offer.next += len;
        offer.deadline = Instant::now() + READ_IDLE;
        let eof = offer.next >= offer.size;
        let chunk = json!({
            "type":"file.chunk",
            "offerId":id,
            "offset":offset,
            "data":base64::engine::general_purpose::STANDARD.encode(&bytes),
            "eof":eof,
        });
        if let Some(c) = registry.clients.get(client) {
            let _ = c.sender.send_text(&chunk.to_string()).await;
        }
        if eof {
            registry.close(id, Outcome::Saved).await;
        }
    }

    /// Offer `file` from `input.session_id`'s pane and wait for the outcome.
    pub async fn offer(
        self: &Arc<Self>,
        input: &OfferRequest,
        file: OfferedFile,
    ) -> Result<(), Failure> {
        let (id, answer) = self.admit(input, file).await?;
        let timer = self.clone();
        let key = id.clone();
        // Owns expiry even when the HTTP caller goes away.
        tokio::spawn(async move {
            while let Some(deadline) = timer.deadline(&key).await {
                tokio::time::sleep_until(deadline).await;
                timer.expire().await;
            }
        });
        let outcome = answer.await.unwrap_or(Outcome::Disconnected);
        tracing::debug!(offer = id, ?outcome, "file offer completed");
        match outcome {
            Outcome::Saved => Ok(()),
            Outcome::Declined => Err((409, "declined", "The file was declined in Splitty")),
            Outcome::Expired => Err((
                408,
                "expired",
                "Nobody chose Save in Splitty within 60 seconds",
            )),
            Outcome::Disconnected => Err((
                409,
                "disconnected",
                "The Splitty window that took the file disconnected or stopped reading",
            )),
            Outcome::Failed => Err((500, "read_failed", "The file could not be read to its end")),
        }
    }

    async fn admit(
        &self,
        input: &OfferRequest,
        file: OfferedFile,
    ) -> Result<(String, oneshot::Receiver<Outcome>), Failure> {
        let mut registry = self.registry.lock().await;
        let recipients: HashSet<String> = registry
            .clients
            .iter()
            .filter(|(_, c)| c.sessions.contains(&input.session_id))
            .map(|(id, _)| id.clone())
            .collect();
        if recipients.is_empty() {
            return Err((
                409,
                "no_client",
                "Show this terminal in an updated Splitty window to receive files",
            ));
        }
        let for_session = registry
            .offers
            .values()
            .filter(|o| o.session == input.session_id)
            .count();
        if registry.offers.len() >= MAX_PENDING || for_session >= MAX_PER_SESSION {
            return Err((
                429,
                "busy",
                "Too many files are waiting for Save; finish them before offering more",
            ));
        }
        let id = hex::encode(rand::random::<[u8; 16]>());
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            + ACCEPT_WITHIN.as_millis();
        let event = json!({
            "type":"file.offer",
            "offerId":id,
            "sessionId":input.session_id,
            "name":file.name,
            "size":file.size,
            "expiresAt":expires_at,
        })
        .to_string();
        let mut reached = HashSet::new();
        for recipient in recipients {
            if let Some(c) = registry.clients.get(&recipient) {
                if c.sender.send_text(&event).await.is_ok() {
                    reached.insert(recipient);
                }
            }
        }
        if reached.is_empty() {
            return Err((
                409,
                "disconnected",
                "The receiving Splitty window disconnected",
            ));
        }
        let (answer, rx) = oneshot::channel();
        registry.offers.insert(
            id.clone(),
            Offer {
                session: input.session_id.clone(),
                file: Arc::new(file.file),
                size: file.size,
                recipients: reached,
                declined: false,
                reader: None,
                next: 0,
                deadline: Instant::now() + ACCEPT_WITHIN,
                answer: Some(answer),
            },
        );
        Ok((id, rx))
    }

    async fn deadline(&self, id: &str) -> Option<Instant> {
        self.registry
            .lock()
            .await
            .offers
            .get(id)
            .map(|o| o.deadline)
    }

    /// Close the offers whose deadline passed: not accepted (`expired`), or
    /// accepted and not read (`disconnected`). Called by the connection
    /// heartbeat, and by an offer's own timer.
    pub async fn expire(&self) {
        let mut registry = self.registry.lock().await;
        let now = Instant::now();
        let expired: Vec<_> = registry
            .offers
            .iter()
            .filter(|(_, o)| now >= o.deadline)
            .map(|(id, o)| {
                let outcome = if o.reader.is_some() {
                    Outcome::Disconnected
                } else {
                    Outcome::Expired
                };
                (id.clone(), outcome)
            })
            .collect();
        for (id, outcome) in expired {
            registry.close(&id, outcome).await;
        }
    }
}

/// `len` bytes of `file` at `offset`; fewer means the file became shorter.
fn read_at(file: &std::fs::File, offset: u64, len: u64) -> io::Result<Vec<u8>> {
    let mut bytes = vec![0; usize::try_from(len).map_err(io::Error::other)?];
    let mut filled = 0;
    while filled < bytes.len() {
        #[cfg(unix)]
        let read = {
            use std::os::unix::fs::FileExt;
            file.read_at(&mut bytes[filled..], offset + filled as u64)
        };
        #[cfg(windows)]
        let read = {
            use std::os::windows::fs::FileExt;
            file.seek_read(&mut bytes[filled..], offset + filled as u64)
        };
        match read {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the file became shorter",
                ))
            }
            Ok(n) => filled += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::ws::Message;
    use tokio::sync::mpsc::UnboundedReceiver;

    async fn client(
        offers: &FileOffers,
        id: &str,
        sessions: &[&str],
    ) -> UnboundedReceiver<Message> {
        let (sender, rx) = WsSender::channel();
        offers
            .register(
                id,
                Arc::new(sender),
                sessions.iter().map(|s| (*s).into()).collect(),
            )
            .await;
        rx
    }

    async fn event(rx: &mut UnboundedReceiver<Message>) -> Value {
        let Message::Text(text) = rx.recv().await.expect("event") else {
            panic!("text frame")
        };
        serde_json::from_str(&text).expect("JSON")
    }

    /// A file of `bytes` in `dir`, opened for an offer.
    fn opened(dir: &Path, bytes: &[u8]) -> OfferedFile {
        let path = dir.join("report.pdf");
        std::fs::write(&path, bytes).expect("file");
        open(&path).expect("open")
    }

    fn request() -> OfferRequest {
        OfferRequest {
            session_id: "s1".into(),
            path: "/srv/app/report.pdf".into(),
        }
    }

    /// terminal-sessions, File Offers, Two windows show the session: the
    /// first to accept takes it, the other drops it, and only the reader
    /// gets chunks.
    #[tokio::test]
    async fn two_windows_show_the_session() {
        let dir = tempfile::tempdir().expect("dir");
        let offers = FileOffers::default();
        let mut a = client(&offers, "a", &["s1"]).await;
        let mut b = client(&offers, "b", &["s1"]).await;
        let mut c = client(&offers, "c", &["other"]).await;
        let bytes: Vec<u8> = (0..(CHUNK_BYTES + 10)).map(|i| i as u8).collect();
        let (id, mut answer) = offers
            .admit(&request(), opened(dir.path(), &bytes))
            .await
            .expect("admit");
        let offer = event(&mut a).await;
        assert_eq!(offer["type"], "file.offer");
        assert_eq!(offer["offerId"], id);
        assert_eq!(offer["sessionId"], "s1");
        assert_eq!(offer["name"], "report.pdf");
        assert_eq!(offer["size"], bytes.len() as u64);
        assert!(offer["expiresAt"].as_u64().is_some());
        assert_eq!(event(&mut b).await["offerId"], id);
        assert!(c.try_recv().is_err());

        // Nothing is read before an accept.
        offers.read("b", &json!({"offerId":id,"offset":0})).await;
        assert!(b.try_recv().is_err());

        offers.accept("b", &json!({"offerId":id})).await;
        let closed = event(&mut a).await;
        assert_eq!(
            closed,
            json!({"type":"file.closed","offerId":id,"reason":"taken"})
        );
        offers.accept("a", &json!({"offerId":id})).await;
        offers.read("a", &json!({"offerId":id,"offset":0})).await;
        assert!(a.try_recv().is_err());

        offers.read("b", &json!({"offerId":id,"offset":0})).await;
        let first = event(&mut b).await;
        assert_eq!(first["type"], "file.chunk");
        assert_eq!(first["offset"], 0);
        assert_eq!(first["eof"], false);
        offers
            .read("b", &json!({"offerId":id,"offset":CHUNK_BYTES}))
            .await;
        let second = event(&mut b).await;
        assert_eq!(second["eof"], true);
        let decode = |v: &Value| {
            base64::engine::general_purpose::STANDARD
                .decode(v["data"].as_str().expect("data"))
                .expect("base64")
        };
        assert_eq!([decode(&first), decode(&second)].concat(), bytes);
        assert_eq!(answer.try_recv().expect("answer"), Outcome::Saved);
        assert!(offers.registry.lock().await.offers.is_empty());
    }

    /// terminal-sessions, File Offers, Reading past the order: a read at
    /// another offset gets no chunk.
    #[tokio::test]
    async fn reading_past_the_order_gets_nothing() {
        let dir = tempfile::tempdir().expect("dir");
        let offers = FileOffers::default();
        let mut a = client(&offers, "a", &["s1"]).await;
        let (id, _answer) = offers
            .admit(&request(), opened(dir.path(), &[1; 100]))
            .await
            .expect("admit");
        event(&mut a).await;
        offers.accept("a", &json!({"offerId":id})).await;
        for offset in [50, 1, 100] {
            offers
                .read("a", &json!({"offerId":id,"offset":offset}))
                .await;
        }
        assert!(a.try_recv().is_err());
        offers.read("a", &json!({"offerId":id,"offset":0})).await;
        assert_eq!(event(&mut a).await["eof"], true);
    }

    /// Dismiss during a download: the reader's `file.decline` stops the
    /// transfer as `declined`, and nothing more is read.
    #[tokio::test]
    async fn dismiss_during_a_download() {
        let dir = tempfile::tempdir().expect("dir");
        let offers = FileOffers::default();
        let mut a = client(&offers, "a", &["s1"]).await;
        let bytes = vec![9u8; (2 * CHUNK_BYTES) as usize];
        let (id, mut answer) = offers
            .admit(&request(), opened(dir.path(), &bytes))
            .await
            .expect("admit");
        event(&mut a).await;
        offers.accept("a", &json!({"offerId":id})).await;
        offers.read("a", &json!({"offerId":id,"offset":0})).await;
        assert_eq!(event(&mut a).await["eof"], false);

        offers.decline("a", &json!({"offerId":id})).await;

        assert_eq!(answer.try_recv().expect("answer"), Outcome::Declined);
        offers
            .read("a", &json!({"offerId":id,"offset":CHUNK_BYTES}))
            .await;
        assert!(a.try_recv().is_err());
        assert!(offers.registry.lock().await.offers.is_empty());
    }

    /// A zero-size file is one empty chunk with `eof`.
    #[tokio::test]
    async fn an_empty_file_is_one_empty_chunk() {
        let dir = tempfile::tempdir().expect("dir");
        let offers = FileOffers::default();
        let mut a = client(&offers, "a", &["s1"]).await;
        let (id, answer) = offers
            .admit(&request(), opened(dir.path(), b""))
            .await
            .expect("admit");
        event(&mut a).await;
        offers.accept("a", &json!({"offerId":id})).await;
        offers.read("a", &json!({"offerId":id,"offset":0})).await;
        let chunk = event(&mut a).await;
        assert_eq!(chunk["data"], "");
        assert_eq!(chunk["eof"], true);
        assert_eq!(answer.await.expect("answer"), Outcome::Saved);
    }

    /// control-api, File Offer Requests: no recipient, declines, the limits,
    /// a window that detaches.
    #[tokio::test]
    async fn offers_need_a_recipient_end_on_decline_and_are_bounded() {
        let dir = tempfile::tempdir().expect("dir");
        let offers = FileOffers::default();
        let file = || opened(dir.path(), b"x");
        assert_eq!(
            offers
                .admit(&request(), file())
                .await
                .expect_err("no client")
                .1,
            "no_client"
        );
        let mut a = client(&offers, "a", &["s1"]).await;
        let mut b = client(&offers, "b", &["s1"]).await;

        let (id, mut answer) = offers.admit(&request(), file()).await.expect("admit");
        event(&mut a).await;
        event(&mut b).await;
        offers.decline("a", &json!({"offerId":id})).await;
        assert!(answer.try_recv().is_err(), "b still shows it");
        offers.decline("b", &json!({"offerId":id})).await;
        assert_eq!(answer.try_recv().expect("answer"), Outcome::Declined);

        let (_one, _a1) = offers.admit(&request(), file()).await.expect("first");
        let (_two, _a2) = offers.admit(&request(), file()).await.expect("second");
        assert_eq!(
            offers.admit(&request(), file()).await.expect_err("third").1,
            "busy"
        );

        let detached = OfferRequest {
            session_id: "s2".into(),
            ..request()
        };
        let mut d = client(&offers, "d", &["s2"]).await;
        let (id, mut answer) = offers.admit(&detached, file()).await.expect("admit");
        event(&mut d).await;
        offers.accept("d", &json!({"offerId":id})).await;
        offers.sessions("d", HashSet::new()).await;
        assert_eq!(answer.try_recv().expect("answer"), Outcome::Disconnected);
    }

    /// Not accepted within the minute: `expired`, and the windows drop it.
    #[tokio::test(start_paused = true)]
    async fn an_offer_nobody_saves_expires() {
        let dir = tempfile::tempdir().expect("dir");
        let offers = Arc::new(FileOffers::default());
        let mut a = client(&offers, "a", &["s1"]).await;
        let file = opened(dir.path(), b"x");
        let worker = {
            let offers = offers.clone();
            tokio::spawn(async move { offers.offer(&request(), file).await })
        };
        let offer = event(&mut a).await;
        tokio::time::advance(ACCEPT_WITHIN + Duration::from_secs(1)).await;
        assert_eq!(
            worker.await.expect("worker").expect_err("expired").1,
            "expired"
        );
        assert_eq!(
            event(&mut a).await,
            json!({"type":"file.closed","offerId":offer["offerId"],"reason":"expired"})
        );
    }

    #[test]
    fn only_regular_files_within_the_limit_are_opened() {
        let dir = tempfile::tempdir().expect("dir");
        assert_eq!(open(dir.path()).expect_err("folder").1, "invalid_path");
        assert_eq!(
            open(&dir.path().join("missing")).expect_err("missing").1,
            "invalid_path"
        );
        #[cfg(unix)]
        {
            let fifo = dir.path().join("fifo");
            nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).expect("fifo");
            assert_eq!(open(&fifo).expect_err("fifo").1, "invalid_path");
            let target = dir.path().join("target.txt");
            std::fs::write(&target, "linked").expect("file");
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&target, &link).expect("link");
            let opened = open(&link).expect("link");
            assert_eq!(opened.size, 6);
            assert_eq!(opened.name, "link");
        }
        let big = dir.path().join("big");
        let file = std::fs::File::create(&big).expect("big");
        file.set_len(MAX_FILE_BYTES + 1).expect("sparse");
        assert_eq!(open(&big).expect_err("big").1, "too_large");
    }
}
