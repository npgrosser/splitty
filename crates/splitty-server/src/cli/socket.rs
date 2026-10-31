//! The server's WebSocket for a client on the same machine (`remote-bridge`,
//! `attach`). It authenticates like the local CLI: the process token buys a
//! single-use ticket over loopback, and the WebSocket is opened with the
//! server's own origin. The token never leaves this process.

use super::{api_post, ServerInfo, Transport};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// An open WebSocket to a server.
pub type ServerSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open the WebSocket of the server on `addr` (loopback) with its process token.
/// The error is the reason to print.
pub async fn open(addr: &str, token: &str) -> Result<ServerSocket, String> {
    let server = ServerInfo {
        transport: Transport::Tcp {
            addr: addr.to_owned(),
            token: token.to_owned(),
        },
        source: "token",
    };
    let ticket = api_post(&server, "/api/ws-ticket", serde_json::json!({}))
        .await
        .map_err(|e| format!("the server gave no WebSocket ticket: {e}"))?;
    let ticket = ticket["ticket"]
        .as_str()
        .ok_or("the server gave no WebSocket ticket")?;

    let mut request = format!("ws://{addr}/ws?ticket={ticket}")
        .into_client_request()
        .map_err(|e| e.to_string())?;
    let origin = format!("http://{addr}")
        .parse()
        .map_err(|_| "invalid origin".to_owned())?;
    request.headers_mut().insert("Origin", origin);
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| format!("cannot open the server's WebSocket: {e}"))?;
    Ok(socket)
}
