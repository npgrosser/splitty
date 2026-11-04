//! Tauri command and bounded output adapter. The broker, rather than request fields or
//! packaged URL alone, fixes document authority and backend routing.
use super::{pipe, provider::Provider, session, Attempt, BackendId, Broker, Handle, Proof};
use crate::navigation::{self, DocumentRole};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt::Write,
    sync::{Arc, Mutex},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Auth {
    document: u64,
    secret: [u8; 32],
}
impl Auth {
    pub(crate) fn proof(&self) -> Proof {
        Proof::from_request(self.document, self.secret)
    }
}

#[derive(Default)]
pub(crate) struct Native {
    pub(crate) broker: Arc<Broker>,
    providers: Mutex<HashMap<BackendId, Arc<Provider>>>,
    streams: Mutex<HashMap<Handle, pipe::Input>>,
    pending: Mutex<HashMap<(String, u64, u32), Handle>>,
    deliveries: Mutex<HashMap<Handle, Delivery>>,
    events: Mutex<HashMap<BackendId, tokio::sync::watch::Sender<BackendEvent>>>,
    readiness: Mutex<HashMap<BackendId, u64>>,
    started: Mutex<std::collections::HashSet<BackendId>>,
}
#[derive(Clone, Debug)]
pub(crate) enum BackendEvent {
    Pending,
    Progress(Arc<splitty_transport::budget::Charged<splitty_transport::wire::Progress>>),
    Ready,
    Failed(Arc<splitty_transport::budget::Charged<splitty_remote::tunnel::Failure>>),
    Closed,
}
struct Delivery {
    window: String,
    proof: Proof,
    token: [u8; 16],
}
/// The completion closure owns the source chunk as well as encoding storage.
/// A native receipt alone cannot advance the writer while an evaluation still
/// owns its script. Unlike ChannelDataIpcQueue, there is no unfetched cache
/// entry whose lifetime silently extends across document navigation.
struct Evaluation {
    _chunk: Option<pipe::Chunk>,
    input: Option<pipe::Input>,
    _failure: Option<Arc<splitty_transport::budget::Charged<splitty_remote::tunnel::Failure>>>,
    _progress: Option<Arc<splitty_transport::budget::Charged<splitty_transport::wire::Progress>>>,
    completion: Option<tokio::sync::oneshot::Sender<bool>>,
    _attempt: Arc<Attempt>,
    _storage: splitty_transport::budget::Lease,
}
impl Evaluation {
    fn complete(mut self, result: &str) {
        if result != "true" {
            if let Some(input) = &self.input {
                input.close();
            }
        }
        if let Some(completion) = self.completion.take() {
            let _ = completion.send(result == "true");
        }
    }
    fn dispatch(self, webview: &tauri::Webview, script: String) -> tauri::Result<()> {
        // Tauri's callback is Fn; native completion (or callback destruction)
        // must release the source and its reservations exactly once.
        let pending = Mutex::new(Some(self));
        webview.eval_with_callback(script, move |result| {
            if let Some(delivery) = pending.lock().expect("native evaluation").take() {
                delivery.complete(&result);
            }
        })
    }
}

async fn await_evaluation(attempt: &Attempt, mut completed: tokio::sync::oneshot::Receiver<bool>) {
    let mut closing = attempt.context.closing();
    let timeout = tokio::time::sleep(pipe::DELIVERY_TIMEOUT);
    tokio::pin!(timeout);
    loop {
        if !attempt.can_report_failure() {
            break;
        }
        tokio::select! { biased;
            // Failed preparation already closed IO. Only a subsequent
            // document/request revocation ends this diagnostic's wait early.
            _ = closing.changed() => {}
            _ = &mut timeout => break,
            _ = &mut completed => break,
        }
    }
    // A timeout/revocation ends the command wait, not ownership of its script.
    // The framework callback still owns Evaluation until completion or drop.
}

