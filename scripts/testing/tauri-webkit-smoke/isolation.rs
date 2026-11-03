// Temporary actual-WebView fixture composition, never a production module.
// Only backend location is substituted; role, broker, frame and window policies
// remain production code. No SSH or independent-host claim is made.
struct SmokeBackend {
    runtime: Arc<splitty_server::service::runtime::Runtime>,
    connector: Arc<splitty_server::native::client::Connector>,
}
fn smoke_backends() -> &'static std::sync::Mutex<std::collections::HashMap<String, SmokeBackend>> {
    static BACKENDS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, SmokeBackend>>> = std::sync::OnceLock::new();
    BACKENDS.get_or_init(Default::default)
}
fn smoke_fixed_mode() -> bool { true }
fn smoke_fixed_backend(target: &str) -> std::io::Result<(Arc<splitty_server::service::State>, Arc<splitty_server::native::client::Connector>)> {
    if !["fixture-one", "fixture-two"].contains(&target) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "fixture target is not native-assigned"));
    }
    let mut backends = smoke_backends().lock().unwrap();
    let backend = backends.entry(target.to_owned()).or_insert_with(|| {
        let control = std::path::PathBuf::from(std::env::var_os("SPLITTY_CONFIG_DIR").unwrap()).join(format!("{target}.sock"));
        SmokeBackend {
            runtime: Arc::new(splitty_server::service::runtime::build("embedded", None,
                Arc::new(splitty_core::pty::terminal::LocalTerminals), &control)),
            connector: Arc::new(Default::default()),
        }
    });
    Ok((backend.runtime.state.clone(), backend.connector.clone()))
}
fn smoke_isolation_reports() -> &'static std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>> {
    static REPORTS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>> = std::sync::OnceLock::new();
    REPORTS.get_or_init(Default::default)
}
fn smoke_external_effects() -> &'static std::sync::Mutex<Vec<String>> {
    static EFFECTS: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();
    EFFECTS.get_or_init(Default::default)
}
fn smoke_external_effect(kind: &str) {
    let mut effects = smoke_external_effects().lock().unwrap();
    if effects.len() < 16 { effects.push(kind.to_owned()); }
}
#[tauri::command]
fn smoke_state(webview: tauri::Webview, native: tauri::State<'_, Arc<Native>>, value: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    let policy = native.broker.registered_policy(webview.label()).map_err(|_| "unregistered fixture caller")?;
    if !matches!(policy, Policy::Main {..} | Policy::Remote {..}) || native.broker.current_document(webview.label()).is_none() {
        return Err("uncommitted fixture caller".into());
    }
    let mut reports = smoke_isolation_reports().lock().unwrap();
    if let Some(value) = value {
        if serde_json::to_vec(&value).map_err(|error|error.to_string())?.len() > 16*1024 { return Err("fixture report too large".into()); }
        if !reports.contains_key(webview.label()) && reports.len() >= 3 { return Err("fixture report capacity".into()); }
        if let Some(phase) = value["phase"].as_str().filter(|phase| ["isolation-complete", "pressure-observed", "pressure-replay"].contains(phase)) {
            smoke_write(&format!("{phase}.json"), &value);
        }
        reports.insert(webview.label().to_owned(), value);
    }
    let counts = splitty_transport::admission::Admission::global().counts();
    let budget = splitty_transport::budget::ProcessBudget::global();
    Ok(serde_json::json!({"reports":*reports,"home":std::env::var("HOME").unwrap(),"externalEffects":*smoke_external_effects().lock().unwrap(),"broker":native.broker.smoke_counts(),"bytes":budget.used(),
        "dataHighWater":budget.data.high_water(),"controlHighWater":budget.control.high_water(),
        "admission":{"streams":counts.streams,"carriers":counts.carriers,"opening":counts.opening,"workers":counts.workers}}))
}
async fn smoke_finish_isolation(app: &AppHandle) -> Result<(), String> {
    for (label, window) in app.webview_windows() {
        if label.starts_with("remote-") { window.destroy().map_err(|error|error.to_string())?; }
    }
    let backends: Vec<_> = smoke_backends().lock().unwrap().drain().map(|(_,backend)|backend.runtime).collect();
    for backend in backends { backend.close().await.map_err(|error|error.to_string())?; }
    Ok(())
}

fn smoke_pressure_file(value: &serde_json::Value) -> bool {
    let Some(path) = value["pressure"]["healthyFile"].as_str() else { return false; };
    let Ok(path) = std::path::Path::new(path).canonicalize() else { return false; };
    let Ok(home) = std::path::Path::new(&std::env::var("HOME").unwrap()).canonicalize() else { return false; };
    if !path.starts_with(home) || path.file_name().and_then(|name|name.to_str()) != Some("webkit-pressure.bin") { return false; }
    let Ok(metadata) = std::fs::metadata(&path) else { return false; };
    if metadata.len() != 65537 { return false; }
    std::fs::read(path).is_ok_and(|bytes| bytes.iter().enumerate().all(|(index, byte)| *byte == (index % 256) as u8))
}
fn smoke_carrier_endings() -> &'static std::sync::Mutex<Vec<serde_json::Value>> {
    static ENDINGS: std::sync::OnceLock<std::sync::Mutex<Vec<serde_json::Value>>> = std::sync::OnceLock::new();
    ENDINGS.get_or_init(Default::default)
}
fn smoke_carrier_ending(handle: String, ending: &Result<std::io::Result<()>, tokio::sync::oneshot::error::RecvError>) {
    let mut endings = smoke_carrier_endings().lock().unwrap();
    if endings.len() >= 64 { return; }
    let value = match ending {
        Ok(Ok(())) => serde_json::json!({"handle":handle,"outcome":"ok"}),
        Ok(Err(error)) => serde_json::json!({"handle":handle,"outcome":"ioError","kind":format!("{:?}",error.kind()),"message":error.to_string().chars().take(512).collect::<String>()}),
        Err(_) => serde_json::json!({"handle":handle,"outcome":"oneshotCanceled"}),
    };
    endings.push(value);
}
fn smoke_session_ending(handle: client_ipc::Handle, stage: &str, result: &std::io::Result<()>) {
    let mut endings = smoke_carrier_endings().lock().unwrap();
    if endings.len() >= 64 { return; }
    let handle: String = handle.bytes().iter().map(|byte| format!("{byte:02x}")).collect();
    let value = match result {
        Ok(()) => serde_json::json!({"handle":handle,"stage":stage,"outcome":"ok"}),
        Err(error) => serde_json::json!({"handle":handle,"stage":stage,"outcome":"ioError","kind":format!("{:?}",error.kind()),"message":error.to_string().chars().take(512).collect::<String>()}),
    };
    endings.push(value);
}
