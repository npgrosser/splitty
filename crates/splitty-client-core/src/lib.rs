//! Experimental UI-free adapter for the existing native session protocol.
//! Connections retain their source identity and delivery leases through UI
//! consumption. The embedded composition is deliberately absent on Windows.

pub mod annotations;
pub mod appearance;
pub mod connection;
pub mod document;
#[cfg(not(target_os = "windows"))]
pub mod embedded;
pub mod external_links;
pub mod hub;
pub mod local_service;
pub mod native_workspace;
pub mod notifications;
pub mod settings;
pub mod shell_profiles;
pub mod templates;
pub mod workspace;
pub mod zoom;

pub use appearance::{AppearanceSnapshot, ColorScheme};
pub use connection::{AnnotationDraft, Client, Event, Output, ScreenIds};
pub use document::{Document, DocumentRead};
#[cfg(not(target_os = "windows"))]
pub use embedded::EmbeddedService;
pub use local_service::LocalServiceClient;
pub use splitty_remote::askpass::is_confirmation as ssh_prompt_is_confirmation;
pub use splitty_remote::askpass::watch_parent as watch_askpass_parent;
pub use splitty_remote::askpass::{
    helper as ssh_askpass_helper, prompts as ssh_prompts, respond as answer_ssh_prompt,
};
pub use splitty_remote::ssh::set_askpass_program;
pub use workspace::{SessionRoute, SourceWorkspace, WindowWorkspace, WorkspaceDelta};

mod api_stream;
pub mod owned_service;
pub mod ports;
