//! Shared bounded HTTP client for owner/scoped Unix sockets and explicit TCP.
mod agent_guide;
pub mod attach;
mod discovery;
pub mod download;
pub mod editor;
pub mod jobs;
pub mod socket;
pub use discovery::{discover, ServerInfo, Transport};
use serde_json::{json, Value};

pub async fn request(
    server: &ServerInfo,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<(u16, Value), CliError> {
    let timeout = std::time::Duration::from_secs(35);
    request_within(server, method, path, body, Some(timeout)).await
}

/// [`request`] with a timeout of its own: `None` waits as long as the server
/// takes, for requests the server bounds itself.
pub async fn request_within(
    server: &ServerInfo,
    method: &str,
    path: &str,
    body: Option<Value>,
    timeout: Option<std::time::Duration>,
) -> Result<(u16, Value), CliError> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(3));
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    let (builder, url, token) = match &server.transport {
        Transport::LocalSocket(socket) => (
            builder.unix_socket(socket.clone()),
            format!("http://localhost{path}"),
            None,
        ),
        Transport::Tcp { addr, token } => (builder, format!("http://{addr}{path}"), Some(token)),
    };
    let client = builder
        .build()
        .map_err(|e| CliError::Request(e.to_string()))?;
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|e| CliError::Request(e.to_string()))?;
    let mut request = client.request(method, url);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let mut response = request
        .send()
        .await
        .map_err(|e| CliError::Request(e.to_string()))?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    const MAX_RESPONSE: usize = 16 * 1024 * 1024;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| CliError::Request(e.to_string()))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err(CliError::Request("response exceeds 16 MiB".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) if !(200..300).contains(&status) => {
            json!({"error":String::from_utf8_lossy(&bytes).chars().take(2000).collect::<String>()})
        }
        Err(e) => return Err(CliError::Request(format!("invalid JSON response: {e}"))),
    };
    Ok((status, value))
}
fn interpret(status: u16, body: Value) -> Result<Value, CliError> {
    if !(200..300).contains(&status) {
        return Err(CliError::Api(
            status,
            body["error"].as_str().unwrap_or("unknown error").into(),
        ));
    }
    Ok(body)
}
pub async fn api_get(server: &ServerInfo, path: &str) -> Result<Value, CliError> {
    let (status, body) = request(server, "GET", path, None).await?;
    interpret(status, body)
}
pub async fn api_post(server: &ServerInfo, path: &str, body: Value) -> Result<Value, CliError> {
    let (status, body) = request(server, "POST", path, Some(body)).await?;
    interpret(status, body)
}
pub async fn send_mux_op(
    server: &ServerInfo,
    screen_id: &str,
    op: Value,
) -> Result<Value, CliError> {
    api_post(server, "/api/mux/op", json!({"screenId":screen_id,"op":op})).await
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("no running Splitty server found (tried SPLITTY_SOCK, daemon.json)")]
    NoServer,
    #[error("request failed: {0}")]
    Request(String),
    #[error("API error ({0}): {1}")]
    Api(u16, String),
    #[error("{0}")]
    Usage(String),
    /// A command that cannot go on, with the reason to print.
    #[error("{0}")]
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn unix_transport_decodes_chunked_unicode_and_plain_error_bodies() {
        let dir = tempfile::tempdir().expect("fixture");
        let socket = dir.path().join("api.sock");
        let listener = tokio::net::UnixListener::bind(&socket).expect("socket");
        let worker = tokio::spawn(async move {
            for error in [false, true] {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut request = vec![0; 8192];
                let _ = stream.read(&mut request).await.expect("request");
                if error {
                    stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 6\r\nConnection: close\r\n\r\ndenied").await.expect("error response");
                } else {
                    stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.expect("head");
                    for bytes in "{\"text\":\"ö🙂\"}".as_bytes().chunks(3) {
                        stream
                            .write_all(format!("{:x}\r\n", bytes.len()).as_bytes())
                            .await
                            .expect("chunk");
                        stream.write_all(bytes).await.expect("data");
                        stream.write_all(b"\r\n").await.expect("end");
                    }
                    stream.write_all(b"0\r\n\r\n").await.expect("finish");
                }
            }
        });
        let server = ServerInfo {
            transport: Transport::LocalSocket(socket.to_string_lossy().into_owned()),
            source: "test",
        };
        assert_eq!(
            api_get(&server, "/api/jobs").await.expect("chunked")["text"],
            "ö🙂"
        );
        assert!(
            matches!(api_get(&server,"/api/jobs").await,Err(CliError::Api(403,message)) if message=="denied")
        );
        worker.await.expect("worker");
    }

    #[tokio::test]
    async fn response_wait_has_a_deadline_beyond_supported_long_poll() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (ready, received) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut bytes = [0; 8192];
            let mut header = Vec::new();
            while !header.windows(4).any(|w| w == b"\r\n\r\n") {
                let count = stream.read(&mut bytes).await.expect("request");
                assert!(
                    count > 0 && header.len() + count <= 16384,
                    "bounded complete request header"
                );
                header.extend_from_slice(&bytes[..count]);
            }
            let _ = ready.send(());
            tokio::time::sleep(std::time::Duration::from_secs(100)).await;
        });
        let client = tokio::spawn(async move {
            api_get(
                &ServerInfo {
                    transport: Transport::Tcp {
                        addr: addr.to_string(),
                        token: "test".into(),
                    },
                    source: "test",
                },
                "/api/jobs",
            )
            .await
        });
        received.await.expect("request received");
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(31)).await;
        assert!(!client.is_finished());
        tokio::time::advance(std::time::Duration::from_secs(5)).await;
        assert!(matches!(
            client.await.expect("client"),
            Err(CliError::Request(_))
        ));
        worker.abort();
    }

    #[tokio::test]
    async fn large_responses_are_bounded_without_truncating_valid_log_data() {
        let dir = tempfile::tempdir().expect("fixture");
        let path = dir.path().join("api.sock");
        let listener = tokio::net::UnixListener::bind(&path).expect("listener");
        let worker = tokio::spawn(async move {
            for size in [350000, 17 * 1024 * 1024] {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let mut request = [0; 8192];
                let _ = socket.read(&mut request).await.expect("request");
                let value =
                    serde_json::to_vec(&json!({"data":"x".repeat(size)})).expect("response");
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            value.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .expect("headers");
                let _ = socket.write_all(&value).await;
            }
        });
        let server = ServerInfo {
            transport: Transport::LocalSocket(path.to_string_lossy().into_owned()),
            source: "test",
        };
        assert_eq!(
            api_get(&server, "/api/jobs/id/logs")
                .await
                .expect("large log")["data"]
                .as_str()
                .expect("data")
                .len(),
            350000
        );
        assert!(
            matches!(api_get(&server,"/api/jobs").await,Err(CliError::Request(message)) if message.contains("16 MiB"))
        );
        worker.await.expect("worker");
    }
}
