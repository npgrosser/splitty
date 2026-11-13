//! Explicit browser access to one assigned native backend. Credentials and
//! assets terminate here; API/session connections have independent lifetimes.
use super::operation::{self, parse_query, rejected};
use super::{browser_wire, middleware, routes, session, BrowserState};
use crate::{
    hub::Step,
    native::client::{Client, Connector, Pending},
    service::catalog::{self, Cancellation, Operation, Role},
};
use axum::{
    extract::{FromRef, Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, on, post, MethodFilter},
    Json, Router,
};
use serde_json::{json, Value};
use splitty_remote::{
    bridge::NativeBridge,
    handshake::RunningDaemon,
    tunnel::{Remote, SshRemote},
};
use splitty_transport::{
    budget::Charged,
    budget::{ConnectionBudget, Lease},
    peer::{Incoming, Sender},
    wire::{self, Kind},
};
use std::{io, sync::Arc};
use tokio::sync::watch;

mod api_client;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Control, Data, OpCode};

#[derive(Clone)]
enum Descriptor {
    Remote(RunningDaemon),
    Embedded(String),
    #[cfg(unix)]
    Local(splitty_core::daemon::DaemonInfo),
}
enum Source {
    Remote(Arc<SshRemote>),
    Embedded {
        state: Arc<crate::service::State>,
        connector: Connector,
    },
    #[cfg(unix)]
    Local(Connector),
}
struct Ticket {
    descriptor: Descriptor,
    revision: watch::Receiver<Option<Descriptor>>,
}

struct ResponseBody {
    bytes: Vec<u8>,
    _lease: Lease,
}
impl AsRef<[u8]> for ResponseBody {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// Only the command's trusted supervisor can replace discovery. A browser
/// method, query parameter or upstream reply cannot select another backend.
pub struct Backend {
    source: Source,
    current: watch::Sender<Option<Descriptor>>,
    api: api_client::Pool,
}
impl Backend {
    /// The caller owns the service lifetime. This route never discovers a
    /// daemon/keeper or creates a listening endpoint.
    pub fn embedded(state: Arc<crate::service::State>) -> Arc<Self> {
        let (current, _) = watch::channel(Some(Descriptor::Embedded(state.instance.clone())));
        Arc::new(Self {
            source: Source::Embedded {
                state,
                connector: Connector::default(),
            },
            current,
            api: api_client::Pool::default(),
        })
    }
    pub fn close(&self) {
        self.current.send_replace(None);
    }
    pub fn remote(remote: Arc<SshRemote>) -> Arc<Self> {
        let (current, _) = watch::channel(None);
        Arc::new(Self {
            source: Source::Remote(remote),
            current,
            api: api_client::Pool::default(),
        })
    }
    pub fn set_remote(&self, daemon: Option<RunningDaemon>) {
        assert!(matches!(self.source, Source::Remote(_)));
        self.current.send_replace(daemon.map(Descriptor::Remote));
    }
    #[cfg(unix)]
    pub fn daemon(info: splitty_core::daemon::DaemonInfo) -> Arc<Self> {
        let (current, _) = watch::channel(Some(Descriptor::Local(info)));
        Arc::new(Self {
            source: Source::Local(Connector::default()),
            current,
            api: api_client::Pool::default(),
        })
    }
    #[cfg(unix)]
    pub fn set_daemon(&self, info: Option<splitty_core::daemon::DaemonInfo>) {
        assert!(matches!(self.source, Source::Local(_)));
        self.current.send_replace(info.map(Descriptor::Local));
    }
    fn admit_host(&self) -> io::Result<Pending> {
        match &self.source {
            Source::Embedded { connector, .. } => connector.admit(),
            #[cfg(unix)]
            Source::Local(connector) => connector.admit(),
            Source::Remote(_) => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "an assigned remote gateway cannot open other hosts",
            )),
        }
    }
    async fn open_host(
        &self,
        descriptor: Descriptor,
        target: splitty_remote::ssh::SshTarget,
        update: bool,
        pending: Pending,
        reports: splitty_transport::queue::Sender<wire::Progress>,
    ) -> io::Result<Connection> {
        let progress = move |report: splitty_transport::budget::Charged<wire::Progress>| {
            // Negotiation and this queue share the pending connection budget.
            // Transfer the parser reservation without copying or recharging.
            let _ = reports.forward(report);
        };
        match (&self.source, descriptor) {
            (Source::Embedded { state, connector }, Descriptor::Embedded(instance))
                if state.instance == instance =>
            {
                connector
                    .host_admitted(state.clone(), target, update, pending, progress)
                    .await
                    .map(Connection::Local)
            }
            #[cfg(unix)]
            (Source::Local(connector), Descriptor::Local(info)) => connector
                .local_host_admitted(&info, target, update, pending, progress)
                .await
                .map(Connection::Local),
            _ => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "backend cannot open hosts",
            )),
        }
    }
    fn role(&self) -> Role {
        match self.source {
            Source::Remote(_) => Role::Remote,
            Source::Embedded { .. } => Role::Main,
            #[cfg(unix)]
            Source::Local(_) => Role::Main,
        }
    }
    fn ticket(&self) -> io::Result<Ticket> {
        let mut revision = self.current.subscribe();
        let descriptor = revision.borrow_and_update().clone().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "backend is reconnecting")
        })?;
        Ok(Ticket {
            descriptor,
            revision,
        })
    }
    async fn open(
        &self,
        ticket: Ticket,
    ) -> io::Result<(Connection, watch::Receiver<Option<Descriptor>>)> {
        let Ticket {
            descriptor,
            mut revision,
        } = ticket;
        let connect = async {
            match (&self.source, descriptor) {
                (Source::Remote(remote), Descriptor::Remote(daemon)) => {
                    let pending = remote.admit().map_err(io::Error::other)?;
                    remote
                        .open_bridge(&daemon, pending)
                        .await
                        .map(Connection::Remote)
                        .map_err(io::Error::other)
                }
                (Source::Embedded { state, connector }, Descriptor::Embedded(instance))
                    if instance == state.instance =>
                {
                    connector
                        .embedded(state.clone(), Role::Main)
                        .await
                        .map(Connection::Local)
                }
                #[cfg(unix)]
                (Source::Local(connector), Descriptor::Local(info)) => {
                    connector.local(&info).await.map(Connection::Local)
                }
                _ => Err(io::Error::other("backend assignment mismatch")),
            }
        };
        let connection = tokio::select! { biased;
            _ = revision.changed() => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "backend changed during connection setup")),
            result = connect => result?,
        };
        Ok((connection, revision))
    }
}

