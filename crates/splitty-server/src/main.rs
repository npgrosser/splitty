//! Binary entrypoint for the Splitty terminal multiplexer server.
//!
//! Provides subcommands via `clap`:
//! - `serve` — starts the server with the embedded frontend and optionally opens a browser.
//! - `daemon start|stop|status` — manages a persistent background daemon.
//! - `connect <user@host>` — opens an SSH tunnel to a remote daemon (and, with
//!   `--forward`, to ports of the host), with text or (`--json`) one JSON event
//!   per line for the desktop shells.
//!
//! Handles token resolution (flag, token file, or random one-shot), constructs the
//! shared `AppState`, wires up the PTY manager, notification engine, and mux store,
//! then launches the Axum HTTP server with graceful shutdown on Ctrl+C.

use splitty_server::{cli, control, http, ws};

use std::sync::{Arc, Mutex, RwLock};

use clap::{Parser, Subcommand};
use splitty_core::mux::Store;
use splitty_core::protocol::notify::NotificationEngine;
use splitty_core::pty::Backend;

/// Emit one machine-readable startup line, surviving a launcher that stopped
/// reading.
///
/// The native launchers own the read end of this pipe. `println!` panics when a
/// write fails, so a launcher that closed stdout early — it crashed, or it
/// parsed the port and dropped the pipe — would take the backend down with it
/// on the *next* line. A dropped handshake line is recoverable; a dead backend
/// serving reset connections is not.
///
/// Flushed per line because these are handshake output: the reader is blocked
/// on them, and the process may fork or block long before a buffer would fill.
macro_rules! emit_line {
    ($($arg:tt)*) => {{
        let mut out = std::io::stdout().lock();
        let _ = std::io::Write::write_fmt(&mut out, format_args!($($arg)*));
        let _ = std::io::Write::write_all(&mut out, b"\n");
        let _ = std::io::Write::flush(&mut out);
    }};
}

#[derive(Parser)]
#[command(
    name = "splitty",
    version,
    about = "Splitty terminal multiplexer server"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Global options for CLI control commands (server discovery).
#[derive(clap::Args, Clone)]
struct CtlOpts {
    /// Server address (overrides auto-discovery)
    #[arg(long, global = true)]
    addr: Option<String>,

