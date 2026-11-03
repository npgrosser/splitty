//! Packaged desktop shell with native document authority and IPC streams.
//! Unix desktops own an in-process terminal service; Windows owns one WSL
//! helper. Remote windows use SSH exec pipes. No UI network listener starts.

use std::sync::Arc;

use tauri::window::Color;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, Wry};

mod capability;
mod client_ipc;
mod downloads;
mod editor;
#[cfg(not(target_os = "windows"))]
mod login_path;
mod navigation;
mod remote;
#[cfg(any(target_os = "windows", test))]
mod windows_wsl;

use client_ipc::{
    carrier::{Auth, Native},
    provider::Provider,
    BackendId, Policy,
};
use navigation::DocumentRole;

fn role(policy: &Policy) -> DocumentRole {
    match policy {
        Policy::Main { .. } => DocumentRole::Main,
        Policy::Remote { .. } => DocumentRole::Remote,
        Policy::Setup => DocumentRole::Setup,
    }
}
fn challenge(
    webview: &tauri::Webview,
    native: &Native,
    value: client_ipc::Challenge,
) -> Result<(), String> {
    let receiver = native
        .broker
        .bootstrap_receiver(webview.label())
        .map_err(|_| "native window unavailable")?;
    webview
        .eval(client_ipc::bootstrap::challenge_script(
            &receiver,
            &value.nonce,
            value.token,
        ))
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn native_bootstrap_begin(
    webview: tauri::Webview,
    native: tauri::State<'_, Arc<Native>>,
    bootstrap: [u8; 32],
    nonce: String,
) -> Result<(), String> {
    let policy = native
        .broker
        .registered_policy(webview.label())
        .map_err(|_| "native window unavailable")?;
    if !navigation::packaged_entry(&webview.url().map_err(|e| e.to_string())?, role(&policy)) {
        return Err("native document denied".into());
    }
    if let Some(value) = native
        .broker
        .bootstrap_begin(webview.label(), &bootstrap, &nonce)
        .map_err(|_| "native bootstrap denied")?
    {
        challenge(&webview, &native, value)?;
    }
    Ok(())
}
#[tauri::command]
fn native_bootstrap_confirm(
    webview: tauri::Webview,
    native: tauri::State<'_, Arc<Native>>,
    bootstrap: [u8; 32],
    nonce: String,
    challenge: [u8; 32],
) -> Result<serde_json::Value, String> {
    let policy = native
        .broker
        .registered_policy(webview.label())
        .map_err(|_| "native window unavailable")?;
    if !navigation::packaged_entry(&webview.url().map_err(|e| e.to_string())?, role(&policy)) {
        return Err("native document denied".into());
    }
    let (proof, policy) = native
        .broker
        .bootstrap_confirm(webview.label(), &bootstrap, &nonce, &challenge)
        .map_err(|_| "native bootstrap denied")?;
    let (role, target) = match policy {
        Policy::Main { .. } => ("main", None),
        Policy::Setup => ("setup", None),
        Policy::Remote { target, .. } => ("remote", Some(target.as_str().to_owned())),
    };
    Ok(
        serde_json::json!({"auth":{"document":proof.document,"secret":proof.secret()},"role":role,"target":target}),
    )
}

/// The native registration fixes the role. Only packaged main documents get
/// a private capsule; popouts share their parent's explicit frontend objects.
pub(crate) fn configure_window<'a, M: Manager<Wry>>(
    builder: WebviewWindowBuilder<'a, Wry, M>,
    app: AppHandle,
    label: &str,
    initial_role: DocumentRole,
) -> WebviewWindowBuilder<'a, Wry, M> {
    let native = app.state::<Arc<Native>>().inner().clone();
    let key = native
        .broker
        .bootstrap_key(label)
        .expect("registered native window");
    let entry = if initial_role == DocumentRole::Setup {
        "setup.html"
    } else {
        "index.html"
    };
    #[cfg(target_os = "windows")]
    let entry = format!("http://tauri.localhost/{entry}");
    #[cfg(not(target_os = "windows"))]
    let entry = format!("tauri://localhost/{entry}");
    let script = client_ipc::bootstrap::script(
        key,
        &native
            .broker
            .bootstrap_receiver(label)
            .expect("registered window"),
        &entry,
    );
    let nav_native = native.clone();
    let popouts = app.state::<Arc<navigation::Popouts>>().inner().clone();
    let nav_popouts = popouts.clone();
    let new_popouts = popouts.clone();
    let nav_label = label.to_owned();
    let nav_app = app.clone();
    let new_native = native.clone();
    let new_label = label.to_owned();
    let new_app = app.clone();
    let downloads = app.state::<Arc<downloads::Downloads>>().inner().clone();
    let download_native = native.clone();
    builder
        .initialization_script(script)
        .on_navigation(move |url| {
            let Ok(policy) = nav_native.broker.registered_policy(&nav_label) else {
                return false;
            };
            let decision = navigation::packaged_navigation(url, role(&policy));
            if decision == navigation::Decision::Allow {
                nav_popouts.revoke_parent(&nav_app, &nav_label, || {
                    nav_native.broker.navigating(&nav_label)
                });
            }
            navigation::follow(&nav_app, url, decision)
        })
        .on_new_window(move |url, features| {
            let decision = new_native
                .broker
                .registered_policy(&new_label)
                .map(|policy| navigation::packaged_new_window(&url, role(&policy)))
                .unwrap_or(navigation::Decision::Block);
            if decision == navigation::Decision::Allow {
                let Some(document) = new_native.broker.current_document(&new_label) else {
                    return tauri::webview::NewWindowResponse::Deny;
                };
                let broker = new_native.broker.clone();
                let parent = new_label.clone();
                new_popouts.open(
                    &new_app,
                    &new_label,
                    document,
                    features,
                    Arc::new(move || broker.current_document(&parent) == Some(document)),
                )
            } else {
                navigation::follow_new_window(&new_app, &url, decision)
            }
        })
        .on_page_load(move |window, payload| {
            let Ok(policy) = native.broker.registered_policy(window.label()) else {
                return;
            };
            if !navigation::packaged_entry(payload.url(), role(&policy)) {
                popouts.revoke_parent(window.app_handle(), window.label(), || {
                    native.broker.navigating(window.label())
                });
                return;
            }
            match payload.event() {
                // on_navigation revoked the previous epoch. Initialization may
                // run before this commit event, so preserve its pending nonce.
                tauri::webview::PageLoadEvent::Started => {
                    let _ = native.broker.bootstrap_committed(window.label());
                }
                tauri::webview::PageLoadEvent::Finished => {
                    if let Ok(Some(value)) = native.broker.bootstrap_loaded(window.label()) {
                        let _ = challenge(window.as_ref(), &native, value);
                    }
                }
            }
        })
        .on_download(move |webview, event| {
            let label = webview.label();
            let Some(document) = download_native.broker.current_document(label) else {
                return false;
            };
            let Ok(policy) = download_native.broker.registered_policy(label) else {
                return false;
            };
            let owner = downloads::Owner {
                window: label.to_owned(),
                document,
            };
            match event {
                tauri::webview::DownloadEvent::Requested { url, destination } => {
                    if !navigation::packaged_download(&url, role(&policy)) {
                        return false;
                    }
                    downloads.requested(owner, url.as_str(), destination);
                    true
                }
                tauri::webview::DownloadEvent::Finished { url, path, success } => {
                    downloads.finished(&owner, url.as_str(), path, success);
                    true
                }
                _ => false,
            }
        })
}

