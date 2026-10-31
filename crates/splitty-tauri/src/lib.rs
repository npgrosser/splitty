//! Tauri desktop shell for Splitty.
//!
//! This crate bootstraps the Splitty desktop application by:
//! 1. Resolving (or generating) an authentication token.
//! 2. Loading user configuration and resolving the active theme to set the
//!    window background color before the webview appears.
//! 3. Spawning the backend on a random localhost port: embedded on macOS/Linux,
//!    with the login shell's `PATH` when the desktop started the app (`login_path`),
//!    or through the lifecycle-bound WSL bridge on Windows.
//! 4. Opening a Tauri webview window pointed at the local server URL.
//!
//! The application UI communicates with the backend over HTTP/WebSocket. Tauri
//! IPC is limited to native window integration, the Windows WSL setup flow and
//! opening hosts in remote windows (`remote`), explicit editor handoff (`editor`)
//! and showing a saved download (`downloads`), and granted to the main window
//! only on its backend's exact origin (`capability`). Every navigation and
//! new-window request of a window passes the guard in `navigation`.

use std::sync::Arc;

use tauri::window::Color;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, Wry};

#[cfg(not(target_os = "windows"))]
use std::sync::RwLock;

mod capability;
mod downloads;
mod editor;
#[cfg(not(target_os = "windows"))]
mod login_path;
mod navigation;
mod remote;
#[cfg(any(target_os = "windows", test))]
mod windows_wsl;

/// Attach the navigation, new-window and download guards to the main window.
///
/// `origins` is asked for the backend origins on every request: they are fixed
/// for the embedded server, and only known once the WSL backend is up on
/// Windows. `local_app` admits the bundled setup page.
fn guard_window<'a, M: Manager<Wry>>(
    builder: WebviewWindowBuilder<'a, Wry, M>,
    app: AppHandle,
    origins: impl Fn() -> Vec<String> + Clone + Send + Sync + 'static,
    local_app: bool,
) -> WebviewWindowBuilder<'a, Wry, M> {
    let navigation_app = app.clone();
    let navigation_origins = origins.clone();
    let download_origins = origins.clone();
    let downloads = Arc::clone(app.state::<Arc<downloads::Downloads>>().inner());

    builder
        .on_navigation(move |url| {
            let decision = navigation::navigation(url, &navigation_origins(), local_app);
            navigation::follow(&navigation_app, url, decision)
        })
        .on_new_window(move |url, _features| {
            let decision = navigation::new_window(&url, &origins());
            navigation::follow_new_window(&app, &url, decision)
        })
        // Files saved from a pane (`splitty download`) go where the webview
        // saves downloads, and are remembered to be shown; anything else is
        // refused.
        .on_download(move |_webview, event| match event {
            tauri::webview::DownloadEvent::Requested { url, destination } => {
                let allowed = navigation::download(&url, &download_origins());
                if allowed {
                    downloads.requested(url.as_str(), destination);
                } else {
                    tracing::warn!("refused a download of a {}: URL", url.scheme());
                }
                allowed
            }
            tauri::webview::DownloadEvent::Finished { url, path, success } => {
                if !success {
                    tracing::warn!("a download did not finish");
                }
                downloads.finished(url.as_str(), path, success);
                true
            }
            _ => true,
        })
}

