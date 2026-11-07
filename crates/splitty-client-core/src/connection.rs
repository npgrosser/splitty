//! A window's ordered native stream, without renderer or endpoint authority.
use serde_json::{json, Value};
use splitty_core::mux::ScreenOp;
use splitty_transport::{
    budget::{Charged, Lease},
    codec,
    peer::{Incoming, Sender},
    wire::{self, Kind},
};
use std::{collections::HashMap, io};
use uuid::Uuid;

pub(crate) const MAX_ATTACHMENTS: usize = 128;
const MAX_ID_BYTES: usize = 128;

enum Backend {
    #[cfg(not(target_os = "windows"))]
    Service(splitty_server::native::client::Client),
    Owned(splitty_remote::owner::Connection),
}

/// A local identity that cannot be confused with another connection to the
/// same service. Events and pending document reads carry it across UI queues.
pub struct Client {
    backend: Backend,
    id: Uuid,
    attachments: HashMap<String, Attachment>,
}

struct Attachment {
    replay: bool,
    _lease: Lease,
}

/// Both the received frame/credit and its decoded allocation survive until
/// the consumer drops this event. No unbounded intermediate event queue exists.
pub struct Event {
    pub connection_id: Uuid,
    raw: Charged<Incoming>,
    json: Option<Charged<Value>>,
    replay: bool,
}

pub struct Output<'a> {
    pub session_id: &'a str,
    pub bytes: &'a [u8],
    /// Suppress emulator-generated terminal replies and clipboard effects.
    pub replay: bool,
}

impl Event {
    pub fn json(&self) -> Option<&Value> {
        self.json.as_ref().map(|value| &value.value)
    }