#[tauri::command]
fn set_title_bar_color(
    window: tauri::Window,
    webview: tauri::Webview,
    native: tauri::State<'_, Arc<Native>>,
    auth: Auth,
    r: u8,
    g: u8,
    b: u8,
) -> Result<(), String> {
    native.main_action(&webview, &auth)?;
    #[cfg(target_os = "macos")]
    window
        .set_background_color(Some(Color(r, g, b, 255)))
        .map_err(|error| error.to_string())?;

    let _ = (window, r, g, b);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt::init();

    #[cfg(target_os = "windows")]
    run_windows();

    #[cfg(not(target_os = "windows"))]
    {
        // Before any thread starts: it changes the process environment.
        login_path::adopt();
        run_embedded();
    }
}

#[tauri::command]
fn close_popout(
    app: AppHandle,
    webview: tauri::Webview,
    native: tauri::State<'_, Arc<Native>>,
    popouts: tauri::State<'_, Arc<navigation::Popouts>>,
    auth: Auth,
    label: String,
) -> Result<(), String> {
    if label.len() > 64 {
        return Err("native popout close denied".into());
    }
    let proof = native.authenticate(&webview, &auth)?;
    native
        .broker
        .main_action(webview.label(), &proof)
        .map_err(|_| "native popout close denied")?;
    popouts.close_owned(&app, webview.label(), proof.document, &label, || {
        native.broker.main_action(webview.label(), &proof).is_ok()
    })
}