    /// Auth token (overrides auto-discovery)
    #[arg(long, global = true)]
    token: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(flatten)]
    JobControl(cli::jobs::Commands),
    /// Start the server with embedded frontend (opens browser UI)
    Serve {
        /// Address to listen on
        #[arg(long, env = "SPLITTY_ADDR", default_value = "127.0.0.1:9800")]
        addr: String,

        /// Process token for CLI callers (read from the token file if not
        /// provided). Note that values passed here are visible in the process
        /// list; prefer the token file.
        #[arg(long)]
        token: Option<String>,

        /// Generate a random one-shot token instead of resolving from file
        #[arg(long)]
        oneshot_token: bool,

        /// Removed: direct remote mode is refused. Use `splitty connect`.
        #[arg(long, hide = true)]
        remote: bool,

        /// Open the UI in the default browser after starting
        #[arg(long)]
        open: bool,

        /// Shut down when the desktop bridge closes stdin
        #[arg(long, hide = true)]
        desktop_stdio: bool,
    },

    /// Manage the background daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },

    /// Connect to a remote Splitty daemon via SSH tunnel
    ///
    /// Asks the host's `splitty` for its daemon, forwards the daemon's port to
    /// 127.0.0.1 here and opens a session in the browser. The tunnel is rebuilt
    /// when it drops; Ctrl+C ends it. The target is recorded as a recent host.
    Connect {
        /// SSH target (e.g., user@host)
        target: String,

        /// Print the session URL without opening a browser
        #[arg(long)]
        no_open: bool,

        /// Forward ports of the host as well, in the same tunnel: PORT or
        /// LOCAL:REMOTE, comma-separated, repeatable. Each is bound to
        /// 127.0.0.1 here, takes another free port when its own is busy, and is
        /// announced as http://localhost:<port>
        #[arg(long, value_name = "PORTS")]
        forward: Vec<String>,

        /// Print one JSON object per line instead of text (progress, the
        /// session URL, the forwards, reconnects, the final error)
        #[arg(long)]
        json: bool,

        /// Accepted for compatibility; opening the browser is the default
        #[arg(long, hide = true)]
        open: bool,
    },

    /// Request a file or folder in the local client's editor (requires confirmation)
    Open {
        #[command(flatten)]
        ctl: CtlOpts,
        /// Existing file or folder, relative to this shell
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
        /// Override the client-side editor preference
        #[arg(long = "with", value_parser = ["vscode", "zed"])]
        editor: Option<String>,
        /// Originating terminal (normally inherited from Splitty)
        #[arg(long, env = "SPLITTY_SESSION_ID")]
        session: Option<String>,
    },

    /// Offer a file of this terminal to the Splitty windows showing it; one
    /// of them saves it as a download (requires Save there). A folder goes as
    /// a zip archive.
    Download {
        #[command(flatten)]
        ctl: CtlOpts,
        /// Regular file or folder, relative to this shell
        path: std::path::PathBuf,
        /// Originating terminal (normally inherited from Splitty)
        #[arg(long, env = "SPLITTY_SESSION_ID")]
        session: Option<String>,
    },

    // ── CLI control commands ───────────────────────────────────────────
    /// Split the current pane
    Split {
        #[command(flatten)]
        ctl: CtlOpts,

        /// Split horizontally (left/right)
        #[arg(short = 'H', long)]
        horizontal: bool,

        /// Target pane ID (defaults to $SPLITTY_PANE_ID)
        #[arg(long)]
        pane: Option<String>,

        /// Target screen ID (defaults to $SPLITTY_SCREEN_ID)
        #[arg(long)]
        screen: Option<String>,
    },

    /// Create a new screen
    #[command(name = "new-screen")]
    NewScreen {
        #[command(flatten)]
        ctl: CtlOpts,

        /// Screen name
        #[arg(long)]
        name: Option<String>,
    },

    /// Close a pane
    #[command(name = "close-pane")]
    ClosePane {
        #[command(flatten)]
        ctl: CtlOpts,

        /// Target pane ID (defaults to $SPLITTY_PANE_ID)
        #[arg(long)]
        pane: Option<String>,

        /// Target screen ID (defaults to $SPLITTY_SCREEN_ID)
        #[arg(long)]
        screen: Option<String>,
    },

    /// Close a screen
    #[command(name = "close-screen")]
    CloseScreen {
        #[command(flatten)]
        ctl: CtlOpts,

        /// Target screen ID (defaults to $SPLITTY_SCREEN_ID)
        #[arg(long)]
        screen: Option<String>,
    },

    /// Rename a screen
    #[command(name = "rename-screen")]
    RenameScreen {
        #[command(flatten)]
        ctl: CtlOpts,

        /// New name
        name: String,

        /// Target screen ID (defaults to $SPLITTY_SCREEN_ID)
        #[arg(long)]
        screen: Option<String>,
    },

    /// List all screens
    #[command(name = "list-screens")]
    ListScreens {
        #[command(flatten)]
        ctl: CtlOpts,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// List panes in a screen
    #[command(name = "list-panes")]
    ListPanes {
        #[command(flatten)]
        ctl: CtlOpts,

        /// Target screen ID (defaults to $SPLITTY_SCREEN_ID)
        #[arg(long)]
        screen: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Show server status
    Status {
        #[command(flatten)]
        ctl: CtlOpts,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Mint a single-use bootstrap code for a browser session.
    ///
    /// Invoked over SSH by `splitty connect` on the remote host
    /// (security review SR-13): the tunnel used to work by
    /// reading a long-lived token out of the remote's `daemon.json`, so a
    /// credential valid for the life of that daemon travelled the link and
    /// landed in a URL. A code is now valid for exactly one session exchange.
    #[command(name = "issue-bootstrap")]
    IssueBootstrap {
        #[command(flatten)]
        ctl: CtlOpts,
    },

    /// Describe this host to a client over SSH (internal)
    ///
    /// Prints one JSON object: this server, the daemon and its screens. With
    /// `--ensure-daemon`, a missing daemon is started on 127.0.0.1:<port>.
    #[command(name = "remote-handshake", hide = true)]
    RemoteHandshake {
        #[arg(long, required = true)]
        json: bool,

        #[arg(long, requires = "port")]
        ensure_daemon: bool,

        #[arg(long)]
        port: Option<u16>,
    },

    /// Relay the daemon's WebSocket over stdin and stdout (internal)
    #[command(name = "remote-bridge", hide = true)]
    RemoteBridge,

    /// Keep one daemon session's PTY and program alive (internal)
    ///
    /// A daemon starts one per session, with the spawn spec on standard input.
    #[command(name = "session-keeper", hide = true)]
    SessionKeeper,

    /// List the known remote hosts as one JSON array (internal)
    ///
    /// The recent hosts of `connect` first, then the `Host` names of
    /// `~/.ssh/config`, for the desktop shells' host picker.
    #[command(name = "remote-hosts", hide = true)]
    RemoteHosts {
        #[arg(long, required = true)]
        json: bool,
    },

    /// Show a session of the daemon in this terminal until you detach (Ctrl+\)
    ///
    /// The session keeps running in the daemon, and the windows that show it
    /// keep showing it. Without a target, pick a pane from a list. Over SSH,
    /// run it on the host: `ssh -t host splitty attach`.
    Attach {
        #[command(flatten)]
        ctl: CtlOpts,

        /// A pane id, a session id, or a unique prefix of either (at least 4
        /// characters)
        target: Option<String>,

        /// The key that detaches: ctrl-\, ctrl-], ctrl-^, ctrl-_ or ctrl-<letter>
        #[arg(long, value_name = "KEY", default_value = "ctrl-\\")]
        detach_key: cli::attach::DetachKey,
    },

    /// Send keys to a pane
    #[command(name = "send-keys")]
    SendKeys {
        #[command(flatten)]
        ctl: CtlOpts,

        /// Keys to send (use quotes for special chars, e.g. "ls -la\n")
        keys: String,

        /// Target pane ID (defaults to $SPLITTY_PANE_ID)
        #[arg(long)]
        pane: Option<String>,

        /// Target screen ID (defaults to $SPLITTY_SCREEN_ID)
        #[arg(long)]
        screen: Option<String>,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start the daemon in the background
    Start {
        /// Address to listen on
        #[arg(long, env = "SPLITTY_ADDR", default_value = "127.0.0.1:9800")]
        addr: String,

        /// Process token for CLI callers (read from the token file if not
        /// provided). Note that values passed here are visible in the process
        /// list; prefer the token file.
        #[arg(long)]
        token: Option<String>,

        /// Removed: direct remote mode is refused. Use `splitty connect`.
        #[arg(long, hide = true)]
        remote: bool,
    },

    /// Stop the running daemon; its sessions keep running in their keepers
    Stop {
        /// Also end every kept session, also when no daemon runs
        #[arg(long)]
        end_sessions: bool,
    },

    /// Stop the running daemon and start this binary's in its place, keeping
    /// the sessions (the update)
    Restart {
        /// Address to listen on (default: the stopped daemon's, else
        /// SPLITTY_ADDR or 127.0.0.1:9800)
        #[arg(long)]
        addr: Option<String>,

        /// End every kept session before the start
        #[arg(long)]
        end_sessions: bool,
    },

    /// Show daemon status
    Status {
        /// Print one JSON object (`running`, and pid, port, socket, startedAt
        /// and version of a running daemon), for scripts and `splitty connect`
        #[arg(long)]
        json: bool,
    },

    /// The daemon process itself, started by `daemon start` (internal)
    #[command(hide = true)]
    Run {
        /// Inherited descriptor of the listener `daemon start` bound
        #[arg(long)]
        listen_fd: i32,

        /// Process token passed on from `daemon start --token`
        #[arg(long)]
        token: Option<String>,
    },
}

/// First word of the line `daemon run` prints once it serves.
#[cfg(unix)]
const DAEMON_READY: &str = "SPLITTY_DAEMON_READY";
/// First word of the line `daemon run` prints when it cannot start.
#[cfg(unix)]
const DAEMON_ERROR: &str = "SPLITTY_DAEMON_ERROR";
/// How long `daemon start` waits for one of those lines.
#[cfg(unix)]
const DAEMON_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Human-readable description of how the CLI reached the server.
fn describe_transport(transport: &cli::Transport) -> String {
    match transport {
        cli::Transport::LocalSocket(path) => path.clone(),
        cli::Transport::Tcp { addr, .. } => addr.clone(),
    }
}

fn resolve_token(token_flag: Option<String>, oneshot: bool) -> String {
    if oneshot {
        let mut buf = [0u8; 16];
        rand::Rng::fill(&mut rand::thread_rng(), &mut buf);
        return hex::encode(buf);
    }
    match token_flag {
        Some(t) if !t.is_empty() => t,
        _ => {
            let flag_val = token_flag.as_deref().unwrap_or("");
            match splitty_core::auth::resolve(flag_val) {
                Ok((t, source)) => {
                    tracing::info!("using token from {source}");
                    t
                }
                Err(e) => {
                    tracing::warn!("failed to resolve token: {e}, generating random");
                    let mut buf = [0u8; 16];
                    rand::Rng::fill(&mut rand::thread_rng(), &mut buf);
                    hex::encode(buf)
                }
            }
        }
    }
}

/// Resolve a requested bind address and refuse anything but loopback.
///
/// This is the invariant that actually keeps the service off the network
/// (security review SR-07). Previously `--addr 0.0.0.0:8080` was
/// accepted independently of `--remote`, so a plaintext HTTP service carrying
/// full terminal control could be exposed to the LAN by a single flag, and the
/// `Host` header check was the only thing in the way — a header the client
/// chooses. Misconfiguration now fails at startup instead of at review time.
///
/// Every resolved address must be loopback: a name resolving to both loopback
/// and a routable address is rejected rather than partially honoured.
fn resolve_loopback_bind(addr: &str, remote: bool) -> std::net::SocketAddr {
    use std::net::ToSocketAddrs;

    if remote {
        eprintln!(
            "error: direct remote mode is not supported.\n\
             \n\
             It served HTTP and WebSocket traffic — including the auth token and\n\
             all terminal I/O — without TLS, and allowed any origin. Use an SSH\n\
             tunnel to reach a remote Splitty instead:\n\
             \n\
             \x20   splitty connect <user@host>\n\
             \n\
             The backend stays bound to loopback on the remote machine."
        );
        std::process::exit(2);
    }

    let resolved: Vec<std::net::SocketAddr> = match addr.to_socket_addrs() {
        Ok(addrs) => addrs.collect(),
        Err(e) => {
            eprintln!("error: could not resolve bind address {addr:?}: {e}");
            std::process::exit(2);
        }
    };

    if resolved.is_empty() {
        eprintln!("error: bind address {addr:?} resolved to nothing");
        std::process::exit(2);
    }

    if let Some(routable) = resolved.iter().find(|a| !a.ip().is_loopback()) {
        eprintln!(
            "error: refusing to bind to {routable} — Splitty listens on loopback only.\n\
             \n\
             The server grants full terminal control over plaintext HTTP, so binding\n\
             it to a routable address would expose command execution to the network.\n\
             Use 127.0.0.1, and reach remote instances over an SSH tunnel."
        );
        std::process::exit(2);
    }

    resolved[0]
}

fn build_state(
    addr: &str,
    token: String,
    server_mode: &str,
    initial_state: Option<splitty_core::mux::WorkspaceState>,
    terminals: Arc<dyn splitty_core::pty::terminal::TerminalFactory>,
    control_socket: &std::path::Path,
) -> (http::AppState, Arc<splitty_core::pty::manager::Manager>) {
    let config = splitty_core::config::load();
    let shell_profiles = Arc::new(RwLock::new(
        splitty_core::shell_profiles::state_from_settings(&config.shells),
    ));
    let server_config = splitty_core::config::load_server();
    let themes_dir = splitty_core::config::themes_dir();

    let template_store = match splitty_core::template::FileStore::new_default() {
        Ok(store) => Arc::new(store),
        Err(e) => {
            tracing::warn!("failed to create template store: {e}, using fallback");
            Arc::new(splitty_core::template::FileStore::new(
                splitty_core::config::config_dir().join("templates"),
            ))
        }
    };

    // Create PTY backend and protocol dispatcher
    let pty_manager = Arc::new(splitty_core::pty::manager::Manager::with_terminals(
        shell_profiles.clone(),
        terminals,
    ));
    let mut dispatcher = splitty_core::protocol::dispatcher::Dispatcher::new(pty_manager.clone());
    dispatcher.configure_sizes(splitty_core::protocol::sizing::SizeConfig::from_settings(
        &config.terminal.size_policy,
        config.terminal.size_idle_seconds,
    ));

    // Spawned shells learn the path to the local control socket, not a network
    // address and master token (security review SR-05).
    dispatcher.env = splitty_core::mux::effect_runner::EnvContext {
        control_socket: control_socket.to_string_lossy().into_owned(),
    };

    // Create notification broadcast channel and engine
    let (notify_tx, _) = tokio::sync::broadcast::channel::<String>(256);
    let tx = notify_tx.clone();
    let engine = NotificationEngine::new(Box::new(move |event| {
        if let Ok(json) = serde_json::to_string(&event) {
            let _ = tx.send(json);
        }
    }));

    dispatcher.notify = Some(Arc::new(Mutex::new(engine)));
    let dispatcher = Arc::new(dispatcher);

    let mut initial_state = initial_state;
    if let (Some(state), Some(default_profile)) = (
        initial_state.as_mut(),
        shell_profiles
            .read()
            .unwrap()
            .default_profile()
            .map(|p| p.id.clone()),
    ) {
        for screen in state.screens.values_mut() {
            for session in screen.sessions.values_mut() {
                if session.shell_profile_id.is_none() && session.shell.is_none() {
                    session.shell_profile_id = Some(default_profile.clone());
                }
            }
        }
    }

    let mut mux_store = match initial_state {
        Some(state) => Store::with_state(state),
        None => Store::new(),
    };

    if server_mode == "daemon" {
        mux_store.set_on_change(|state| {
            if let Err(e) = splitty_core::daemon::save_state(state) {
                tracing::warn!("failed to persist workspace state: {e}");
            }
        });
    }

    let mux_store = Arc::new(mux_store);

    // Create effect runner for mux-driven PTY lifecycle
    let effect_runner = Arc::new(splitty_core::mux::effect_runner::EffectRunner::new(
        pty_manager.clone() as Arc<dyn splitty_core::pty::Backend>,
    ));
    let env = dispatcher.env.clone();

    let ws_state = Arc::new(ws::WsState::new(
        mux_store.clone(),
        dispatcher,
        shell_profiles.clone(),
        effect_runner,
        env,
        notify_tx,
        server_mode.to_string(),
    ));

    let app_state = http::AppState {
        token,
        sessions: Arc::new(http::session::SessionStore::new()),
        allowed_origins: Arc::new(http::allowed_origins(addr)),
        mux_store,
        ws_state,
        config: Arc::new(RwLock::new(config)),
        shell_profiles,
        server_config: Arc::new(RwLock::new(server_config)),
        template_store,
        themes_dir,
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: option_env!("SPLITTY_COMMIT")
            .unwrap_or("unknown")
            .to_string(),
        build_time: option_env!("SPLITTY_BUILD_TIME")
            .unwrap_or("unknown")
            .to_string(),
        listen_addr: addr.to_string(),
        server_mode: server_mode.to_string(),
    };

    (app_state, pty_manager)
}

fn main() {
    // Before a reinstall can move the binary away (see `cli_path`).
    splitty_core::pty::cli_path::remember_executable();
    // A session keeper is a few blocking threads that outlive the daemon, not
    // a runtime with a worker per core: it starts before one is built.
    #[cfg(unix)]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "session-keeper")
    {
        run_session_keeper();
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start the Tokio runtime")
        .block_on(async_main());
}

/// `splitty session-keeper` (internal): keep one daemon session's PTY and
/// program alive, talking to the daemon over the socket pair on standard
/// input. See `splitty_core::keeper`.
#[cfg(unix)]
fn run_session_keeper() -> ! {
    use std::os::fd::AsFd;

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    // Holding no directory busy that the daemon was started in.
    let _ = std::env::set_current_dir("/");
    let control = match std::io::stdin().as_fd().try_clone_to_owned() {
        Ok(fd) => std::os::unix::net::UnixStream::from(fd),
        Err(error) => {
            tracing::error!("session keeper without its connection: {error}");
            std::process::exit(1);
        }
    };
    match splitty_core::keeper::process::run(control, splitty_core::keeper::process::RETENTION) {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            tracing::error!("session keeper: {error}");
            std::process::exit(1);
        }
    }
}

async fn async_main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if error.exit_code() != 0 && std::env::args().any(|a| a == "--json") {
                println!(
                    "{}",
                    serde_json::json!({"code":"usage","error":error.to_string()})
                );
            } else {
                let _ = error.print();
            }
            std::process::exit(error.exit_code());
        }
    };
    if std::env::var_os("SPLITTY_AGENT_SOCK").is_some_and(|s| !s.is_empty()) {
        let allowed = match &cli.command {
            Commands::JobControl(command) => !command.is_admin(),
            Commands::Split { .. }
            | Commands::NewScreen { .. }
            | Commands::ClosePane { .. }
            | Commands::CloseScreen { .. }
            | Commands::RenameScreen { .. }
            | Commands::ListScreens { .. }
            | Commands::ListPanes { .. }
            | Commands::Status { .. }
            | Commands::SendKeys { .. } => true,
            _ => false,
        };
        if !allowed {
            let error = "administrative commands are unavailable under an agent grant";
            if std::env::args().any(|a| a == "--json") {
                println!(
                    "{}",
                    serde_json::json!({"code":"scope_denied","error":error})
                );
            } else {
                eprintln!("Error: {error}");
            }
            std::process::exit(2);
        }
    }

    if matches!(
        cli.command,
        Commands::Daemon {
            action: DaemonAction::Run { .. }
        } | Commands::Connect { json: true, .. }
            | Commands::RemoteHosts { .. }
            | Commands::JobControl(_)
            | Commands::Attach { .. }
    ) {
        // The daemon's stdout is its readiness handshake with `daemon start`;
        // its logs go to stderr, which `daemon start` points at daemon.log.
        // The stdout of `connect --json` and `remote-hosts` is JSON only, and
        // `attach` owns the terminal's stdout.
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt::init();
    }

    match cli.command {
        Commands::SessionKeeper => {
            // Dispatched in `main` before the runtime starts.
            #[cfg(unix)]
            run_session_keeper();
            #[cfg(not(unix))]
            {
                eprintln!("Error: session keepers need Unix");
                std::process::exit(1);
            }
        }
        Commands::JobControl(command) => {
            let status = cli::jobs::run(command).await;
            if status != 0 {
                std::process::exit(status);
            }
        }
        Commands::Serve {
            addr,
            token,
            oneshot_token,
            remote,
            open,
            desktop_stdio,
        } => {
            let token = resolve_token(token, oneshot_token);
            let bind_addr = resolve_loopback_bind(&addr, remote);
            let listener = tokio::net::TcpListener::bind(bind_addr).await.unwrap();
            let local_addr = listener.local_addr().unwrap();
            let (state, pty) = build_state(
                &local_addr.to_string(),
                token.clone(),
                "embedded",
                None,
                Arc::new(splitty_core::pty::terminal::LocalTerminals),
                &control::socket_path(),
            );
            // A single-use bootstrap code, not the process token
            // (security review SR-04). The launcher navigates to
            // this URL once; the page immediately exchanges the code for an
            // HttpOnly session cookie and clears the address bar. Capturing the
            // URL afterwards — from a log, a screenshot, a crash report — yields
            // a code that has already been spent.
            let bootstrap = state.sessions.issue_bootstrap();
            let sessions = state.sessions.clone();
            let jobs = state.ws_state.jobs.clone();
            let control_task = control::spawn(state.clone());
            let app = http::build_router_with_frontend(state);

            let url = format!("http://{local_addr}/?bootstrap={bootstrap}&desktop=1");

            // Machine-readable output
            emit_line!("SPLITTY_PORT={}", local_addr.port());
            emit_line!("SPLITTY_BOOTSTRAP={bootstrap}");
            emit_line!("SPLITTY_URL={url}");

            // No token or bootstrap fragment in the log line: this stream is
            // captured by the Windows launcher's debug logging and ends up in
            // support bundles.
            tracing::info!("splitty (serve) listening on {}", local_addr);

            if open {
                if let Err(e) = open::that(&url) {
                    tracing::warn!("failed to open browser: {e}");
                }
            }

            axum::serve(listener, app)
                .with_graceful_shutdown(serve_shutdown_signal(desktop_stdio, pty, sessions, jobs))
                .await
                .unwrap();

            control_task.shutdown();
            tracing::info!("server shut down");
        }
        Commands::Daemon { action } => {
            handle_daemon(action).await;
        }
        Commands::Connect {
            target,
            no_open,
            forward,
            json,
            ..
        } => {
            handle_connect(&target, &forward, !no_open, json).await;
        }
        Commands::RemoteHosts { .. } => {
            let ssh_dir = splitty_remote::hosts::default_ssh_dir();
            let known = splitty_remote::hosts::known(
                &splitty_core::config::config_dir(),
                ssh_dir.as_deref(),
            );
            match serde_json::to_string(&known) {
                Ok(line) => emit_line!("{line}"),
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }

        // ── CLI control commands ───────────────────────────────────────
        Commands::Open {
            ctl,
            path,
            editor,
            session,
        } => {
            handle_ctl(async {
                let session = session.filter(|s| !s.is_empty()).ok_or_else(|| {
                    cli::CliError::Usage(
                        "Run this command in a Splitty terminal, or supply --session <id>".into(),
                    )
                })?;
                let server = discover_server(&ctl)?;
                cli::editor::open(&server, &path, &session, editor.as_deref()).await
            })
            .await;
        }
        Commands::Download { ctl, path, session } => {
            handle_ctl(async {
                let session = session.filter(|s| !s.is_empty()).ok_or_else(|| {
                    cli::CliError::Usage(
                        "Run this command in a Splitty terminal, or supply --session <id>".into(),
                    )
                })?;
                let server = discover_server(&ctl)?;
                cli::download::download(&server, &path, &session).await
            })
            .await;
        }
        Commands::Split {
            ctl,
            horizontal,
            pane,
            screen,
        } => {
            handle_ctl(async {
                let server = discover_server(&ctl)?;
                let screen_id = resolve_screen(&screen);
                let pane_id = resolve_pane(&pane);
                let axis = if horizontal { "horizontal" } else { "vertical" };
                cli::send_mux_op(
                    &server,
                    &screen_id,
                    serde_json::json!({
                        "type": "split_pane",
                        "paneId": pane_id,
                        "axis": axis,
                    }),
                )
                .await?;
                println!("Split {} ({})", pane_id, axis);
                Ok(())
            })
            .await;
        }

        Commands::NewScreen { ctl, name } => {
            handle_ctl(async {
                let server = discover_server(&ctl)?;
                cli::send_mux_op(
                    &server,
                    "",
                    serde_json::json!({
                        "type": "create_screen",
                        "name": name,
                    }),
                )
                .await?;
                println!(
                    "Screen created{}",
                    name.as_ref()
                        .map(|n| format!(" ({})", n))
                        .unwrap_or_default()
                );
                Ok(())
            })
            .await;
        }

        Commands::ClosePane { ctl, pane, screen } => {
            handle_ctl(async {
                let server = discover_server(&ctl)?;
                let screen_id = resolve_screen(&screen);
                let pane_id = resolve_pane(&pane);
                cli::send_mux_op(
                    &server,
                    &screen_id,
                    serde_json::json!({
                        "type": "close_pane",
                        "paneId": pane_id,
                    }),
                )
                .await?;
                println!("Closed pane {}", pane_id);
                Ok(())
            })
            .await;
        }

        Commands::CloseScreen { ctl, screen } => {
            handle_ctl(async {
                let server = discover_server(&ctl)?;
                let screen_id = resolve_screen(&screen);
                cli::send_mux_op(
                    &server,
                    &screen_id,
                    serde_json::json!({
                        "type": "close_screen",
                    }),
                )
                .await?;
                println!("Closed screen {}", screen_id);
                Ok(())
            })
            .await;
        }

        Commands::RenameScreen { ctl, name, screen } => {
            handle_ctl(async {
                let server = discover_server(&ctl)?;
                let screen_id = resolve_screen(&screen);
                cli::send_mux_op(
                    &server,
                    &screen_id,
                    serde_json::json!({
                        "type": "rename_screen",
                        "name": name,
                    }),
                )
                .await?;
                println!("Renamed screen {} to \"{}\"", screen_id, name);
                Ok(())
            })
            .await;
        }

        Commands::ListScreens { ctl, json } => {
            handle_ctl(async {
                let server = discover_server(&ctl)?;
                let screens = cli::api_get(&server, "/api/mux/screens").await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&screens).unwrap());
                } else {
                    if let Some(arr) = screens.as_array() {
                        for s in arr {
                            let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                            let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                            let panes = s.get("paneCount").and_then(|v| v.as_u64()).unwrap_or(0);
                            let active = s.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
                            let marker = if active { " *" } else { "" };
                            println!("{}{}\t{}\t{} pane(s)", id, marker, name, panes);
                        }
                    }
                }
                Ok(())
            })
            .await;
        }

        Commands::ListPanes { ctl, screen, json } => {
            handle_ctl(async {
                let server = discover_server(&ctl)?;
                let screen_id = resolve_screen(&screen);
                let screen_data =
                    cli::api_get(&server, &format!("/api/mux/screen/{}", screen_id)).await?;
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &screen_data.get("panes").unwrap_or(&serde_json::json!({}))
                        )
                        .unwrap()
                    );
                } else {
                    if let Some(panes) = screen_data.get("panes").and_then(|v| v.as_object()) {
                        for (id, pane) in panes {
                            let title = pane.get("title").and_then(|v| v.as_str()).unwrap_or("-");
                            let cwd = pane.get("cwd").and_then(|v| v.as_str()).unwrap_or("-");
                            let session = pane
                                .get("sessionId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("-");
                            println!("{}\t{}\t{}\tsession={}", id, title, cwd, session);
                        }
                    }
                }
                Ok(())
            })
            .await;
        }

        Commands::Status { ctl, json } => {
            handle_ctl(async {
                let server = discover_server(&ctl)?;
                let status = cli::api_get(&server, "/api/mux/status").await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&status).unwrap());
                } else {
                    println!("Server:   {}", describe_transport(&server.transport));
                    println!("Source:   {}", server.source);
                    println!(
                        "Mode:     {}",
                        status
                            .get("serverMode")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                    );
                    println!(
                        "Version:  {}",
                        status
                            .get("version")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                    );
                    println!(
                        "Screens:  {}",
                        status.get("screens").and_then(|v| v.as_u64()).unwrap_or(0)
                    );
                    println!(
                        "Sessions: {}",
                        status.get("sessions").and_then(|v| v.as_u64()).unwrap_or(0)
                    );
                }
                Ok(())
            })
            .await;
        }

        Commands::RemoteHandshake {
            ensure_daemon,
            port,
            ..
        } => {
            let ensure = port.filter(|_| ensure_daemon);
            match splitty_server::host::handshake::run(ensure).await {
                Ok(handshake) => match serde_json::to_string(&handshake) {
                    Ok(line) => emit_line!("{line}"),
                    Err(e) => {
                        eprintln!("Failed to encode the handshake: {e}");
                        std::process::exit(1);
                    }
                },
                Err(reason) => {
                    eprintln!("{reason}");
                    std::process::exit(1);
                }
            }
        }
        Commands::RemoteBridge => {
            if let Err(reason) = splitty_server::host::bridge::run().await {
                eprintln!("{reason}");
                std::process::exit(1);
            }
        }
        Commands::IssueBootstrap { ctl } => {
            handle_ctl(async {
                // Reaches the server over TCP with the process token rather than
                // through the control socket. Minting a browser session is a
                // privileged act, and the token lives in a 0600 file the user
                // owns — a process running inside a pane has no way to get it.
                let addr = match ctl.addr.clone() {
                    Some(addr) => addr,
                    None => {
                        let info = splitty_core::daemon::read_info()
                            .filter(|info| splitty_core::daemon::is_running(info.pid))
                            .ok_or(cli::CliError::NoServer)?;
                        format!("127.0.0.1:{}", info.port)
                    }
                };
                let token = match ctl.token.clone() {
                    Some(token) => token,
                    None => splitty_core::auth::load_or_create()
                        .map_err(|e| cli::CliError::Request(e.to_string()))?,
                };

                let server = cli::ServerInfo {
                    transport: cli::Transport::Tcp { addr, token },
                    source: "token file",
                };
                let response =
                    cli::api_post(&server, "/api/bootstrap", serde_json::json!({})).await?;
                let code = response
                    .get("bootstrap")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        cli::CliError::Request("server returned no bootstrap code".into())
                    })?;

                println!("SPLITTY_BOOTSTRAP={code}");
                Ok(())
            })
            .await;
        }

        Commands::Attach {
            ctl,
            target,
            detach_key,
        } => {
            handle_ctl(async {
                let outcome = cli::attach::run(cli::attach::Options {
                    target,
                    detach_key,
                    addr: ctl.addr,
                    token: ctl.token,
                })
                .await?;
                if let Some(note) = outcome.note() {
                    eprint!("\r\n{note}\n");
                }
                Ok(())
            })
            .await;
        }

        Commands::SendKeys {
            ctl,
            keys,
            pane,
            screen,
        } => {
            handle_ctl(async {
                let server = discover_server(&ctl)?;
                let screen_id = resolve_screen(&screen);
                let pane_id = resolve_pane(&pane);

                // Resolve session ID for this pane
                let screen_data =
                    cli::api_get(&server, &format!("/api/mux/screen/{}", screen_id)).await?;
                let session_id = screen_data
                    .get("panes")
                    .and_then(|p| p.get(&pane_id))
                    .and_then(|p| p.get("sessionId"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        cli::CliError::Api(404, format!("no session for pane {}", pane_id))
                    })?;

                // Unescape common sequences
                let data = keys
                    .replace("\\n", "\n")
                    .replace("\\t", "\t")
                    .replace("\\r", "\r");
                cli::api_post(
                    &server,
                    &format!("/api/session/{}/send", session_id),
                    serde_json::json!({
                        "data": data,
                    }),
                )
                .await?;
                Ok(())
            })
            .await;
        }
    }
}