struct Output {
    receiver: String,
    document: u64,
    callback: u32,
}
impl Output {
    fn progress_script(
        &self,
        attempt: &Arc<Attempt>,
        progress: Arc<splitty_transport::budget::Charged<splitty_transport::wire::Progress>>,
    ) -> Result<(String, Evaluation, tokio::sync::oneshot::Receiver<bool>), &'static str> {
        // Reserve escaped JSON, script and framework copies before encoding.
        // The original Progress lease remains in the evaluation through its
        // actual callback completion, independently of command cancellation.
        let capacity = progress
            .phase
            .len()
            .checked_add(progress.message.len())
            .and_then(|size| size.checked_mul(6))
            .and_then(|size| size.checked_add(self.receiver.len() + 256))
            .ok_or("native progress too large")?;
        let storage = attempt
            .context
            .budget
            .reserve(
                capacity.checked_mul(6).ok_or("native progress too large")?,
                false,
            )
            .map_err(|_| "native progress budget exhausted")?;
        let payload =
            serde_json::to_string(&**progress).map_err(|_| "native progress unavailable")?;
        let mut script = String::with_capacity(capacity);
        write!(
            script,
            "window[{}]?.({},{},{{progress:{payload}}}) === true",
            serde_json::to_string(&format!("{}_output", self.receiver))
                .map_err(|_| "native progress unavailable")?,
            self.document,
            self.callback,
        )
        .map_err(|_| "native progress unavailable")?;
        debug_assert!(script.len() <= capacity);
        let (completion, completed) = tokio::sync::oneshot::channel();
        Ok((
            script,
            Evaluation {
                _chunk: None,
                input: None,
                _failure: None,
                _progress: Some(progress),
                completion: Some(completion),
                _attempt: attempt.clone(),
                _storage: storage,
            },
            completed,
        ))
    }
    fn failure_script(
        &self,
        attempt: &Arc<Attempt>,
        failure: Arc<splitty_transport::budget::Charged<splitty_remote::tunnel::Failure>>,
        retryable: bool,
    ) -> Result<(String, Evaluation, tokio::sync::oneshot::Receiver<bool>), &'static str> {
        if !attempt.can_report_failure() {
            return Err("native diagnostic document closed");
        }
        #[derive(Serialize)]
        struct Diagnostic<'a> {
            message: &'a str,
            hint: &'a Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            migration: Option<&'a splitty_remote::tunnel::MigrationOffer>,
            code: &'static str,
            retryable: bool,
        }
        // JSON can escape each source byte six times. Reserve all construction
        // and native UTF-16/script copies before serializing borrowed metadata.
        let json_capacity = failure
            .message
            .len()
            .checked_add(failure.hint.as_ref().map_or(0, String::len))
            .and_then(|size| {
                size.checked_add(
                    failure
                        .migration
                        .as_ref()
                        .map_or(0, |offer| offer.from_version.len() + offer.to_version.len()),
                )
            })
            .and_then(|size| size.checked_mul(6))
            .and_then(|size| size.checked_add(512))
            .ok_or("native diagnostic unavailable")?;
        let script_capacity = json_capacity
            .checked_add(self.receiver.len() + 128)
            .ok_or("native diagnostic unavailable")?;
        let bytes = script_capacity
            .checked_mul(6)
            .ok_or("native diagnostic unavailable")?;
        let storage = attempt
            .context
            .budget
            .reserve(bytes, false)
            .or_else(|_| attempt.context.budget.reserve(bytes, true))
            .map_err(|_| "native diagnostic unavailable")?;
        let diagnostic = serde_json::to_string(&Diagnostic {
            message: &failure.message,
            hint: &failure.hint,
            migration: failure.migration.as_ref(),
            code: "backendLost",
            retryable,
        })
        .map_err(|_| "native diagnostic unavailable")?;
        let mut script = String::with_capacity(script_capacity);
        write!(
            script,
            "window[{}]?.({},{},{{failure:{diagnostic}}}) === true",
            serde_json::to_string(&format!("{}_output", self.receiver))
                .map_err(|_| "native diagnostic unavailable")?,
            self.document,
            self.callback
        )
        .map_err(|_| "native diagnostic unavailable")?;
        debug_assert!(script.len() <= script_capacity);
        let (completion, completed) = tokio::sync::oneshot::channel();
        Ok((
            script,
            Evaluation {
                _chunk: None,
                input: None,
                _failure: Some(failure),
                _progress: None,
                completion: Some(completion),
                _attempt: attempt.clone(),
                _storage: storage,
            },
            completed,
        ))
    }
    fn script(
        &self,
        attempt: &Arc<Attempt>,
        index: u32,
        chunk: Option<pipe::Chunk>,
        input: pipe::Input,
    ) -> Result<(String, Evaluation), String> {
        let Self {
            receiver,
            document,
            callback,
        } = self;
        let size = chunk.as_ref().map_or(1, |chunk| chunk.bytes().len() + 17);
        let encoded_size = base64::encoded_len(size, true).ok_or("native output too large")?;
        let script_capacity = encoded_size + receiver.len() + 256;
        // Reserve before packet/Base64/script construction. Keep allowance for
        // the runtime's script copy and UTF-16 conversion through evaluation.
        let storage = attempt
            .context
            .budget
            .reserve(size + encoded_size + 4 * script_capacity, chunk.is_none())
            .map_err(|_| "native carrier byte budget exhausted")?;
        let mut packet = Vec::with_capacity(size);
        if let Some(chunk) = &chunk {
            packet.push(1);
            packet.extend_from_slice(&chunk.token());
            packet.extend_from_slice(chunk.bytes());
        } else {
            packet.push(0);
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(&packet);
        let mut script = String::with_capacity(script_capacity);
        write!(script,
            "window[{}]?.({document},{callback},{{index:{index},message:Uint8Array.from(atob('{encoded}'),c=>c.charCodeAt(0)).buffer}}) === true",
            serde_json::to_string(&format!("{receiver}_output")).map_err(|_| "native output unavailable")?
        ).map_err(|_| "native output unavailable")?;
        debug_assert!(script.len() <= script_capacity);
        Ok((
            script,
            Evaluation {
                _chunk: chunk,
                input: Some(input),
                _failure: None,
                _progress: None,
                completion: None,
                _attempt: attempt.clone(),
                _storage: storage,
            },
        ))
    }
}
impl Native {
    pub(crate) fn provider(&self, id: BackendId, provider: Arc<Provider>) {
        self.providers
            .lock()
            .expect("native providers")
            .insert(id, provider);
        self.events
            .lock()
            .expect("native backend events")
            .entry(id)
            .or_insert_with(|| tokio::sync::watch::channel(BackendEvent::Pending).0);
    }
    pub(crate) fn remove_provider(&self, id: BackendId) {
        self.providers.lock().expect("native providers").remove(&id);
        self.events
            .lock()
            .expect("native backend events")
            .remove(&id);
        self.readiness.lock().expect("native readiness").remove(&id);
        self.started
            .lock()
            .expect("native start markers")
            .remove(&id);
    }
    pub(crate) fn watch_backend(
        &self,
        id: BackendId,
    ) -> Option<tokio::sync::watch::Receiver<BackendEvent>> {
        self.events
            .lock()
            .expect("native backend events")
            .get(&id)
            .map(|event| event.subscribe())
    }
    pub(crate) fn preparing(&self, id: BackendId) {
        let events = self.events.lock().expect("native backend events");
        let Some(event) = events.get(&id) else {
            return;
        };
        self.started
            .lock()
            .expect("native start markers")
            .insert(id);
        event.send_replace(BackendEvent::Pending);
    }
    pub(crate) fn backend_started(&self, id: BackendId) -> bool {
        self.started
            .lock()
            .expect("native start markers")
            .contains(&id)
    }
    pub(crate) fn ready_epoch(&self, id: BackendId) -> u64 {
        self.readiness
            .lock()
            .expect("native readiness")
            .get(&id)
            .copied()
            .unwrap_or(0)
    }
    fn event(&self, id: BackendId, value: BackendEvent) {
        let events = self.events.lock().expect("native backend events");
        let Some(event) = events.get(&id) else {
            return;
        };
        if matches!(value, BackendEvent::Ready) {
            let mut readiness = self.readiness.lock().expect("native readiness");
            let epoch = readiness.entry(id).or_default();
            *epoch = epoch.saturating_add(1);
        }
        event.send_replace(value);
    }
    pub(crate) fn authenticate(
        &self,
        webview: &tauri::Webview,
        auth: &Auth,
    ) -> Result<Proof, String> {
        Self::packaged_document(webview)?;
        // Actual window identity always comes from the command's Webview.
        let proof = auth.proof();
        self.broker
            .authorize(webview.label(), &proof)
            .map_err(error)?;
        Ok(proof)
    }
    fn packaged_document(webview: &tauri::Webview) -> Result<(), String> {
        let url = webview.url().map_err(|_| "native document unavailable")?;
        if ![
            DocumentRole::Main,
            DocumentRole::Remote,
            DocumentRole::Setup,
        ]
        .into_iter()
        .any(|role| navigation::packaged_entry(&url, role))
        {
            return Err("native document denied".into());
        }
        Ok(())
    }
    pub(crate) fn main_action(&self, webview: &tauri::Webview, auth: &Auth) -> Result<(), String> {
        let proof = self.authenticate(webview, auth)?;
        self.broker
            .main_action(webview.label(), &proof)
            .map_err(error)
    }
    pub(crate) fn launcher_action(
        &self,
        webview: &tauri::Webview,
        auth: &Auth,
    ) -> Result<(), String> {
        let proof = self.authenticate(webview, auth)?;
        self.broker
            .main_action(webview.label(), &proof)
            .or_else(|_| self.broker.setup_action(webview.label(), &proof))
            .map_err(error)
    }
    fn input(
        &self,
        webview: &tauri::Webview,
        auth: &Auth,
        handle: &str,
    ) -> Result<pipe::Input, String> {
        let proof = self.authenticate(webview, auth)?;
        let handle = handle_from_string(handle)?;
        self.broker
            .connection(webview.label(), &proof, handle)
            .map_err(error)?;
        self.streams
            .lock()
            .expect("native streams")
            .get(&handle)
            .cloned()
            .ok_or("native connection closed".into())
    }
    /// Revoke document authority and metadata. Pending evaluations own their
    /// reservations independently until the framework completes or drops them.
    pub(crate) fn destroyed(&self, label: &str) {
        // Publication and destruction share this lock. Always take delivery
        // ownership before the broker, matching receipt validation below.
        let mut deliveries = self.deliveries.lock().expect("native delivery ownership");
        let backend = self
            .broker
            .destroyed(label)
            .and_then(|policy| match policy {
                super::Policy::Main { backend } | super::Policy::Remote { backend, .. } => {
                    Some(backend)
                }
                super::Policy::Setup => None,
            });
        if let Some(backend) = backend {
            self.remove_provider(backend);
        }
        deliveries.retain(|_, delivery| delivery.window != label);
    }
    fn publish(
        &self,
        attempt: &Attempt,
        delivery: Delivery,
        send: impl FnOnce() -> Result<(), tauri::Error>,
    ) -> Result<bool, tauri::Error> {
        let mut deliveries = self.deliveries.lock().expect("native delivery ownership");
        // Serialize publication with document destruction. Evaluation owns
        // the actual buffers; this ledger only validates consumption tokens.
        if !attempt.current() {
            return Ok(false);
        }
        deliveries.insert(attempt.handle(), delivery);
        send()?;
        Ok(true)
    }
    fn consumed(
        &self,
        label: &str,
        proof: &Proof,
        handle: Handle,
        token: [u8; 16],
    ) -> Result<(), String> {
        // Keep the existing deliveries -> broker lock order used by publish
        // and destruction. This proof authorizes only release of this exact
        // already-delivered buffer, never current document IO.
        let mut deliveries = self.deliveries.lock().expect("native delivery ownership");
        let delivery = deliveries
            .get(&handle)
            .ok_or("no native delivery in flight")?;
        if delivery.window != label || !delivery.proof.matches(proof) || delivery.token != token {
            return Err("native delivery denied".into());
        }
        if let Some(input) = self
            .streams
            .lock()
            .expect("native streams")
            .get(&handle)
            .cloned()
        {
            // A revoked/closed input rejects this; acknowledging fetched
            // framework storage does not revive that stream.
            let _ = input.consumed(token);
        }
        deliveries.remove(&handle);
        Ok(())
    }
    pub(crate) fn shutdown(&self) {
        self.broker.shutdown();
        self.streams.lock().expect("native streams").clear();
        self.pending.lock().expect("native preparations").clear();
    }
}
struct PendingGuard {
    native: Arc<Native>,
    key: (String, u64, u32),
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.native
            .pending
            .lock()
            .expect("native preparations")
            .remove(&self.key);
    }
}
fn error(error: super::Error) -> String {
    format!("native request rejected: {error:?}")
}
fn handle_string(handle: Handle) -> String {
    handle
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn handle_from_string(value: &str) -> Result<Handle, String> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid native handle".into());
    }
    let mut bytes = [0; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "invalid native handle")?;
    }
    Ok(Handle::from_request(bytes))
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Assignment {
    handle: String,
    document: String,
    generation: u32,
    instance: String,
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
}