#[cfg(target_os = "windows")]
mod windows_commands {
    use super::*;
    use windows_wsl::{platform, WindowsDesktopState, WslDistribution, WslStartResult};
    #[tauri::command]
    pub(crate) fn wsl_setup_state(
        app: AppHandle,
        webview: tauri::Webview,
        native: tauri::State<'_, Arc<Native>>,
        auth: Auth,
    ) -> Result<WindowsDesktopState, String> {
        native.launcher_action(&webview, &auth)?;
        platform::wsl_setup_state(app)
    }
    #[tauri::command]
    pub(crate) async fn wsl_list_distributions(
        app: AppHandle,
        webview: tauri::Webview,
        native: tauri::State<'_, Arc<Native>>,
        auth: Auth,
    ) -> Result<Vec<WslDistribution>, String> {
        native.launcher_action(&webview, &auth)?;
        platform::wsl_list_distributions(app).await
    }
    #[tauri::command]
    pub(crate) async fn wsl_start_backend(
        app: AppHandle,
        webview: tauri::Webview,
        native: tauri::State<'_, Arc<Native>>,
        auth: Auth,
        state: tauri::State<'_, Arc<platform::WindowsWslState>>,
        distro: Option<String>,
    ) -> Result<WslStartResult, String> {
        let proof = native.authenticate(&webview, &auth)?;
        native
            .broker
            .setup_action(webview.label(), &proof)
            .map_err(|_| "setup document required")?;
        let result = platform::wsl_start_backend(app, state, distro).await?;
        native
            .broker
            .authorize(webview.label(), &proof)
            .map_err(|_| "setup document closed")?;
        Ok(result)
    }
    #[tauri::command]
    pub(crate) fn wsl_open_backend(
        app: AppHandle,
        webview: tauri::Webview,
        native: tauri::State<'_, Arc<Native>>,
        auth: Auth,
        state: tauri::State<'_, Arc<platform::WindowsWslState>>,
    ) -> Result<(), String> {
        let proof = native.authenticate(&webview, &auth)?;
        native
            .broker
            .setup_action(webview.label(), &proof)
            .map_err(|_| "setup document required")?;
        let owner = state.owner()?;
        let backend = BackendId::new();
        native.provider(backend, Arc::new(Provider::owned(owner)));
        native
            .broker
            .replace(webview.label(), Policy::Main { backend })
            .map_err(|_| "setup document closed")?;
        let window = app
            .get_webview_window(webview.label())
            .ok_or("main window closed")?;
        window
            .navigate(
                "http://tauri.localhost/index.html"
                    .parse()
                    .expect("packaged entry"),
            )
            .map_err(|e| e.to_string())
    }
    #[tauri::command]
    pub(crate) async fn wsl_switch_distribution(
        app: AppHandle,
        webview: tauri::Webview,
        native: tauri::State<'_, Arc<Native>>,
        auth: Auth,
        distro: String,
    ) -> Result<(), String> {
        native.main_action(&webview, &auth)?;
        platform::wsl_switch_distribution(app, distro).await
    }
}