enum Connection {
    Remote(NativeBridge),
    Local(Client),
}
impl Connection {
    fn sender(&self) -> Sender {
        match self {
            Self::Remote(client) => client.sender(),
            Self::Local(client) => client.sender(),
        }
    }
    async fn recv(&mut self) -> Option<Charged<Incoming>> {
        match self {
            Self::Remote(client) => client.recv().await,
            Self::Local(client) => client.recv().await,
        }
    }
    async fn finish(self) {
        match self {
            Self::Remote(client) => {
                let _ = client.finish().await;
            }
            Self::Local(client) => {
                let _ = client.finish().await;
            }
        }
    }
}

#[derive(Clone)]
struct Gateway {
    backend: Arc<Backend>,
    browser: BrowserState,
}
impl FromRef<Gateway> for BrowserState {
    fn from_ref(state: &Gateway) -> Self {
        state.browser.clone()
    }
}

pub fn router(backend: Arc<Backend>, browser: BrowserState) -> Router {
    if let Source::Embedded { state, .. } = &backend.source {
        if let Ok(address) = browser.listen_addr.parse::<std::net::SocketAddr>() {
            state.hub.reserve_local_port(address.port());
        }
    }
    let state = Gateway {
        backend,
        browser: browser.clone(),
    };
    let mut app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route(
            "/api/session",
            post(session::exchange_bootstrap)
                .layer(axum::extract::DefaultBodyLimit::max(4096))
                .layer(axum::middleware::from_fn(bootstrap_body)),
        )
        .route("/api/ws-ticket", post(session::issue_ws_ticket))
        .route("/api/bootstrap", post(session::issue_bootstrap))
        .route("/api/client-assignment", get(client_assignment))
        .route("/ws", get(upgrade))
        .route("/ws/remote", get(remote_upgrade));
    for entry in catalog::OPERATIONS {
        let operation = entry.operation;
        let method = MethodFilter::try_from(
            Method::from_bytes(entry.method.as_bytes()).expect("catalog method"),
        )
        .expect("catalog filter");
        app = app.route(
            entry.path,
            on(
                method,
                move |State(state): State<Gateway>, request: Request| {
                    api(state, operation, request)
                },
            ),
        );
    }
    app.fallback(routes::serve_frontend)
        .layer(axum::middleware::from_fn_with_state(
            browser.clone(),
            fixed_gateway,
        ))
        .layer(axum::middleware::from_fn_with_state(
            browser.clone(),
            middleware::require_token,
        ))
        .layer(axum::middleware::from_fn_with_state(
            browser.clone(),
            middleware::require_origin,
        ))
        .layer(axum::middleware::from_fn_with_state(browser, exact_host))
        .with_state(state)
}

/// Presentation identity comes from the local gateway's fixed command-line
/// assignment, never from a query or an upstream service response. This remains
/// available while that backend reconnects and inherits all API guards.
async fn client_assignment(State(state): State<Gateway>) -> Response {
    // The random cookie *name* identifies this gateway, without exposing a
    // session secret. Its source/backend cannot change during this lifetime.
    let gateway = state.browser.sessions.cookie_name();
    let assignment = match &state.backend.source {
        Source::Remote(remote) => {
            json!({ "gateway": gateway, "role": "remote", "target": remote.host() })
        }
        _ => json!({ "gateway": gateway, "role": "main" }),
    };
    (
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(assignment),
    )
        .into_response()
}

/// An already loaded page must not act on a replacement gateway merely because
/// another tab obtained its cookie. Check before reading bodies or issuing a
/// ticket. CLI/browser callers without an assigned document retain normal auth.
async fn fixed_gateway(
    State(browser): State<BrowserState>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(expected) = request.headers().get("x-splitty-gateway") {
        if expected.as_bytes() != browser.sessions.cookie_name().as_bytes() {
            return (
                StatusCode::CONFLICT,
                [
                    ("x-splitty-gateway-changed", "1"),
                    ("cache-control", "no-store"),
                ],
                "browser gateway changed; reload this document",
            )
                .into_response();
        }
    }
    next.run(request).await
}

