//! Session keepers: in daemon mode, every session's PTY and program belong to
//! a keeper process of their own, the `splitty` binary run as
//! `splitty session-keeper`, which outlives the daemon. The daemon is the
//! keeper's controller: it writes input, resizes, hangs up or stops, and reads
//! the output over a private Unix socket. A starting daemon adopts the live
//! keepers instead of starting the programs again.
//!
//! - [`wire`] -- the framed protocol between a keeper and its controller.
//! - [`dir`] -- where keepers are found, and the peer checks of both sides.
//! - [`process`] -- the keeper process itself.
//! - [`client`] -- the controller's side: the kept terminal a daemon's session
//!   runs on, the daemon's terminal factory, and finding running keepers.

pub mod client;
pub mod dir;
pub mod process;
pub mod wire;