#[tauri::command]
pub(crate) async fn native_open(
    webview: tauri::Webview,
    state: tauri::State<'_, Arc<Native>>,
    auth: Auth,
    request: u32,
    target: Option<String>,
    update: Option<bool>,
    output: u32,
) -> Result<Assignment, String> {
    let native = state.inner().clone();
    let proof = native.authenticate(&webview, &auth)?;
    // Admission is synchronous and precedes connector tasks and allocations.
    let attempt = Arc::new(
        native
            .broker
            .open(
                webview.label(),
                &proof,
                request,
                target.as_deref(),
                update.unwrap_or(false),
            )
            .map_err(error)?,
    );
    let output = Output {
        receiver: native
            .broker
            .bootstrap_receiver(webview.label())
            .map_err(error)?,
        document: proof.document,
        callback: output,
    };
    let key = (webview.label().to_owned(), proof.document, request);
    let provider = native
        .providers
        .lock()
        .expect("native providers")
        .get(&attempt.plan.backend)
        .cloned()
        .ok_or("native backend unavailable")?;
    native
        .pending
        .lock()
        .expect("native preparations")
        .insert(key.clone(), attempt.handle());
    let _pending = PendingGuard {
        native: native.clone(),
        key,
    };
    native
        .broker
        .with_authority(webview.label(), &proof, || {
            native.preparing(attempt.plan.backend)
        })
        .map_err(error)?;
    let progress_native = native.clone();
    let backend_id = attempt.plan.backend;
    // Keep only the latest report while one evaluation is pending. A frozen
    // WebView cannot accumulate progress tasks or framework evaluations.
    let (reports, mut updates) = tokio::sync::watch::channel(None);
    let connecting = provider.connect(attempt.clone(), move |progress| {
        let progress = Arc::new(progress);
        progress_native.event(backend_id, BackendEvent::Progress(progress.clone()));
        reports.send_replace(Some(progress));
    });
    let reporting = async {
        loop {
            if updates.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
            let progress = updates
                .borrow_and_update()
                .clone()
                .expect("published progress");
            if !attempt.current() {
                return Err(std::io::Error::other("native document closed"));
            }
            let (script, evaluation, completed) = output
                .progress_script(&attempt, progress)
                .map_err(std::io::Error::other)?;
            evaluation
                .dispatch(&webview, script)
                .map_err(std::io::Error::other)?;
            let mut closing = attempt.context.closing();
            let accepted = tokio::select! { biased;
                _ = closing.wait_for(|closed| *closed) => false,
                result = tokio::time::timeout(pipe::DELIVERY_TIMEOUT, completed) => matches!(result, Ok(Ok(true))),
            };
            if !accepted {
                attempt.context.close();
                return Err(std::io::Error::other("native progress delivery ended"));
            }
        }
    };
    let backend = tokio::select! { biased;
        result = connecting => result,
        result = reporting => result,
    };
    let backend = match backend {
        Ok(backend) => backend,
        Err(error) => {
            let remote = error
                .get_ref()
                .and_then(|source| source.downcast_ref::<splitty_remote::tunnel::RemoteError>());
            let retryable = remote.map_or_else(
                || {
                    matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::UnexpectedEof
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                    )
                },
                |remote| remote.retryable(),
            );
            let failure = remote
                .map_or_else(
                    || {
                        splitty_remote::tunnel::Failure::from_error_budgeted(
                            &error,
                            &attempt.context.budget,
                        )
                    },
                    |remote| remote.failure_budgeted(&attempt.context.budget),
                )
                .or_else(|_| {
                    attempt
                        .context
                        .budget
                        .display(
                            &format_args!("Native backend failed; diagnostic unavailable"),
                            true,
                        )
                        .map(|message| {
                            message.map(|message| splitty_remote::tunnel::Failure {
                                message,
                                hint: None,
                                migration: None,
                            })
                        })
                });
            let failure = match failure {
                Ok(failure) => failure,
                Err(_) => {
                    native.event(backend_id, BackendEvent::Closed);
                    return Err("SplittyNativeFailureUnavailable".into());
                }
            };
            let failure = Arc::new(failure);
            native.event(backend_id, BackendEvent::Failed(failure.clone()));
            if let Ok((script, evaluation, completed)) =
                output.failure_script(&attempt, failure, retryable)
            {
                if evaluation.dispatch(&webview, script).is_ok() {
                    await_evaluation(&attempt, completed).await;
                }
            }
            return Err("SplittyNativeFailureDelivered".into());
        }
    };
    let ready = backend.ready().clone();
    let assignment = Assignment {
        handle: handle_string(attempt.handle()),
        document: proof.document.to_string(),
        generation: ready.generation,
        instance: ready.instance.clone(),
        role: if attempt.plan.target.is_some() {
            "remote"
        } else {
            "main"
        },
        target: attempt
            .plan
            .target
            .as_ref()
            .map(|target| target.as_str().to_owned()),
    };
    let session::Opened { mut channel, ended } = session::open(
        attempt.clone(),
        attempt.plan.backend,
        backend,
        super::provider::MUX_PROTOCOL,
    )
    .map_err(|error| error.to_string())?;
    native.event(backend_id, BackendEvent::Ready);
    native
        .streams
        .lock()
        .expect("native streams")
        .insert(attempt.handle(), channel.input());
    let label = webview.label().to_owned();
    tauri::async_runtime::spawn(async move {
        let mut index = 0;
        while let Ok(Some(chunk)) = channel.next().await {
            if !attempt.current() {
                break;
            }
            let delivery = Delivery {
                window: label.clone(),
                proof: proof.clone(),
                token: chunk.token(),
            };
            let Ok((script, evaluation)) =
                output.script(&attempt, index, Some(chunk), channel.input())
            else {
                break;
            };
            if !matches!(
                native.publish(&attempt, delivery, || evaluation.dispatch(&webview, script)),
                Ok(true)
            ) {
                break;
            }
            let Some(next) = index.checked_add(1) else {
                break;
            };
            index = next;
        }
        let input = channel.input();
        channel.input().close();
        // Dropping the Channel revokes IO. Retain native admission through
        // the relay's actual cleanup rather than treating cancellation as exit.
        drop(channel);
        let _ = ended.await;
        native.event(backend_id, BackendEvent::Closed);
        native
            .streams
            .lock()
            .expect("native streams")
            .remove(&attempt.handle());
        native
            .deliveries
            .lock()
            .expect("native delivery ownership")
            .remove(&attempt.handle());
        if native.broker.authorize(&label, &proof).is_ok() {
            if let Ok((script, evaluation)) = output.script(&attempt, index, None, input) {
                let _ = evaluation.dispatch(&webview, script);
            }
        }
    });
    Ok(assignment)
}

