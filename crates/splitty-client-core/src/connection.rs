//! A window's ordered native stream, without renderer or endpoint authority.
use base64::Engine as _;
use serde_json::{json, Value};
use splitty_core::mux::ScreenOp;
use splitty_core::protocol::message_type::MessageType;
use splitty_core::protocol::{capability, MAX_INPUT_BYTES};
use splitty_transport::{
    budget::{Charged, Lease},
    codec,
    peer::{Incoming, Sender},
    wire::{self, Kind},
};
use std::{collections::HashMap, io, sync::Arc};
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
    byte_input: bool,
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
    /// A bounded real carrier fixture for production-view tests. The returned
    /// event still owns its frame and JSON charges after both peers are joined.
    #[cfg(any(test, feature = "ui-test-support"))]
    #[doc(hidden)]
    pub async fn ui_test_json(connection_id: Uuid, value: &Value) -> io::Result<Self> {
        use splitty_transport::{peer::Peer, queue};
        let ready = wire::Ready {
            generation: 1,
            instance: "a".repeat(32),
            mux_protocol: wire::MUX_PROTOCOL_VERSION,
            purpose: wire::Purpose::Direct,
            limits: wire::Limits::default(),
        };
        let (left, right) = tokio::io::duplex(4096);
        let (read_left, write_left) = tokio::io::split(left);
        let (read_right, write_right) = tokio::io::split(right);
        let context = queue::Context::default();
        let left = Peer::start(read_left, write_left, &ready, queue::Context::default());
        let mut right = Peer::start(read_right, write_right, &ready, context.clone());
        let result = async {
            left.sender.json(Kind::Text, value)?;
            let raw = right
                .recv()
                .await
                .ok_or_else(|| invalid("fixture carrier closed"))?;
            let json = decode_event_json(&raw, &context.budget)?;
            Ok(Self {
                connection_id,
                raw,
                json: Some(json),
                replay: false,
            })
        }
        .await;
        let (left, right) = tokio::join!(left.finish(), right.finish());
        left?;
        right?;
        result
    }

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

