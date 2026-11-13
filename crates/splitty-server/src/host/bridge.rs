//! The host end of `remote-bridge --transport native-v1`. SSH authenticates
//! the owner; only this host resolves its private daemon endpoint. Each bridge
//! owns one native client identity, independent of other remote windows.

use crate::native::relay::{forward, validate_input};
use splitty_transport::{admission, negotiation, peer::Peer, queue, wire::Kind};
use std::io;

#[cfg(unix)]
pub async fn run() -> Result<(), String> {
    let info = splitty_core::daemon::read_info_checked()
        .map_err(|e| e.to_string())?
        .filter(|info| splitty_core::daemon::is_running(info.pid))
        .ok_or("No Splitty daemon is running on this host.")?;
    serve(tokio::io::stdin(), tokio::io::stdout(), &info)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(unix))]
pub async fn run() -> Result<(), String> {
    Err("The native daemon endpoint requires Linux or macOS; use WSL on Windows.".into())
}

#[cfg(unix)]
async fn serve<R, W>(
    mut read: R,
    mut write: W,
    info: &splitty_core::daemon::DaemonInfo,
) -> io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let admission = admission::Admission::global();
    let _carrier = admission.carrier().map_err(io::Error::other)?;
    let opening = admission
        .begin(admission::Assignment::new(
            admission::Backend::new(),
            admission::Domain::new(),
        ))
        .map_err(io::Error::other)?;
    let context = queue::Context::default();
    negotiation::server(&mut read, &mut write, &context.budget).await?;
    let connector = crate::native::client::Connector::default();
    let mut daemon = connector.local(info).await?;
    // The verified daemon instance crosses the bridge; its local filesystem
    // path and the browser adapter's credentials never do.
    negotiation::offer(&mut read, &mut write, &context.budget, &daemon.ready).await?;
    let _stream = opening.admit().map_err(io::Error::other)?;
    let mut client = Peer::start(read, write, &daemon.ready, context);
    let result = async {
        loop {
            tokio::select! {
                incoming = client.recv() => {
                    let Some(message) = incoming else { break; };
                    validate_input(&message.frame, &client.sender.context().budget)?;
                    forward(&daemon.sender(), &message.frame)?;
                }
                outgoing = daemon.recv() => {
                    let Some(message) = outgoing else { break; };
                    if !matches!(message.frame.kind, Kind::Text | Kind::Binary | Kind::Reply) {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "unexpected daemon reply"));
                    }
                    forward(&client.sender, &message.frame)?;
                }
            }
        }
        Ok(())
    }.await;
    // Invalid framing remains an error even when the receive queue ended.
    // Both writers/readers finish before their permits are released.
    let (client_result, daemon_result) = tokio::join!(client.finish(), daemon.finish());
    result.and(client_result).and(daemon_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_transport::{codec::Frame, wire};
    #[test]
    fn a_host_stream_cannot_acquire_hub_or_browser_authority() {
        use crate::service::catalog::{Scope, OPERATIONS};
        for description in OPERATIONS
            .iter()
            .filter(|description| description.scope == Scope::LocalHub)
        {
            let args = description
                .arguments
                .iter()
                .filter(|argument| !argument.optional)
                .map(|argument| argument.name)
                .map(|name| {
                    (
                        name.to_owned(),
                        if name == "body" {
                            serde_json::json!({})
                        } else {
                            serde_json::json!("target")
                        },
                    )
                })
                .collect();
            let frame = Frame::data_json(
                Kind::Request,
                wire::Delivery::new(1),
                &wire::Request {
                    id: 1,
                    operation: description.name.into(),
                    args,
                },
            )
            .unwrap();
            assert_eq!(
                validate_input(&frame, &Default::default())
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::PermissionDenied,
                "{}",
                description.name
            );
        }
        for operation in ["bootstrap", "proxy", "agent.create"] {
            let frame = Frame::data_json(
                Kind::Request,
                wire::Delivery::new(1),
                &wire::Request {
                    id: 1,
                    operation: operation.into(),
                    args: serde_json::Map::new(),
                },
            )
            .unwrap();
            assert!(
                validate_input(&frame, &Default::default()).is_err(),
                "{operation}"
            );
        }
        let frame = Frame::data_json(
            Kind::Request,
            wire::Delivery::new(1),
            &wire::Request {
                id: 1,
                operation: "mux.status".into(),
                args: serde_json::Map::new(),
            },
        )
        .unwrap();
        validate_input(&frame, &Default::default()).unwrap();
    }
}