    pub fn output(&self) -> Option<Output<'_>> {
        if self.raw.frame.kind != Kind::Binary {
            return None;
        }
        let (_, bytes) = self.raw.frame.application().ok()?;
        let (session_id, bytes) = binary(bytes)?;
        Some(Output {
            session_id,
            bytes,
            replay: self.replay,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ScreenIds {
    pub screen_id: String,
    pub pane_id: String,
}

impl ScreenIds {
    pub fn new() -> Self {
        Self {
            screen_id: Uuid::new_v4().to_string(),
            pane_id: Uuid::new_v4().to_string(),
        }
    }
}

impl Default for ScreenIds {
    fn default() -> Self {
        Self::new()
    }
}

/// A source selection; byte-range mapping belongs to the native text layout.
pub struct AnnotationDraft {
    pub quote: String,
    pub prefix: String,
    pub suffix: String,
    pub start_line: u32,
    pub end_line: u32,
    pub body: String,
}

impl Client {
    #[cfg(not(target_os = "windows"))]
    pub fn from_service(client: splitty_server::native::client::Client) -> Self {
        Self::new(Backend::Service(client))
    }

    /// The trusted launcher supplies an already negotiated owned connection;
    /// document content never chooses a helper, host, role or endpoint.
    pub fn from_owned(connection: splitty_remote::owner::Connection) -> Self {
        Self::new(Backend::Owned(connection))
    }

    fn new(backend: Backend) -> Self {
        Self {
            backend,
            id: Uuid::new_v4(),
            attachments: HashMap::new(),
        }
    }

    pub fn connection_id(&self) -> Uuid {
        self.id
    }

    pub fn ready(&self) -> &wire::Ready {
        match &self.backend {
            #[cfg(not(target_os = "windows"))]
            Backend::Service(client) => &client.ready,
            Backend::Owned(client) => client.ready(),
        }
    }

    pub(crate) fn sender(&self) -> Sender {
        match &self.backend {
            #[cfg(not(target_os = "windows"))]
            Backend::Service(client) => client.sender(),
            Backend::Owned(client) => client.sender(),
        }
    }

    /// Only this method receives the stream, preserving replay/event order.
    /// Cancellation of a pending receive leaves the carrier reader intact.
    pub async fn next_event(&mut self) -> io::Result<Option<Event>> {
        let result = self.receive_event().await;
        if result.is_err() {
            // A malformed or unaccountable event cannot be skipped while
            // pretending the ordered session stream is still intact.
            self.sender().close();
        }
        result
    }

    async fn receive_event(&mut self) -> io::Result<Option<Event>> {
        loop {
            let raw = match &mut self.backend {
                #[cfg(not(target_os = "windows"))]
                Backend::Service(client) => client.recv().await,
                Backend::Owned(client) => client.recv().await,
            };
            let Some(raw) = raw else { return Ok(None) };
            let mut replay = false;
            let decoded = match raw.frame.kind {
                Kind::Text => {
                    let (_, bytes) = raw.frame.application()?;
                    let lease = codec::reserve_json(bytes, &self.sender().context().budget, false)?;
                    let value: Value = serde_json::from_slice(bytes)
                        .map_err(|_| invalid("invalid session JSON"))?;
                    if let Some(session) = value["sessionId"].as_str() {
                        observe_attachment(&mut self.attachments, session, value["type"].as_str());
                    }
                    Some(lease.charge(value))
                }
                Kind::Binary => {
                    let (_, bytes) = raw.frame.application()?;
                    let (id, _) = binary(bytes).ok_or_else(|| invalid("invalid session output"))?;
                    let Some(attachment) = self.attachments.get(id) else {
                        // Output already queued when a session was detached
                        // never reaches a new terminal or another connection.
                        continue;
                    };
                    replay = attachment.replay;
                    None
                }
                _ => return Err(invalid("unexpected native session frame")),
            };
            return Ok(Some(Event {
                connection_id: self.id,
                raw,
                json: decoded,
                replay,
            }));
        }
    }

    /// Close only this stream. The service/owner lifetime is independent.
    pub async fn finish(self) -> io::Result<()> {
        match self.backend {
            #[cfg(not(target_os = "windows"))]
            Backend::Service(client) => client.finish().await,
            Backend::Owned(client) => client.finish().await,
        }
    }

    // Charge temporary command DOMs as well as Sender's queued serialization.
    pub(crate) fn command(
        &self,
        text_bytes: usize,
        make: impl FnOnce() -> Value,
    ) -> io::Result<()> {
        let bytes = text_bytes
            .checked_add(2048)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| invalid("command size overflow"))?;
        let sender = self.sender();
        let _scratch = sender
            .context()
            .budget
            .reserve(bytes, false)
            .map_err(io::Error::other)?;
        sender.json(Kind::Text, &make())
    }

    pub fn create_screen(&self, name: &str) -> io::Result<ScreenIds> {
        let ids = ScreenIds::new();
        self.create_screen_with_ids(&ids, name)?;
        self.resync(&ids.screen_id)?;
        Ok(ids)
    }

    /// Submit a screen creation with IDs reserved by the caller. A later
    /// catalog or screen refresh is separate from mutation admission.
    pub fn create_screen_with_ids(&self, ids: &ScreenIds, name: &str) -> io::Result<()> {
        if name.len() > 256 {
            return Err(invalid("screen name too long"));
        }
        identifier(&ids.screen_id)?;
        identifier(&ids.pane_id)?;
        self.command(name.len(), || json!({"type":"mux.op", "screenId":"", "op":{
            "type":"create_screen", "screenId":ids.screen_id, "newPaneId":ids.pane_id, "name":name
        }}))
    }

    /// Refill a service-owned screen that ran empty, preserving its ID and name.
    pub fn recreate_screen(&self, screen_id: &str, new_pane_id: &str) -> io::Result<()> {
        self.submit_recreate_screen(screen_id, new_pane_id)?;
        self.resync(screen_id)
    }

    pub(crate) fn submit_recreate_screen(
        &self,
        screen_id: &str,
        new_pane_id: &str,
    ) -> io::Result<()> {
        identifier(screen_id)?;
        identifier(new_pane_id)?;
        self.command(0, || {
            json!({"type":"mux.op", "screenId":screen_id, "op":{
                "type":"create_screen", "screenId":screen_id, "newPaneId":new_pane_id
            }})
        })
    }

    pub fn resync(&self, screen_id: &str) -> io::Result<()> {
        identifier(screen_id)?;
        self.command(0, || json!({"type":"mux.resync", "screenId":screen_id}))
    }

    pub fn request_workspace(&self) -> io::Result<()> {
        self.command(0, || json!({"type":"mux.workspace_resync"}))
    }

    pub fn rename_screen(&self, screen_id: &str, name: &str) -> io::Result<()> {
        identifier(screen_id)?;
        if name.len() > 256 {
            return Err(invalid("screen name too long"));
        }
        self.command(name.len(), || {
            json!({"type":"mux.op", "screenId":screen_id, "op":{
                "type":"rename_screen", "name":name
            }})
        })
    }

    pub fn close_screen(&self, screen_id: &str) -> io::Result<()> {
        identifier(screen_id)?;
        self.command(0, || {
            json!({"type":"mux.op", "screenId":screen_id, "op":{
                "type":"close_screen"
            }})
        })
    }

    pub fn set_active_screen(&self, screen_id: &str) -> io::Result<()> {
        identifier(screen_id)?;
        self.command(0, || {
            json!({"type":"mux.op", "screenId":screen_id, "op":{
                "type":"set_active_screen"
            }})
        })
    }

    /// Send a typed mux operation and request the authoritative result. The
    /// origin of a successful operation does not receive a mutation nudge.
    pub fn screen_op(&self, screen_id: &str, op: &ScreenOp) -> io::Result<()> {
        self.submit_screen_op(screen_id, op)?;
        self.resync(screen_id)
    }

    pub(crate) fn submit_screen_op(&self, screen_id: &str, op: &ScreenOp) -> io::Result<()> {
        identifier(screen_id)?;
        let mut size = CountBytes(0);
        serde_json::to_writer(&mut size, op).map_err(io::Error::other)?;
        self.command(
            size.0,
            || json!({"type":"mux.op", "screenId":screen_id, "op":op}),
        )
    }

    /// Repeated local attachments coalesce: the service streams a session at
    /// most once per connection. A reconnect uses a fresh Client and identity.
    pub fn attach(&mut self, session_id: &str) -> io::Result<()> {
        identifier(session_id)?;
        if self.attachments.contains_key(session_id) {
            return Ok(());
        }
        if self.attachments.len() >= MAX_ATTACHMENTS {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "too many attachments",
            ));
        }
        let lease = self
            .sender()
            .context()
            .budget
            .reserve(512, false)
            .map_err(io::Error::other)?;
        self.command(0, || json!({"type":"attach", "id":session_id}))?;
        self.attachments.insert(
            session_id.into(),
            Attachment {
                replay: true,
                _lease: lease,
            },
        );
        Ok(())
    }