async fn bootstrap_body(request: Request, next: Next) -> Response {
    let budget = ConnectionBudget::default();
    // Json's collector may retain chunks while assembling its contiguous body.
    let _body = match budget.reserve(2 * 4096, false) {
        Ok(lease) => lease,
        Err(error) => return rejected(StatusCode::SERVICE_UNAVAILABLE, error, false),
    };
    match tokio::time::timeout(std::time::Duration::from_secs(15), next.run(request)).await {
        Ok(response) => response,
        Err(_) => rejected(
            StatusCode::REQUEST_TIMEOUT,
            "bootstrap request body timed out",
            false,
        ),
    }
}

async fn exact_host(State(browser): State<BrowserState>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get("host")
        .and_then(|value| value.to_str().ok());
    if !host.is_some_and(|host| {
        browser
            .allowed_origins
            .iter()
            .any(|origin| origin.strip_prefix("http://") == Some(host))
    }) {
        return (StatusCode::FORBIDDEN, "Forbidden host").into_response();
    }
    next.run(request).await
}

async fn api(state: Gateway, operation: Operation, request: Request) -> Response {
    let description = operation.description();
    if let Some(refused) = operation::refuse_content_type(description, request.headers()) {
        return refused;
    }
    let mut ticket = match state.backend.ticket() {
        Ok(ticket) => ticket,
        Err(error) => return rejected(StatusCode::SERVICE_UNAVAILABLE, error, false),
    };
    // This slot bounds body parsers, connection waiters and submitted RPCs.
    // A cancelled RPC retains it until its reply or carrier cleanup is observed.
    let capacity = match state.backend.api.admit() {
        Ok(capacity) => capacity,
        Err(error) => return rejected(StatusCode::SERVICE_UNAVAILABLE, error, false),
    };
    let budget = ConnectionBudget::default();
    let (query, _parameter_bytes) =
        match operation::query_arguments(description, request.uri(), &budget) {
            Ok(query) => query.into_parts(),
            Err(response) => return *response,
        };
    // Decode path parameters only after reserving their storage too. The same
    // lease follows both maps and the resulting API arguments through execution.
    let (mut parts, body) = request.into_parts();
    let path = match operation::path_arguments(&mut parts).await {
        Ok(path) => path,
        Err(response) => return *response,
    };
    let body_limit = operation::body_limit(description);
    let _body = match budget.reserve(2 * body_limit, false) {
        Ok(lease) => lease,
        Err(error) => return rejected(StatusCode::SERVICE_UNAVAILABLE, error, false),
    };
    let read = tokio::select! { biased;
        _ = ticket.revision.changed() => return rejected(StatusCode::SERVICE_UNAVAILABLE, "backend changed before request admission", false),
        result = operation::read_body(body, body_limit) => result,
    };
    let bytes = match read {
        Ok(bytes) => bytes,
        Err(response) => return *response,
    };
    let _decoded = match splitty_transport::codec::reserve_json(&bytes, &budget, false) {
        Ok(lease) => lease,
        Err(error) => return rejected(StatusCode::SERVICE_UNAVAILABLE, error, false),
    };
    let request =
        match operation::native_request(operation, path, query, &bytes, state.backend.role()) {
            Ok(request) => request,
            Err(response) => return *response,
        };
    let mut revision = ticket.revision.clone();
    let connection = match state.backend.api.connection(&state.backend, ticket).await {
        Ok(connection) => connection,
        Err(error) => return rejected(StatusCode::SERVICE_UNAVAILABLE, error, false),
    };
    let mut call = match connection.begin(&request, capacity) {
        Ok(call) => call,
        Err(error) => return rejected(StatusCode::SERVICE_UNAVAILABLE, error, false),
    };
    let deadline = description
        .deadline
        .duration()
        .map(|duration| tokio::time::Instant::now() + duration + std::time::Duration::from_secs(1));
    let result = tokio::select! { biased;
        _ = revision.changed() => Err(io::Error::new(io::ErrorKind::ConnectionAborted, "backend changed; the request was not replayed")),
        _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => Err(io::Error::new(io::ErrorKind::TimedOut, "backend request timed out")),
        reply = call.recv() => reply,
    };
    match result {
        Ok((reply, lease)) => native_reply(reply, lease, &budget),
        Err(error) => rejected(
            StatusCode::SERVICE_UNAVAILABLE,
            error,
            description.cancellation == Cancellation::Mutation,
        ),
    }
}

