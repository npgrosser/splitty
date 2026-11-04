//! Owned, serialized operation results, independent of an HTTP implementation.
//!
//! Serializing here commits a result while any job authorization guard is still
//! held. Adapters can then deliver it without retaining a grant lock over I/O.

use http::StatusCode;
use serde::Serialize;
use splitty_transport::budget::{Buffer, ConnectionBudget, Lease};

tokio::task_local! { static DELIVERY: ConnectionBudget; }

pub async fn with_budget<T>(
    budget: ConnectionBudget,
    operation: impl std::future::Future<Output = T>,
) -> T {
    DELIVERY.scope(budget, operation).await
}

#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<&'static str>,
    pub(crate) bytes: Vec<Lease>,
}

pub struct ResponseBytes {
    body: Vec<u8>,
    _bytes: Vec<Lease>,
}
impl AsRef<[u8]> for ResponseBytes {
    fn as_ref(&self) -> &[u8] {
        &self.body
    }
}
impl Reply {
    pub fn into_body(self) -> ResponseBytes {
        ResponseBytes {
            body: self.body,
            _bytes: self.bytes,
        }
    }
}

pub(crate) struct Payload<T>(pub T);

pub(crate) trait IntoReply {
    fn into_reply(self) -> Reply;
}

impl IntoReply for Reply {
    fn into_reply(self) -> Reply {
        self
    }
}

impl<T: Serialize> IntoReply for Payload<T> {
    fn into_reply(self) -> Reply {
        (StatusCode::OK, self).into_reply()
    }
}

impl<T: Serialize> IntoReply for (StatusCode, Payload<T>) {
    fn into_reply(self) -> Reply {
        let budget = DELIVERY.try_with(Clone::clone).unwrap_or_default();
        let mut buffer = Buffer::new(budget, splitty_transport::wire::MAX_FRAME_BYTES);
        match serde_json::to_writer(&mut buffer, &self.1 .0) {
            Ok(()) => {
                let (body, bytes) = buffer.finish();
                Reply {
                    status: self.0.as_u16(),
                    body,
                    content_type: Some("application/json"),
                    bytes,
                }
            }
            Err(error) => {
                tracing::error!(%error, "service reply serialization failed");
                Reply {
                    status: 500,
                    body: b"{\"error\":\"response serialization failed\"}".to_vec(),
                    content_type: Some("application/json"),
                    bytes: Vec::new(),
                }
            }
        }
    }
}

impl IntoReply for StatusCode {
    fn into_reply(self) -> Reply {
        Reply {
            status: self.as_u16(),
            body: Vec::new(),
            content_type: None,
            bytes: Vec::new(),
        }
    }
}

impl IntoReply for crate::jobs::JobError {
    fn into_reply(self) -> Reply {
        (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Payload(self),
        )
            .into_reply()
    }
}

pub(crate) fn job(access: crate::jobs::Access, status: StatusCode) -> Reply {
    let reply = (status, Payload(&access.value)).into_reply();
    drop(access);
    reply
}