#[tauri::command]
fn set_title_bar_color(window: tauri::Window, r: u8, g: u8, b: u8) {
    #[cfg(target_os = "macos")]
    {
        use cocoa::appkit::{NSColor, NSWindow};
        use cocoa::base::{id, nil};

        let ns_window = window.ns_window().unwrap() as id;
        unsafe {
            let color = NSColor::colorWithRed_green_blue_alpha_(
                nil,
                r as f64 / 255.0,
                g as f64 / 255.0,
                b as f64 / 255.0,
                1.0,
            );
            ns_window.setBackgroundColor_(color);
        }
    }

    let _ = (window, r, g, b);
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

#[cfg(target_os = "windows")]
fn run_windows() {
    let state = Arc::new(windows_wsl::platform::WindowsWslState::new());
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
        .manage(state)
        .manage(Arc::new(remote::Remotes::default()))
        .manage(Arc::new(downloads::Downloads::default()))
        .invoke_handler(tauri::generate_handler![
            set_title_bar_color,
            windows_wsl::platform::wsl_setup_state,
            windows_wsl::platform::wsl_list_distributions,
            windows_wsl::platform::wsl_start_backend,
            windows_wsl::platform::wsl_switch_distribution,
            remote::remote_hosts,
            remote::remote_open,
            remote::remote_cancel,
            editor::open_remote_editor,
            editor::open_local_editor,
            downloads::reveal_download,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let state = Arc::clone(
                app.state::<Arc<windows_wsl::platform::WindowsWslState>>()
                    .inner(),
            );
            let builder =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("setup.html".into()))
                    .title("Splitty")
                    .inner_size(920.0, 640.0)
                    .min_inner_size(680.0, 480.0)
                    .decorations(false)
                    .disable_drag_drop_handler()
                    .background_color(Color(18, 18, 20, 255));
            guard_window(builder, handle, move || state.backend_origins(), true).build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
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
    // Resolve token
    let token = match splitty_core::auth::resolve("") {
        Ok((t, source)) => {
            tracing::info!("using token from {source}");
            t
        }
        Err(_) => {
            let mut buf = [0u8; 16];
            rand::Rng::fill(&mut rand::thread_rng(), &mut buf);
            hex::encode(buf)
        }
    };

    // Load config + theme early so we can set the window background color
    let config = splitty_core::config::load();
    let bg_color = resolve_title_bar_color(&config);
    let native_title_bar = config.window.native_title_bar;
    #[cfg(target_os = "macos")]
    let native_title_bar_uses_override = native_title_bar_uses_override(&config);

    // A running daemon, or one started for the kept sessions of an earlier
    // daemon, before an embedded server.
    let (port, bootstrap, embedded) = match daemon_for_app() {
        Some(info) => {
            tracing::info!(
                "connecting to the daemon (pid={}, port={})",
                info.pid,
                info.port
            );
            match bootstrap_from_daemon(info.port, &token) {
                Some(code) => (info.port, code, None),
                None => {
                    tracing::warn!(
                        "daemon did not issue a bootstrap code, starting embedded server"
                    );
                    start_embedded_server(token, config)
                }
            }
        }
        None => start_embedded_server(token, config),
    };

    // A single-use bootstrap code, not the process token (security review
    // SR-04). The page exchanges it for an HttpOnly session cookie
    // and clears the address bar; capturing this URL afterwards is worthless.
    let url = format!("http://127.0.0.1:{port}/?bootstrap={bootstrap}&desktop=1");
    let backend_origins = navigation::backend_origins(port);

    let app = tauri::Builder::default()
        .manage(embedded)
        .plugin(tauri_plugin_opener::init())
        .manage(Arc::new(remote::Remotes::default()))
        .manage(Arc::new(downloads::Downloads::default()))
        .invoke_handler(tauri::generate_handler![
            set_title_bar_color,
            remote::remote_hosts,
            remote::remote_open,
            remote::remote_cancel,
            editor::open_remote_editor,
            editor::open_local_editor,
            downloads::reveal_download,
        ])
        .setup(move |app| {
            // Before the window loads the backend: its page gets the native
            // commands on exactly this origin.
            app.add_capability(capability::main_window(port))?;
            let handle = app.handle().clone();
            let builder =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse().unwrap()))
                    .title("Splitty")
                    .inner_size(1200.0, 800.0)
                    .disable_drag_drop_handler()
                    .background_color(bg_color);
            let origins = backend_origins.clone();
            let mut builder = guard_window(builder, handle, move || origins.clone(), false);

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
                use cocoa::appkit::{NSColor, NSWindow};
                use cocoa::base::{id, nil};

                let ns_window = window.as_ref().window().ns_window().unwrap() as id;
                unsafe {
                    let color = NSColor::colorWithRed_green_blue_alpha_(
                        nil,
                        bg_color.0 as f64 / 255.0,
                        bg_color.1 as f64 / 255.0,
                        bg_color.2 as f64 / 255.0,
                        1.0,
                    );
                    ns_window.setBackgroundColor_(color);
                }
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            app_handle.state::<Arc<remote::Remotes>>().shutdown();
            if let Some(server) = app_handle
                .state::<Option<EmbeddedServer>>()
                .inner()
                .as_ref()
            {
                server.shutdown();
            }
        }
    });
}

/// The daemon the app connects to: the running one, or, when none runs but
/// sessions of an earlier daemon are kept (`session-keepers`), a daemon
/// started for them with the executable those keepers run.
#[cfg(not(target_os = "windows"))]
fn daemon_for_app() -> Option<splitty_core::daemon::DaemonInfo> {
    if let Some(info) = splitty_core::daemon::read_info() {
        if splitty_core::daemon::is_running(info.pid) {
            return Some(info);
        }
        tracing::info!("stale daemon info found");
        splitty_core::daemon::remove_info();
    }
    #[cfg(unix)]
    {
        let kept = splitty_core::keeper::client::find(&splitty_core::keeper::dir::keepers_dir());
        let executable = executable_for_kept(
            kept.iter().map(|keeper| keeper.hello.executable.clone()),
            |path| std::path::Path::new(path).is_file(),
            splitty_in_path,
        )?;
        drop(kept);
        tracing::info!(
            "sessions are kept without a daemon; starting one with {}",
            executable.display()
        );
        start_daemon_for_kept(&executable)
    }
    #[cfg(not(unix))]
    None
}