fn discover_server(ctl: &CtlOpts) -> Result<cli::ServerInfo, cli::CliError> {
    cli::discover(ctl.addr.as_deref(), ctl.token.as_deref())
}

fn resolve_screen(flag: &Option<String>) -> String {
    flag.clone()
        .or_else(|| std::env::var("SPLITTY_SCREEN_ID").ok())
        .unwrap_or_default()
}

fn resolve_pane(flag: &Option<String>) -> String {
    flag.clone()
        .or_else(|| std::env::var("SPLITTY_PANE_ID").ok())
        .unwrap_or_default()
}

async fn handle_ctl(fut: impl std::future::Future<Output = Result<(), cli::CliError>>) {
    if let Err(e) = fut.await {
        eprintln!("Error: {e}");
        std::process::exit(if matches!(e, cli::CliError::Usage(_)) {
            2
        } else {
            1
        });
    }
}

/// Ctrl+C, and SIGTERM on Unix (`kill`, a service manager): either ends
/// `serve` the same way, so the hub's forwards and bridges end with it instead
/// of outliving it and holding their local ports.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.expect("failed to install Ctrl+C handler");
                tracing::info!("received Ctrl+C, shutting down...");
            }
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM, shutting down...");
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
        tracing::info!("received Ctrl+C, shutting down...");
    }
}

