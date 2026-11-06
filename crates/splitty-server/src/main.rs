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

use splitty_server::{cli, control, http};

use std::sync::Arc;

use clap::{Parser, Subcommand};
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
    /// Explicit HTTP gateway address (otherwise use local native discovery)
    #[arg(long, global = true)]
    addr: Option<String>,

    /// Token for an explicitly selected HTTP gateway
    #[arg(long, global = true)]
    token: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(flatten)]
    JobControl(cli::jobs::Commands),
    /// Own a desktop backend over inherited native pipes (internal)
    #[command(name = "desktop-backend", hide = true)]
    DesktopBackend {
        #[arg(long, value_parser = ["native-v1"])]
        transport: String,
        #[arg(long, value_parser = ["embedded", "ssh"])]
        backend: String,
        #[arg(long, required_if_eq("backend", "ssh"), value_parser = splitty_remote::ssh::SshTarget::parse)]
        target: Option<splitty_remote::ssh::SshTarget>,
        /// Record an explicit desktop host-open intent after successful admission
        #[arg(long, requires = "target", conflicts_with = "owner")]
        remember_target: bool,
        /// One explicit initial SSH update; never retained for reconnection
        #[arg(long, requires = "target", conflicts_with = "owner")]
        update: bool,
        /// Keep one embedded service across independent native substreams
        #[arg(long, conflicts_with = "target")]
        owner: bool,
    },
    /// Start an explicit browser gateway
    Serve {
        /// Serve a browser gateway for the existing native daemon
        #[arg(long, conflicts_with = "desktop_stdio")]
        daemon: bool,
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

        /// Removed: desktop launchers use the native child entry point
        #[arg(long, hide = true)]
        desktop_stdio: bool,
    },

    /// Manage the background daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },

    /// Open a local browser gateway to a remote daemon over native SSH pipes
    ///
    /// Installs/discovers the host's private daemon and opens a local browser
    /// session. SSH reconnects preserve the gateway URL and browser session;
    /// Ctrl+C closes the gateway. The host's sessions keep running.
    Connect {
        /// SSH target (e.g., user@host)
        target: String,

        /// Print the session URL without opening a browser
        #[arg(long)]
        no_open: bool,

        /// Forward host application ports over separate SSH workers: PORT or
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

    /// Show a file in a viewer pane next to this terminal; request a folder,
    /// or a file with --with or --editor, in the local client's editor
    /// (requires confirmation)
    Open {
        #[command(flatten)]
        ctl: CtlOpts,
        /// Existing file or folder, relative to this shell
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
        /// Override the client-side editor preference
        #[arg(long = "with", value_parser = ["vscode", "zed"])]
        editor: Option<String>,
        /// Request the file in the client's preferred editor instead
        #[arg(long = "editor")]
        in_editor: bool,
        /// Show the file as a tab of the pane instead of beside it
        #[arg(long, conflicts_with_all = ["editor", "in_editor"])]
        tab: bool,
        /// Pane to show the file next to (defaults to $SPLITTY_PANE_ID)
        #[arg(long, conflicts_with_all = ["editor", "in_editor"])]
        pane: Option<String>,
        /// Screen of that pane (defaults to $SPLITTY_SCREEN_ID)
        #[arg(long, conflicts_with_all = ["editor", "in_editor"])]
        screen: Option<String>,
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

    /// Print the comments the user handed over on the viewer panes of this
    /// screen, or of one document, as Markdown; `comments reply` answers in
    /// a thread
    #[command(args_conflicts_with_subcommands = true)]
    Comments {
        #[command(flatten)]
        ctl: CtlOpts,
        /// A file a viewer pane of this screen shows, relative to this shell,
        /// or the ID of a viewer pane
        document: Option<String>,
        /// Also print resolved annotations
        #[arg(long)]
        all: bool,
        /// Print the server's answer as JSON
        #[arg(long)]
        json: bool,
        /// Calling pane (defaults to $SPLITTY_PANE_ID)
        #[arg(long)]
        pane: Option<String>,
        /// Screen of that pane (defaults to $SPLITTY_SCREEN_ID)
        #[arg(long)]
        screen: Option<String>,
        #[command(subcommand)]
        action: Option<CommentsAction>,
    },

    /// Show a file, or a text from standard input, in a viewer pane and wait
    /// for the user's verdict: exit 0 on approval, 3 when the user requests
    /// changes; the review, if any, is printed
    Review {
        #[command(flatten)]
        ctl: CtlOpts,
        /// The file to review, relative to this shell
        #[arg(required_unless_present_any = ["text", "hook"], conflicts_with = "text")]
        path: Option<std::path::PathBuf>,
        /// Review the text read from standard input (at most 256 KiB)
        #[arg(long)]
        text: bool,
        /// The text document's title [default: Review]
        #[arg(long, conflicts_with = "path")]
        title: Option<String>,
        /// How the window shows the text: rendered as Markdown, or as plain text
        #[arg(long, value_parser = ["markdown", "text"], conflicts_with = "path")]
        format: Option<String>,
        /// Print the outcome as JSON
        #[arg(long)]
        json: bool,
        /// Serve as an agent's hook: review the plan of the hook event on
        /// standard input and print the agent's decision; always exits 0
        #[arg(long, value_parser = ["claude-code"], conflicts_with_all = ["path", "text", "title", "format", "json"])]
        hook: Option<String>,
        /// Calling pane (defaults to $SPLITTY_PANE_ID)
        #[arg(long)]
        pane: Option<String>,
        /// Screen of that pane (defaults to $SPLITTY_SCREEN_ID)
        #[arg(long)]
        screen: Option<String>,
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
    /// `--ensure-daemon`, a missing daemon starts with its private endpoint.
    #[command(name = "remote-handshake", hide = true)]
    RemoteHandshake {
        #[arg(long, required = true)]
        json: bool,

        #[arg(long)]
        ensure_daemon: bool,
    },

    /// Explicitly migrate a verified keeper-backed legacy daemon (internal)
    #[command(name = "remote-migrate", hide = true)]
    RemoteMigrate {
        #[arg(long)]
        expected: String,
    },

    /// Relay the private daemon endpoint over native stdin/stdout (internal)
    #[command(name = "remote-bridge", hide = true)]
    RemoteBridge {
        #[arg(long, value_parser = ["native-v1"])]
        transport: String,
    },

    /// Serve the host side of a reverse forward (internal)
    ///
    /// `prepare` prints the private relay directory as one JSON line; `serve`
    /// listens on loopback for this user's programs and relays them to the
    /// socket of the client's `ssh -R`.
    #[command(name = "remote-relay", hide = true)]
    RemoteRelay {
        #[command(subcommand)]
        action: RelayAction,
    },

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
enum CommentsAction {
    /// Reply in the thread of an annotation the user handed over
    Reply {
        /// A file a viewer pane of this screen shows, relative to this shell,
        /// or the ID of a viewer pane
        document: String,
        /// The annotation's number
        number: u32,
        /// The reply; `-` reads it from standard input
        text: String,
        /// Calling pane (defaults to $SPLITTY_PANE_ID)
        #[arg(long)]
        pane: Option<String>,
        /// Screen of that pane (defaults to $SPLITTY_SCREEN_ID)
        #[arg(long)]
        screen: Option<String>,
    },
}

#[derive(Subcommand)]
enum RelayAction {
    /// Make the relay directory and print it
    Prepare,
    /// Listen on 127.0.0.1:<port> and [::1]:<port> and relay to <socket>
    Serve {
        #[arg(long)]
        port: u16,

        #[arg(long)]
        socket: std::path::PathBuf,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start the daemon in the background
    Start {
        /// Removed: use `serve --daemon --addr` for an explicit browser gateway
        #[arg(long, hide = true)]
        addr: Option<String>,

        /// Removed: browser tokens belong only to explicit gateways
        #[arg(long, hide = true)]
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
        /// Stop only this verified native daemon instance (internal update).
        #[arg(long, hide = true, conflicts_with = "end_sessions")]
        if_instance: Option<String>,
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
        /// Print running state and format-2 native discovery, without tokens
        /// or a TCP port; stopped daemons may report surviving keepers
        #[arg(long)]
        json: bool,
    },

    /// The daemon process itself, started by `daemon start` (internal)
    #[command(hide = true)]
    Run {
        /// Inherited descriptor holding the private daemon instance lock
        #[arg(long)]
        lock_fd: i32,
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
        // Keep the no-runtime fast path inside the same owner-command gate.
        // Scoped callers must not reach a keeper's inherited control socket.
        if std::env::var_os("SPLITTY_AGENT_SOCK").is_some_and(|s| !s.is_empty()) {
            eprintln!("Error: administrative commands are unavailable under an agent grant");
            std::process::exit(2);
        }
        if let Err(error) = Cli::try_parse() {
            error.exit();
        }
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

fn grant_command_allowed(command: &Commands) -> bool {
    match command {
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
    }
}

async fn async_main() {
    let framed_stdio = std::env::args()
        .nth(1)
        .is_some_and(|command| matches!(command.as_str(), "desktop-backend" | "remote-bridge"));
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if !framed_stdio && error.exit_code() != 0 && std::env::args().any(|a| a == "--json") {
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
        let allowed = grant_command_allowed(&cli.command);
        if !allowed {
            let error = "administrative commands are unavailable under an agent grant";
            if matches!(cli.command, Commands::Review { hook: Some(_), .. }) {
                // A hook that fails leaves the decision to the agent: status
                // 2 would deny its plan.
                eprintln!("Error: {error}; the agent asks the user itself");
                std::process::exit(0);
            }
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
            | Commands::DesktopBackend { .. }
            | Commands::RemoteBridge { .. }
            | Commands::RemoteHandshake { .. }
            | Commands::RemoteMigrate { .. }
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
        Commands::DesktopBackend {
            transport: _,
            backend,
            target,
            owner,
            remember_target,
            update,
        } => {
            let result = match (backend.as_str(), target, owner, remember_target, update) {
                ("embedded", None, true, false, false) => {
                    splitty_server::native::helper::owner::embedded(
                        tokio::io::stdin(),
                        tokio::io::stdout(),
                        connect_shutdown(),
                    )
                    .await
                }
                ("embedded", None, false, false, false) => {
                    splitty_server::native::helper::embedded(
                        tokio::io::stdin(),
                        tokio::io::stdout(),
                        connect_shutdown(),
                    )
                    .await
                }
                ("ssh", Some(target), false, remember, update) => {
                    splitty_server::native::helper::ssh(
                        tokio::io::stdin(),
                        tokio::io::stdout(),
                        target,
                        remember,
                        update,
                        connect_shutdown(),
                    )
                    .await
                }
                _ => {
                    eprintln!("desktop backend: --target is required only for --backend ssh");
                    std::process::exit(2);
                }
            };
            let cleanup = splitty_server::native::helper::settle_workers().await;
            if let Err(error) = result.and(cleanup) {
                eprintln!("desktop backend: {error}");
                std::process::exit(1);
            }
            // Owned service cleanup is complete. Tokio's stdin implementation
            // may still have a blocking read when stdout failed first; do not
            // wait for that unrelated read during runtime teardown.
            std::process::exit(0);
        }
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
            daemon,
            addr,
            token,
            oneshot_token,
            remote,
            open,
            desktop_stdio,
        } => {
            if desktop_stdio {
                eprintln!("serve --desktop-stdio was removed; desktop launchers must use `desktop-backend --transport native-v1 --backend embedded`");
                std::process::exit(2);
            }
            let bind_addr = resolve_loopback_bind(&addr, remote);
            if daemon {
                #[cfg(unix)]
                if let Err(error) =
                    serve_daemon_gateway(bind_addr, token, oneshot_token, open).await
                {
                    eprintln!("Cannot serve the daemon: {error}");
                    std::process::exit(1);
                }
                #[cfg(not(unix))]
                {
                    eprintln!("Native daemon browser access requires Unix; use WSL on Windows.");
                    std::process::exit(1);
                }
                return;
            }
            if let Err(error) = serve_embedded_gateway(bind_addr, token, oneshot_token, open).await
            {
                eprintln!("Cannot serve the embedded workspace: {error}");
                std::process::exit(1);
            }
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
            in_editor,
            tab,
            pane,
            screen,
            session,
        } => {
            handle_ctl(async {
                if opens_in_viewer(&path, editor.as_deref(), in_editor) {
                    let request = cli::viewer::prepare(
                        &path,
                        &resolve_screen(&screen),
                        &resolve_pane(&pane),
                        tab,
                    )?;
                    let server = discover_server(&ctl)?;
                    return cli::viewer::submit(&server, &request).await;
                }
                if tab || pane.is_some() || screen.is_some() {
                    return Err(cli::CliError::Usage(
                        "--tab, --pane and --screen apply only to a file shown in a viewer pane"
                            .into(),
                    ));
                }
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
        Commands::Comments {
            ctl,
            document,
            all,
            json,
            pane,
            screen,
            action,
        } => {
            handle_ctl(async {
                use cli::comments::{self, Caller};
                match action {
                    Some(CommentsAction::Reply {
                        document,
                        number,
                        text,
                        pane,
                        screen,
                    }) => {
                        let caller = Caller::new(resolve_screen(&screen), resolve_pane(&pane))?;
                        let document = comments::document(&document)?;
                        let text = comments::reply_text(&text, std::io::stdin().lock())?;
                        let server = discover_server(&ctl)?;
                        comments::reply(&server, &caller, &document, number, &text).await
                    }
                    None => {
                        let caller = Caller::new(resolve_screen(&screen), resolve_pane(&pane))?;
                        let document = document.as_deref().map(comments::document).transpose()?;
                        let server = discover_server(&ctl)?;
                        comments::list(&server, &caller, document.as_deref(), all, json).await
                    }
                }
            })
            .await;
        }
        Commands::Review {
            ctl,
            hook: Some(_),
            pane,
            screen,
            ..
        } => {
            // Claude Code's hook (`cli`, Review Hook For Claude Code): a
            // decision only for the user's verdict; on every failure nothing,
            // status 0 and the reason on standard error, so that Claude Code
            // asks the user itself.
            let decided = async {
                let request = cli::review::plan_request(std::io::stdin().lock())
                    .map_err(cli::CliError::Failed)?;
                let (screen, pane) = (resolve_screen(&screen), resolve_pane(&pane));
                let review = request.review(&screen, &pane)?;
                let server = discover_server(&ctl)?;
                cli::review::run_hook(&server, &review, &request).await
            }
            .await;
            match decided {
                Ok(decision) => {
                    use std::io::Write;
                    let mut stdout = std::io::stdout().lock();
                    if let Err(error) = stdout
                        .write_all(decision.as_bytes())
                        .and_then(|()| stdout.flush())
                    {
                        eprintln!("Error: Cannot print the decision: {error}");
                    }
                }
                Err(error) => eprintln!("Error: {error}; Claude Code asks the user itself"),
            }
        }
        Commands::Review {
            ctl,
            path,
            text,
            title,
            format,
            json,
            pane,
            screen,
            hook: None,
        } => {
            handle_ctl(async {
                let (screen, pane) = (resolve_screen(&screen), resolve_pane(&pane));
                let review = match path {
                    Some(path) if !text => cli::review::file(&path, &screen, &pane)?,
                    _ => {
                        let text = cli::review::read_text(std::io::stdin().lock())?;
                        let here = std::env::current_dir().ok();
                        cli::review::text(
                            text,
                            title.as_deref().unwrap_or("Review"),
                            format.as_deref() != Some("text"),
                            &screen,
                            &pane,
                            here.as_deref(),
                        )?
                    }
                };
                let server = discover_server(&ctl)?;
                // The review is printed and flushed; a request for changes has its own status.
                if cli::review::run(&server, &review, json).await?
                    == cli::review::Verdict::ChangesRequested
                {
                    std::process::exit(cli::review::CHANGES_REQUESTED_STATUS);
                }
                Ok(())
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

        Commands::RemoteHandshake { ensure_daemon, .. } => {
            match splitty_server::host::handshake::run(ensure_daemon).await {
                Ok(handshake) => match serde_json::to_string(&handshake) {
                    Ok(line) => emit_line!("{line}"),
                    Err(e) => {
                        eprintln!("Failed to encode the handshake: {e}");
                        std::process::exit(1);
                    }
                },
                Err(reason) => {
                    eprintln!("{reason}");
                    std::process::exit(reason.exit_code());
                }
            }
        }
        Commands::RemoteMigrate { expected } => {
            if let Err(reason) = splitty_server::host::migration::migrate(&expected).await {
                eprintln!("{reason}");
                std::process::exit(reason.exit_code());
            }
            // A concurrent start is discovered before binding; native starts
            // additionally serialize through the existing endpoint lock.
            handle_daemon(DaemonAction::Start {
                addr: None,
                token: None,
                remote: false,
            })
            .await;
        }
        Commands::RemoteBridge { transport: _ } => {
            if let Err(reason) = splitty_server::host::bridge::run().await {
                eprintln!("{reason}");
                std::process::exit(1);
            }
            // A failed output pipe can leave Tokio's blocking stdin read alive.
            // Carrier cleanup is complete; no daemon or keeper belongs to us.
            std::process::exit(0);
        }
        Commands::RemoteRelay { action } => match action {
            RelayAction::Prepare => match splitty_server::host::relay::prepare() {
                Ok(line) => emit_line!("{line}"),
                Err(reason) => {
                    eprintln!("{reason}");
                    std::process::exit(1);
                }
            },
            RelayAction::Serve { port, socket } => {
                let status = splitty_server::host::relay::serve(port, &socket).await;
                if status != 0 {
                    std::process::exit(status);
                }
            }
        },
        Commands::IssueBootstrap { ctl } => {
            handle_ctl(async {
                // Reaches the server over TCP with the process token rather than
                // through the control socket. Minting a browser session is a
                // privileged act, and the token lives in a 0600 file the user
                // owns — a process running inside a pane has no way to get it.
                let addr = ctl.addr.clone().ok_or_else(|| cli::CliError::Failed(
                    "issue-bootstrap needs --addr for an explicit browser gateway (splitty serve --daemon)".into()
                ))?;
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

/// Whether `splitty open` shows `path` in a viewer pane: a file does, a
/// folder and a file with `--with` or `--editor` go to the client's editor.
fn opens_in_viewer(path: &std::path::Path, editor: Option<&str>, in_editor: bool) -> bool {
    editor.is_none() && !in_editor && !path.is_dir()
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

async fn handle_daemon(action: DaemonAction) {
    use splitty_core::daemon;

    match action {
        DaemonAction::Start {
            addr,
            token,
            remote,
        } => {
            if addr.is_some() || token.is_some() || remote {
                eprintln!("daemon start has no TCP listener or browser token; use splitty serve --daemon --addr <loopback-address> for browser access");
                std::process::exit(2);
            }
            prepare_daemon_config();
            match daemon::read_info_checked() {
                Ok(Some(info)) if daemon::is_running(info.pid) => {
                    if info.legacy() {
                        println!("Daemon already running (pid={}, legacy transport; explicit migration required)", info.pid);
                    } else {
                        println!(
                            "Daemon already running (pid={}, clientSocket={})",
                            info.pid,
                            info.client_socket.as_deref().unwrap_or_default()
                        );
                    }
                    return;
                }
                // Stale publication is replaced only after acquiring the instance lock.
                Ok(Some(_)) => {}
                Ok(None) => {}
                Err(error) => {
                    eprintln!("Cannot safely start a daemon: cannot verify {}: {error}. Inspect the existing daemon before replacing its discovery record.", daemon::info_path().display());
                    std::process::exit(1);
                }
            }
            lock_and_start_daemon().await;
        }
        DaemonAction::Restart { addr, end_sessions } => {
            if addr.is_some() {
                eprintln!("daemon restart has no TCP listener; use splitty serve --daemon --addr <loopback-address>");
                std::process::exit(2);
            }
            prepare_daemon_config();
            let existing = checked_daemon_for_action();
            if let Some(info) = existing.as_ref().filter(|i| daemon::is_running(i.pid)) {
                if !info.keeps_sessions && !end_sessions {
                    eprintln!("This legacy daemon cannot preserve sessions across a restart. End them deliberately with daemon restart --end-sessions.");
                    std::process::exit(1);
                }
            }
            if let Some(pid) = stop_daemon(existing) {
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
                while daemon::is_running(pid) && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                if daemon::is_running(pid) {
                    eprintln!("Daemon {pid} has not exited; refusing to start a replacement");
                    std::process::exit(1);
                }
            }
            if end_sessions {
                end_kept_sessions();
            }
            lock_and_start_daemon().await;
        }
        DaemonAction::Run { lock_fd } => {
            #[cfg(unix)]
            {
                run_daemon(lock_fd).await;
            }
            #[cfg(not(unix))]
            {
                let _ = lock_fd;
                eprintln!("Daemon mode is only supported on Unix");
                std::process::exit(1);
            }
        }
        DaemonAction::Stop {
            end_sessions,
            if_instance,
        } => {
            if let Some(instance) = if_instance {
                match stop_daemon_instance(&instance).await {
                    Ok(true) => {}
                    Ok(false) => {
                        eprintln!("Daemon instance changed; refresh discovery.");
                        std::process::exit(75);
                    }
                    Err(error) => {
                        eprintln!("Cannot safely update the daemon: {error}");
                        std::process::exit(1);
                    }
                }
            } else {
                stop_daemon(checked_daemon_for_action());
            }
            if end_sessions {
                end_kept_sessions();
            }
        }
        DaemonAction::Status { json } => {
            let info = checked_daemon_for_action();
            let stale = info.as_ref().filter(|info| !daemon::is_running(info.pid));
            let cleaned = stale.is_some_and(cleanup_stale_daemon);
            // A starting daemon may have published while cleanup was trying
            // the lock. Report fresh discovery without deleting its record.
            let running = checked_daemon_for_action().filter(|info| daemon::is_running(info.pid));

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
                if cleaned {
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
            if info.legacy() {
                println!("  Legacy:  explicit migration required");
            }
            if let Some(path) = &info.client_socket {
                println!("  Client:  {path}");
            }
            println!("  Started: {}", info.started_at);
            println!("  Version: {}", info.version);
        }
    }
}

/// Hold the instance lock before detaching; the child binds its own endpoint.
async fn lock_and_start_daemon() {
    #[cfg(unix)]
    {
        let path = splitty_server::native::endpoint::socket_path();
        let lock = splitty_server::native::endpoint::InstanceLock::acquire(
            path.parent().expect("native endpoint parent"),
        )
        .unwrap_or_else(|error| {
            eprintln!("Failed to lock native endpoint {}: {error}", path.display());
            // Another launcher may own the instance before it publishes
            // discovery. Preserve the lock and let native clients refresh.
            std::process::exit(if error.kind() == std::io::ErrorKind::WouldBlock {
                75
            } else {
                1
            });
        });
        if let Err(reason) = start_daemon(lock).await {
            daemon_start_failed(
                &reason,
                &splitty_core::config::config_dir().join("daemon.log"),
            );
        }
    }
    #[cfg(not(unix))]
    {
        eprintln!("Daemon mode is only supported on Unix");
        std::process::exit(1);
    }
}

fn prepare_daemon_config() {
    splitty_core::config::prepare_daemon_config_dir().unwrap_or_else(|error| {
        eprintln!("Cannot prepare daemon configuration: {error}");
        std::process::exit(1);
    });
}

fn checked_daemon_for_action() -> Option<splitty_core::daemon::DaemonInfo> {
    splitty_core::daemon::read_info_checked().unwrap_or_else(|error| {
        eprintln!("Cannot verify daemon discovery: cannot verify {}: {error}. Inspect the existing daemon before replacing its discovery record.", splitty_core::daemon::info_path().display());
        std::process::exit(1);
    })
}

/// An old status/stop read must never unlink publication by a newer start.
/// Native writers hold this same lock until their final cleanup has ended.
fn cleanup_stale_daemon(expected: &splitty_core::daemon::DaemonInfo) -> bool {
    #[cfg(unix)]
    {
        let directory = splitty_core::config::run_dir();
        let _lock = match splitty_server::native::endpoint::InstanceLock::acquire(&directory) {
            Ok(lock) => lock,
            Err(_) => return false,
        };
        match splitty_core::daemon::read_info_checked() {
            Ok(Some(current))
                if &current == expected && !splitty_core::daemon::is_running(current.pid) =>
            {
                splitty_core::daemon::remove_info_if(expected).unwrap_or(false)
            }
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = expected;
        false
    }
}

/// Compare at both ends: discovery chooses an owner socket, and the accepting
/// daemon checks its own immutable instance before initiating its own shutdown.
/// No PID signal or legacy fallback is safe for a conditional remote update.
async fn stop_daemon_instance(instance: &str) -> Result<bool, String> {
    use splitty_core::daemon;
    if instance.len() != 32
        || !instance
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid expected daemon instance".into());
    }
    let Some(info) = daemon::read_info_checked().map_err(|error| error.to_string())? else {
        return Ok(false);
    };
    if info.instance.as_deref() != Some(instance) {
        return Ok(false);
    }
    if info.socket.is_empty() {
        return Err(
            "the daemon's owner socket is unavailable; explicitly restart it on the host".into(),
        );
    }
    let server = cli::ServerInfo {
        transport: cli::Transport::LocalSocket(info.socket),
        source: "verified daemon update",
    };
    let response = cli::request_within(
        &server,
        "POST",
        "/api/daemon/stop",
        Some(serde_json::json!({"instance":instance})),
        Some(std::time::Duration::from_secs(5)),
    )
    .await;
    match response {
        Ok((409, _)) => return Ok(false),
        Ok((200, ref body)) if body["stopping"] == instance => {},
        Ok((status, _)) => return Err(format!("the daemon cannot confirm instance-bound shutdown (status {status}); explicitly restart it on the host")),
        Err(error) => {
            // A confirmed old-instance disappearance is sufficient even when
            // the owner listener closed before its acknowledgement was read.
            let current = daemon::read_info_checked().map_err(|e| e.to_string())?;
            if current.as_ref().and_then(|info| info.instance.as_deref()) != Some(instance) { return Ok(false); }
            return Err(error.to_string());
        }
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let current = daemon::read_info_checked().map_err(|error| error.to_string())?;
        if current.as_ref().and_then(|info| info.instance.as_deref()) != Some(instance) {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("the checked daemon did not stop within its deadline".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Stop the already checked daemon. Do not reread discovery between a restart's
/// preservation check and its effects; malformed discovery is never absence.
fn stop_daemon(info: Option<splitty_core::daemon::DaemonInfo>) -> Option<u32> {
    use splitty_core::daemon;

    let Some(info) = info else {
        println!("No daemon running");
        return None;
    };

    if !daemon::is_running(info.pid) {
        if cleanup_stale_daemon(&info) {
            println!("No daemon running (stale info file cleaned up)");
        } else {
            println!("No daemon running");
        }
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
            std::process::exit(1);
        }

        // Wait briefly for process to exit
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !daemon::is_running(info.pid) {
                println!("Daemon stopped (pid={})", info.pid);
                return Some(info.pid);
            }
        }
        println!(
            "Sent SIGTERM to pid {} (may still be shutting down)",
            info.pid
        );
        Some(info.pid)
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
            match found.end() {
                Ok(()) => ending.push(socket),
                Err(e) => eprintln!("Cannot end kept session: {e}"),
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
/// clean process image and deliberately inherits only the instance lock.
#[cfg(unix)]
async fn start_daemon(lock: splitty_server::native::endpoint::InstanceLock) -> Result<(), String> {
    use splitty_core::daemon;
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    let client_socket = splitty_server::native::endpoint::socket_path();
    let config_dir = splitty_core::config::config_dir();
    let directory = splitty_core::config::PrivateDir::prepare_config(&config_dir)
        .map_err(|error| format!("cannot open daemon log directory: {error}"))?;
    let log = directory
        .open_file(
            std::ffi::OsStr::new("daemon.log"),
            nix::fcntl::OFlag::O_WRONLY
                | nix::fcntl::OFlag::O_CREAT
                | nix::fcntl::OFlag::O_APPEND
                | nix::fcntl::OFlag::O_NONBLOCK,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .map_err(|error| format!("cannot open daemon log: {error}"))?;
    directory.verify().map_err(|error| error.to_string())?;
    use std::os::unix::fs::MetadataExt;
    let metadata = log.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o600
    {
        return Err("daemon log must be an owned 0600 regular file without hardlinks".into());
    }
    let exe = std::env::current_exe()
        .map_err(|error| format!("cannot locate splitty binary: {error}"))?;

    let lock_fd = lock.file().as_raw_fd();
    let mut command = std::process::Command::new(exe);
    command.args(["daemon", "run", "--lock-fd", &lock_fd.to_string()]);
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
                lock_fd,
                nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::empty()),
            )?;
            Ok(())
        });
    }

    let mut child = tokio::process::Command::from(command)
        .spawn()
        .map_err(|error| format!("cannot start daemon process: {error}"))?;
    // Keep this copy of the lock through readiness or owned-child cleanup.

    let child_pid = child.id();
    let stdout = child.stdout.take().expect("daemon stdout is piped");
    let readiness = async {
        wait_for_daemon_ready(stdout).await?;
        let info = daemon::read_info_checked()
            .map_err(|error| error.to_string())?
            .filter(|info| {
                Some(info.pid) == child_pid
                    && info.client_socket.as_deref() == client_socket.to_str()
            })
            .ok_or_else(|| "daemon readiness does not match its discovery record".to_owned())?;
        // Prove native negotiation, not merely that the child printed a line.
        let client = splitty_server::native::client::Connector::default()
            .local(&info)
            .await
            .map_err(|error| format!("daemon endpoint did not become ready: {error}"))?;
        let instance = client.ready.instance.clone();
        client.shutdown().await;
        Ok::<_, String>(instance)
    };
    let reason = match tokio::time::timeout(DAEMON_READY_TIMEOUT, readiness).await {
        Ok(Ok(instance)) => {
            emit_line!("SPLITTY_CLIENT_SOCKET={}", client_socket.display());
            emit_line!("SPLITTY_INSTANCE={instance}");
            return Ok(());
        }
        Ok(Err(reason)) => reason,
        Err(_) => format!(
            "the daemon did not report ready within {} s",
            DAEMON_READY_TIMEOUT.as_secs()
        ),
    };

    // Exited or hung: either way it must neither linger unregistered nor stay
    // registered without serving.
    let terminated =
        child.kill().await.is_ok() || child.try_wait().is_ok_and(|status| status.is_some());
    if !terminated {
        return Err(format!("{reason}; daemon exit could not be verified, so its endpoint and discovery were retained"));
    }
    if let Some(info) = daemon::read_info().filter(|info| Some(info.pid) == child_pid) {
        let _ = daemon::remove_info_if(&info);
    }
    let _ = lock.cleanup_socket(&client_socket).await;
    Err(reason)
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
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    let mut reader = tokio::io::BufReader::new(stdout).take(4097);
    let mut bytes = Vec::new();
    reader
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(|error| format!("cannot read daemon startup: {error}"))?;
    if bytes.len() > 4096 || !bytes.ends_with(b"\n") {
        return Err("daemon exited or sent an invalid startup line".into());
    }
    let line = std::str::from_utf8(&bytes).map_err(|_| "invalid daemon startup encoding")?;
    if line.starts_with(&format!("{DAEMON_READY} pid=")) {
        return Ok(());
    }
    if let Some(reason) = line.strip_prefix(DAEMON_ERROR) {
        return Err(reason.trim().into());
    }
    Err("unexpected daemon startup output".into())
}

/// The daemon process: `daemon run`, started by [`start_daemon`].
///
/// Its stdout is the handshake with `daemon start` and nothing else: one
/// `SPLITTY_DAEMON_READY pid=<pid>` line once the control socket is bound or
/// skipped and `daemon.json` is written, or one `SPLITTY_DAEMON_ERROR <reason>`
/// line before it gives up. `daemon.json` is written only after every step that
/// can fail, so it never names a daemon that is not about to serve.
#[cfg(unix)]
async fn run_daemon(lock_fd: i32) {
    use splitty_core::daemon;

    tracing::info!("daemon started (pid={})", std::process::id());

    let client_path = splitty_server::native::endpoint::socket_path();
    let lock = splitty_server::native::endpoint::adopt_lock(
        lock_fd,
        client_path.parent().expect("native endpoint parent"),
    )
    .unwrap_or_else(|error| daemon_run_failed(&format!("cannot adopt native lock: {error}")));
    splitty_core::config::prepare_daemon_config_dir().unwrap_or_else(|error| {
        daemon_run_failed(&format!("cannot prepare daemon configuration: {error}"))
    });
    let bound = splitty_server::native::endpoint::bind_locked(&client_path, lock)
        .await
        .unwrap_or_else(|error| {
            daemon_run_failed(&format!("cannot bind native endpoint: {error}"))
        });

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
    let executable = std::env::current_exe().unwrap_or_else(|error| {
        daemon_run_failed(&format!("cannot locate keeper executable: {error}"))
    });
    let terminals = Arc::new(splitty_core::keeper::client::Keepers::new(
        keepers_dir,
        Arc::new(splitty_core::keeper::client::ProcessLauncher::new(
            executable,
        )),
    ));
    let runtime = splitty_server::service::runtime::build(
        "daemon",
        initial_state.clone(),
        terminals,
        &control_path,
    );
    let pty_manager = runtime.pty;
    let service = runtime.state;
    // The owner-control adapter has no HTTP listener or browser credentials.
    let state = http::AppState {
        service: service.clone(),
        token: String::new(),
        listen_addr: String::new(),
        sessions: Arc::new(http::session::SessionStore::new()),
        allowed_origins: Arc::new(Vec::new()),
    };

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
    state.client_state.jobs.resume_adopted();

    // The daemon is the case where the control socket matters most: it is
    // the long-lived server the in-shell CLI discovers through daemon.json.
    let control_task = control::spawn_at(state.clone(), control_path.clone());

    let info = daemon::DaemonInfo {
        pid: std::process::id(),
        format: Some(2),
        port: None,
        client_socket: Some(client_path.to_string_lossy().into_owned()),
        instance: Some(service.instance.clone()),
        transport_version: Some(u32::from(splitty_transport::wire::VERSION)),
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
    let jobs = state.client_state.jobs.clone();
    if let Err(e) = daemon::write_info(&info) {
        jobs.release().await;
        if let Err(error) = pty_manager.shutdown(true).await {
            tracing::error!("terminal release failed: {error}");
        }
        control_task.shutdown();
        drop(bound.guard);
        daemon_run_failed(&format!(
            "cannot write {}: {e}",
            daemon::info_path().display()
        ));
    }

    emit_line!("{DAEMON_READY} pid={}", std::process::id());
    let _ = nix::unistd::dup2(2, 1);
    tracing::info!("splitty daemon serving {}", client_path.display());
    let owner_shutdown = service.owner_shutdown.clone();
    let shutdown = async {
        tokio::select! { _ = daemon_shutdown_signal() => {}, _ = owner_shutdown.notified() => {} }
    };
    if let Err(error) = splitty_server::native::listen(bound.listener, service, shutdown).await {
        tracing::error!(%error, "native daemon listener ended");
    }
    jobs.release().await;

    // Persist final state before exit
    let final_state = mux_store.get_workspace_state();
    if let Err(e) = daemon::save_state(&final_state) {
        tracing::warn!("failed to persist final state: {e}");
    }
    // Let go of every session now: the keepers keep the programs running for
    // the next daemon, and a session that runs in this process hangs up. Left
    // to the runtime's teardown, a session still referenced by a client
    // connection kept its shell alive, and the runtime waited for it forever.
    if let Err(error) = pty_manager.shutdown(true).await {
        tracing::error!("terminal release failed: {error}");
    }
    let _ = daemon::remove_info_if(&info);
    control_task.shutdown();
    drop(bound.guard);
    tracing::info!("daemon shut down");
}

/// Tell `daemon start` why the daemon cannot start, and exit.
#[cfg(unix)]
fn daemon_run_failed(reason: &str) -> ! {
    tracing::error!("daemon failed to start: {reason}");
    emit_line!("{DAEMON_ERROR} {}", reason.replace('\n', " "));
    std::process::exit(1);
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

/// A deliberately exposed browser workspace owns its service but keeps all
/// browser state at the gateway, using the same native operation boundary.
async fn serve_embedded_gateway(
    address: std::net::SocketAddr,
    token: Option<String>,
    oneshot: bool,
    open: bool,
) -> Result<(), String> {
    if address.ip() != std::net::Ipv4Addr::LOCALHOST
        && address.ip() != std::net::Ipv6Addr::LOCALHOST
    {
        return Err("browser gateways require 127.0.0.1 or ::1".into());
    }
    let listener = splitty_remote::tunnel::exposure::bind_gateway(address)
        .await
        .map_err(|error| format!("cannot bind {address}: {error}"))?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let browser = http::BrowserState::new(address, resolve_token(token, oneshot))
        .map_err(|error| error.to_string())?;
    let runtime = splitty_server::service::runtime::build(
        "embedded",
        None,
        Arc::new(splitty_core::pty::terminal::LocalTerminals),
        &control::socket_path(),
    );
    let state = http::AppState {
        service: runtime.state.clone(),
        token: String::new(),
        sessions: Arc::new(http::session::SessionStore::new()),
        listen_addr: String::new(),
        allowed_origins: Arc::new(Vec::new()),
    };
    let control_task = control::spawn(state);
    let backend = http::gateway::Backend::embedded(runtime.state.clone());
    let bootstrap = browser.sessions.issue_bootstrap();
    let url = format!("http://{address}/?bootstrap={bootstrap}&desktop=1");
    let app = http::gateway::router(backend.clone(), browser);
    emit_line!("SPLITTY_PORT={}", address.port());
    emit_line!("SPLITTY_BOOTSTRAP={bootstrap}");
    emit_line!("SPLITTY_URL={url}");
    tracing::info!("splitty (serve) listening on {address}");
    if open {
        if let Err(error) = open::that(&url) {
            tracing::warn!(%error, "could not open browser");
        }
    }
    let result = http::server::serve_registered(listener, app, shutdown_signal()).await;
    backend.close();
    let cleanup = runtime.close().await;
    control_task.shutdown();
    tracing::info!("server shut down");
    result.and(cleanup).map_err(|error| error.to_string())
}

#[cfg(unix)]
async fn serve_daemon_gateway(
    address: std::net::SocketAddr,
    token: Option<String>,
    oneshot: bool,
    open: bool,
) -> Result<(), String> {
    use splitty_core::daemon;
    let info = daemon::read_info_checked().map_err(|error| error.to_string())?
        .filter(|info| daemon::is_running(info.pid))
        .ok_or("no daemon is running; start one with `splitty daemon start`, or use plain `splitty serve`")?;
    let connector = splitty_server::native::client::Connector::default();
    let client = connector
        .local(&info)
        .await
        .map_err(|error| error.to_string())?;
    let backend = http::gateway::Backend::daemon(info);
    let listener = splitty_remote::tunnel::exposure::bind_gateway(address)
        .await
        .map_err(|error| error.to_string())?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let browser = http::BrowserState::new(address, resolve_token(token, oneshot))
        .map_err(|error| error.to_string())?;
    let bootstrap = browser.sessions.issue_bootstrap();
    let url = format!("http://{address}/?bootstrap={bootstrap}&desktop=1");
    let app = http::gateway::router(backend.clone(), browser);
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(async move {
        http::server::serve_registered(listener, app, async {
            let _ = stopped.await;
        })
        .await
    });
    emit_line!("SPLITTY_PORT={}", address.port());
    emit_line!("SPLITTY_BOOTSTRAP={bootstrap}");
    emit_line!("SPLITTY_URL={url}");
    if open {
        if let Err(error) = open::that(&url) {
            tracing::warn!(%error, "could not open browser");
        }
    }
    let outcome = tokio::select! { biased;
        _ = connect_shutdown() => Ok(()),
        result = monitor_daemon_gateway(backend.clone(), client, connector) => result,
        result = &mut server => Err(format!("browser gateway ended: {result:?}")),
    };
    backend.set_daemon(None);
    let _ = stop.send(());
    if !server.is_finished()
        && tokio::time::timeout(std::time::Duration::from_secs(5), &mut server)
            .await
            .is_err()
    {
        server.abort();
    }
    outcome
}

#[cfg(unix)]
async fn monitor_daemon_gateway(
    backend: Arc<http::gateway::Backend>,
    mut client: splitty_server::native::client::Client,
    connector: splitty_server::native::client::Connector,
) -> Result<(), String> {
    use splitty_core::daemon;
    loop {
        while client.recv().await.is_some() {}
        backend.set_daemon(None);
        if let Err(error) = client.finish().await {
            if matches!(
                error.kind(),
                std::io::ErrorKind::InvalidData
                    | std::io::ErrorKind::Unsupported
                    | std::io::ErrorKind::PermissionDenied
            ) {
                return Err(error.to_string());
            }
        }
        let deadline = tokio::time::Instant::now() + splitty_remote::tunnel::GIVE_UP_AFTER;
        let mut failures = 0;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err("timed out waiting for the daemon to return; start it with `splitty daemon start`".into());
            }
            let info = daemon::read_info_checked()
                .map_err(|error| format!("cannot verify daemon discovery: {error}"))?
                .filter(|info| daemon::is_running(info.pid));
            if let Some(info) = info {
                let attempt = tokio::time::timeout_at(deadline, connector.local(&info)).await;
                match attempt {
                    Ok(Ok(next)) => {
                        client = next;
                        backend.set_daemon(Some(info));
                        break;
                    }
                    Ok(Err(error))
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
                        ) =>
                    {
                        return Err(error.to_string())
                    }
                    Ok(Err(error))
                        if error.kind() == std::io::ErrorKind::InvalidData
                            && !splitty_transport::negotiation::stale_instance(&error) =>
                    {
                        return Err(error.to_string())
                    }
                    _ => {}
                }
            }
            let delay = splitty_remote::tunnel::backoff(failures)
                .min(deadline.saturating_duration_since(tokio::time::Instant::now()));
            failures += 1;
            tokio::time::sleep(delay).await;
        }
    }
}

async fn handle_connect(target: &str, forward: &[String], open: bool, json: bool) {
    use splitty_remote::tunnel::{self, ConnectEvent};
    use splitty_remote::{hosts, install, ssh};

    let target =
        ssh::SshTarget::parse(target).unwrap_or_else(|e| connect_rejected(json, e.to_string()));
    let requested_forwards =
        tunnel::parse_forwards(forward).unwrap_or_else(|e| connect_rejected(json, e.to_string()));
    let host = target.as_str().to_owned();
    let report = |event: ConnectEvent| report_connect(json, &host, &event);
    let mut shutdown = std::pin::pin!(connect_shutdown());

    if requested_forwards.len() > 32 {
        connect_rejected(json, "at most 32 development forwards are supported".into());
    }
    let assignment = splitty_transport::admission::Assignment::new(
        splitty_transport::admission::Backend::new(),
        splitty_transport::admission::Domain::new(),
    );
    let mut flow = Box::pin(tunnel::open(
        ssh::Ssh::stdio_only(target.clone()),
        env!("CARGO_PKG_VERSION"),
        splitty_core::protocol::PROTOCOL_VERSION,
        install::payload_for,
        assignment,
        |progress| {
            let (event, _storage) = progress.map(ConnectEvent::from).into_parts();
            report(event);
        },
    ));
    let opening = tokio::select! {
        result = &mut flow => Some(result),
        () = &mut shutdown => None,
    };
    drop(flow);
    let (remote, connected) = match opening {
        Some(Ok(connection)) => connection,
        Some(Err(error)) => {
            let _ = splitty_server::native::helper::settle_workers().await;
            connect_failed(json, &host, &error);
        }
        None => {
            let _ = splitty_server::native::helper::settle_workers().await;
            report(ConnectEvent::Closed);
            return;
        }
    };
    let remote = Arc::new(remote);
    let backend = http::gateway::Backend::remote(remote.clone());
    backend.set_remote(Some(connected.daemon.clone()));
    // An explicit gateway owns a local listener, assets and credentials. No
    // reusable browser credential is obtained from or sent to the SSH host.
    let listener = match splitty_remote::tunnel::exposure::bind_gateway(std::net::SocketAddr::from(
        (std::net::Ipv4Addr::LOCALHOST, 0),
    ))
    .await
    {
        Ok(listener) => listener,
        Err(error) => {
            drop(connected);
            let _ = splitty_server::native::helper::settle_workers().await;
            connect_failed(
                json,
                &host,
                &tunnel::RemoteError::PortUnavailable {
                    port: 0,
                    source: error,
                },
            );
        }
    };
    let address = listener.local_addr().expect("bound browser gateway");
    let browser = http::BrowserState::new(
        address,
        splitty_core::auth::generate_oneshot().expect("browser process token"),
    )
    .expect("canonical loopback gateway");
    let sessions = browser.sessions.clone();
    let (stop_http, http_stopped) = tokio::sync::oneshot::channel();
    let router = http::gateway::router(backend.clone(), browser);
    let mut gateway = tokio::spawn(async move {
        http::server::serve_registered(listener, router, async {
            let _ = http_stopped.await;
        })
        .await
    });
    let forwards =
        splitty_server::hub::Forwards::transient(Arc::new(splitty_server::hub::SshConnector));
    let mut failure = None;
    let mut cancelled = false;
    let mut announced_forwards = Vec::new();
    for requested in requested_forwards {
        let reserved = tunnel::reserve(requested);
        let (mapping, reservation) = match reserved {
            Ok(reserved) => reserved,
            Err(error) => {
                failure = Some(tunnel::RemoteError::Forward(error.to_string()));
                break;
            }
        };
        drop(reservation);
        let opened = tokio::select! {
            _ = &mut shutdown => None,
            result = forwards.open(&host, i64::from(mapping.remote), Some(i64::from(mapping.local)), false) => Some(result),
        };
        match opened {
            Some(Ok(info)) => announced_forwards.push(tunnel::Forward {
                remote: mapping.remote,
                local: info.local,
                asked: mapping.asked,
            }),
            Some(Err(error)) => {
                failure = Some(tunnel::RemoteError::Forward(error.to_string()));
                break;
            }
            None => {
                cancelled = true;
                break;
            }
        }
    }
    if failure.is_none() && !cancelled {
        if let Err(error) = hosts::record_recent(&splitty_core::config::config_dir(), &target) {
            report(ConnectEvent::Notice {
                message: format!("Could not record {host} as a recent host: {error}"),
            });
        }
        let url = tunnel::session_url(address.port(), &sessions.issue_bootstrap(), &host);
        report(ConnectEvent::Ready {
            url: url.clone(),
            port: address.port(),
        });
        for forward in announced_forwards {
            report(forward.into());
        }
        if !json {
            println!("Browser gateway active. Press Ctrl+C to disconnect.");
        }
        if open {
            open_in_browser(json, &host, &url);
        }
        let supervised = tunnel::supervise(
            remote.as_ref(),
            connected,
            tunnel::GIVE_UP_AFTER,
            &mut shutdown,
            |mut connection| {
                let backend = backend.clone();
                let host = host.clone();
                async move {
                    backend.set_remote(Some(connection.daemon.clone()));
                    while connection.client.recv().await.is_some() {}
                    backend.set_remote(None);
                    let (result, ended) = connection.client.finish().await;
                    let result = match result {
                        Err(error) if splitty_remote::bridge::protocol_failure(&error) => {
                            return Err(tunnel::RemoteError::Native(error))
                        }
                        result => result,
                    };
                    if let Ok(status) = ended.status {
                        if status.code() == Some(255) {
                            return Err(tunnel::RemoteError::Ssh(ssh::classify(
                                &host,
                                status,
                                &ended.stderr,
                            )));
                        }
                    }
                    result
                        .map(|()| ended.stderr.to_string())
                        .map_err(tunnel::RemoteError::Native)
                }
            },
            |event| {
                if matches!(event, tunnel::Event::Lost { .. }) {
                    backend.set_remote(None);
                }
                report(ConnectEvent::from_event(&host, event));
            },
        );
        tokio::pin!(supervised);
        failure = tokio::select! {
            result = &mut supervised => result.err(),
            result = &mut gateway => Some(tunnel::RemoteError::Native(std::io::Error::other(format!("browser gateway ended: {result:?}")))),
        };
    } else {
        drop(connected);
    }
    backend.set_remote(None);
    let _ = stop_http.send(());
    forwards.close().await;
    if !gateway.is_finished()
        && tokio::time::timeout(std::time::Duration::from_secs(5), &mut gateway)
            .await
            .is_err()
    {
        gateway.abort();
    }
    let _ = splitty_server::native::helper::settle_workers().await;
    match failure {
        Some(error) => connect_failed(json, &host, &error),
        None => report(ConnectEvent::Closed),
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
mod editor_cli_tests {
    use super::*;
    #[test]
    fn native_helpers_and_all_owner_entry_points_are_unavailable_to_grants() {
        for args in [
            vec![
                "desktop-backend",
                "--transport",
                "native-v1",
                "--backend",
                "embedded",
            ],
            vec![
                "desktop-backend",
                "--transport",
                "native-v1",
                "--backend",
                "ssh",
                "--target",
                "user@vm",
            ],
            vec!["remote-bridge", "--transport", "native-v1"],
            vec!["remote-handshake", "--json"],
            vec!["remote-migrate", "--expected", "invalid"],
            vec!["session-keeper"],
            vec!["daemon", "start"],
            vec!["attach"],
            vec!["serve"],
            vec!["issue-bootstrap"],
            // cli, Open In A Viewer Pane: still refused under a grant.
            vec!["open", "PLAN.md"],
            vec!["open", "--editor", "PLAN.md"],
            // cli, Comments Command (Under a grant) and Review Command.
            vec!["comments"],
            vec!["comments", "PLAN.md", "--all", "--json"],
            vec!["comments", "reply", "PLAN.md", "2", "Done."],
            vec!["review", "PLAN.md"],
            vec!["review", "--text", "--title", "Plan"],
        ] {
            let parsed = Cli::try_parse_from(std::iter::once("splitty").chain(args)).unwrap();
            assert!(!grant_command_allowed(&parsed.command));
        }
        let scoped = Cli::try_parse_from(["splitty", "list-screens", "--json"]).unwrap();
        assert!(grant_command_allowed(&scoped.command));
        assert!(
            Cli::try_parse_from(["splitty", "desktop-backend", "--backend", "embedded"]).is_err()
        );
        assert!(Cli::try_parse_from([
            "splitty",
            "desktop-backend",
            "--transport",
            "legacy",
            "--backend",
            "embedded"
        ])
        .is_err());
    }

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

    /// cli, Open In A Viewer Pane (As a tab) and Open In Client Editor: the
    /// viewer's flags, `--editor`, and no viewer flag next to an editor.
    #[test]
    fn open_takes_the_viewer_flags_or_the_editor_flags() {
        let cli = Cli::try_parse_from([
            "splitty",
            "open",
            "--tab",
            "--pane",
            "p1",
            "--screen",
            "s1",
            "notes.txt",
        ])
        .expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Open { path, tab: true, pane: Some(pane), screen: Some(screen), in_editor: false, editor: None, .. }
                if path == std::path::Path::new("notes.txt") && pane == "p1" && screen == "s1"
        ));
        let cli =
            Cli::try_parse_from(["splitty", "open", "--editor", "src/main.rs"]).expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Open {
                in_editor: true,
                editor: None,
                tab: false,
                ..
            }
        ));
        assert!(
            Cli::try_parse_from(["splitty", "open", "--editor", "--with", "zed", "a.rs"]).is_ok()
        );

        // cli, Open In A Viewer Pane (A folder still goes to the editor) and
        // Open In Client Editor (Relative file and default folder).
        let dir = tempfile::tempdir().expect("fixture");
        let file = dir.path().join("PLAN.md");
        std::fs::write(&file, "# Plan").expect("file");
        assert!(opens_in_viewer(&file, None, false));
        assert!(!opens_in_viewer(dir.path(), None, false));
        assert!(!opens_in_viewer(std::path::Path::new("."), None, false));
        assert!(!opens_in_viewer(&file, None, true));
        assert!(!opens_in_viewer(&file, Some("zed"), false));
        for flags in [
            ["--tab", "--editor"],
            ["--tab", "--with=zed"],
            ["--pane=p1", "--editor"],
            ["--screen=s1", "--with=vscode"],
        ] {
            assert!(
                Cli::try_parse_from(["splitty", "open"].into_iter().chain(flags).chain(["a.rs"]))
                    .is_err(),
                "{flags:?}"
            );
        }
    }

    /// cli, Comments Command and Review Command: the listing, `reply` with
    /// its document, number and text, the caller's pane and screen, and a
    /// review of a file or of a text with its title.
    #[test]
    fn comments_and_review_take_their_document_and_the_caller() {
        let cli = Cli::try_parse_from([
            "splitty", "comments", "PLAN.md", "--all", "--json", "--pane", "p1", "--screen", "s1",
        ])
        .expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Comments { document: Some(document), all: true, json: true, pane: Some(pane), screen: Some(screen), action: None, .. }
                if document == "PLAN.md" && pane == "p1" && screen == "s1"
        ));
        let cli = Cli::try_parse_from(["splitty", "comments"]).expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Comments {
                document: None,
                all: false,
                json: false,
                action: None,
                ..
            }
        ));
        let cli = Cli::try_parse_from([
            "splitty",
            "comments",
            "reply",
            "PLAN.md",
            "2",
            "Switched to on-demand computation.",
            "--pane",
            "p1",
        ])
        .expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Comments {
                document: None,
                action: Some(CommentsAction::Reply { document, number: 2, text, pane: Some(pane), screen: None }),
                ..
            } if document == "PLAN.md" && text == "Switched to on-demand computation." && pane == "p1"
        ));
        let cli = Cli::try_parse_from([
            "splitty",
            "comments",
            "reply",
            "viewer-0123456789abcdef",
            "3",
            "-",
        ])
        .expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Comments { action: Some(CommentsAction::Reply { number: 3, text, .. }), .. } if text == "-"
        ));
        for args in [
            vec!["comments", "reply", "PLAN.md", "two", "x"],
            vec!["comments", "reply", "PLAN.md", "2"],
            vec!["comments", "a.md", "b.md"],
            vec!["comments", "PLAN.md", "reply", "PLAN.md", "2", "x"],
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("splitty").chain(args.clone())).is_err(),
                "{args:?}"
            );
        }

        let cli = Cli::try_parse_from(["splitty", "review", "PLAN.md", "--json"]).expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Review { path: Some(path), text: false, title: None, json: true, .. }
                if path == std::path::Path::new("PLAN.md")
        ));
        let cli =
            Cli::try_parse_from(["splitty", "review", "--text", "--title", "Plan"]).expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Review { path: None, text: true, title: Some(title), format: None, .. } if title == "Plan"
        ));
        let cli = Cli::try_parse_from(["splitty", "review", "--text", "--format", "text"])
            .expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Review { text: true, format: Some(format), .. } if format == "text"
        ));
        let cli =
            Cli::try_parse_from(["splitty", "review", "--hook", "claude-code"]).expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Review { path: None, text: false, hook: Some(hook), .. } if hook == "claude-code"
        ));
        for args in [
            vec!["review"],
            vec!["review", "PLAN.md", "--text"],
            vec!["review", "PLAN.md", "--title", "Plan"],
            vec!["review", "--title", "Plan"],
            vec!["review", "PLAN.md", "--format", "text"],
            vec!["review", "--text", "--format", "html"],
            vec!["review", "--hook", "codex"],
            vec!["review", "--hook", "claude-code", "PLAN.md"],
            vec!["review", "--hook", "claude-code", "--text"],
            vec!["review", "--hook", "claude-code", "--title", "Plan"],
            vec!["review", "--hook", "claude-code", "--json"],
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("splitty").chain(args.clone())).is_err(),
                "{args:?}"
            );
        }
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
                action: DaemonAction::Stop {
                    end_sessions: true,
                    ..
                }
            }
        ));
        let cli = Cli::try_parse_from(["splitty", "daemon", "stop"]).expect("parse");
        assert!(matches!(
            cli.command,
            Commands::Daemon {
                action: DaemonAction::Stop {
                    end_sessions: false,
                    ..
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