#[tauri::command]
pub(crate) fn native_cancel_open(
    webview: tauri::Webview,
    state: tauri::State<'_, Arc<Native>>,
    auth: Auth,
    request: u32,
) -> Result<(), String> {
    let proof = state.authenticate(&webview, &auth)?;
    if let Some(handle) = state
        .pending
        .lock()
        .expect("native preparations")
        .get(&(webview.label().to_owned(), proof.document, request))
        .copied()
    {
        state
            .broker
            .cancel(webview.label(), &proof, handle)
            .map_err(error)?;
    }
    Ok(())
}
#[tauri::command]
pub(crate) fn native_start(
    webview: tauri::Webview,
    state: tauri::State<'_, Arc<Native>>,
    auth: Auth,
    handle: String,
) -> Result<(), String> {
    state
        .input(&webview, &auth, &handle)?
        .start()
        .map_err(|error| error.to_string())
}
#[tauri::command]
pub(crate) async fn native_write(
    webview: tauri::Webview,
    state: tauri::State<'_, Arc<Native>>,
    auth: Auth,
    handle: String,
    bytes: Vec<u8>,
) -> Result<(), String> {
    // Tauri/Wry has already allocated its request body at this point. This
    // bound covers adapter admission, not that unresolved framework allocation.
    if bytes.is_empty() || bytes.len() > pipe::CHUNK_BYTES {
        return Err("invalid native IPC chunk size".into());
    }
    let accepted = state
        .input(&webview, &auth, &handle)?
        .write(&bytes)
        .map_err(|error| error.to_string())?;
    drop(bytes);
    accepted
        .await
        .map_err(|_| "native connection closed".into())
}
#[tauri::command]
pub(crate) fn native_consumed(
    webview: tauri::Webview,
    state: tauri::State<'_, Arc<Native>>,
    auth: Auth,
    handle: String,
    token: [u8; 16],
) -> Result<(), String> {
    // Receipt cleanup retains its original private proof and actual WebView
    // identity after navigation. All open/data/action commands continue to
    // require the current broker proof through authenticate().
    Native::packaged_document(&webview)?;
    let handle = handle_from_string(&handle)?;
    state.consumed(webview.label(), &auth.proof(), handle, token)
}
#[tauri::command]
pub(crate) fn native_close(
    webview: tauri::Webview,
    state: tauri::State<'_, Arc<Native>>,
    auth: Auth,
    handle: String,
) -> Result<(), String> {
    let proof = state.authenticate(&webview, &auth)?;
    let handle = handle_from_string(&handle)?;
    state
        .broker
        .cancel(webview.label(), &proof, handle)
        .map_err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_ipc::Policy;

    #[test]
    fn handles_reject_untrusted_lengths_and_unicode_without_panicking() {
        for value in [
            "",
            "00",
            "../../etc/passwd",
            "éééééééééééééééé",
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        ] {
            assert!(handle_from_string(value).is_err(), "{value}");
        }
        let handle = Handle::from_request([17; 16]);
        assert_eq!(handle_from_string(&handle_string(handle)).unwrap(), handle);
    }

    #[tokio::test]
    async fn evaluation_owns_source_after_receipt_navigation_and_destroy_until_completion() {
        use splitty_transport::wire;
        use tokio::io::AsyncWriteExt;
        let native = Native::default();
        native
            .broker
            .register(
                "main",
                Policy::Main {
                    backend: BackendId::new(),
                },
            )
            .unwrap();
        let proof = native.broker.committed("main").unwrap();
        let attempt = Arc::new(native.broker.open("main", &proof, 1, None, false).unwrap());
        attempt
            .ready(
                &wire::Ready {
                    generation: 1,
                    instance: "a".repeat(32),
                    mux_protocol: 1,
                    purpose: wire::Purpose::Direct,
                    limits: wire::Limits::default(),
                },
                1,
            )
            .unwrap();
        let budget = attempt.context.budget.clone();
        let (mut channel, mut io) = pipe::pair(attempt.clone()).unwrap();
        let input = channel.input();
        input.start().unwrap();
        native
            .streams
            .lock()
            .unwrap()
            .insert(attempt.handle(), input.clone());
        let writer = tokio::spawn(async move {
            io.write_all(&[0, 255, 128]).await.unwrap();
            io.flush().await
        });
        let chunk = channel.next().await.unwrap().unwrap();
        let token = chunk.token();
        let output = Output {
            receiver: "private_receiver".into(),
            document: proof.document,
            callback: 7,
        };
        let (script, evaluation) = output
            .script(&attempt, 0, Some(chunk), input.clone())
            .unwrap();
        native
            .publish(
                &attempt,
                Delivery {
                    window: "main".into(),
                    proof: proof.clone(),
                    token,
                },
                || Ok(()),
            )
            .unwrap();
        native
            .consumed("main", &proof, attempt.handle(), token)
            .unwrap();
        tokio::task::yield_now().await;
        assert!(
            !writer.is_finished(),
            "a receipt cannot release an unevaluated source chunk"
        );
        assert!(budget.used() > 2 * pipe::CHUNK_BYTES);
        native.broker.navigating("main");
        native.broker.committed("main").unwrap();
        native.destroyed("main");
        native.streams.lock().unwrap().clear();
        drop(channel);
        drop(input);
        drop(attempt);
        assert!(writer.await.unwrap().is_err());
        assert_eq!(native.broker.counts(), (1, 0));
        assert!(budget.used() > 0, "revocation is not evaluation completion");
        drop(script);
        drop(evaluation); // callback execution or framework callback destruction
        assert_eq!(native.broker.counts(), (0, 0));
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn output_encoding_is_charged_and_does_not_interpolate_receiver_as_code() {
        let native = Native::default();
        native
            .broker
            .register(
                "main",
                Policy::Main {
                    backend: BackendId::new(),
                },
            )
            .unwrap();
        let proof = native.broker.committed("main").unwrap();
        let attempt = Arc::new(native.broker.open("main", &proof, 1, None, false).unwrap());
        let (channel, io) = pipe::pair(attempt.clone()).unwrap();
        let budget = attempt.context.budget.clone();
        let baseline = budget.used();
        let output = Output {
            receiver: "receiver\"\\\n".into(),
            document: proof.document,
            callback: 7,
        };
        let (script, evaluation) = output.script(&attempt, 0, None, channel.input()).unwrap();
        assert!(script.starts_with("window[\"receiver\\\"\\\\\\n_output\"]?.("));
        assert!(script.contains("atob('AA==')"));
        assert!(budget.used() >= baseline + 4 * script.len());
        drop(script);
        drop(evaluation);
        assert_eq!(budget.used(), baseline);
        let held = budget
            .reserve(
                splitty_transport::wire::CONNECTION_BYTES
                    - splitty_transport::wire::CONNECTION_CONTROL_BYTES
                    - baseline,
                false,
            )
            .unwrap();
        let (script, evaluation) = output.script(&attempt, 0, None, channel.input()).unwrap();
        drop(script);
        drop(evaluation);
        let control = budget
            .reserve(splitty_transport::wire::CONNECTION_CONTROL_BYTES, true)
            .unwrap();
        assert!(output.script(&attempt, 0, None, channel.input()).is_err());
        drop(control);
        drop(held);
        drop(channel);
        drop(io);
        drop(attempt);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn destroyed_document_cannot_publish_after_the_final_delivery_purge() {
        let native = Native::default();
        native
            .broker
            .register(
                "main",
                Policy::Main {
                    backend: BackendId::new(),
                },
            )
            .unwrap();
        let proof = native.broker.committed("main").unwrap();
        let attempt = Arc::new(native.broker.open("main", &proof, 1, None, false).unwrap());
        let budget = attempt.context.budget.clone();
        let delivery = Delivery {
            window: "main".into(),
            proof: proof.clone(),
            token: [7; 16],
        };
        assert!(attempt.current()); // preflight before the destruction race
        native.destroyed("main");
        assert!(!native
            .publish(&attempt, delivery, || panic!("post-destruction evaluation"))
            .unwrap());
        drop(attempt);
        assert!(native.deliveries.lock().unwrap().is_empty());
        assert_eq!(budget.used(), 0);
        assert_eq!(native.broker.counts(), (0, 0));
    }
    #[test]
    fn revoked_receipt_proof_releases_only_its_original_delivery_and_never_io_authority() {
        let native = Native::default();
        native
            .broker
            .register(
                "main",
                Policy::Main {
                    backend: BackendId::new(),
                },
            )
            .unwrap();
        let old = native.broker.committed("main").unwrap();
        let attempt = Arc::new(native.broker.open("main", &old, 1, None, false).unwrap());
        let old_handle = attempt.handle();
        let old_budget = attempt.context.budget.clone();
        let (channel, io) = pipe::pair(attempt.clone()).unwrap();
        native.deliveries.lock().unwrap().insert(
            old_handle,
            Delivery {
                window: "main".into(),
                proof: old.clone(),
                token: [7; 16],
            },
        );
        native.broker.navigating("main");
        assert!(channel.input().write(&[1]).is_err());
        drop(io);
        drop(channel);
        let current = native.broker.committed("main").unwrap();
        let next = Arc::new(
            native
                .broker
                .open("main", &current, 1, None, false)
                .unwrap(),
        );
        let next_handle = next.handle();
        let next_budget = next.context.budget.clone();
        native.deliveries.lock().unwrap().insert(
            next_handle,
            Delivery {
                window: "main".into(),
                proof: current.clone(),
                token: [9; 16],
            },
        );
        let forged = Proof::from_request(old.document, *current.secret());
        for (label, proof, handle, token) in [
            ("main", &current, old_handle, [7; 16]),
            ("other-window", &old, old_handle, [7; 16]),
            ("main", &forged, old_handle, [7; 16]),
            ("main", &old, old_handle, [8; 16]),
            ("main", &old, next_handle, [9; 16]),
        ] {
            assert!(native.consumed(label, proof, handle, token).is_err());
        }
        assert_eq!(native.broker.counts(), (2, 2));
        assert_eq!(old_budget.used(), 0);
        assert!(native.broker.authorize("main", &old).is_err());
        assert!(native.broker.main_action("main", &old).is_err());
        assert!(native.broker.open("main", &old, 2, None, false).is_err());
        assert!(native.broker.connection("main", &old, old_handle).is_err());
        native.consumed("main", &old, old_handle, [7; 16]).unwrap();
        assert_eq!(old_budget.used(), 0);
        assert_eq!(native.broker.counts(), (2, 2));
        assert_eq!(next_budget.used(), 0);
        assert!(native.broker.authorize("main", &old).is_err());
        assert!(native.broker.main_action("main", &old).is_err());
        assert!(native.broker.open("main", &old, 2, None, false).is_err());
        assert!(native.broker.connection("main", &old, next_handle).is_err());
        assert!(native.consumed("main", &old, old_handle, [7; 16]).is_err());
        native
            .consumed("main", &current, next_handle, [9; 16])
            .unwrap();
        assert_eq!(next_budget.used(), 0);
        drop(attempt);
        drop(next);
        assert_eq!(native.broker.counts(), (0, 0));
    }
    #[tokio::test]
    async fn diagnostic_evaluation_retains_shared_failure_and_attempt_until_completion_or_drop() {
        for completed in [true, false] {
            let native = Native::default();
            let backend = BackendId::new();
            native
                .broker
                .register("main", Policy::Main { backend })
                .unwrap();
            let proof = native.broker.committed("main").unwrap();
            let attempt = Arc::new(native.broker.open("main", &proof, 1, None, false).unwrap());
            let budget = attempt.context.budget.clone();
            let failure = Arc::new(
                splitty_remote::tunnel::Failure::from_error_budgeted(
                    &format_args!("{}", "sensitive diagnostic".repeat(1024)),
                    &budget,
                )
                .unwrap(),
            );
            native.events.lock().unwrap().insert(
                backend,
                tokio::sync::watch::channel(BackendEvent::Pending).0,
            );
            native.event(backend, BackendEvent::Failed(failure.clone()));
            let events = native.watch_backend(backend).unwrap();
            let BackendEvent::Failed(shared) = events.borrow().clone() else {
                panic!("missing failure");
            };
            assert!(Arc::ptr_eq(&failure, &shared));
            let output = Output {
                receiver: "private_receiver".into(),
                document: proof.document,
                callback: 7,
            };
            let (script, evaluation, completion) = output
                .failure_script(&attempt, failure.clone(), false)
                .unwrap();
            assert!(script.contains("backendLost"));
            assert!(!script.contains("atob("));
            drop(script);
            native.event(backend, BackendEvent::Closed);
            drop(events);
            drop(shared);
            drop(failure);
            native.destroyed("main");
            drop(attempt);
            assert!(budget.used() > 0);
            assert_eq!(native.broker.counts(), (1, 1));
            if completed {
                evaluation.complete("true");
                assert_eq!(completion.await, Ok(true));
            } else {
                drop(evaluation);
                assert!(completion.await.is_err());
            }
            assert_eq!(budget.used(), 0);
            assert_eq!(native.broker.counts(), (0, 0));
        }
    }

    #[test]
    fn diagnostic_script_refuses_capacity_before_serialization_and_does_not_clone_failure() {
        let native = Native::default();
        native
            .broker
            .register(
                "main",
                Policy::Main {
                    backend: BackendId::new(),
                },
            )
            .unwrap();
        let proof = native.broker.committed("main").unwrap();
        let attempt = Arc::new(native.broker.open("main", &proof, 1, None, false).unwrap());
        let budget = attempt.context.budget.clone();
        let failure = Arc::new(
            splitty_remote::tunnel::Failure::from_error_budgeted(&"failure", &budget).unwrap(),
        );
        let baseline = budget.used();
        let held = budget
            .reserve(
                splitty_transport::wire::CONNECTION_BYTES
                    - splitty_transport::wire::CONNECTION_CONTROL_BYTES
                    - baseline,
                false,
            )
            .unwrap();
        let control = budget
            .reserve(splitty_transport::wire::CONNECTION_CONTROL_BYTES, true)
            .unwrap();
        let output = Output {
            receiver: "private_receiver".into(),
            document: proof.document,
            callback: 7,
        };
        assert!(output
            .failure_script(&attempt, failure.clone(), true)
            .is_err());
        assert_eq!(Arc::strong_count(&failure), 1);
        drop(held);
        drop(control);
        drop(failure);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn removed_backend_cannot_resurrect_start_or_ready_markers() {
        let native = Native::default();
        let backend = BackendId::new();
        native.provider(
            backend,
            Arc::new(Provider::remote(
                splitty_remote::ssh::SshTarget::parse("host").unwrap(),
            )),
        );
        native
            .broker
            .register(
                "remote",
                Policy::Remote {
                    backend,
                    target: splitty_remote::ssh::SshTarget::parse("host").unwrap(),
                },
            )
            .unwrap();
        native.preparing(backend);
        native.event(backend, BackendEvent::Ready);
        assert!(native.backend_started(backend));
        assert_eq!(native.ready_epoch(backend), 1);
        native.destroyed("remote");
        native.preparing(backend);
        native.event(backend, BackendEvent::Ready);
        assert!(!native.backend_started(backend));
        assert_eq!(native.ready_epoch(backend), 0);
        assert!(native.watch_backend(backend).is_none());
    }

    #[test]
    fn progress_delivery_keeps_source_and_encoded_budget_after_revocation() {
        let native = Native::default();
        native
            .broker
            .register(
                "main",
                Policy::Main {
                    backend: BackendId::new(),
                },
            )
            .unwrap();
        let proof = native.broker.committed("main").unwrap();
        let attempt = Arc::new(native.broker.open("main", &proof, 1, None, false).unwrap());
        let budget = attempt.context.budget.clone();
        let progress = Arc::new(budget.reserve(128, false).unwrap().charge(
            splitty_transport::wire::Progress {
                phase: "installing".into(),
                message: "prepared \"quoted\" payload".into(),
            },
        ));
        let output = Output {
            receiver: "private_receiver".into(),
            document: proof.document,
            callback: 7,
        };
        let (script, evaluation, completed) = output.progress_script(&attempt, progress).unwrap();
        assert!(script.contains("progress:"));
        let retained = budget.used();
        assert!(retained > 128);
        native.destroyed("main");
        drop(attempt);
        drop(completed);
        assert_eq!(
            budget.used(),
            retained,
            "revocation does not release framework-owned evaluation"
        );
        drop(script);
        drop(evaluation);
        assert_eq!(budget.used(), 0);
        assert_eq!(native.broker.counts(), (0, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn failed_preparation_reports_before_invoke_completion_without_reopening_io() {
        use splitty_remote::ssh::SshTarget;
        for outcome in ["complete", "cancel", "navigate", "destroy"] {
            let native = Native::default();
            native
                .broker
                .register(
                    "remote",
                    Policy::Remote {
                        backend: BackendId::new(),
                        target: SshTarget::parse("assigned.invalid").unwrap(),
                    },
                )
                .unwrap();
            let proof = native.broker.committed("remote").unwrap();
            let attempt = Arc::new(
                native
                    .broker
                    .open("remote", &proof, 1, None, false)
                    .unwrap(),
            );
            // This uses the real preparation failure/cleanup path without
            // running SSH: an invalid native assignment fails immediately.
            let provider = Provider::remote(SshTarget::parse("different.invalid").unwrap());
            let error = match provider.connect(attempt.clone(), |_| {}).await {
                Ok(_) => panic!("foreign assignment admitted"),
                Err(error) => error,
            };
            assert!(attempt.context.is_closed());
            assert!(!attempt.current(), "failed IO stays unusable");
            assert!(attempt.can_report_failure());
            let budget = attempt.context.budget.clone();
            let failure = Arc::new(
                splitty_remote::tunnel::Failure::from_error_budgeted(&error, &budget).unwrap(),
            );
            let output = Output {
                receiver: "private_receiver".into(),
                document: proof.document,
                callback: 7,
            };
            let (script, evaluation, completed) = output
                .failure_script(&attempt, failure.clone(), false)
                .unwrap();
            assert!(script.contains("native host assignment mismatch"));
            drop(script);
            let mut evaluation = Some(evaluation);
            let waiting_attempt = attempt.clone();
            let waiting = tokio::spawn(async move {
                await_evaluation(&waiting_attempt, completed).await;
            });
            tokio::task::yield_now().await;
            assert!(
                !waiting.is_finished(),
                "the invoke marker must wait for the diagnostic despite failed IO"
            );
            match outcome {
                "complete" => evaluation.take().unwrap().complete("true"),
                "cancel" => native
                    .broker
                    .cancel("remote", &proof, attempt.handle())
                    .unwrap(),
                "navigate" => {
                    native.broker.navigating("remote");
                    let next = native.broker.committed("remote").unwrap();
                    assert_ne!(next.document, proof.document);
                }
                "destroy" => native.destroyed("remote"),
                _ => unreachable!(),
            }
            waiting.await.unwrap();
            assert!(attempt.context.is_closed());
            if outcome != "complete" {
                assert!(!attempt.can_report_failure());
                assert!(output
                    .failure_script(&attempt, failure.clone(), false)
                    .is_err());
            }
            drop(failure);
            drop(attempt);
            if evaluation.is_some() {
                assert!(budget.used() > 0, "revocation retains evaluation storage");
                assert_eq!(native.broker.counts(), (1, 1));
            }
            drop(evaluation);
            assert_eq!(native.broker.counts(), (0, 0));
            assert_eq!(budget.used(), 0);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_or_timed_out_diagnostic_wait_keeps_actual_evaluation_ownership() {
        for cancel in [true, false] {
            let native = Native::default();
            native
                .broker
                .register(
                    "main",
                    Policy::Main {
                        backend: BackendId::new(),
                    },
                )
                .unwrap();
            let proof = native.broker.committed("main").unwrap();
            let attempt = Arc::new(native.broker.open("main", &proof, 1, None, false).unwrap());
            let budget = attempt.context.budget.clone();
            let context = attempt.context.clone();
            let failure = Arc::new(
                splitty_remote::tunnel::Failure::from_error_budgeted(&"failure", &budget).unwrap(),
            );
            let output = Output {
                receiver: "private_receiver".into(),
                document: proof.document,
                callback: 7,
            };
            let (script, evaluation, completion) =
                output.failure_script(&attempt, failure, true).unwrap();
            let waiting_attempt = attempt.clone();
            let waiting =
                tokio::spawn(async move { await_evaluation(&waiting_attempt, completion).await });
            tokio::task::yield_now().await;
            assert!(!waiting.is_finished());
            if cancel {
                native
                    .broker
                    .cancel("main", &proof, attempt.handle())
                    .unwrap();
                assert!(context.is_closed());
            } else {
                tokio::time::advance(pipe::DELIVERY_TIMEOUT).await;
            }
            waiting.await.unwrap();
            native.destroyed("main");
            drop(attempt);
            assert!(budget.used() > 0);
            assert_eq!(native.broker.counts(), (1, 1));
            drop(script);
            drop(evaluation);
            assert_eq!(budget.used(), 0);
            assert_eq!(native.broker.counts(), (0, 0));
        }
    }
}
