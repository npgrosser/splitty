//! Reaching a remote host with the system `ssh`, installing the client's own
//! server there and keeping a tunnel to its daemon.
//!
//! Platform-neutral and free of the server crates, so the CLI (`splitty
//! connect`) and the desktop apps can both use it: [`tunnel`] opens and keeps
//! the tunnel, [`hosts`] lists the hosts the user knows. What `ssh` sends is
//! one command line that the host's login shell parses again; [`ssh`] makes
//! that line explicit, and every value in it is quoted.

pub mod bridge;
pub mod handshake;
pub mod hosts;
pub mod install;
pub mod ssh;
pub mod tunnel;