async fn serve_shutdown_signal(
    desktop_stdio: bool,
    pty: Arc<splitty_core::pty::manager::Manager>,
    sessions: Arc<http::session::SessionStore>,
    jobs: Arc<splitty_server::jobs::Jobs>,
) {
    if !desktop_stdio {
        shutdown_signal().await;
        jobs.shutdown().await;
        pty.close_all();
        return;
    }

    tokio::select! {
        _ = shutdown_signal() => {}
        result = wait_for_eof(tokio::io::stdin(), Some(sessions)) => {
            if let Err(error) = result {
                tracing::warn!("desktop stdin failed: {error}");
            } else {
                tracing::info!("desktop bridge closed stdin, shutting down...");
            }
        }
    }
    jobs.shutdown().await;
    pty.close_all();
}

/// Serve the native launcher's control channel until it closes stdin.
///
/// Originally this only waited for EOF. It now also answers one request:
/// a line reading `bootstrap` mints a fresh single-use code and writes
/// `SPLITTY_BOOTSTRAP=<code>` back on stdout.
///
/// That request exists because of SR-03: `wsl_start_backend` used to hand the
/// frontend the *stored* launch URL whenever the backend was already running,
/// which meant a reusable credential was returned to any caller that could
/// invoke the command. Codes are single-use now, so a stored URL is worthless
/// on re-entry — the launcher has to ask for a new one, and this is the channel
/// it asks on. It is inherently trustworthy: only the process that spawned the
/// backend holds the write end of its stdin.
async fn wait_for_eof<R>(
    mut reader: R,
    sessions: Option<Arc<http::session::SessionStore>>,
) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut buffer = [0_u8; 256];
    let mut pending = String::new();

    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }

        let Some(sessions) = sessions.as_ref() else {
            continue;
        };

        pending.push_str(&String::from_utf8_lossy(&buffer[..read]));
        while let Some(newline) = pending.find('\n') {
            let line: String = pending.drain(..=newline).collect();
            if line.trim() == "bootstrap" {
                emit_line!("SPLITTY_BOOTSTRAP={}", sessions.issue_bootstrap());
            }
        }

        // Bound the buffer so a launcher that never sends a newline cannot grow
        // it without limit.
        if pending.len() > 4096 {
            pending.clear();
        }
    }
}