/// The executable to start a daemon for kept sessions with: the first that a
/// keeper runs and that still exists, else `splitty` from the `PATH`; none
/// without kept sessions.
#[cfg(any(unix, test))]
fn executable_for_kept(
    executables: impl IntoIterator<Item = String>,
    exists: impl Fn(&str) -> bool,
    from_path: impl FnOnce() -> Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    let executables: Vec<String> = executables.into_iter().collect();
    if executables.is_empty() {
        return None;
    }
    executables
        .iter()
        .find(|path| !path.is_empty() && exists(path))
        .map(std::path::PathBuf::from)
        .or_else(from_path)
}

/// `splitty` from the `PATH`, the login shell's on macOS and Linux.
#[cfg(unix)]
fn splitty_in_path() -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("splitty"))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// `daemon start` with `executable`, on the default address and else on one
/// the system assigns; the daemon once it serves. The command returns only
/// once its daemon serves, within its own 10-second limit.
#[cfg(unix)]
fn start_daemon_for_kept(executable: &std::path::Path) -> Option<splitty_core::daemon::DaemonInfo> {
    for extra in [&[][..], &["--addr", "127.0.0.1:0"][..]] {
        let started = std::process::Command::new(executable)
            .args(["daemon", "start"])
            .args(extra)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match started {
            Ok(status) if status.success() => {
                if let Some(info) = splitty_core::daemon::read_info()
                    .filter(|info| splitty_core::daemon::is_running(info.pid))
                {
                    return Some(info);
                }
            }
            Ok(status) => tracing::warn!("daemon start for the kept sessions failed: {status}"),
            Err(error) => tracing::warn!("cannot run {}: {error}", executable.display()),
        }
    }
    tracing::warn!("the kept sessions wait; starting an embedded server");
    None
}

