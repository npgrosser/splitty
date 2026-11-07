//! A dedicated API stream. Session event delivery uses a different connection.
use serde::de::DeserializeOwned;
use splitty_core::native_api::{Operation, Request, Role};
use splitty_transport::{budget::Charged, codec, peer::Sender, wire};
use std::io;

pub(crate) enum ApiStream {
    #[cfg(not(target_os = "windows"))]
    Embedded(splitty_server::native::client::Client),
    Owned(splitty_remote::owner::Connection),
}

/// Decode a bounded API reply while its wire lease is still held. This is
/// shared by independent local readers without coupling their schedulers.
pub(crate) fn decode<T: DeserializeOwned>(
    client: &ApiStream,
    reply: &wire::Reply,
) -> io::Result<Charged<T>> {
    decode_with_budget(&client.sender().context().budget, reply)
}

pub(crate) fn decode_with_budget<T: DeserializeOwned>(
    budget: &splitty_transport::budget::ConnectionBudget,
    reply: &wire::Reply,
) -> io::Result<Charged<T>> {
    if !(200..300).contains(&reply.status) || reply.content_type != wire::ContentType::Json {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("local API returned status {}", reply.status),
        ));
    }
    let bytes = reply.body.as_bytes();
    if bytes.len() > splitty_core::native_api::MAX_API_BODY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local API response exceeds native limit",
        ));
    }
    let lease = codec::reserve_json_typed(bytes, budget, false)?;
    let value = serde_json::from_slice(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid local API response"))?;
    Ok(lease.charge(value))
}
#[cfg(not(target_os = "windows"))]
impl From<splitty_server::native::client::Client> for ApiStream {
    fn from(client: splitty_server::native::client::Client) -> Self {
        Self::Embedded(client)
    }
}
impl ApiStream {
    pub fn sender(&self) -> Sender {
        match self {
            #[cfg(not(target_os = "windows"))]
            Self::Embedded(client) => client.sender(),
            Self::Owned(client) => client.sender(),
        }
    }
    pub async fn request(
        &mut self,
        operation: Operation,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> io::Result<Charged<wire::Reply>> {
        let request = Request { operation, args };
        request.validate(Role::Main).map_err(io::Error::other)?;
        match self {
            #[cfg(not(target_os = "windows"))]
            Self::Embedded(client) => client.request(operation, request.args).await,
            Self::Owned(client) => {
                let name = serde_json::to_value(operation)?
                    .as_str()
                    .ok_or_else(|| io::Error::other("invalid operation"))?
                    .to_owned();
                let deadline = operation
                    .description()
                    .deadline
                    .duration()
                    .map(|d| d + std::time::Duration::from_secs(1));
                client.request(name, request.args, deadline).await
            }
        }
    }
    pub async fn finish(self) -> io::Result<()> {
        match self {
            #[cfg(not(target_os = "windows"))]
            Self::Embedded(client) => client.finish().await,
            Self::Owned(client) => client.finish().await,
        }
    }
}