async fn handle_daemon(action: DaemonAction) {
    use splitty_core::daemon;

    match action {
        DaemonAction::Start {
            addr,
            token,
            remote,
        } => {
            // Check if daemon is already running
            if let Some(info) = daemon::read_info() {
                if daemon::is_running(info.pid) {
                    println!(
                        "Daemon already running (pid={}, port={})",
                        info.pid, info.port
                    );
                    return;
                }
                // Stale info file — clean up
                daemon::remove_info();
            }
            bind_and_start_daemon(&addr, token, remote).await;
        }
        DaemonAction::Restart { addr, end_sessions } => {
            let stopped = stop_daemon();
            if end_sessions {
                end_kept_sessions();
            }
            let addr = addr
                .or_else(|| stopped.map(|port| format!("127.0.0.1:{port}")))
                .or_else(|| std::env::var("SPLITTY_ADDR").ok())
                .unwrap_or_else(|| "127.0.0.1:9800".to_owned());
            bind_and_start_daemon(&addr, None, false).await;
        }
        DaemonAction::Run { listen_fd, token } => {
            #[cfg(unix)]
            {
                run_daemon(listen_fd, token).await;
            }

            #[cfg(not(unix))]
            {
                let _ = (listen_fd, token);
                eprintln!("Daemon mode is only supported on Unix");
                std::process::exit(1);
            }
        }
        DaemonAction::Stop { end_sessions } => {
            stop_daemon();
            if end_sessions {
                end_kept_sessions();
            }
        }
        DaemonAction::Status { json } => {
            let info = daemon::read_info();
            let stale = info
                .as_ref()
                .is_some_and(|info| !daemon::is_running(info.pid));
            if stale {
                daemon::remove_info();
            }
            let running = info.filter(|_| !stale);

            if json {
                match serde_json::to_string(&daemon::DaemonStatus::of(running)) {
                    Ok(line) => println!("{line}"),
                    Err(e) => {
                        eprintln!("Failed to encode the daemon status: {e}");
                        std::process::exit(1);
                    }
                }
                return;
            }

            let Some(info) = running else {
                if stale {
                    println!("No daemon running (stale info file cleaned up)");
                } else {
                    println!("No daemon running");
                }
                #[cfg(unix)]
                {
                    let kept = splitty_core::keeper::client::find(
                        &splitty_core::keeper::dir::keepers_dir(),
                    )
                    .len();
                    if kept > 0 {
                        println!(
                            "{kept} session(s) kept: `splitty daemon start` adopts them, `splitty attach` shows one"
                        );
                    }
                }
                return;
            };

            println!("Daemon running:");
            println!("  PID:     {}", info.pid);
            println!("  Port:    {}", info.port);
            println!("  Started: {}", info.started_at);
            println!("  Version: {}", info.version);
        }
    }
}

/// Bind `addr` in the foreground and start a daemon on it: a busy port fails
/// here and leaves no background process behind. The loopback invariant is
/// enforced here too: the daemon is the long-lived, most likely to be
/// misconfigured entry point.
async fn bind_and_start_daemon(addr: &str, token: Option<String>, remote: bool) {
    let bind_addr = resolve_loopback_bind(addr, remote);
    let std_listener = std::net::TcpListener::bind(bind_addr).unwrap_or_else(|e| {
        eprintln!("Failed to bind {addr}: {e}");
        std::process::exit(1);
    });

    #[cfg(unix)]
    {
        start_daemon(std_listener, token).await;
    }

    #[cfg(not(unix))]
    {
        let _ = (std_listener, token);
        eprintln!("Daemon mode is only supported on Unix");
        std::process::exit(1);
    }
}

