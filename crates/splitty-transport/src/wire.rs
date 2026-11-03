//! Native transport v1. Mux/session versioning is independent.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::time::Duration;

pub const VERSION: u16 = 1;
/// Application mux version shared by native desktop clients on every platform.
pub const MUX_PROTOCOL_VERSION: u32 = 1;
pub const MAGIC: &[u8; 8] = b"SPLITTY\x01";
pub const HEADER_BYTES: usize = 5;
pub const DELIVERY_BYTES: usize = 20; // generation u32 + unpredictable receipt (16 bytes)
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CONTROL_BYTES: usize = 4096;
pub const MAX_API_BYTES: usize = 1024 * 1024;
pub const MAX_PENDING_REQUESTS: usize = 64;
pub const CONNECTION_BYTES: usize = 32 * 1024 * 1024;
pub const PROCESS_BYTES: usize = 128 * 1024 * 1024;
pub const CONNECTION_CONTROL_BYTES: usize = 64 * 1024;
pub const PROCESS_CONTROL_BYTES: usize = 8 * 1024 * 1024;
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
pub const FRAME_TIMEOUT: Duration = Duration::from_secs(60);
pub const PING_INTERVAL: Duration = Duration::from_secs(15);
pub const LIVENESS_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Kind {
    Hello = 0,
    Text = 1,
    Binary = 2,
    Accept = 3,
    Ready = 4,
    Request = 5,
    Reply = 6,
    Cancel = 7,
    Ping = 8,
    Pong = 9,
    Credit = 10,
    Progress = 11,
    Closed = 12,
    ReadyAccept = 13,
}

impl Kind {
    pub fn is_data(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Binary | Self::Request | Self::Reply
        )
    }

    pub fn payload_limit(self) -> usize {
        if self == Self::Request {
            MAX_API_BYTES + DELIVERY_BYTES
        } else if self.is_data() {
            MAX_FRAME_BYTES - HEADER_BYTES
        } else {
            MAX_CONTROL_BYTES - HEADER_BYTES
        }
    }
}

impl TryFrom<u8> for Kind {
    type Error = &'static str;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => Self::Hello,
            1 => Self::Text,
            2 => Self::Binary,
            3 => Self::Accept,
            4 => Self::Ready,
            5 => Self::Request,
            6 => Self::Reply,
            7 => Self::Cancel,
            8 => Self::Ping,
            9 => Self::Pong,
            10 => Self::Credit,
            11 => Self::Progress,
            12 => Self::Closed,
            13 => Self::ReadyAccept,
            _ => return Err("unknown native frame kind"),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hello {
    pub transport_version: u16,
    /// Explicit host-open intent is accepted only by the local owner endpoint.
    /// A remote bridge/direct embedded carrier rejects it before preparation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_host: Option<OpenHost>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenHost {
    pub owner_instance: String,
    pub target: String,
    pub update: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub frame_bytes: u32,
    pub api_bytes: u32,
    pub pending_requests: u16,
    pub connection_bytes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            frame_bytes: 16 * 1024 * 1024,
            api_bytes: 1024 * 1024,
            pending_requests: 64,
            connection_bytes: 32 * 1024 * 1024,
        }
    }
}

impl Limits {
    /// A peer cannot raise local ceilings or force an oversized allocation by
    /// announcing another limit. A service must support at least one frame.
    pub fn valid(&self) -> bool {
        let local = Self::default();
        self.frame_bytes > 25
            && self.frame_bytes <= local.frame_bytes
            && self.api_bytes > 0
            && self.api_bytes <= local.api_bytes
            && self.pending_requests > 0
            && self.pending_requests <= local.pending_requests
            && self.connection_bytes >= self.frame_bytes
            && self.connection_bytes <= local.connection_bytes
    }
}

/// Describes a connection the adapter already opened. It grants no authority
/// to change the backend or to trust a host-supplied target identifier.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Purpose {
    Direct,
    Host {
        target: String,
    },
    /// Launcher-owned embedded service carrier, never a daemon/host endpoint.
    Owner,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ready {
    pub generation: u32,
    pub instance: String,
    pub mux_protocol: u32,
    pub purpose: Purpose,
    pub limits: Limits,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadyAccept {
    pub generation: u32,
    pub instance: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: u32,
    pub operation: String,
    #[serde(default)]
    pub args: Map<String, Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentType {
    Json,
    Text,
    Empty,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reply {
    pub id: u32,
    pub status: u16,
    pub content_type: ContentType,
    /// Serialized response bytes as UTF-8 (JSON APIs have no arbitrary headers).
    pub body: String,
    /// A cancelled/lost mutation response is not proof that nothing happened.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub uncertain: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cancel {
    pub generation: u32,
    pub id: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ping {
    pub nonce: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Credit {
    pub generation: u32,
    /// Hex receipt from a delivered data frame, once only. A cumulative byte
    /// count or guessable next sequence must not release undelivered IPC data.
    pub receipt: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub phase: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum CloseCode {
    Incompatible,
    Busy,
    Timeout,
    Protocol,
    BackendLost,
    Closed,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Closed {
    pub code: CloseCode,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Delivery {
    pub generation: u32,
    pub receipt: [u8; 16],
}

impl Delivery {
    pub fn new(generation: u32) -> Self {
        use rand::RngCore;
        let mut receipt = [0; 16];
        rand::thread_rng().fill_bytes(&mut receipt);
        Self {
            generation,
            receipt,
        }
    }

    pub fn decode(bytes: &[u8]) -> Option<(Self, &[u8])> {
        if bytes.len() < DELIVERY_BYTES {
            return None;
        }
        let generation = u32::from_be_bytes(bytes[..4].try_into().ok()?);
        if generation == 0 {
            return None;
        }
        let end = bytes.len() - 16;
        let receipt = bytes[end..].try_into().ok()?;
        Some((
            Self {
                generation,
                receipt,
            },
            &bytes[4..end],
        ))
    }
}

// Only these wire types may use the control decoder's prevalidated conversion.
// Keep the trait sealed: arbitrary Deserialize implementations have no such bound.
mod sealed {
    pub trait Control {}
}
pub trait ControlMessage: serde::de::DeserializeOwned + sealed::Control {
    const KIND: Kind;
    fn retained_bytes(&self) -> usize;
}
macro_rules! control_messages {
    ($($type:ident => $kind:ident, |$value:ident| $heap:expr);+ $(;)?) => { $(
        impl sealed::Control for $type {}
        impl ControlMessage for $type {
            const KIND: Kind = Kind::$kind;
            fn retained_bytes(&$value) -> usize { std::mem::size_of::<Self>() + $heap }
        }
    )+ };
}
control_messages! {
    Hello => Hello, |self| self.open_host.as_ref().map_or(0, |host| host.owner_instance.capacity() + host.target.capacity());
    Ready => Ready, |self| self.instance.capacity() + match &self.purpose { Purpose::Host { target } => target.capacity(), _ => 0 };
    ReadyAccept => ReadyAccept, |self| self.instance.capacity();
    Cancel => Cancel, |self| 0;
    Ping => Ping, |self| 0;
    Credit => Credit, |self| self.receipt.capacity();
    Progress => Progress, |self| self.phase.capacity() + self.message.capacity();
    Closed => Closed, |self| self.message.capacity();
}