/// Ask an already-running daemon for a single-use bootstrap code.
///
/// Uses the process token from its 0600 file. Deliberately not the control
/// socket: minting a browser session must stay off the channel that is offered
/// to every pane (see `splitty_server::http::session::issue_bootstrap`).
#[cfg(not(target_os = "windows"))]
fn bootstrap_from_daemon(port: u16, token: &str) -> Option<String> {
    // Reuses the CLI's request path rather than opening a second HTTP client
    // here: one implementation of "talk to a Splitty server", already covered by
    // the server crate's tests.
    let server = splitty_server::cli::ServerInfo {
        transport: splitty_server::cli::Transport::Tcp {
            addr: format!("127.0.0.1:{port}"),
            token: token.to_string(),
        },
        source: "token file",
    };

    let response = tokio::runtime::Runtime::new()
        .ok()?
        .block_on(splitty_server::cli::api_post(
            &server,
            "/api/bootstrap",
            serde_json::json!({}),
        ))
        .ok()?;

    response
        .get("bootstrap")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Start the in-process server and return its port plus a bootstrap code.
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
    let (port, _, server) = start_embedded_server("test-token".into(), Default::default());
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
        let job=splitty_server::cli::api_post(&owner,"/api/jobs",serde_json::json!({"requestId":"f5b1fc42-44f9-41df-9148-65a1fbdba804","cwd":std::env::var("HOME").expect("fixture home"),"argv":["/bin/sh","-c","read line"],"placement":{"kind":"screen"}})).await.expect("job");
        (grant,job)
    });
    assert_eq!(job["state"], "running");
    server.expect("owned embedded server").shutdown();
    assert!(!std::path::Path::new(grant["socket"].as_str().expect("socket")).exists());
    assert!(std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).is_err());
    let (_, _, new) = start_embedded_server("test-token".into(), Default::default());
    let list = runtime
        .block_on(splitty_server::cli::api_get(&owner, "/api/jobs"))
        .expect("new service");
    assert_eq!(list["jobs"], serde_json::json!([]));
    new.expect("replacement server").shutdown();
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
fn start_embedded_server(
    token: String,
    config: splitty_core::config::HomeConfig,
) -> (u16, String, Option<EmbeddedServer>) {
    let token_clone = token.clone();
    let (port_tx, port_rx) = std::sync::mpsc::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let template_store = match splitty_core::template::FileStore::new_default() {
                Ok(store) => Arc::new(store),
                Err(e) => {
                    tracing::warn!("failed to create template store: {e}");
                    Arc::new(splitty_core::template::FileStore::new(
                        splitty_core::config::config_dir().join("templates"),
                    ))
                }
            };

            let shell_profiles = Arc::new(RwLock::new(
                splitty_core::shell_profiles::state_from_settings(&config.shells),
            ));
            let pty_manager = Arc::new(splitty_core::pty::manager::Manager::with_shell_profiles(
                shell_profiles.clone(),
            ));
            let mut dispatcher =
                splitty_core::protocol::dispatcher::Dispatcher::new(pty_manager.clone());
            dispatcher.configure_sizes(splitty_core::protocol::sizing::SizeConfig::from_settings(
                &config.terminal.size_policy,
                config.terminal.size_idle_seconds,
            ));

            let (notify_tx, _) = tokio::sync::broadcast::channel::<String>(256);
            let tx = notify_tx.clone();
            let engine =
                splitty_core::protocol::notify::NotificationEngine::new(Box::new(move |event| {
                    if let Ok(json) = serde_json::to_string(&event) {
                        let _ = tx.send(json);
                    }
                }));

            dispatcher.notify = Some(Arc::new(std::sync::Mutex::new(engine)));
            // Spawned shells learn the control socket path, never a credential
            // (security review SR-05).
            dispatcher.env = splitty_core::mux::effect_runner::EnvContext {
                control_socket: splitty_server::control::socket_path()
                    .to_string_lossy()
                    .into_owned(),
            };
            let env = dispatcher.env.clone();
            let dispatcher = Arc::new(dispatcher);

            let effect_runner = Arc::new(splitty_core::mux::effect_runner::EffectRunner::new(
                pty_manager as Arc<dyn splitty_core::pty::Backend>,
            ));

            let mux_store = Arc::new(splitty_core::mux::Store::new());
            let ws_state = Arc::new(splitty_server::ws::WsState::new(
                mux_store.clone(),
                dispatcher,
                shell_profiles.clone(),
                effect_runner,
                env,
                notify_tx,
                "embedded".to_string(),
            ));

            // Bind before building state: the origin allowlist is derived from
            // the port actually in use, and binding to port 0 only reveals it
            // afterwards. Building state first would produce an allowlist that
            // matches nothing and lock the UI out of its own API.
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let listen_addr = listener.local_addr().unwrap();
            let port = listen_addr.port();

            let state = splitty_server::http::AppState {
                token: token_clone.clone(),
                sessions: Arc::new(splitty_server::http::session::SessionStore::new()),
                allowed_origins: Arc::new(splitty_server::http::allowed_origins(
                    &listen_addr.to_string(),
                )),
                mux_store,
                ws_state,
                config: Arc::new(RwLock::new(config)),
                shell_profiles,
                server_config: Arc::new(RwLock::new(splitty_core::config::load_server())),
                template_store,
                themes_dir: splitty_core::config::themes_dir(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                commit: option_env!("SPLITTY_COMMIT")
                    .unwrap_or("unknown")
                    .to_string(),
                build_time: option_env!("SPLITTY_BUILD_TIME")
                    .unwrap_or("unknown")
                    .to_string(),
                listen_addr: listen_addr.to_string(),
                server_mode: "embedded".to_string(),
            };

            let bootstrap = state.sessions.issue_bootstrap();
            let jobs = state.ws_state.jobs.clone();
            let backend = state.ws_state.dispatcher.backend().clone();
            let control = splitty_server::control::spawn(state.clone());
            let app = splitty_server::http::build_router_with_frontend(state);

            let _ = port_tx.send((port, bootstrap));

            tracing::info!("tauri backend on port {port}");
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stop_rx.await;
                    jobs.shutdown().await;
                    backend.close_all();
                })
                .await
                .unwrap();
            control.shutdown();
            let _ = done_tx.send(());
        });
    });

    let (port, bootstrap) = port_rx.recv().expect("failed to get server port");
    (
        port,
        bootstrap,
        Some(EmbeddedServer {
            stop: std::sync::Mutex::new(Some(stop_tx)),
            done: std::sync::Mutex::new(done_rx),
        }),
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

/// `server-modes`, Desktop App Hosting On macOS And Linux: kept sessions get
/// a daemon of the executable their keepers run, else `splitty` from the
/// `PATH`; without kept sessions, nothing is started.
#[cfg(test)]
#[test]
fn kept_sessions_choose_the_executable_their_keepers_run() {
    let from_path = || Some(std::path::PathBuf::from("/usr/local/bin/splitty"));
    let exists = |path: &str| path == "/opt/splitty/0.1.18/splitty";
    assert_eq!(
        executable_for_kept(
            [
                "/gone/splitty".to_owned(),
                "/opt/splitty/0.1.18/splitty".to_owned()
            ],
            exists,
            || panic!("the keepers' own executable comes first"),
        ),
        Some(std::path::PathBuf::from("/opt/splitty/0.1.18/splitty"))
    );
    assert_eq!(
        executable_for_kept(
            ["/gone/splitty".to_owned(), String::new()],
            exists,
            from_path
        ),
        Some(std::path::PathBuf::from("/usr/local/bin/splitty")),
        "an executable that is gone, or none named by an older keeper"
    );
    assert_eq!(
        executable_for_kept(Vec::<String>::new(), exists, || panic!("nothing kept")),
        None
    );
}