    pub fn detach(&mut self, session_id: &str) -> io::Result<()> {
        identifier(session_id)?;
        self.command(0, || json!({"type":"detach", "id":session_id}))?;
        self.attachments.remove(session_id);
        self.attachments.shrink_to_fit();
        Ok(())
    }

    pub fn input(&self, session_id: &str, text: &str) -> io::Result<()> {
        self.live(session_id)?;
        if text.len() > 64 * 1024 {
            return Err(invalid("input exceeds chunk limit"));
        }
        self.command(
            text.len(),
            || json!({"type":"input", "id":session_id, "data":text}),
        )
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16, redraw: bool) -> io::Result<()> {
        self.live(session_id)?;
        if cols == 0 || rows == 0 {
            return Err(invalid("empty terminal size"));
        }
        self.command(
            0,
            || json!({"type":"resize", "id":session_id, "cols":cols, "rows":rows, "redraw":redraw}),
        )
    }

    fn live(&self, session_id: &str) -> io::Result<()> {
        match self.attachments.get(session_id) {
            Some(Attachment { replay: false, .. }) => Ok(()),
            _ => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "session is not attached yet",
            )),
        }
    }

    pub fn activity(&self, takeover: bool) -> io::Result<()> {
        self.command(0, || json!({"type":"activity", "takeover":takeover}))
    }

    pub fn idle(&self) -> io::Result<()> {
        self.command(0, || json!({"type":"idle"}))
    }

    pub fn hide(&self, session_id: &str) -> io::Result<()> {
        identifier(session_id)?;
        self.command(0, || json!({"type":"hide", "id":session_id}))
    }

    pub fn open_markdown(
        &self,
        screen_id: &str,
        anchor_pane_id: &str,
        text: &str,
        title: &str,
    ) -> io::Result<String> {
        identifier(screen_id)?;
        identifier(anchor_pane_id)?;
        if text.len() > 1024 * 1024 || title.len() > 1024 {
            return Err(invalid("document source too large"));
        }
        let pane = Uuid::new_v4().to_string();
        self.command(text.len() + title.len(), || {
            json!({"type":"mux.op", "screenId":screen_id, "op":{
                "type":"open_viewer", "paneId":anchor_pane_id, "newPaneId":pane,
                "source":{"kind":"text", "title":title, "text":text, "format":"markdown"}
            }})
        })?;
        self.resync(screen_id)?;
        Ok(pane)
    }

    pub fn put_annotation(
        &self,
        screen_id: &str,
        pane_id: &str,
        draft: AnnotationDraft,
    ) -> io::Result<String> {
        identifier(screen_id)?;
        identifier(pane_id)?;
        if draft.quote.is_empty()
            || draft.quote.len() > 16 * 1024
            || draft.body.len() > 16 * 1024
            || draft.prefix.len() > 256
            || draft.suffix.len() > 256
            || draft.start_line == 0
            || draft.end_line < draft.start_line
            || draft.end_line > 10_000_000
        {
            return Err(invalid("invalid annotation selection"));
        }
        let id = format!("ann-{}", &Uuid::new_v4().simple().to_string()[..16]);
        let size = draft.quote.len() + draft.body.len() + draft.prefix.len() + draft.suffix.len();
        self.command(size, || json!({"type":"mux.op", "screenId":screen_id, "op":{
            "type":"put_annotations", "paneId":pane_id, "annotations":[{
                "id":id, "kind":"comment", "body":draft.body,
                "target":{"view":"source", "quote":draft.quote, "prefix":draft.prefix,
                    "suffix":draft.suffix, "startLine":draft.start_line, "endLine":draft.end_line}
            }]
        }}))?;
        self.resync(screen_id)?;
        Ok(id)
    }
}

