//! Core library for the Splitty terminal multiplexer.
//!
//! This crate contains all shared domain logic, data types, and subsystems
//! used by the Splitty server. It is transport-agnostic -- the HTTP/WebSocket
//! layer lives in a separate server crate.
//!
//! # Modules
//!
//! - [`ids`] -- Strongly-typed identifiers (pane, screen, session, node, client, floating).
//! - [`auth`] -- Token-based authentication: generation, persistence, and constant-time validation.
//! - [`config`] -- User and server configuration (TOML on disk, camelCase JSON on the wire).
//! - [`daemon`] -- Daemon lifecycle helpers: info file, PID checks, state persistence, session restore.
//! - [`keeper`] -- Session keepers: the process that keeps a daemon session's PTY and program alive across daemon restarts, its protocol and the daemon's side of it.
//! - [`mux`] -- Multiplexer state machine: layout tree, reducer, effects, and store.
//! - [`protocol`] -- WebSocket protocol messages, dispatcher, and notification engine.
//! - [`pty`] -- PTY session management: spawn, read/write, resize, scrollback, OSC parsing.
//! - [`template`] -- Saved layout templates (YAML-based CRUD).
//! - [`theme`] -- Theme system: palette-based token derivation, bundled and custom themes.

pub mod auth;
pub mod config;
pub mod daemon;
pub mod ids;
#[cfg(unix)]
pub mod keeper;
pub mod mux;
pub mod protocol;
pub mod pty;
pub mod shell_profiles;
pub mod template;
pub mod theme;