/// Ask the running daemon to stop, wait up to about two seconds and say how it
/// went. The port of a daemon that stopped, so a restart can take it again.
fn stop_daemon() -> Option<u16> {
    use splitty_core::daemon;

    let Some(info) = daemon::read_info() else {
        println!("No daemon running");
        return None;
    };

    if !daemon::is_running(info.pid) {
        println!("No daemon running (stale info file cleaned up)");
        daemon::remove_info();
        return None;
    }

    #[cfg(unix)]
    {
        use nix::sys::signal;
        use nix::unistd::Pid;
        let Ok(pid) = i32::try_from(info.pid) else {
            eprintln!("Daemon pid {} is out of range", info.pid);
            return None;
        };
        if let Err(e) = signal::kill(Pid::from_raw(pid), signal::Signal::SIGTERM) {
            eprintln!("Failed to send SIGTERM to pid {}: {e}", info.pid);
            return None;
        }

        // Wait briefly for process to exit
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !daemon::is_running(info.pid) {
                println!("Daemon stopped (pid={})", info.pid);
                return Some(info.port);
            }
        }
        println!(
            "Sent SIGTERM to pid {} (may still be shutting down)",
            info.pid
        );
        None
    }

    #[cfg(not(unix))]
    {
        eprintln!("Daemon mode is only supported on Unix");
        None
    }
}

/// End every kept session of this configuration directory: take each keeper
/// over and close its session, then wait up to 5 seconds for them to go.
fn end_kept_sessions() {
    #[cfg(unix)]
    {
        use splitty_core::keeper;

        let mut ending = Vec::new();
        for found in keeper::client::find(&keeper::dir::keepers_dir()) {
            let socket = found.socket.clone();
            let session = found.hello.session_id.clone();
            match found.end() {
                Ok(()) => ending.push(socket),
                Err(e) => eprintln!("Cannot end kept session {session}: {e}"),
            }
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while ending.iter().any(|socket| socket.exists()) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let still = ending.iter().filter(|socket| socket.exists()).count();
        println!("Ended {} kept session(s)", ending.len() - still);
        if still > 0 {
            println!("{still} kept session(s) are still ending");
        }
    }

    #[cfg(not(unix))]
    println!("Ended 0 kept session(s)");
}

/// Start the daemon as a fresh process of this binary and wait until it serves.
///
/// The daemon used to be forked from this process. That copies a process that
/// already runs a multi-threaded Tokio runtime: only the forking thread
/// survives, locks held by the other threads stay locked, and the child still
/// believed it was inside the runtime, so it panicked on its first `block_on`
/// while this command reported success. A spawned `daemon run` starts from a
/// clean process image and inherits nothing but the bound listener.
#[cfg(unix)]
async fn start_daemon(listener: std::net::TcpListener, token: Option<String>) {
    use splitty_core::daemon;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;

    let port = listener.local_addr().unwrap().port();
    let config_dir = splitty_core::config::config_dir();
    let log_path = config_dir.join("daemon.log");

    let log = splitty_core::config::ensure_private_dir(&config_dir).and_then(|()| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&log_path)
    });
    let log = log
        .unwrap_or_else(|e| daemon_start_failed(&format!("cannot open the log: {e}"), &log_path));
    let exe = std::env::current_exe().unwrap_or_else(|e| {
        daemon_start_failed(&format!("cannot locate the splitty binary: {e}"), &log_path)
    });

    let listen_fd = listener.as_raw_fd();
    let mut command = std::process::Command::new(exe);
    command.args(["daemon", "run", "--listen-fd", &listen_fd.to_string()]);
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        // Visible in the process list for the daemon's lifetime, as it was
        // when the daemon was forked and kept this command's arguments.
        command.args(["--token", &token]);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(log);
    // SAFETY: the closure runs in the forked child before exec, where only
    // async-signal-safe calls are allowed; `setsid` and `fcntl` are plain
    // system calls.
    unsafe {
        command.pre_exec(move || {
            // A session of its own: closing the terminal this command runs in
            // must not hang up the daemon.
            nix::unistd::setsid()?;
            // The one descriptor the daemon inherits on purpose. Cleared in
            // this child only, so nothing else started from here gets it.
            nix::fcntl::fcntl(
                listen_fd,
                nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::empty()),
            )?;
            Ok(())
        });
    }

    let mut child = tokio::process::Command::from(command)
        .spawn()
        .unwrap_or_else(|e| {
            daemon_start_failed(&format!("cannot start the daemon process: {e}"), &log_path)
        });
    // The daemon holds its own copy of the listener now.
    drop(listener);

    let child_pid = child.id();
    let stdout = child.stdout.take().expect("daemon stdout is piped");
    let reason =
        match tokio::time::timeout(DAEMON_READY_TIMEOUT, wait_for_daemon_ready(stdout)).await {
            Ok(Ok(())) => {
                // Printed only now, so a launcher that reads it can rely on the
                // daemon serving. No credential here (security review SR-04):
                // local clients authorise through the control socket, and a
                // deliberate remote client uses `--addr` with an explicitly
                // supplied token.
                emit_line!("SPLITTY_PORT={port}");
                return;
            }
            Ok(Err(reason)) => reason,
            Err(_) => format!(
                "the daemon did not report ready within {} s",
                DAEMON_READY_TIMEOUT.as_secs()
            ),
        };

    // Exited or hung: either way it must neither linger unregistered nor stay
    // registered without serving.
    let _ = child.kill().await;
    if daemon::read_info().is_some_and(|info| Some(info.pid) == child_pid) {
        daemon::remove_info();
    }
    daemon_start_failed(&reason, &log_path);
}

#[cfg(unix)]
fn daemon_start_failed(reason: &str, log_path: &std::path::Path) -> ! {
    eprintln!(
        "Failed to start daemon: {reason} (see {})",
        log_path.display()
    );
    std::process::exit(1);
}

/// Read the daemon's handshake: `Ok` on its ready line, `Err` with the reason
/// on its error line or when it exits without either.
#[cfg(unix)]
async fn wait_for_daemon_ready(stdout: tokio::process::ChildStdout) -> Result<(), String> {
    use tokio::io::AsyncBufReadExt;

    let mut lines = tokio::io::BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.starts_with(DAEMON_READY) {
                    return Ok(());
                }
                if let Some(reason) = line.strip_prefix(DAEMON_ERROR) {
                    return Err(reason.trim().to_string());
                }
            }
            Ok(None) => return Err("the daemon exited during startup".to_string()),
            Err(e) => return Err(format!("cannot read the daemon's startup output: {e}")),
        }
    }
}