#[cfg(target_os = "windows")]
fn run_windows() {
    let state = Arc::new(windows_wsl::platform::WindowsWslState::new());
    let native = Arc::new(Native::default());
    native
        .broker
        .register("main", Policy::Setup)
        .expect("setup policy");
    let app = tauri::Builder::default()
        // Tauri requires the single-instance plugin to be initialized first.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                window.state::<Arc<navigation::Popouts>>().destroyed(
                    window.app_handle(),
                    window.label(),
                    || window.state::<Arc<Native>>().destroyed(window.label()),
                );
            }
        })
        .manage(state)
        .manage(native)
        .manage(Arc::new(navigation::Popouts::default()))
        .manage(Arc::new(remote::Remotes::default()))
        .manage(Arc::new(downloads::Downloads::default()))
        .invoke_handler(tauri::generate_handler![
            set_title_bar_color,
            close_popout,
            native_bootstrap_begin,
            native_bootstrap_confirm,
            client_ipc::carrier::native_open,
            client_ipc::carrier::native_cancel_open,
            client_ipc::carrier::native_start,
            client_ipc::carrier::native_write,
            client_ipc::carrier::native_consumed,
            client_ipc::carrier::native_close,
            windows_commands::wsl_setup_state,
            windows_commands::wsl_list_distributions,
            windows_commands::wsl_start_backend,
            windows_commands::wsl_open_backend,
            windows_commands::wsl_switch_distribution,
            remote::native_connection_state,
            remote::remote_progress,
            remote::remote_hosts,
            remote::remote_open,
            remote::remote_cancel,
            editor::open_remote_editor,
            editor::open_local_editor,
            downloads::reveal_download,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            app.add_capability(capability::main_window())?;
            app.add_capability(capability::remote_windows())?;
            let builder =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("setup.html".into()))
                    .title("Splitty")
                    .inner_size(920.0, 640.0)
                    .min_inner_size(680.0, 480.0)
                    .decorations(false)
                    .disable_drag_drop_handler()
                    .background_color(Color(18, 18, 20, 255));
            configure_window(builder, handle, "main", DocumentRole::Setup).build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
            app_handle.state::<Arc<Native>>().shutdown();
            app_handle
                .state::<Arc<windows_wsl::platform::WindowsWslState>>()
                .shutdown();
        }
        if matches!(event, tauri::RunEvent::Exit) {
            app_handle.state::<Arc<remote::Remotes>>().shutdown();
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn run_embedded() {
    // Load config + theme early so we can set the window background color
    let config = splitty_core::config::load();
    let bg_color = resolve_title_bar_color(&config);
    let native_title_bar = config.window.native_title_bar;
    #[cfg(target_os = "macos")]
    let native_title_bar_uses_override = native_title_bar_uses_override(&config);

    // Desktop-local work always belongs to this app. An independent daemon
    // and its keepers have no influence on startup or shutdown.
    let (service, embedded) = start_local_backend();
    let native = Arc::new(Native::default());
    let backend = BackendId::new();
    native.provider(backend, Arc::new(Provider::embedded(service)));
    native
        .broker
        .register("main", Policy::Main { backend })
        .expect("main policy");

    let app = tauri::Builder::default()
        .manage(embedded)
        .manage(native)
        .manage(Arc::new(navigation::Popouts::default()))
        .plugin(tauri_plugin_opener::init())
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                window.state::<Arc<navigation::Popouts>>().destroyed(
                    window.app_handle(),
                    window.label(),
                    || window.state::<Arc<Native>>().destroyed(window.label()),
                );
            }
        })
        .manage(Arc::new(remote::Remotes::default()))
        .manage(Arc::new(downloads::Downloads::default()))
        .invoke_handler(tauri::generate_handler![
            set_title_bar_color,
            close_popout,
            native_bootstrap_begin,
            native_bootstrap_confirm,
            client_ipc::carrier::native_open,
            client_ipc::carrier::native_cancel_open,
            client_ipc::carrier::native_start,
            client_ipc::carrier::native_write,
            client_ipc::carrier::native_consumed,
            client_ipc::carrier::native_close,
            remote::native_connection_state,
            remote::remote_progress,
            remote::remote_hosts,
            remote::remote_open,
            remote::remote_cancel,
            editor::open_remote_editor,
            editor::open_local_editor,
            downloads::reveal_download,
        ])
        .setup(move |app| {
            app.add_capability(capability::main_window())?;
            app.add_capability(capability::remote_windows())?;
            let handle = app.handle().clone();
            let builder =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("Splitty")
                    .inner_size(1200.0, 800.0)
                    .disable_drag_drop_handler()
                    .background_color(bg_color);
            let mut builder = configure_window(builder, handle, "main", DocumentRole::Main);

            #[cfg(target_os = "macos")]
            if native_title_bar_uses_override {
                builder = builder.title_bar_style(tauri::TitleBarStyle::Transparent);
            }

            if !native_title_bar {
                #[cfg(target_os = "macos")]
                {
                    builder = builder
                        .title_bar_style(tauri::TitleBarStyle::Overlay)
                        .hidden_title(true);
                }
                #[cfg(not(target_os = "macos"))]
                {
                    builder = builder.decorations(false);
                }
            }

            #[cfg_attr(
                not(target_os = "macos"),
                expect(
                    unused_variables,
                    reason = "only macOS colors the native window after it is built"
                )
            )]
            let window = builder.build()?;

            #[cfg(target_os = "macos")]
            if native_title_bar_uses_override {
                window
                    .as_ref()
                    .window()
                    .set_background_color(Some(Color(bg_color.0, bg_color.1, bg_color.2, 255)))?;
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            app_handle.state::<Arc<Native>>().shutdown();
            app_handle.state::<Arc<remote::Remotes>>().shutdown();
            app_handle.state::<EmbeddedServer>().shutdown();
        }
    });
}

