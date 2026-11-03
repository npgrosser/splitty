"""Temporary source hooks shared by the actual GTK/WKWebView fixtures."""
import json
import hashlib
from pathlib import Path


def once(source, before, after):
    if source.count(before) != 1: raise RuntimeError('isolation fixture anchor changed: '+before[:100])
    return source.replace(before,after,1)


def instrument(harness,repo,source):
    here=Path(__file__).parent
    provider=(repo/'crates/splitty-tauri/src/client_ipc/provider.rs').read_text()
    hook='''                if crate::smoke_fixed_mode() {
                    let (state, connector) = crate::smoke_fixed_backend(target.as_str())?;
                    let pending = connector.admit_context(context.clone())?;
                    let mut client = connector.embedded_admitted(state,
                        splitty_server::service::catalog::Role::Remote, pending).await?;
                    let ready = negotiation::validate_ready(&client.ready, None, &wire::Purpose::Direct, MUX_PROTOCOL)
                        .and_then(|()| negotiation::host_ready(&client.ready, target.as_str(), &context.budget));
                    match ready {
                        Ok(ready) => client.ready = ready,
                        Err(error) => { let _ = client.finish().await; return Err(error); }
                    }
                    return Ok(Backend::Service(Box::new(client)));
                }
'''
    provider=once(provider,'                let result = tunnel::open(',hook+'                let result = tunnel::open(')
    path=harness/'src/client_ipc/provider.rs';path.unlink();path.write_text(provider)
    carrier=harness/'src/client_ipc/carrier.rs'
    carrier.write_text(once(carrier.read_text(), '        let _ = ended.await;', '''        let ending = ended.await;
        crate::smoke_carrier_ending(handle_string(attempt.handle()), &ending);'''))
    session=harness/'src/client_ipc/session.rs'
    text=session.read_text()
    text=once(text,'        let cleanup = owner.finish().await;', '''        crate::smoke_session_ending(attempt.handle(), "relay", &result);
        let cleanup = owner.finish().await;''')
    text=once(text,'        if cleaned {', '''        if let Some(attempt) = &self.guard.0 {
            crate::smoke_session_ending(attempt.handle(), "frontendFinish", &frontend);
            crate::smoke_session_ending(attempt.handle(), "backendFinish", &backend);
        }
        if cleaned {''')
    session.unlink();session.write_text(text)
    path=harness/'src/client_ipc.rs'
    path.write_text(path.read_text()+'''
impl Broker { pub(crate) fn smoke_counts(&self)->serde_json::Value {
    let registry=self.registry.lock().unwrap();
    serde_json::json!({"handles":registry.entries.len(),"opening":registry.opening,
        "mainHandles":registry.entries.values().filter(|entry|entry.window=="main").count()})
} }
''')
    # Replace only effects, retaining the production authorization and decisions.
    for name,begin,end,replacement in [
        ('navigation','fn open_externally(', '\n}\n', 'fn open_externally(_app: &AppHandle, _url: &Url) { crate::smoke_external_effect("navigation"); }'),
        ('editor','fn dispatch(', '\n}\n', 'fn dispatch(_app: &tauri::AppHandle, _uri: String) -> Result<(), String> { crate::smoke_external_effect("editor"); Err("fixture external opener blocked".into()) }'),
    ]:
        path=harness/'src'/(name+'.rs');text=path.read_text();start=text.index(begin);stop=text.index(end,start)+len(end)
        path.unlink();path.write_text((text[:start]+replacement+'\n'+text[stop:]).replace('use tauri_plugin_opener::OpenerExt;',''))
    source=source.replace('smoke_report, smoke_windows, native_bootstrap_begin,','smoke_report, smoke_windows, smoke_state, native_bootstrap_begin,')
    source=once(source,'smoke_event("begin");','if webview.label()=="main" { smoke_event("begin"); }')
    source=once(source,'smoke_event("confirm"); smoke_proofs().lock().unwrap().push(proof.clone());','if webview.label()=="main" { smoke_event("confirm"); smoke_proofs().lock().unwrap().push(proof.clone()); }')
    for event in ['started','finished']:
        source=once(source,f'smoke_event("{event}");',f'if window.label()=="main" {{ smoke_event("{event}"); }}')
    if source.count('app.add_capability(capability::main_window())?;') != 2: raise RuntimeError('main capability anchors changed')
    source=source.replace('app.add_capability(capability::main_window())?;', '''app.add_capability(capability::main_window())?;
app.add_capability(tauri::ipc::CapabilityBuilder::new("smoke-isolation-state").window("main").window("remote-*").local(true).permission("allow-smoke-state"))?;''')
    source=once(source,'    if value["fullUi"] == true {','''    if value["isolationConsumer"] == true { smoke_finish_isolation(&app).await?; }
    if value["fullUi"] == true || value["isolationConsumer"] == true {''')
    source=once(source,'    let success = retained_holds_explained','''    let isolation_ok = first["isolationConsumer"] != true || (
        first["pressure"]["captureReplay"] == true && first["pressure"]["window32"] == true
        && first["pressure"]["excessRefused"] == true && first["pressure"]["replacementAdmitted"] == true
        && smoke_pressure_file(first) && smoke_external_effects().lock().unwrap().is_empty());
    let success = isolation_ok && retained_holds_explained''')
    source=source.replace('"reloadQuotaStable":reload_quota_stable,','"isolationConsumer":first["isolationConsumer"],"isolationOk":isolation_ok,"reloadQuotaStable":reload_quota_stable,')
    source+='\n'+(here/'isolation.rs').read_text()
    source=once(source,'    if value.get("error").is_some() {','''    smoke_write("carrier-endings.json", &serde_json::json!(*smoke_carrier_endings().lock().unwrap()));
    if value.get("error").is_some() {''')
    build=harness/'build.rs';build.write_text(once(build.read_text(),'"smoke_report", "smoke_windows",','"smoke_report", "smoke_windows", "smoke_state",'))
    (harness/'backend-substitution.json').write_text(json.dumps({
        'targets':['fixture-one','fixture-two'],'backend':'two real embedded services with production Remote role',
        'substituted':'SSH backend location only','unchanged':'production remote window builder, bootstrap, broker, capabilities, frame and role checks',
        'effects':'editor/navigation OS openers replaced by a bounded counter and error',
        'scope':'actual WebView trust domains and consumer behavior; no SSH/remote filesystem or full production UI claim',
        'generatedSources':{name:hashlib.sha256((harness/name).read_bytes()).hexdigest() for name in ['src/client_ipc/provider.rs','src/editor.rs','src/navigation.rs']},
    },indent=2))
    return source