/// The daemon process: `daemon run`, started by [`start_daemon`].
///
/// Its stdout is the handshake with `daemon start` and nothing else: one
/// `SPLITTY_DAEMON_READY pid=<pid>` line once the control socket is bound or
/// skipped and `daemon.json` is written, or one `SPLITTY_DAEMON_ERROR <reason>`
/// line before it gives up. `daemon.json` is written only after every step that
/// can fail, so it never names a daemon that is not about to serve.
#[cfg(unix)]
async fn run_daemon(listen_fd: i32, token: Option<String>) {
    use splitty_core::daemon;

    tracing::info!("daemon started (pid={})", std::process::id());

    let (listener, local_addr) = adopt_listener(listen_fd).unwrap_or_else(|e| {
        daemon_run_failed(&format!(
            "cannot use the inherited listener (fd {listen_fd}): {e}"
        ))
    });

    let token = resolve_token(token, false);

    // Sessions get `bin/` of the run directory once its `splitty` names this
    // daemon's CLI, so shells that outlive an update find the new one.
    if let Err(error) = splitty_core::pty::cli_path::publish_stable_cli() {
        tracing::warn!(%error, "cannot point the stable CLI link at this daemon; sessions get the CLI's own directory");
    }

    // The keepers of an earlier daemon: their sessions are taken over, not
    // started again (`session-keepers`).
    let keepers_dir = splitty_core::keeper::dir::keepers_dir();
    let adoptions: Vec<_> = splitty_core::keeper::client::find(&keepers_dir)
        .into_iter()
        .filter_map(|found| {
            let session = found.hello.session_id.clone();
            match found.take_over() {
                Ok(adoption) => Some(adoption),
                Err(e) => {
                    tracing::warn!(%session, "cannot take over a keeper: {e}");
                    None
                }
            }
        })
        .collect();
    let adopted: std::collections::HashSet<String> = adoptions
        .iter()
        .map(|adoption| adoption.hello.session_id.clone())
        .collect();

    // Load persisted state if available
    let mut initial_state = daemon::load_state();
    if let Some(state) = initial_state.as_mut() {
        tracing::info!(
            "restoring workspace state from {}",
            daemon::state_path().display()
        );
        daemon::restore_jobs(state, &adopted);
    }

    let control_path = control::daemon_socket_path();
    let terminals: Arc<dyn splitty_core::pty::terminal::TerminalFactory> =
        match std::env::current_exe() {
            Ok(executable) => Arc::new(splitty_core::keeper::client::Keepers::new(
                keepers_dir,
                Arc::new(splitty_core::keeper::client::ProcessLauncher::new(
                    executable,
                )),
            )),
            Err(e) => {
                tracing::warn!("sessions run in the daemon and end with it: {e}");
                Arc::new(splitty_core::pty::terminal::LocalTerminals)
            }
        };
    let (state, pty_manager) = build_state(
        &local_addr.to_string(),
        token,
        "daemon",
        initial_state.clone(),
        terminals,
        &control_path,
    );

    // Sessions on the adopted keepers, with their buffered output, before any
    // client can attach. A keeper the workspace does not know becomes a
    // session without a pane.
    if !adoptions.is_empty() {
        tracing::info!("adopting {} kept sessions", adoptions.len());
    }
    for adoption in adoptions {
        let session = adoption.hello.session_id.clone();
        let managed = adoption.hello.managed;
        let adopted = pty_manager.adopt(&session, managed, |feed| {
            adoption
                .run(feed)
                .map(|terminal| {
                    Box::new(terminal) as Box<dyn splitty_core::pty::terminal::Terminal>
                })
                .map_err(|e| splitty_core::pty::PtyError::Other(e.to_string()))
        });
        if let Err(e) = adopted {
            tracing::warn!(%session, "cannot adopt a kept session: {e}");
        }
    }
    state.ws_state.jobs.resume_adopted();

    // The daemon is the case where the control socket matters most: it is
    // the long-lived server the in-shell CLI discovers through daemon.json.
    let control_task = control::spawn_at(state.clone(), control_path.clone());

    let info = daemon::DaemonInfo {
        pid: std::process::id(),
        port: local_addr.port(),
        // Empty when the socket could not be bound: discovery skips an empty
        // path instead of failing on one that does not exist.
        socket: control_task
            .path()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        started_at: chrono_now(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol: splitty_core::protocol::PROTOCOL_VERSION,
        keeps_sessions: true,
    };
    if let Err(e) = daemon::write_info(&info) {
        control_task.shutdown();
        daemon_run_failed(&format!(
            "cannot write {}: {e}",
            daemon::info_path().display()
        ));
    }

    // Restore the sessions no keeper kept from persisted state
    if let Some(ref ws) = initial_state {
        let effects = daemon::restore_effects(ws, |id| pty_manager.get(id).is_some());
        if !effects.is_empty() {
            tracing::info!("restoring {} sessions from persisted state", effects.len());
            let runner = splitty_core::mux::effect_runner::EffectRunner::new(Arc::clone(
                &pty_manager,
            )
                as Arc<dyn splitty_core::pty::Backend>);
            let env = splitty_core::mux::effect_runner::EnvContext {
                control_socket: control_path.to_string_lossy().into_owned(),
            };
            let errors = runner.execute(effects, &env).await;
            for e in errors
                .iter()
                .filter_map(|outcome| outcome.result.as_ref().err())
            {
                tracing::warn!("session restore error: {e}");
            }
        }
    }

    let mux_store = state.mux_store.clone();
    let jobs = state.ws_state.jobs.clone();
    let app = http::build_router_with_frontend(state);

    emit_line!("{DAEMON_READY} pid={}", std::process::id());
    // `daemon start` stops reading after that line. Anything written to stdout
    // later goes to the log rather than into a pipe nobody reads.
    let _ = nix::unistd::dup2(2, 1);
    tracing::info!("splitty (daemon) listening on {local_addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            daemon_shutdown_signal().await;
            jobs.release().await;
        })
        .await
        .unwrap();

    // Persist final state before exit
    let final_state = mux_store.get_workspace_state();
    if let Err(e) = daemon::save_state(&final_state) {
        tracing::warn!("failed to persist final state: {e}");
    }
    // Let go of every session now: the keepers keep the programs running for
    // the next daemon, and a session that runs in this process hangs up. Left
    // to the runtime's teardown, a session still referenced by a client
    // connection kept its shell alive, and the runtime waited for it forever.
    pty_manager.release_all();
    daemon::remove_info();
    control_task.shutdown();
    tracing::info!("daemon shut down");
}

/// Tell `daemon start` why the daemon cannot start, and exit.
#[cfg(unix)]
fn daemon_run_failed(reason: &str) -> ! {
    tracing::error!("daemon failed to start: {reason}");
    emit_line!("{DAEMON_ERROR} {}", reason.replace('\n', " "));
    std::process::exit(1);
}

/// Take over the listening socket `daemon start` bound and passed down.
///
/// `daemon run` can also be started by hand with any descriptor, so it is
/// checked before ownership is taken: it must be an open, listening TCP socket
/// on loopback, the same invariant `resolve_loopback_bind` enforces.
#[cfg(unix)]
fn adopt_listener(fd: i32) -> std::io::Result<(tokio::net::TcpListener, std::net::SocketAddr)> {
    use std::io::{Error, ErrorKind};
    use std::os::fd::{BorrowedFd, FromRawFd};

    use nix::fcntl::{fcntl, FcntlArg, FdFlag};
    use nix::sys::socket::{getsockopt, sockopt};

    if fd < 3 {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "not a listening socket",
        ));
    }
    // Fails with EBADF unless the descriptor is open.
    fcntl(fd, FcntlArg::F_GETFD)?;
    // SAFETY: open, as just checked, and borrowed for this call only.
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let listening = match getsockopt(&borrowed, sockopt::AcceptConn) {
        Ok(listening) => listening,
        // macOS answers SO_ACCEPTCONN with ENOPROTOOPT. There a stream socket
        // on which listen() succeeds is taken: listen() leaves a listening
        // socket listening and fails on a connected one.
        Err(nix::errno::Errno::ENOPROTOOPT) => {
            getsockopt(&borrowed, sockopt::SockType)? == nix::sys::socket::SockType::Stream
                && nix::sys::socket::listen(&borrowed, nix::sys::socket::Backlog::MAXCONN).is_ok()
        }
        Err(error) => return Err(error.into()),
    };
    if !listening {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "not a listening socket",
        ));
    }
    // Shells started in panes must not inherit it: a listener that outlives
    // the daemon would keep its port bound.
    fcntl(fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;

    // SAFETY: an open listening socket that nothing else in this process owns.
    let listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
    let local_addr = listener.local_addr()?;
    if !local_addr.ip().is_loopback() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("refusing to serve on {local_addr}: Splitty listens on loopback only"),
        ));
    }
    listener.set_nonblocking(true)?;
    Ok((tokio::net::TcpListener::from_std(listener)?, local_addr))
}

/// Shutdown signal handler for daemon mode (handles both SIGTERM and SIGINT).
#[cfg(unix)]
async fn daemon_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");

    tokio::select! {
        _ = sigterm.recv() => {
            tracing::info!("received SIGTERM, shutting down...");
        }
        _ = sigint.recv() => {
            tracing::info!("received SIGINT, shutting down...");
        }
    }
}