fn native_reply(reply: wire::Reply, lease: Lease, budget: &ConnectionBudget) -> Response {
    if reply.uncertain {
        // Native uncertainty is outside the serialized operation body.
        // Preserve it explicitly for the browser before releasing leases.
        let _decoded =
            match splitty_transport::codec::reserve_json(reply.body.as_bytes(), budget, false) {
                Ok(lease) => lease,
                Err(error) => return rejected(StatusCode::SERVICE_UNAVAILABLE, error, true),
            };
        let mut body = serde_json::from_str::<Value>(&reply.body)
            .unwrap_or_else(|_| json!({"error":reply.body}));
        if let Some(object) = body.as_object_mut() {
            object.insert("uncertain".into(), Value::Bool(true));
        } else {
            body = json!({"error":"backend outcome is uncertain","uncertain":true});
        }
        let size = match splitty_transport::codec::json_size(&body, wire::MAX_FRAME_BYTES) {
            Ok(size) => size,
            Err(error) => return rejected(StatusCode::BAD_GATEWAY, error, true),
        };
        let output = match budget.reserve(size, false) {
            Ok(lease) => lease,
            Err(error) => return rejected(StatusCode::SERVICE_UNAVAILABLE, error, true),
        };
        let mut bytes = Vec::with_capacity(size);
        if let Err(error) = serde_json::to_writer(&mut bytes, &body) {
            return rejected(StatusCode::BAD_GATEWAY, error, true);
        }
        return (
            StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY),
            [("content-type", "application/json")],
            axum::body::Bytes::from_owner(ResponseBody {
                bytes,
                _lease: output,
            }),
        )
            .into_response();
    }
    let content_type = match reply.content_type {
        wire::ContentType::Json => "application/json",
        wire::ContentType::Text => "text/plain; charset=utf-8",
        wire::ContentType::Empty => "application/octet-stream",
    };
    (
        StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY),
        [("content-type", content_type)],
        axum::body::Bytes::from_owner(ResponseBody {
            bytes: reply.body.into_bytes(),
            _lease: lease,
        }),
    )
        .into_response()
}

async fn upgrade(State(state): State<Gateway>, request: Request) -> Response {
    upgrade_with(state, request, false).await
}
async fn remote_upgrade(State(state): State<Gateway>, request: Request) -> Response {
    upgrade_with(state, request, true).await
}
async fn upgrade_with(state: Gateway, mut request: Request, host: bool) -> Response {
    let budget = ConnectionBudget::default();
    let allowed: &[&str] = if host {
        &["ticket", "target", "update"]
    } else {
        &["ticket"]
    };
    let params = match parse_query(request.uri(), allowed, &budget) {
        Ok(params) => params,
        Err(response) => return *response,
    };
    if let Some(response) = routes::refuse_upgrade(&state.browser, &params, &request) {
        return response;
    }
    if request.version() != axum::http::Version::HTTP_11
        || request
            .extensions()
            .get::<hyper::upgrade::OnUpgrade>()
            .is_none()
    {
        return rejected(
            StatusCode::BAD_REQUEST,
            "HTTP/1.1 WebSocket upgrade required",
            false,
        );
    }
    let response =
        match tokio_tungstenite::tungstenite::handshake::server::create_response_with_body(
            &request,
            axum::body::Body::empty,
        ) {
            Ok(response) => response,
            Err(error) => return rejected(StatusCode::BAD_REQUEST, error, false),
        };
    // Admission and parser storage follow the IO through the upgrade.
    let Some(admitted) = request
        .extensions()
        .get::<super::server::Admitted>()
        .cloned()
    else {
        return rejected(
            StatusCode::SERVICE_UNAVAILABLE,
            "browser listener admission required",
            false,
        );
    };
    let ticket = match state.backend.ticket() {
        Ok(ticket) => ticket,
        Err(error) => return rejected(StatusCode::SERVICE_UNAVAILABLE, error, false),
    };
    if host {
        let pending = match state.backend.admit_host() {
            Ok(pending) => pending,
            Err(error) => {
                return rejected(
                    if error.kind() == io::ErrorKind::PermissionDenied {
                        StatusCode::FORBIDDEN
                    } else {
                        StatusCode::SERVICE_UNAVAILABLE
                    },
                    error,
                    false,
                )
            }
        };
        let target = splitty_remote::ssh::SshTarget::parse(
            params.get("target").map(String::as_str).unwrap_or_default(),
        );
        let update = params.get("update").is_some_and(|value| value == "1");
        let upgrade = hyper::upgrade::on(&mut request);
        tokio::spawn(async move {
            let _admitted = admitted;
            if let Ok(socket) = upgrade.await {
                host_session(
                    hyper_util::rt::TokioIo::new(socket),
                    state.backend,
                    ticket,
                    target,
                    update,
                    pending,
                )
                .await;
            }
        });
        return response;
    }
    let (connection, revision) = match state.backend.open(ticket).await {
        Ok(connection) => connection,
        Err(error) => return rejected(StatusCode::SERVICE_UNAVAILABLE, error, false),
    };
    let upgrade = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        let _admitted = admitted;
        match upgrade.await {
            Ok(socket) => session(hyper_util::rt::TokioIo::new(socket), connection, revision).await,
            Err(_) => connection.finish().await,
        }
    });
    response
}

async fn deliver<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut browser_wire::Writer<W>,
    opcode: OpCode,
    bytes: &[u8],
    revision: &mut watch::Receiver<Option<Descriptor>>,
    context: &splitty_transport::queue::Context,
    liveness_deadline: tokio::time::Instant,
) -> io::Result<()> {
    let mut closing = context.closing();
    let deadline = liveness_deadline.min(tokio::time::Instant::now() + wire::LIVENESS_TIMEOUT);
    tokio::select! { biased;
        _ = revision.changed() => Err(io::Error::new(io::ErrorKind::ConnectionAborted, "backend changed during browser delivery")),
        _ = async { let _ = closing.wait_for(|closed| *closed).await; } => Err(io::Error::new(io::ErrorKind::ConnectionAborted, "native carrier closed during browser delivery")),
        result = tokio::time::timeout_at(deadline, writer.send(opcode, bytes)) =>
            result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "browser delivery timed out"))?,
    }
}

