//! Experimental UI-free adapter for the existing native session protocol.
//! Connections retain their source identity and delivery leases through UI
//! consumption. The embedded composition is deliberately absent on Windows.

pub mod connection;
pub mod document;
#[cfg(not(target_os = "windows"))]
pub mod embedded;

pub use connection::{AnnotationDraft, Client, Event, Output, ScreenIds};
pub use document::{Document, DocumentRead};
#[cfg(not(target_os = "windows"))]
pub use embedded::EmbeddedService;