/// Ctrl+C, and SIGTERM on Unix: the ways a user ends `splitty connect`.
///
/// The handlers are installed when this is called, before the first `ssh`
/// starts, so a signal during the connect is caught too and the `ssh` process
/// is ended rather than left behind.
fn connect_shutdown() -> impl std::future::Future<Output = ()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let terminate = signal(SignalKind::terminate());
        let interrupt = signal(SignalKind::interrupt());
        async move {
            match (terminate, interrupt) {
                (Ok(mut terminate), Ok(mut interrupt)) => {
                    tokio::select! {
                        _ = terminate.recv() => {}
                        _ = interrupt.recv() => {}
                    }
                }
                // Without handlers the default action ends the process.
                _ => std::future::pending().await,
            }
        }
    }
    #[cfg(not(unix))]
    {
        async {
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}

/// Print one line of `connect`: the event as JSON (`--json`), or its text.
fn report_connect(json: bool, host: &str, event: &splitty_remote::tunnel::ConnectEvent) {
    use splitty_remote::tunnel::ConnectEvent;

    if json {
        match serde_json::to_string(event) {
            Ok(line) => emit_line!("{line}"),
            Err(error) => eprintln!("error: {error}"),
        }
        return;
    }
    match event {
        ConnectEvent::Progress { message, .. } => println!("{message}"),
        ConnectEvent::Notice { message } => println!("Note: {message}"),
        ConnectEvent::Ready { url, .. } => emit_line!("SPLITTY_URL={url}"),
        ConnectEvent::Forward {
            remote,
            local,
            url,
            asked,
        } => {
            if local == asked {
                println!("Forwarding {url} -> {host}:{remote}");
            } else {
                println!("Forwarding {url} -> {host}:{remote} (local port {asked} is not free)");
            }
        }
        ConnectEvent::Lost { message } => eprintln!("{message}"),
        ConnectEvent::Retry { message, retry_in } => {
            eprintln!("{message}\nRetrying in {retry_in} s...")
        }
        ConnectEvent::Restored => println!("SSH tunnel restored."),
        ConnectEvent::Url { url } => {
            println!("The daemon on {host} was restarted or moved; it needs a new session:");
            emit_line!("SPLITTY_URL={url}");
            println!("SSH tunnel restored.");
        }
        ConnectEvent::Failed { message, hint } => {
            eprintln!("{message}");
            if let Some(hint) = hint {
                eprintln!("{hint}");
            }
        }
        ConnectEvent::Closed => println!("Closing tunnel..."),
    }
}

/// Refuse a command line `connect` cannot use, before any `ssh` runs: exit
/// with status 2, after a `failed` event with `--json`.
fn connect_rejected(json: bool, message: String) -> ! {
    if json {
        let failed = splitty_remote::tunnel::ConnectEvent::Failed {
            message,
            hint: None,
        };
        report_connect(json, "", &failed);
    } else {
        eprintln!("error: {message}");
    }
    std::process::exit(2);
}

/// Print what the user needs to act on a failed connect, and exit with 1.
fn connect_failed(json: bool, host: &str, error: &splitty_remote::tunnel::RemoteError) -> ! {
    report_connect(json, host, &error.failure().into());
    std::process::exit(1);
}

fn open_in_browser(json: bool, host: &str, url: &str) {
    if let Err(e) = open::that(url) {
        let message = format!("Failed to open browser: {e}");
        if json {
            let notice = splitty_remote::tunnel::ConnectEvent::Notice { message };
            report_connect(json, host, &notice);
        } else {
            eprintln!("{message}");
        }
    }
}

async fn handle_connect(target: &str, forward: &[String], open: bool, json: bool) {
    use splitty_remote::tunnel::{self, ConnectEvent};
    use splitty_remote::{hosts, install, ssh};

    let target =
        ssh::SshTarget::parse(target).unwrap_or_else(|e| connect_rejected(json, e.to_string()));
    let forwards =
        tunnel::parse_forwards(forward).unwrap_or_else(|e| connect_rejected(json, e.to_string()));
    let host = target.as_str().to_owned();
    let report = |event: ConnectEvent| report_connect(json, &host, &event);
    let mut shutdown = std::pin::pin!(connect_shutdown());

    // The host gets this client's own server when it lacks it; then every
    // command runs that server by its absolute path. Dropping the flow on
    // Ctrl+C kills any ssh it started.
    let flow = tunnel::open_forwarding(
        ssh::Ssh::new(target.clone()),
        env!("CARGO_PKG_VERSION"),
        install::payload_for,
        forwards,
        |progress| report(progress.into()),
    );
    let (remote, connected) = tokio::select! {
        result = flow => match result {
            Ok(opened) => opened,
            Err(error) => connect_failed(json, &host, &error),
        },
        () = &mut shutdown => {
            report(ConnectEvent::Closed);
            return;
        }
    };

    // Before the URL is printed, so whoever reads it finds the host recorded.
    if let Err(error) = hosts::record_recent(&splitty_core::config::config_dir(), &target) {
        let message = format!("Could not record {host} as a recent host: {error}");
        if json {
            report(ConnectEvent::Notice { message });
        } else {
            eprintln!("warning: {message}");
        }
    }
    report(ConnectEvent::Ready {
        url: connected.url.clone(),
        port: connected.daemon.port,
    });
    for forward in &connected.forwards {
        report((*forward).into());
    }
    if !json {
        println!("SSH tunnel active. Press Ctrl+C to disconnect.");
    }
    if open {
        open_in_browser(json, &host, &connected.url);
    }

    let on_event = |event: tunnel::Event| {
        let event = ConnectEvent::from_event(&host, event);
        report_connect(json, &host, &event);
        if let (ConnectEvent::Url { url }, true) = (&event, open) {
            open_in_browser(json, &host, url);
        }
    };
    let supervised = tunnel::supervise(
        &remote,
        connected,
        tunnel::GIVE_UP_AFTER,
        &mut shutdown,
        on_event,
    );
    match supervised.await {
        Ok(()) => report(ConnectEvent::Closed),
        Err(error) => connect_failed(json, &host, &error),
    }
}

/// Simple ISO 8601 timestamp without external dependencies.
#[cfg(unix)]
fn chrono_now() -> String {
    use std::time::SystemTime;
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::wait_for_eof;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn desktop_stdio_waits_until_eof() {
        let (mut writer, reader) = tokio::io::duplex(32);
        let waiter = tokio::spawn(wait_for_eof(reader, None));

        writer.write_all(b"keepalive").await.unwrap();
        assert!(!waiter.is_finished());
        drop(writer);

        waiter.await.unwrap().unwrap();
    }

    /// The launcher can ask for a fresh bootstrap code over the same channel.
    ///
    /// This is what lets a re-entry into an already-running backend get a usable
    /// credential without the launcher ever holding a reusable one (SR-03/SR-04).
    #[tokio::test]
    async fn desktop_stdio_issues_a_fresh_bootstrap_on_request() {
        let sessions = Arc::new(crate::http::session::SessionStore::new());
        let (mut writer, reader) = tokio::io::duplex(64);
        let waiter = tokio::spawn(wait_for_eof(reader, Some(sessions.clone())));

        writer.write_all(b"bootstrap\n").await.unwrap();

        // Give the reader a chance to mint the code before asserting.
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }

        assert_eq!(
            sessions.pending_bootstrap_count(),
            1,
            "a `bootstrap` line should mint exactly one code",
        );

        drop(writer);
        waiter.await.unwrap().unwrap();
    }
}

#[cfg(test)]
mod editor_cli_tests {
    use super::*;
    #[test]
    fn open_defaults_and_explicit_editor_and_path() {
        let cli = Cli::try_parse_from(["splitty", "open", "--session", "s1"]).expect("parse");
        assert!(
            matches!(cli.command, Commands::Open { path, editor: None, session: Some(id), .. } if path == std::path::Path::new(".") && id == "s1")
        );
        let cli = Cli::try_parse_from([
            "splitty",
            "open",
            "--with",
            "zed",
            "--session",
            "s1",
            "file with spaces.rs",
        ])
        .expect("parse");
        assert!(
            matches!(cli.command, Commands::Open { path, editor: Some(editor), .. } if path == std::path::Path::new("file with spaces.rs") && editor == "zed")
        );
        assert!(Cli::try_parse_from(["splitty", "open", "--with", "sh"]).is_err());
        assert!(Cli::try_parse_from(["splitty", "open", "--in", "splitty"]).is_err());
    }

    #[test]
    fn download_takes_one_file_and_the_session() {
        let cli =
            Cli::try_parse_from(["splitty", "download", "--session", "s1", "build/report.pdf"])
                .expect("parse");
        assert!(
            matches!(cli.command, Commands::Download { path, session: Some(id), .. } if path == std::path::Path::new("build/report.pdf") && id == "s1")
        );
        assert!(Cli::try_parse_from(["splitty", "download", "--session", "s1"]).is_err());
        assert!(Cli::try_parse_from(["splitty", "download", "a", "b", "--session", "s1"]).is_err());
    }
}

#[cfg(test)]
mod keeper_cli_tests {
    use super::*;

    #[test]
    fn the_keeper_and_daemon_commands_parse() {
        let cli = Cli::try_parse_from(["splitty", "session-keeper"]).expect("parse");
        assert!(matches!(cli.command, Commands::SessionKeeper));
        let cli =
            Cli::try_parse_from(["splitty", "daemon", "stop", "--end-sessions"]).expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Daemon {
                action: DaemonAction::Stop { end_sessions: true }
            }
        ));
        let cli = Cli::try_parse_from(["splitty", "daemon", "stop"]).expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Daemon {
                action: DaemonAction::Stop {
                    end_sessions: false
                }
            }
        ));
        let cli = Cli::try_parse_from(["splitty", "daemon", "restart", "--addr", "127.0.0.1:9900"])
            .expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Daemon {
                action: DaemonAction::Restart { addr: Some(addr), end_sessions: false }
            } if addr == "127.0.0.1:9900"
        ));
    }
}
