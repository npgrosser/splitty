//! Experimental UI-free adapter for the existing native session protocol.
//! Connections retain their source identity and delivery leases through UI
//! consumption. The embedded composition is deliberately absent on Windows.

pub mod appearance;
pub mod connection;
pub mod document;
#[cfg(not(target_os = "windows"))]
pub mod embedded;
pub mod external_links;
#[cfg(not(target_os = "windows"))]
pub mod local_settings;
pub mod native_workspace;
pub mod shell_profiles;
pub mod workspace;

pub use appearance::{AppearanceSnapshot, ColorScheme};
pub use connection::{AnnotationDraft, Client, Event, Output, ScreenIds};
pub use document::{Document, DocumentRead};
#[cfg(not(target_os = "windows"))]
pub use embedded::EmbeddedService;
#[cfg(not(target_os = "windows"))]
pub use local_settings::LocalSettingsReader;
pub use workspace::{LocalWorkspace, SessionRoute, WorkspaceDelta};