struct Heartbeat {
    pulse: tokio::time::Interval,
    last_pong: tokio::time::Instant,
    awaiting_pong: Option<[u8; 8]>,
}
impl Heartbeat {
    fn new() -> Self {
        let last_pong = tokio::time::Instant::now();
        let mut pulse =
            tokio::time::interval_at(last_pong + wire::PING_INTERVAL, wire::PING_INTERVAL);
        pulse.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Self {
            pulse,
            last_pong,
            awaiting_pong: None,
        }
    }
}

/// The HTTP handshake completes before slow SSH setup. Only counted, bounded
/// input is retained; it is admitted once and never moved to another attempt.
async fn host_session<I>(
    socket: I,
    backend: Arc<Backend>,
    ticket: Ticket,
    target: Result<splitty_remote::ssh::SshTarget, splitty_remote::ssh::TargetError>,
    update: bool,
    pending: Pending,
) where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let context = pending.context.clone();
    let mut closing = context.closing();
    let (read, write) = tokio::io::split(socket);
    let reader = browser_wire::Reader::preparing(read, context.clone());
    let mut reader = reader;
    let mut writer = browser_wire::Writer::new(write);
    let (reports, mut progress) = context.channel(8);
    let Ticket {
        descriptor,
        mut revision,
    } = ticket;
    let Heartbeat {
        mut pulse,
        mut last_pong,
        mut awaiting_pong,
    } = Heartbeat::new();
    let mut progress_open = true;
    let mut browser_closed = false;
    let mut last_step = None;
    let result: io::Result<Option<Connection>> = {
        let prepare = async {
            let target =
                target.map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
            backend
                .open_host(descriptor, target, update, pending, reports)
                .await
        };
        tokio::pin!(prepare);
        let mut completed = false;
        let result = async {
        loop {
            tokio::select! { biased;
                _ = revision.changed() => return Ok(None),
                _ = async { let _ = closing.wait_for(|closed| *closed).await; } => return Ok(None),
                _ = tokio::time::sleep_until(last_pong + wire::LIVENESS_TIMEOUT) => return Ok(None),
                control = reader.next() => {
                    let Some(control) = control else { return Ok(None); };
                    match control.opcode {
                        Control::Pong => if awaiting_pong.as_ref().map(|nonce| nonce.as_slice()) == Some(control.body.as_slice()) {
                            awaiting_pong = None; last_pong = tokio::time::Instant::now();
                        },
                        Control::Ping => deliver(&mut writer, OpCode::Control(Control::Pong), &control.body,
                            &mut revision, &context, last_pong + wire::LIVENESS_TIMEOUT).await?,
                        Control::Close => {
                            deliver(&mut writer, OpCode::Control(Control::Close), &control.body,
                                &mut revision, &context, last_pong + wire::LIVENESS_TIMEOUT).await?;
                            browser_closed = true;
                            return Ok(None);
                        },
                        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "unsupported browser control")),
                    }
                },
                _ = pulse.tick(), if awaiting_pong.is_none() => {
                    use rand::RngCore;
                    let mut nonce = [0; 8]; rand::thread_rng().fill_bytes(&mut nonce);
                    deliver(&mut writer, OpCode::Control(Control::Ping), &nonce,
                        &mut revision, &context, last_pong + wire::LIVENESS_TIMEOUT).await?;
                    awaiting_pong = Some(nonce);
                },
                next = progress.recv(), if progress_open => if let Some(report) = next {
                    host_progress(&report, &mut last_step, &mut writer, &mut revision,
                        &context, last_pong + wire::LIVENESS_TIMEOUT).await?;
                } else { progress_open = false; },
                result = &mut prepare => {
                    completed = true;
                    let connection = result?;
                    while let Ok(report) = progress.try_recv() {
                        if let Err(error) = host_progress(&report, &mut last_step, &mut writer, &mut revision,
                            &context, last_pong + wire::LIVENESS_TIMEOUT).await {
                            connection.finish().await;
                            return Err(error);
                        }
                    }
                    return Ok(Some(connection));
                },
            }
        }
        }.await;
        if !completed {
            context.close();
            if let Ok(connection) = prepare.await {
                connection.finish().await;
            }
        }
        result
    };
    let mut close_code = reader.close_code();
    match result {
        Ok(Some(connection)) => {
            if !context.is_closed()
                && !revision.has_changed().unwrap_or(true)
                && reader.admit(connection.sender()).is_ok()
            {
                session_parts(
                    reader,
                    writer,
                    connection,
                    revision,
                    Heartbeat {
                        pulse,
                        last_pong,
                        awaiting_pong,
                    },
                    true,
                )
                .await;
                return;
            }
            connection.finish().await;
        }
        Err(error) if !context.is_closed() && !writer.tainted => {
            let _ = host_failure(
                &error,
                &mut writer,
                &mut revision,
                &context,
                last_pong + wire::LIVENESS_TIMEOUT,
            )
            .await;
        }
        Ok(None) | Err(_) => {
            if revision.has_changed().unwrap_or(true) {
                close_code = 4001;
            }
        }
    }
    context.close();
    reader.finish().await;
    if !browser_closed && !writer.tainted {
        let _ = tokio::time::timeout(
            wire::HANDSHAKE_TIMEOUT,
            writer.send(OpCode::Control(Control::Close), &close_code.to_be_bytes()),
        )
        .await;
    }
    let _ = tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, writer.shutdown()).await;
}

