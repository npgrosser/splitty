//! Transport-independent UI operations and their fixed authority boundary.
//!
//! Browser authentication and owner/grant administration belong to their
//! adapters. A native UI client receives only the operations in [`catalog`].

pub mod catalog;
mod dispatch;
pub(crate) mod jobs;
pub(crate) mod operations;
pub(crate) mod pane_state;
pub mod reply;
pub(crate) mod review;
pub mod runtime;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// State of one terminal service. It has no browser token, session store,
/// origin or listening address and can be shared by independent adapters.
#[derive(Clone)]
pub struct State {
    pub instance: String,
    /// Triggered only by the owner socket after an exact daemon instance check.
    pub owner_shutdown: Arc<tokio::sync::Notify>,
    pub mux_store: Arc<splitty_core::mux::Store>,
    pub client_state: Arc<crate::client_session::ClientState>,
    pub config: Arc<RwLock<splitty_core::config::HomeConfig>>,
    /// The settings file that settings writes replace.
    pub config_path: PathBuf,
    pub shell_profiles: Arc<RwLock<splitty_core::shell_profiles::ShellProfilesState>>,
    pub template_store: Arc<splitty_core::template::FileStore>,
    pub themes_dir: PathBuf,
    pub version: String,
    pub commit: String,
    pub build_time: String,
    pub server_mode: String,
    pub hub: Arc<crate::hub::Hub>,
}

pub fn new_instance() -> String {
    use rand::RngCore;
    let mut bytes = [0; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}