/// App-owned embedded service shutdown, without a network listener.
///
/// Windows has no embedded backend — it runs the server inside WSL through the
/// bridge in `windows_wsl` — and `axum` is not even a dependency there, so this
/// has to be gated to match its only caller, `run_embedded`.
#[cfg(not(target_os = "windows"))]
struct EmbeddedServer {
    stop: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    done: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

#[cfg(all(test, unix))]
#[test]
fn embedded_managed_jobs_start_and_shutdown_with_the_shared_service() {
    // Isolate HOME/runtime for the real composition root without changing the
    // test runner's process environment or starting a native window.
    if std::env::var_os("SPLITTY_EMBEDDED_TEST_CHILD").is_none() {
        // Short: below a macOS temporary directory, the agent socket in
        // ~/Library/Application Support/splitty exceeds what a socket allows.
        let home = tempfile::Builder::new()
            .prefix("sg")
            .tempdir_in("/tmp")
            .expect("fixture");
        let mut child =
            std::process::Command::new(std::env::current_exe().expect("test executable"));
        child.args([
            "--exact",
            "embedded_managed_jobs_start_and_shutdown_with_the_shared_service",
            "--nocapture",
        ]);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("SPLITTY_") {
                child.env_remove(key);
            }
        }
        let result = child
            .env("SPLITTY_EMBEDDED_TEST_CHILD", "1")
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env("XDG_RUNTIME_DIR", home.path().join("run"))
            .output()
            .expect("isolated test");
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    // Live but incompatible discovery, an unrelated keeper and a token file
    // must remain untouched. Listening sentinels detect even attempted dials.
    let config = splitty_core::config::config_dir();
    let run = splitty_core::config::run_dir();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(run.join("keepers")).unwrap();
    let daemon_socket = run.join("client.sock");
    let keeper_socket = run.join("keepers/independent.sock");
    let daemon = std::os::unix::net::UnixListener::bind(&daemon_socket).unwrap();
    let keeper = std::os::unix::net::UnixListener::bind(&keeper_socket).unwrap();
    daemon.set_nonblocking(true).unwrap();
    keeper.set_nonblocking(true).unwrap();
    let discovery = serde_json::json!({"pid":std::process::id(), "format":2,
        "clientSocket":daemon_socket, "instance":"a".repeat(32), "transportVersion":999,
        "socket":run.join("daemon.sock"), "version":"999.0.0", "protocol":999,
        "keepsSessions":true,"startedAt":"fixture"})
    .to_string();
    let discovery_path = config.join("daemon.json");
    let token_path = config.join("auth-token");
    std::fs::write(&discovery_path, &discovery).unwrap();
    std::fs::write(&token_path, "independent credential").unwrap();
    let (service, server) = start_local_backend();
    let instance = service.instance.clone();
    let owner = splitty_server::cli::ServerInfo {
        transport: splitty_server::cli::Transport::LocalSocket(
            splitty_server::control::socket_path()
                .to_string_lossy()
                .into_owned(),
        ),
        source: "test",
    };
    let (grant,job)=runtime.block_on(async {
        let grant=splitty_server::cli::api_post(&owner,"/api/agents",serde_json::json!({"name":"embedded"})).await.expect("grant");
        let job=splitty_server::cli::api_post(&owner,"/api/jobs",serde_json::json!({"requestId":"f5b1fc42-44f9-41df-9148-65a1fbdba804","cwd":std::env::var("HOME").expect("fixture home"),"argv":["/bin/sh","-c","echo $$ > job.pid; read line"],"placement":{"kind":"screen"}})).await.expect("job");
        (grant,job)
    });
    assert_eq!(job["state"], "running");
    let pid_file = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join("job.pid");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let pid = loop {
        if let Ok(contents) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = contents.trim().parse::<u32>() {
                break pid;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "job did not publish its PID"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let alive = || {
        std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("probe job process")
            .success()
    };
    assert!(alive(), "the job must be alive before app shutdown");
    server.shutdown();
    assert!(!alive(), "app shutdown left its managed job running");
    assert!(!std::path::Path::new(grant["socket"].as_str().expect("socket")).exists());
    let (next, new) = start_local_backend();
    assert_ne!(next.instance, instance);
    let list = runtime
        .block_on(splitty_server::cli::api_get(&owner, "/api/jobs"))
        .expect("new service");
    assert_eq!(list["jobs"], serde_json::json!([]));
    new.shutdown();
    assert_eq!(std::fs::read_to_string(&discovery_path).unwrap(), discovery);
    for stale in [
        "not valid JSON",
        r#"{"pid":4294967295,"port":1,"version":"old","protocol":1,"socket":"unused","startedAt":"fixture"}"#,
    ] {
        std::fs::write(&discovery_path, stale).unwrap();
        let (_, backend) = start_local_backend();
        backend.shutdown();
        assert_eq!(std::fs::read_to_string(&discovery_path).unwrap(), stale);
    }
    assert_eq!(
        std::fs::read_to_string(token_path).unwrap(),
        "independent credential"
    );
    assert!(daemon_socket.exists() && keeper_socket.exists());
    assert_eq!(
        daemon.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(
        keeper.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
#[cfg(not(target_os = "windows"))]
impl EmbeddedServer {
    fn shutdown(&self) {
        if let Some(stop) = self.stop.lock().expect("embedded shutdown").take() {
            let _ = stop.send(());
            if let Err(error) = self
                .done
                .lock()
                .expect("embedded completion")
                .recv_timeout(std::time::Duration::from_secs(15))
            {
                tracing::warn!(%error,"embedded shutdown did not finish in time");
            }
        }
    }
}
#[cfg(not(target_os = "windows"))]
fn start_local_backend() -> (Arc<splitty_server::service::State>, EmbeddedServer) {
    let (state_tx, state_rx) = std::sync::mpsc::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("embedded runtime");
        rt.block_on(async move {
            let control_path = splitty_server::control::socket_path();
            let backend = splitty_server::service::runtime::build(
                "embedded",
                None,
                Arc::new(splitty_core::pty::terminal::LocalTerminals),
                &control_path,
            );
            let control =
                splitty_server::control::spawn_service(backend.state.clone(), control_path);
            if state_tx.send(backend.state.clone()).is_ok() {
                let _ = stop_rx.await;
            }
            for client in backend
                .state
                .client_state
                .clients
                .lock()
                .expect("clients")
                .values()
            {
                client.sender.close();
            }
            if let Err(error) = backend.close().await {
                tracing::warn!(%error,"owned local service shutdown failed");
            }
            control.shutdown();
            let _ = done_tx.send(());
        });
    });
    let state = state_rx.recv().expect("embedded service startup");
    (
        state,
        EmbeddedServer {
            stop: std::sync::Mutex::new(Some(stop_tx)),
            done: std::sync::Mutex::new(done_rx),
        },
    )
}

/// Determine the title bar color based on the resolved theme's surface color.
#[cfg(not(target_os = "windows"))]
fn resolve_title_bar_color(config: &splitty_core::config::HomeConfig) -> Color {
    let themes_dir = splitty_core::config::themes_dir();
    let theme = splitty_core::theme::resolve_theme(&config.appearance.theme, &themes_dir);

    match config.window.title_bar_color_mode.as_str() {
        "terminal" => parse_hex_color(&theme.terminal.bg),
        "activeTab" => parse_hex_color(&theme.ui.surface_base),
        "native" => Color(24, 24, 24, 255),
        _ => parse_hex_color(&theme.ui.surface_raised),
    }
}

#[cfg(target_os = "macos")]
fn native_title_bar_uses_override(config: &splitty_core::config::HomeConfig) -> bool {
    config.window.native_title_bar && config.window.title_bar_color_mode != "native"
}

/// Parse a "#RRGGBB" hex string into a Tauri Color.
#[cfg(not(target_os = "windows"))]
fn parse_hex_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    if hex.len() >= 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        Color(r, g, b, 255)
    } else {
        Color(24, 24, 24, 255)
    }
}
