//! Strongly-typed identifier newtypes for domain entities.
//!
//! Each ID wraps a `String` (UUID-based by default) and is serialized transparently.
//! The `define_id!` macro generates `PaneId`, `ScreenId`, `SessionId`, `NodeId`,
//! `ClientId`, `FloatingId`, `JobId`, and `AgentId` with serde support.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }

            /// Generate a short prefixed ID (e.g. "pane-a1b2c3d4").
            pub fn with_prefix(prefix: &str) -> Self {
                Self(format!(
                    "{}-{}",
                    prefix,
                    &uuid::Uuid::new_v4().to_string()[..8]
                ))
            }

            pub fn from(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }
    };
}

define_id!(PaneId);
define_id!(ScreenId);
define_id!(SessionId);
define_id!(NodeId);
define_id!(ClientId);
define_id!(FloatingId);
define_id!(JobId);
define_id!(AgentId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ids_are_unique() {
        let a = PaneId::new();
        let b = PaneId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn from_string() {
        let id = ScreenId::from("test-123");
        assert_eq!(id.as_str(), "test-123");
        assert_eq!(id.to_string(), "test-123");
    }

    #[test]
    fn serde_roundtrip() {
        let id = PaneId::from("pane-abc");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"pane-abc\"");
        let back: PaneId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn type_safety() {
        // This ensures PaneId and ScreenId are not interchangeable at compile time.
        // The following would fail to compile:
        // let pane: PaneId = ScreenId::from("x");
        let _pane = PaneId::from("x");
        let _screen = ScreenId::from("x");
    }
}
