// Included only by the generated runtime smoke crate.
fn smoke_events() -> &'static std::sync::Mutex<Vec<String>> {
    static EVENTS: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();
    EVENTS.get_or_init(Default::default)
}
fn smoke_event(event: &str) {
    let mut events = smoke_events().lock().unwrap();
    events.push(event.into());
    smoke_write("events.json", &serde_json::json!(*events));
}
fn smoke_proofs() -> &'static std::sync::Mutex<Vec<client_ipc::Proof>> {
    static PROOFS: std::sync::OnceLock<std::sync::Mutex<Vec<client_ipc::Proof>>> =
        std::sync::OnceLock::new();
    PROOFS.get_or_init(Default::default)
}
fn smoke_write(name: &str, value: &serde_json::Value) {
    let path =
        std::path::PathBuf::from(std::env::var_os("SPLITTY_SMOKE_EVIDENCE").unwrap()).join(name);
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
fn smoke_epochs() -> &'static std::sync::Mutex<Vec<serde_json::Value>> {
    static EPOCHS: std::sync::OnceLock<std::sync::Mutex<Vec<serde_json::Value>>> = std::sync::OnceLock::new();
    EPOCHS.get_or_init(Default::default)
}
#[tauri::command]
async fn smoke_report(
    app: AppHandle,
    native: tauri::State<'_, Arc<Native>>,
    mut value: serde_json::Value,
) -> Result<(), String> {
    if value.get("error").is_some() {
        smoke_write("result.json", &value);
        app.exit(1);
        return Ok(());
    }
    let before = native.broker.counts();
    let stale_handles = native.broker.smoke_stale_handles();
    let delivery_handles = native.smoke_delivery_handles();
    let retained_holds_explained = stale_handles.is_subset(&delivery_handles);
    let active = splitty_transport::admission::Admission::global().counts();
    value["nativeSnapshot"] = serde_json::json!({"handles":before.0,"opening":before.1,
        "staleHandles":stale_handles.len(),"frameworkDeliveries":delivery_handles.len(),"retainedHoldsExplained":retained_holds_explained,
        "carriers":active.carriers,"streams":active.streams,"workers":active.workers});
    let epochs = {
        let mut epochs = smoke_epochs().lock().unwrap();
        epochs.push(value.clone());
        smoke_write("epochs.json", &serde_json::json!(*epochs));
        epochs.clone()
    };
    if value["phase"] != "reloaded" {
        if value["phase"] == "first" { smoke_write("first.json", &value); }
        return Ok(());
    }
    let first = &epochs[0];
    let expected_epochs = value["expectedReloads"].as_u64().unwrap_or(1) as usize + 1;
    let stale_denied = {
        let proofs = smoke_proofs().lock().unwrap();
        proofs.len() == expected_epochs
            && proofs[..proofs.len()-1].iter().all(|proof| native.broker.authorize("main", proof).is_err())
            && native.broker.authorize("main", proofs.last().unwrap()).is_ok()
    };
    let reload_quota_stable = value["fullUi"] != true || epochs.iter().all(|epoch|
        epoch["nativeSnapshot"]["handles"].as_u64() <= first["nativeSnapshot"]["handles"].as_u64()
        && epoch["nativeSnapshot"]["opening"] == 0
        && epoch["nativeSnapshot"]["staleHandles"] == 0);
    let same_instance = epochs.iter().all(|epoch| epoch["instance"] == first["instance"]);
    let unique_documents: std::collections::HashSet<_> = epochs.iter().map(|epoch| epoch["document"].as_str()).collect();
    if value["fullUi"] == true {
        // Actual destruction purges framework-held Channels; never simulate it
        // with a broker call while the production document still exists.
        native.shutdown();
        app.get_webview_window("main").ok_or("main window missing")?.destroy().map_err(|error|error.to_string())?;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let budget = splitty_transport::budget::ProcessBudget::global();
    let admission = splitty_transport::admission::Admission::global();
    while (native.broker.counts() != (0,0)
        || budget.used() != 0
        || admission.counts().streams != 0
        || admission.counts().carriers != 0)
        && std::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let counts = admission.counts();
    let events = smoke_events().lock().unwrap().clone();
    let callbacks: Vec<_> = events
        .iter()
        .filter(|event| !event.starts_with("navigation:"))
        .map(String::as_str)
        .collect();
    let handshake_observed = callbacks.len() == 4 * expected_epochs
        && callbacks.chunks_exact(4).all(|epoch| {
            epoch.contains(&"started")
                && epoch.contains(&"begin")
                && epoch[2] == "finished"
                && epoch[3] == "confirm"
        });
    let broker_counts = native.broker.counts();
    let success = retained_holds_explained
        && reload_quota_stable
        && epochs.len() == expected_epochs
        && unique_documents.len() == expected_epochs
        && broker_counts == (0,0)
        && handshake_observed
        && stale_denied
        && same_instance
        && first["document"] != value["document"]
        && budget.used() == 0
        && counts.streams == 0
        && counts.carriers == 0
        && counts.workers == 0;
    smoke_write(
        "result.json",
        &serde_json::json!({ "success":success, "first":first,"reloaded":value,"epochs":epochs,"reloads":expected_epochs-1,
        "reloadQuotaStable":reload_quota_stable,"retainedHoldsExplained":retained_holds_explained,"nativeHandles":broker_counts.0,"nativeOpening":broker_counts.1,
        "handshakeObserved":handshake_observed,"staleProofDenied":stale_denied,"sameInstance":same_instance,"events":events,
        "bytes":budget.used(),"dataHighWater":budget.data.high_water(),"controlHighWater":budget.control.high_water(),
        "streams":counts.streams,"carriers":counts.carriers,"workers":counts.workers }),
    );
    app.exit(if success { 0 } else { 1 });
    Ok(())
}

#[tauri::command]
async fn smoke_windows(app: AppHandle) -> Result<serde_json::Value, String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let observed = app.clone();
    app.run_on_main_thread(move || {
        use gtk::prelude::*;
        let mut managed: Vec<_> = observed.webview_windows().keys().cloned().collect();
        managed.sort();
        let visible = gtk::Window::list_toplevels().iter().filter(|widget| widget.is_visible()).count();
        let _ = send.send(serde_json::json!({"managed":managed,"nativeVisible":visible}));
    }).map_err(|error|error.to_string())?;
    receive.await.map_err(|error|error.to_string())
}