fn decode_event_json(
    raw: &Incoming,
    budget: &splitty_transport::budget::ConnectionBudget,
) -> io::Result<Charged<Value>> {
    let (_, bytes) = raw.frame.application()?;
    let lease = codec::reserve_json(bytes, budget, false)?;
    let value = serde_json::from_slice(bytes).map_err(|_| invalid("invalid session JSON"))?;
    Ok(lease.charge(value))
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
            byte_input: false,
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

    /// The actual transport closure, if received. Earlier accepted events
    /// still need to be drained through `next_event` to EOF.
    pub fn closed_reason(&self) -> Option<Arc<Charged<wire::Closed>>> {
        match &self.backend {
            #[cfg(not(target_os = "windows"))]
            Backend::Service(client) => client.closed_reason(),
            Backend::Owned(client) => client.closed_reason(),
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
                    let value = decode_event_json(&raw, &self.sender().context().budget)?;
                    let kind = MessageType::of(&value);
                    if kind == MessageType::MuxWelcome {
                        self.byte_input =
                            capability::offered(&value, capability::TERMINAL_INPUT_BYTES);
                    }
                    if let Some(session) = value["sessionId"].as_str() {
                        observe_attachment(&mut self.attachments, session, kind);
                    }
                    Some(value)
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
        self.submit_create_screen(ids, Some(name))
    }

    /// Use the owning service's atomic default naming and launch settings.
    pub(crate) fn create_default_screen_with_ids(&self, ids: &ScreenIds) -> io::Result<()> {
        self.submit_create_screen(ids, None)
    }

    /// The first Screen of an empty workspace (`onlyIfEmpty`): a service with
    /// any Screen refuses it with `mux.op_failed`.
    pub(crate) fn create_first_screen_with_ids(
        &self,
        ids: &ScreenIds,
        name: Option<&str>,
    ) -> io::Result<()> {
        self.submit_screen_creation(ids, name, true)
    }

    pub(crate) fn submit_create_screen(
        &self,
        ids: &ScreenIds,
        name: Option<&str>,
    ) -> io::Result<()> {
        self.submit_screen_creation(ids, name, false)
    }

    fn submit_screen_creation(
        &self,
        ids: &ScreenIds,
        name: Option<&str>,
        only_if_empty: bool,
    ) -> io::Result<()> {
        if name.is_some_and(|name| name.len() > 256) {
            return Err(invalid("screen name too long"));
        }
        identifier(&ids.screen_id)?;
        identifier(&ids.pane_id)?;
        self.command(name.map_or(0, str::len), || {
            create_screen_message(ids, name, only_if_empty)
        })
    }

    pub(crate) fn submit_screen_transfer(
        &self,
        source: &str,
        pane: &str,
        target: &str,
        create: bool,
    ) -> io::Result<()> {
        identifier(source)?;
        identifier(pane)?;
        identifier(target)?;
        self.command(0, || json!({"type":"mux.op","screenId":source,"op":{
            "type":"move_pane_to_screen","paneId":pane,"targetScreenId":target,"createScreen":create
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

    /// The allowance this connection's decoded values are charged to.
    pub(crate) fn budget(&self) -> splitty_transport::budget::ConnectionBudget {
        self.sender().context().budget.clone()
    }

    /// Ask the service for a Screen's layout as a template
    /// (`mux.template_extracted` answers with the same request ID).
    pub fn extract_template(
        &self,
        request_id: &str,
        screen_id: &str,
        name: &str,
        root: &str,
    ) -> io::Result<()> {
        identifier(screen_id)?;
        if name.len() > 512 || root.len() > 4096 {
            return Err(invalid("Template name or root exceeds limit"));
        }
        self.command(name.len() + root.len(), || {
            json!({"type":"mux.extract_template", "requestId":request_id,
                "screenId":screen_id, "name":name, "root":root})
        })
    }

    /// Ask the service what closing `scope` ends (`mux.close_assessed`
    /// answers with the same request ID; `mux-protocol`, Close Assessment).
    pub fn assess_close(
        &self,
        request_id: &str,
        scope: &splitty_core::mux::CloseScope,
    ) -> io::Result<()> {
        let ids: Vec<&str> = match scope {
            splitty_core::mux::CloseScope::Panes {
                screen_id,
                pane_ids,
            } => std::iter::once(screen_id.as_str())
                .chain(pane_ids.iter().map(|id| id.as_str()))
                .collect(),
            splitty_core::mux::CloseScope::Screens { screen_ids } => {
                screen_ids.iter().map(|id| id.as_str()).collect()
            }
            splitty_core::mux::CloseScope::Workspace => Vec::new(),
        };
        if ids.len() > splitty_core::mux::CLOSE_SCOPE_LIMIT + 1 {
            return Err(invalid("Close scope exceeds limit"));
        }
        for id in &ids {
            identifier(id)?;
        }
        let size = ids.iter().map(|id| id.len() + 3).sum();
        self.command(
            size,
            || json!({"type":"mux.assess_close", "requestId":request_id, "scope":scope}),
        )
    }

    pub fn rename_screen(&self, screen_id: &str, name: &str) -> io::Result<()> {
        identifier(screen_id)?;
        validate_rename_name(name)?;
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

    /// Tell the local service which Screens a host has now, as
    /// `hub.catalog.observe`; only a main window's local connection may.
    pub(crate) fn report_host_screens(
        &self,
        target: &str,
        screens: &[splitty_core::native_api::directory::ObservedScreen],
    ) -> io::Result<()> {
        let text = screens.iter().fold(target.len(), |bytes, screen| {
            bytes + screen.id.len() + screen.name.as_ref().map_or(0, String::len) + 64
        });
        self.command(
            text,
            || json!({"type":"hub.catalog.observe", "target":target, "screens":screens}),
        )
    }

    pub fn set_active_screen(&self, screen_id: &str) -> io::Result<()> {
        identifier(screen_id)?;
        self.command(0, || {
            json!({"type":"mux.op", "screenId":screen_id, "op":{
                "type":"set_active_screen"
            }})
        })
    }

    /// Submit relative Screen order once; the source owns ordered catalog refresh.
    pub fn move_screen(&self, screen_id: &str, before: Option<&str>) -> io::Result<()> {
        identifier(screen_id)?;
        if let Some(before) = before {
            identifier(before)?;
        }
        self.command(before.map_or(0, str::len), || {
            json!({"type":"mux.op", "screenId":screen_id, "op":{
                "type":"move_screen", "beforeScreenId":before
            }})
        })
    }

    /// Send a typed mux operation and request the authoritative result. The
    /// origin of a successful operation does not receive a mutation nudge.
    pub fn screen_op(&self, screen_id: &str, op: &ScreenOp) -> io::Result<()> {
        self.submit_screen_op(screen_id, op)?;
        self.resync(screen_id)
    }

    pub(crate) fn submit_template(
        &self,
        screen_id: &str,
        request: &splitty_core::mux::ApplyTemplate,
    ) -> io::Result<()> {
        identifier(screen_id)?;
        let mut size = CountBytes(0);
        serde_json::to_writer(&mut size, request).map_err(io::Error::other)?;
        self.command(size.0, || {
            let mut op = serde_json::to_value(request).expect("serializable template");
            op["type"] = json!("apply_template");
            json!({"type":"mux.op","screenId":screen_id,"op":op})
        })
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
        if text.len() > MAX_INPUT_BYTES {
            return Err(invalid("input exceeds chunk limit"));
        }
        self.command(
            text.len(),
            || json!({"type":"input", "id":session_id, "data":text}),
        )
    }

    /// Preserve emulator input bytes. UTF-8 uses the existing text message;
    /// other bytes require the current service's advertised extension.
    pub fn input_bytes(&self, session_id: &str, bytes: &[u8]) -> io::Result<()> {
        self.live(session_id)?;
        if let Some(text) = text_input(bytes, self.byte_input)? {
            return self.input(session_id, text);
        }
        self.command(bytes.len().div_ceil(3) * 4, || {
            json!({"type":"input_bytes", "id":session_id,
                "data":base64::engine::general_purpose::STANDARD.encode(bytes)})
        })
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

    /// Answer the process waiting for a review of a viewer pane
    /// (`mux-protocol`, Review Answers).
    pub fn answer_review(
        &self,
        screen_id: &str,
        pane_id: &str,
        wait_id: &str,
        verdict: crate::annotations::Verdict,
        review: Option<&str>,
    ) -> io::Result<()> {
        identifier(screen_id)?;
        identifier(pane_id)?;
        identifier(wait_id)?;
        let review = review.filter(|text| !text.is_empty());
        if review.is_some_and(|text| text.len() > 1024 * 1024)
            || (review.is_none() && verdict == crate::annotations::Verdict::ChangesRequested)
        {
            return Err(invalid("invalid review"));
        }
        self.command(review.map_or(0, str::len), || {
            let mut message = json!({"type":"viewer.review_answer", "screenId":screen_id,
                "paneId":pane_id, "waitId":wait_id, "outcome":verdict.outcome()});
            if let Some(review) = review {
                message["review"] = json!(review);
            }
            message
        })
    }

    /// Ask for the anchors of a viewer pane's annotations in its document
    /// (`viewer.anchor`); the answer names the returned request ID.
    pub fn ask_viewer_anchors(&self, pane_id: &str) -> io::Result<String> {
        identifier(pane_id)?;
        let request_id = Uuid::new_v4().to_string();
        self.command(
            0,
            || json!({"type":"viewer.anchor", "requestId":request_id, "paneId":pane_id}),
        )?;
        Ok(request_id)
    }

    /// Ask for a viewer pane's review (`viewer.review`), as the answer to a
    /// waiting process with `verdict`; the answer names the returned request ID.
    pub fn ask_viewer_review(
        &self,
        pane_id: &str,
        verdict: Option<crate::annotations::Verdict>,
        changed_during_wait: bool,
    ) -> io::Result<String> {
        identifier(pane_id)?;
        let request_id = Uuid::new_v4().to_string();
        self.command(0, || {
            let mut message =
                json!({"type":"viewer.review", "requestId":request_id, "paneId":pane_id});
            if let Some(verdict) = verdict {
                message["verdict"] = json!(verdict.outcome());
                message["changedDuringWait"] = json!(changed_during_wait);
            }
            message
        })?;
        Ok(request_id)
    }

    /// A plain text in a viewer pane to the right of `anchor_pane_id`
    /// (`viewer-panes`, Terminal Selection In A Viewer).
    pub fn open_text_viewer(
        &self,
        screen_id: &str,
        anchor_pane_id: &str,
        title: &str,
        text: &str,
        cwd: Option<&str>,
    ) -> io::Result<String> {
        identifier(screen_id)?;
        identifier(anchor_pane_id)?;
        if text.is_empty() || text.len() > 262_144 {
            return Err(invalid("The selection is too large for a viewer"));
        }
        let title: String = title
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect();
        let pane = Uuid::new_v4().to_string();
        self.command(text.len() + title.len(), || {
            let mut op = json!({
                "type":"open_viewer", "paneId":anchor_pane_id, "newPaneId":pane,
                "placement":"right",
                "source":{"kind":"text", "title":title, "text":text}
            });
            if let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty() && cwd.len() <= 4096) {
                op["cwd"] = json!(cwd);
            }
            json!({"type":"mux.op", "screenId":screen_id, "op":op})
        })?;
        Ok(pane)
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

/// Decide the wire encoding before any command admission or allocation.
fn text_input(bytes: &[u8], byte_input: bool) -> io::Result<Option<&str>> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(invalid("input exceeds chunk limit"));
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(Some(text)),
        Err(_) if byte_input => Ok(None),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Server needs terminal-input-bytes-v1 for non-UTF-8 terminal input",
        )),
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

pub(crate) fn validate_rename_name(name: &str) -> io::Result<()> {
    if name.len() > 256 {
        return Err(invalid("screen name too long"));
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
    kind: MessageType,
) {
    match kind {
        MessageType::Attached => {
            if let Some(attachment) = attachments.get_mut(session) {
                attachment.replay = false;
            }
        }
        MessageType::Closed => {
            attachments.remove(session);
            attachments.shrink_to_fit();
        }
        MessageType::Error
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

fn create_screen_message(ids: &ScreenIds, name: Option<&str>, only_if_empty: bool) -> Value {
    let mut op = json!({"type":"create_screen", "screenId":ids.screen_id, "newPaneId":ids.pane_id});
    if let Some(name) = name {
        op["name"] = json!(name);
    }
    if only_if_empty {
        op["onlyIfEmpty"] = json!(true);
    }
    json!({"type":"mux.op", "screenId":"", "op":op})
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_transport::budget::ConnectionBudget;

    #[test]
    fn remote_creation_default_wire_has_only_existing_ids_and_named_local_call_stays_compatible() {
        let ids = ScreenIds {
            screen_id: "screen".into(),
            pane_id: "pane".into(),
        };
        let expected = json!({"type":"mux.op","screenId":"","op":{"type":"create_screen","screenId":"screen","newPaneId":"pane"}});
        assert_eq!(create_screen_message(&ids, None, false), expected);
        let mut named = expected.clone();
        named["op"]["name"] = json!("Local name");
        assert_eq!(
            create_screen_message(&ids, Some("Local name"), false),
            named
        );
        let mut first = expected;
        first["op"]["onlyIfEmpty"] = json!(true);
        assert_eq!(create_screen_message(&ids, None, true), first);
    }

    #[test]
    fn byte_input_requires_capability_without_breaking_legacy_text_or_splitting_paste() {
        let paste = "\x1b[200~Grüße\nsecond line\x1b[201~";
        for supports_bytes in [false, true] {
            assert_eq!(
                text_input(paste.as_bytes(), supports_bytes).unwrap(),
                Some(paste)
            );
            assert_eq!(
                text_input(&vec![b'a'; MAX_INPUT_BYTES + 1], supports_bytes)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        let report = b"\x1b[M \x80\xff";
        assert_eq!(
            text_input(report, false).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(text_input(report, true).unwrap(), None);
        assert!(text_input(&vec![0xff; MAX_INPUT_BYTES], true).is_ok());
    }

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
        observe_attachment(&mut attachments, "pending", MessageType::Error);
        assert!(!attachments.contains_key("pending"));
        observe_attachment(&mut attachments, "live", MessageType::Error);
        assert!(attachments.contains_key("live"));
        assert_eq!(budget.used(), 512);
        observe_attachment(&mut attachments, "live", MessageType::Unknown);
        assert_eq!(budget.used(), 512);
        observe_attachment(&mut attachments, "live", MessageType::Closed);
        assert_eq!(budget.used(), 0);
    }
}