/// Translate one transport progress report of a host stream into the
/// browser's `remote.progress`. The native preparation repeats its latest
/// report as a pulse and also reports phases that are no step: the browser
/// gets each step once, in order.
async fn host_progress<W: tokio::io::AsyncWrite + Unpin>(
    report: &wire::Progress,
    last_step: &mut Option<Step>,
    writer: &mut browser_wire::Writer<W>,
    revision: &mut watch::Receiver<Option<Descriptor>>,
    context: &splitty_transport::queue::Context,
    deadline: tokio::time::Instant,
) -> io::Result<()> {
    let Some(step) = Step::from_name(&report.phase) else {
        return Ok(());
    };
    if last_step.is_some_and(|last| step <= last) {
        return Ok(());
    }
    *last_step = Some(step);
    #[derive(serde::Serialize)]
    struct Report<'a> {
        r#type: &'static str,
        step: &'a str,
        message: &'a str,
    }
    let report = Report {
        r#type: "remote.progress",
        step: step.name(),
        message: &report.message,
    };
    let size = splitty_transport::codec::json_size(&report, wire::MAX_CONTROL_BYTES)?;
    let body = context
        .budget
        .charge(size, true, || Vec::with_capacity(size))
        .map_err(io::Error::other)?;
    let mut body = body;
    serde_json::to_writer(&mut body.value, &report)?;
    deliver(
        writer,
        OpCode::Data(Data::Text),
        &body,
        revision,
        context,
        deadline,
    )
    .await?;
    Ok(())
}

async fn host_failure<W: tokio::io::AsyncWrite + Unpin>(
    error: &io::Error,
    writer: &mut browser_wire::Writer<W>,
    revision: &mut watch::Receiver<Option<Descriptor>>,
    context: &splitty_transport::queue::Context,
    deadline: tokio::time::Instant,
) -> io::Result<()> {
    use std::fmt::Write;
    // Bound formatting before allocation, including diagnostics from an IO
    // implementation. A valid typed native failure retains its original code.
    struct Diagnostic(String);
    impl std::fmt::Write for Diagnostic {
        fn write_str(&mut self, value: &str) -> std::fmt::Result {
            let mut size = value.len().min(wire::MAX_CONTROL_BYTES - self.0.len());
            while !value.is_char_boundary(size) {
                size -= 1;
            }
            self.0.push_str(&value[..size]);
            if size < value.len() {
                Err(std::fmt::Error)
            } else {
                Ok(())
            }
        }
    }
    let _temporary = context
        .budget
        .reserve(3 * wire::MAX_CONTROL_BYTES, true)
        .map_err(io::Error::other)?;
    let mut diagnostic = Diagnostic(String::with_capacity(wire::MAX_CONTROL_BYTES));
    let _ = write!(&mut diagnostic, "{error}");
    #[derive(serde::Deserialize)]
    struct FailureType<'a> {
        r#type: &'a str,
    }
    let typed = serde_json::from_str::<FailureType<'_>>(&diagnostic.0).ok();
    if typed.is_some_and(|value| value.r#type == "remote.error") {
        return deliver(
            writer,
            OpCode::Data(Data::Text),
            diagnostic.0.as_bytes(),
            revision,
            context,
            deadline,
        )
        .await;
    }
    let mut end = diagnostic.0.len().min(512);
    while !diagnostic.0.is_char_boundary(end) {
        end -= 1;
    }
    let error = json!({"type":"remote.error", "code":if error.kind() == io::ErrorKind::InvalidInput { "invalid_target" } else { "bridge" }, "message":&diagnostic.0[..end]});
    let size = splitty_transport::codec::json_size(&error, wire::MAX_CONTROL_BYTES)?;
    let mut body = context
        .budget
        .charge(size, true, || Vec::with_capacity(size))
        .map_err(io::Error::other)?;
    serde_json::to_writer(&mut body.value, &error)?;
    deliver(
        writer,
        OpCode::Data(Data::Text),
        &body,
        revision,
        context,
        deadline,
    )
    .await
}

async fn session<I>(
    socket: I,
    connection: Connection,
    revision: watch::Receiver<Option<Descriptor>>,
) where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (read, write) = tokio::io::split(socket);
    let reader = browser_wire::Reader::start(read, connection.sender());
    session_parts(
        reader,
        browser_wire::Writer::new(write),
        connection,
        revision,
        Heartbeat::new(),
        false,
    )
    .await;
}

