//! `splitty remote-bridge`: the daemon's WebSocket over standard input and
//! output, in the frames of `splitty_remote::bridge`.
//!
//! It authenticates here, on the host, like the local CLI: the token from the
//! owner-only token file buys a single-use ticket over loopback, and the
//! WebSocket is opened with the daemon's own origin. Nothing secret goes over
//! stdio; the SSH connection that carries the bridge is its authentication.

use futures::{SinkExt, StreamExt};
use splitty_remote::bridge::{read_frame, write_frame, Frame};
use tokio_tungstenite::tungstenite::Message;

/// Relay until standard input ends or the daemon closes the WebSocket. The
/// error is the reason to print.
pub async fn run() -> Result<(), String> {
    let info = super::running_daemon().ok_or("No Splitty daemon is running on this host.")?;
    let (token, _) = splitty_core::auth::resolve("").map_err(|e| e.to_string())?;
    let socket = crate::cli::socket::open(&format!("127.0.0.1:{}", info.port), &token).await?;
    let (mut to_daemon, mut from_daemon) = socket.split();

    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let upstream = async {
        loop {
            let message = match read_frame(&mut stdin).await {
                Ok(Some(Frame::Text(text))) => Message::text(text),
                Ok(Some(Frame::Binary(bytes))) => Message::binary(bytes),
                Ok(None) => {
                    let _ = to_daemon.close().await;
                    return Ok(());
                }
                Err(e) => return Err(format!("bad frame on stdin: {e}")),
            };
            to_daemon
                .send(message)
                .await
                .map_err(|e| format!("the daemon's WebSocket failed: {e}"))?;
        }
    };
    let downstream = async {
        while let Some(message) = from_daemon.next().await {
            let frame = match message.map_err(|e| format!("the daemon's WebSocket failed: {e}"))? {
                Message::Text(text) => Frame::Text(text.as_str().to_owned()),
                Message::Binary(bytes) => Frame::Binary(bytes.to_vec()),
                Message::Close(_) => break,
                // Pings are answered by the WebSocket itself.
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            };
            write_frame(&mut stdout, &frame)
                .await
                .map_err(|e| format!("cannot write to stdout: {e}"))?;
        }
        Ok(())
    };
    tokio::select! {
        result = upstream => result,
        result = downstream => result,
    }
}
