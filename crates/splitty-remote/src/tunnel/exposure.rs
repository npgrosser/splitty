//! Shared reservation authority for deliberately exposed loopback ports.
//! Platform limitations never select an implicit terminal-service listener.
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod registry;
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
pub(super) use registry::test_share;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use registry::{bind_gateway, check_sharing_supported, GatewayListener, Share};

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub use unsupported::{bind_gateway, check_sharing_supported, GatewayListener, Share};
