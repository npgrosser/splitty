//! Service construction without a browser listener or browser credentials.
use crate::client_session;
use splitty_core::{mux::Store, protocol::notify::NotificationEngine};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

pub struct Runtime {
    pub state: Arc<super::State>,
    pub pty: Arc<splitty_core::pty::manager::Manager>,
}

/// The caller chooses lifetime and terminal ownership explicitly. Embedded
/// callers pass no persisted workspace and use LocalTerminals; only daemon
/// callers adopt keepers or write the daemon workspace.
pub fn build(
    server_mode: &str,
    initial_state: Option<splitty_core::mux::WorkspaceState>,
    terminals: Arc<dyn splitty_core::pty::terminal::TerminalFactory>,
    control_socket: &Path,
) -> Runtime {
    let config = splitty_core::config::load();
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
    assemble(
        server_mode,
        initial_state,
        terminals,
        control_socket,
        Stores {
            config,
            config_path: splitty_core::config::config_path(),
            server_config,
            themes_dir,
            template_store,
            hub: crate::hub::Hub::system(),
        },
    )
}

/// Where a service of its own keeps its settings, templates, themes and
/// host metadata, and how its hub reaches hosts.
pub struct Home {
    pub config_dir: PathBuf,
    /// The SSH configuration whose hosts the directory lists, if any.
    pub ssh_dir: Option<PathBuf>,
    pub connector: Box<dyn crate::hub::Connector>,
}

/// A service that reads and writes only below `home.config_dir`, for
/// callers that must not touch the user's configuration. Unlike `build`, it
/// does not rewrite the settings file at start.
pub fn build_in(
    server_mode: &str,
    initial_state: Option<splitty_core::mux::WorkspaceState>,
    terminals: Arc<dyn splitty_core::pty::terminal::TerminalFactory>,
    control_socket: &Path,
    home: Home,
) -> Runtime {
    let dir = home.config_dir;
    let config_path = dir.join("config.toml");
    assemble(
        server_mode,
        initial_state,
        terminals,
        control_socket,
        Stores {
            config: splitty_core::config::load_from(&config_path),
            config_path,
            server_config: splitty_core::config::load_server_from(&dir.join("server.toml")),
            themes_dir: dir.join("themes"),
            template_store: Arc::new(splitty_core::template::FileStore::new(
                dir.join("templates"),
            )),
            hub: crate::hub::Hub::new(home.connector, dir, home.ssh_dir),
        },
    )
}

/// What a service reads its settings from and keeps its metadata in.
struct Stores {
    config: splitty_core::config::HomeConfig,
    config_path: PathBuf,
    server_config: splitty_core::config::ServerConfig,
    themes_dir: PathBuf,
    template_store: Arc<splitty_core::template::FileStore>,
    hub: crate::hub::Hub,
}

fn assemble(
    server_mode: &str,
    initial_state: Option<splitty_core::mux::WorkspaceState>,
    terminals: Arc<dyn splitty_core::pty::terminal::TerminalFactory>,
    control_socket: &Path,
    stores: Stores,
) -> Runtime {
    if let Ok(program) = std::env::current_exe() {
        splitty_remote::askpass::enable(program);
    }
    let Stores {
        config,
        config_path,
        server_config,
        themes_dir,
        template_store,
        hub,
    } = stores;
    let shell_profiles = Arc::new(RwLock::new(
        splitty_core::shell_profiles::state_from_settings(&config.shells),
    ));

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
    let notify_tx = splitty_core::protocol::notify::NotificationBus::default();
    let tx = notify_tx.clone();
    let engine = NotificationEngine::new(Box::new(move |event| {
        tx.send(&event);
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

    let hub = Arc::new(hub);
    let client_state = Arc::new(
        client_session::ClientState::new(
            mux_store.clone(),
            dispatcher,
            shell_profiles.clone(),
            effect_runner,
            env,
            notify_tx,
            server_mode.to_string(),
        )
        .with_hub(hub.clone()),
    );

    let state = Arc::new(super::State {
        instance: super::new_instance(),
        owner_shutdown: Arc::new(tokio::sync::Notify::new()),
        hub,
        mux_store,
        client_state,
        config: Arc::new(RwLock::new(config)),
        config_path,
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
        server_mode: server_mode.to_string(),
    });
    super::pane_state::start(&state);
    Runtime {
        state,
        pty: pty_manager,
    }
}

impl Runtime {
    /// Embedded service ownership ends here, including managed jobs.
    pub async fn close(&self) -> std::io::Result<()> {
        let ((), result) = tokio::join!(
            self.state.client_state.jobs.shutdown(),
            self.pty.shutdown(false)
        );
        result
    }
}
