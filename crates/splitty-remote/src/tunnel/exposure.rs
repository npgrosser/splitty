//! Shared reservation authority for deliberately exposed loopback ports.
//! Platform limitations never select an implicit terminal-service listener.
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod registry;
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
pub(super) use registry::test_share;
#[cfg(all(feature = "testing", any(target_os = "linux", target_os = "macos")))]
pub use registry::testing;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use registry::{check_sharing_supported, Share};

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub use unsupported::{check_sharing_supported, Share};
