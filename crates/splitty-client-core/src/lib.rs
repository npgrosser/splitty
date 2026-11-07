//! Experimental UI-free adapter for the existing native session protocol.
//! Connections retain their source identity and delivery leases through UI
//! consumption. The embedded composition is deliberately absent on Windows.

pub mod appearance;
pub mod connection;
pub mod document;
#[cfg(not(target_os = "windows"))]
pub mod embedded;
pub mod native_workspace;
pub mod workspace;

#[cfg(not(target_os = "windows"))]
pub use appearance::AppearanceReader;
pub use appearance::{AppearanceSnapshot, ColorScheme};
pub use connection::{AnnotationDraft, Client, Event, Output, ScreenIds};
pub use document::{Document, DocumentRead};
#[cfg(not(target_os = "windows"))]
pub use embedded::EmbeddedService;
pub use workspace::{LocalWorkspace, SessionRoute, WorkspaceDelta};
