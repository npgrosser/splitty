// Temporary read-only fixture diagnostics. Never included in production builds.
#[derive(Default)]
struct FixtureIsolation {
    reports: std::collections::HashMap<String, serde_json::Value>,
    external_opens: Vec<String>,
    setup_proof: Option<client_ipc::Proof>,
}
fn fixture_isolation() -> &'static std::sync::Mutex<FixtureIsolation> {
    static STATE: std::sync::OnceLock<std::sync::Mutex<FixtureIsolation>>=std::sync::OnceLock::new();
    STATE.get_or_init(Default::default)
}
pub(crate) fn fixture_external_open(url:&tauri::Url){
    let mut state=fixture_isolation().lock().unwrap();
    if state.external_opens.len()<16{state.external_opens.push(format!("{}:{}",url.scheme(),url.path()));}
}
#[tauri::command]
fn windows_fixture_state(webview: tauri::Webview, app: AppHandle, native: tauri::State<'_, Arc<Native>>, value: Option<serde_json::Value>) -> Result<serde_json::Value,String> {
    let policy=native.broker.registered_policy(webview.label()).map_err(|_| "unregistered fixture caller")?;
    if native.broker.current_document(webview.label()).is_none(){return Err("uncommitted fixture caller".into())}
    let mut state=fixture_isolation().lock().map_err(|_| "fixture state unavailable")?;
    if let Some(value)=value {
        if webview.label()=="main" && matches!(policy,Policy::Setup) && state.setup_proof.is_none(){state.setup_proof=native.broker.smoke_proof("main");}
        if !state.reports.contains_key(webview.label()) && state.reports.len()>=3{return Err("fixture report capacity".into())}
        state.reports.insert(webview.label().to_owned(),value);
    }
    let phase=match native.broker.registered_policy("main"){Ok(Policy::Main{..})=>"main",Ok(Policy::Setup)=>"setup",_=>"closed"};
    let old_setup_denied=state.setup_proof.as_ref().map(|proof|native.broker.authorize("main",proof).is_err()).unwrap_or(false);
    let counts=splitty_transport::admission::Admission::global().counts();
    let budget=splitty_transport::budget::ProcessBudget::global();
    let reports=state.reports.clone();let external_opens=state.external_opens.clone();drop(state);
    let mut windows:Vec<_>=app.webview_windows().keys().cloned().collect();windows.sort();
    Ok(serde_json::json!({"phase":phase,"oldSetupDenied":old_setup_denied,"reports":reports,"windows":windows,"externalOpens":external_opens,
        "broker":native.broker.smoke_counts(),"bytes":budget.used(),"dataHighWater":budget.data.high_water(),"controlHighWater":budget.control.high_water(),
        "admission":{"streams":counts.streams,"carriers":counts.carriers,"opening":counts.opening,"workers":counts.workers}}))
}