async fn session_parts<W: tokio::io::AsyncWrite + Unpin>(
    mut reader: browser_wire::Reader,
    mut writer: browser_wire::Writer<W>,
    mut connection: Connection,
    mut revision: watch::Receiver<Option<Descriptor>>,
    heartbeat: Heartbeat,
    mut host_prelude: bool,
) {
    let sender = connection.sender();
    let context = sender.context();
    let mut closing = context.closing();
    // Carry the original timer across preparation. Restarting it here would
    // shorten the browser's response window under the existing deadline.
    let Heartbeat {
        pulse: mut ping,
        mut last_pong,
        mut awaiting_pong,
    } = heartbeat;
    let mut closed = false;
    let mut close_code = 4001u16;
    let result: io::Result<()> = async {
        loop {
            tokio::select! { biased;
                _ = revision.changed() => break,
                _ = async { let _ = closing.wait_for(|closed| *closed).await; } => { if reader.close_code() != 1000 { close_code = reader.close_code(); } break; },
                _ = tokio::time::sleep_until(last_pong + wire::LIVENESS_TIMEOUT) => break,
                _ = ping.tick(), if awaiting_pong.is_none() => {
                    let nonce = rand::random::<u64>().to_be_bytes();
                    deliver(&mut writer, OpCode::Control(Control::Ping), &nonce, &mut revision, &context, last_pong + wire::LIVENESS_TIMEOUT).await?;
                    awaiting_pong = Some(nonce);
                }
                control = reader.next() => {
                    let Some(control) = control else { if reader.close_code() != 1000 { close_code = reader.close_code(); } break; };
                    match control.opcode {
                        Control::Pong => {
                            if awaiting_pong.as_ref().map(|nonce| nonce.as_slice()) == Some(control.body.as_slice()) {
                                awaiting_pong = None; last_pong = tokio::time::Instant::now();
                            }
                        }
                        Control::Ping => deliver(&mut writer, OpCode::Control(Control::Pong), &control.body, &mut revision, &context, last_pong + wire::LIVENESS_TIMEOUT).await?,
                        Control::Close => { deliver(&mut writer, OpCode::Control(Control::Close), &control.body, &mut revision, &context, last_pong + wire::LIVENESS_TIMEOUT).await?; closed = true; break; },
                        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "unsupported browser control")),
                    }
                }
                outgoing = connection.recv() => {
                    let Some(message) = outgoing else { break; };
                    let bytes = message.frame.application()?.1;
                    if host_prelude {
                        #[derive(serde::Deserialize)]
                        struct Prelude<'a> { r#type: &'a str }
                        let prelude: Prelude<'_> = serde_json::from_slice(bytes)?;
                        match prelude.r#type {
                            // A hub of the previous release repeats its
                            // progress as text; the browser already got each
                            // step from the transport reports.
                            "remote.progress" => continue,
                            "remote.ready" => host_prelude = false,
                            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "host event before remote.ready")),
                        }
                    }
                    let opcode = match message.frame.kind {
                        Kind::Text => { std::str::from_utf8(bytes).map_err(io::Error::other)?; OpCode::Data(Data::Text) },
                        Kind::Binary => OpCode::Data(Data::Binary),
                        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "unexpected backend session message")),
                    };
                    deliver(&mut writer, opcode, bytes, &mut revision, &context, last_pong + wire::LIVENESS_TIMEOUT).await?;
                    drop(message); // Credit follows the completed browser write.
                }
            }
        }
        Ok(())
    }.await;
    if let Err(error) = result {
        tracing::debug!(%error, "browser native session ended");
    }
    sender.close();
    reader.finish().await;
    if !closed && !writer.tainted {
        let _ = tokio::time::timeout(
            wire::HANDSHAKE_TIMEOUT,
            writer.send(OpCode::Control(Control::Close), &close_code.to_be_bytes()),
        )
        .await;
    }
    let _ = tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, writer.shutdown()).await;
    connection.finish().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_transport::budget::ProcessBudget;

    #[test]
    fn query_storage_is_admitted_before_decode_and_retained_until_consumption() {
        let process = ProcessBudget::new(1024 * 1024, 0);
        let budget = ConnectionBudget::new(process.clone(), 1024 * 1024, 0);
        let uri = format!("/api/home/theme?name={}", "%FF".repeat(10_000))
            .parse()
            .unwrap();
        let parsed = parse_query(&uri, &["name"], &budget).unwrap();
        assert_eq!(parsed["name"], "\u{fffd}".repeat(10_000));
        assert!(budget.used() >= parsed["name"].capacity());
        assert_eq!(budget.used(), process.used());
        drop(parsed);
        assert_eq!(process.used(), 0);
        let held = budget.reserve(1024 * 1024, false).unwrap();
        assert_eq!(
            parse_query(&uri, &["name"], &budget).unwrap_err().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        drop(held);
        assert_eq!(process.used(), 0);
        let flood = format!("/api/home/theme?{}", "name=x&".repeat(7888))
            .parse()
            .unwrap();
        let untouched = ConnectionBudget::new(process.clone(), 1024 * 1024, 0);
        assert_eq!(
            parse_query(&flood, &["name"], &untouched)
                .unwrap_err()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            untouched.data.high_water(),
            0,
            "field flood rejected before acquiring decode storage"
        );
        for uri in [
            "/api/mux/neighbor?dir=left&d%69r=right",
            "/api/home/theme?authority=main",
        ] {
            assert_eq!(
                parse_query(&uri.parse().unwrap(), &["dir?", "pane?"], &budget)
                    .unwrap_err()
                    .status(),
                StatusCode::BAD_REQUEST
            );
            assert_eq!(budget.used(), 0);
        }
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn preparation_handoff_preserves_the_browser_ping_phase_and_pong_window() {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::{protocol::Role as WsRole, Message};
        let dir = tempfile::tempdir().unwrap();
        let state = crate::http::tests::state("127.0.0.1:1", dir.path());
        let connection = Connector::default()
            .embedded(state.service.clone(), Role::Main)
            .await
            .unwrap();
        let (browser, io) = tokio::io::duplex(8192);
        let mut browser =
            tokio_tungstenite::WebSocketStream::from_raw_socket(browser, WsRole::Client, None)
                .await;
        let (read, write) = tokio::io::split(io);
        let reader = browser_wire::Reader::start(read, connection.sender());
        let (_keep, revision) = watch::channel(None);
        let heartbeat = Heartbeat::new();
        let started = tokio::time::Instant::now();
        tokio::time::advance(std::time::Duration::from_secs(14)).await;
        let task = tokio::spawn(session_parts(
            reader,
            browser_wire::Writer::new(write),
            Connection::Local(connection),
            revision,
            heartbeat,
            false,
        ));
        assert!(matches!(
            browser.next().await.unwrap().unwrap(),
            Message::Text(_)
        ));
        let nonce = loop {
            match browser.next().await.unwrap().unwrap() {
                Message::Ping(nonce) => break nonce,
                Message::Text(_) => {}
                other => panic!("expected browser ping, got {other:?}"),
            }
        };
        assert_eq!(
            started.elapsed(),
            wire::PING_INTERVAL,
            "admission must not postpone the old ping tick"
        );
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        browser.send(Message::Pong(nonce)).await.unwrap();
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(std::time::Duration::from_secs(14)).await;
        assert!(
            !task.is_finished(),
            "a healthy delayed Pong must extend the original deadline"
        );
        browser.close(None).await.unwrap();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn typed_host_failure_keeps_the_complete_small_screen_list_and_error_bytes() {
        use futures::StreamExt;
        use tokio_tungstenite::tungstenite::{protocol::Role as WsRole, Message};
        let error = json!({"type":"remote.error", "code":"incompatible", "message":"incompatible daemon",
            "keepsSessions":false, "screens":(0..65).map(|id| json!({"id":id.to_string(),"panes":1})).collect::<Vec<_>>()}).to_string();
        let (io, browser) = tokio::io::duplex(8192);
        let mut browser =
            tokio_tungstenite::WebSocketStream::from_raw_socket(browser, WsRole::Client, None)
                .await;
        let mut writer = browser_wire::Writer::new(io);
        let context = splitty_transport::queue::Context::default();
        let (_keep, mut revision) = watch::channel(None);
        host_failure(
            &io::Error::new(io::ErrorKind::PermissionDenied, error.clone()),
            &mut writer,
            &mut revision,
            &context,
            tokio::time::Instant::now() + wire::LIVENESS_TIMEOUT,
        )
        .await
        .unwrap();
        let message = browser.next().await.unwrap().unwrap();
        let Message::Text(actual) = message else {
            panic!("expected typed host failure");
        };
        assert_eq!(actual.as_str(), error);
        assert_eq!(context.budget.used(), 0);
    }
    #[tokio::test(start_paused = true)]
    async fn blocked_browser_delivery_keeps_the_original_liveness_deadline() {
        let (io, _stalled) = tokio::io::duplex(1);
        let mut writer = browser_wire::Writer::new(io);
        let (_keep, mut revision) = watch::channel(None);
        let context = splitty_transport::queue::Context::default();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        let started = tokio::time::Instant::now();
        let error = deliver(
            &mut writer,
            OpCode::Data(Data::Text),
            b"blocked",
            &mut revision,
            &context,
            deadline,
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), std::time::Duration::from_secs(1));
        assert!(writer.tainted);
    }

    #[tokio::test]
    async fn reader_or_native_failure_interrupts_a_blocked_browser_write() {
        let (io, _stalled) = tokio::io::duplex(1);
        let mut writer = browser_wire::Writer::new(io);
        let (_keep, mut revision) = watch::channel(None);
        let context = splitty_transport::queue::Context::default();
        let closed = context.clone();
        let cancel = async move {
            tokio::task::yield_now().await;
            closed.close();
        };
        let (result, ()) = tokio::join!(
            deliver(
                &mut writer,
                OpCode::Data(Data::Text),
                b"blocked",
                &mut revision,
                &context,
                tokio::time::Instant::now() + wire::LIVENESS_TIMEOUT
            ),
            cancel
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert!(writer.tainted);
    }

    #[tokio::test]
    async fn native_uncertainty_preserves_the_error_and_holds_its_body_charge_until_delivery() {
        let process = ProcessBudget::new(1024 * 1024, 8192);
        let budget = ConnectionBudget::new(process.clone(), 1024 * 1024, 8192);
        let lease = budget.reserve(256, false).unwrap();
        let reply = wire::Reply {
            id: 1,
            status: 408,
            content_type: wire::ContentType::Json,
            body: json!({"error":"request timed out","code":"deadline"}).to_string(),
            uncertain: true,
        };
        let response = native_reply(reply, lease, &budget);
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        assert!(
            process.used() > 0,
            "handler return must retain the serialized body lease"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body,
            json!({"error":"request timed out","code":"deadline","uncertain":true})
        );
        assert!(
            process.used() > 0,
            "the last HTTP bytes still own the charge"
        );
        drop(bytes);
        assert_eq!(process.used(), 0);
    }
}