/// Count serialized command bytes before allocating a temporary JSON value.
struct CountBytes(usize);
impl io::Write for CountBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| invalid("command size overflow"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn identifier(id: &str) -> io::Result<()> {
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return Err(invalid("invalid object identifier"));
    }
    Ok(())
}

pub(crate) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn binary(bytes: &[u8]) -> Option<(&str, &[u8])> {
    let (&length, bytes) = bytes.split_first()?;
    let length = usize::from(length);
    if length == 0 || bytes.len() < length {
        return None;
    }
    Some((
        std::str::from_utf8(&bytes[..length]).ok()?,
        &bytes[length..],
    ))
}

fn observe_attachment(
    attachments: &mut HashMap<String, Attachment>,
    session: &str,
    kind: Option<&str>,
) {
    match kind {
        Some("attached") => {
            if let Some(attachment) = attachments.get_mut(session) {
                attachment.replay = false;
            }
        }
        Some("closed") => {
            attachments.remove(session);
            attachments.shrink_to_fit();
        }
        Some("error")
            if attachments
                .get(session)
                .is_some_and(|attachment| attachment.replay) =>
        {
            // Before attached, this answers the pending attach. Errors for
            // subsequent input/resize operations do not revoke a live stream.
            attachments.remove(session);
            attachments.shrink_to_fit();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_transport::budget::ConnectionBudget;

    #[test]
    fn operation_errors_preserve_live_output_but_failed_attaches_release_their_state() {
        let budget = ConnectionBudget::default();
        let mut attachments = HashMap::new();
        for (id, replay) in [("pending", true), ("live", false)] {
            attachments.insert(
                id.to_owned(),
                Attachment {
                    replay,
                    _lease: budget.reserve(512, false).unwrap(),
                },
            );
        }
        observe_attachment(&mut attachments, "pending", Some("error"));
        assert!(!attachments.contains_key("pending"));
        observe_attachment(&mut attachments, "live", Some("error"));
        assert!(attachments.contains_key("live"));
        assert_eq!(budget.used(), 512);
        observe_attachment(&mut attachments, "live", Some("closed"));
        assert_eq!(budget.used(), 0);
    }
}
