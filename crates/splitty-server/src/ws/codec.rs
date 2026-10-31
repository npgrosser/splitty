//! Wire-format codec for WebSocket messages.
//!
//! Provides a compact binary frame format for terminal output (avoiding JSON/base64
//! overhead on the hot path) and JSON deserialization for incoming client messages.
//! Server→client events use `ServerEvent` with serde serialization directly.

// Re-export core protocol types used by other ws modules.
pub use splitty_core::protocol::{Request, MSG_MUX_OP, MSG_MUX_RESYNC};

/// Encode a binary frame for terminal output.
///
/// Format: `[1 byte: session ID length] [N bytes: session ID] [payload bytes]`
///
/// This is the hot path for terminal output — avoids JSON/base64 overhead.
pub fn encode_binary_frame(session_id: &str, data: &[u8]) -> Vec<u8> {
    let id_bytes = session_id.as_bytes();
    let mut frame = Vec::with_capacity(1 + id_bytes.len() + data.len());
    frame.push(id_bytes.len() as u8);
    frame.extend_from_slice(id_bytes);
    frame.extend_from_slice(data);
    frame
}

/// Decode a binary frame back into (session_id, payload).
pub fn decode_binary_frame(data: &[u8]) -> Option<(&str, &[u8])> {
    if data.is_empty() {
        return None;
    }
    let id_len = data[0] as usize;
    if data.len() < 1 + id_len {
        return None;
    }
    let id = std::str::from_utf8(&data[1..1 + id_len]).ok()?;
    let payload = &data[1 + id_len..];
    Some((id, payload))
}

/// JSON wire request — matches the Go `jsonRequest` struct exactly.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonRequest {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub data: String,
    #[serde(default)]
    pub shell: String,
    #[serde(default)]
    pub shell_profile_id: String,
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub rows: u16,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub restore_mode: String,
    #[serde(default)]
    pub pane_id: String,
    #[serde(default)]
    pub screen_id: String,
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub offset: u64,
    #[serde(default)]
    pub takeover: bool,
    #[serde(default)]
    pub redraw: bool,
}

/// Decode a JSON wire request into a protocol Request.
pub fn decode_request(jr: JsonRequest) -> Request {
    let data = if jr.data.is_empty() {
        Vec::new()
    } else {
        jr.data.into_bytes()
    };
    Request {
        msg_type: jr.msg_type,
        id: jr.id,
        data,
        shell: jr.shell,
        shell_profile_id: jr.shell_profile_id,
        cols: jr.cols,
        rows: jr.rows,
        command: jr.command,
        dir: jr.dir,
        root: jr.root,
        restore_mode: jr.restore_mode,
        pane_id: jr.pane_id,
        screen_id: jr.screen_id,
        request_id: jr.request_id,
        name: jr.name,
        size: jr.size,
        offset: jr.offset,
        takeover: jr.takeover,
        redraw: jr.redraw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_frame_roundtrip() {
        let session_id = "sess-abc123";
        let payload = b"hello world terminal output";

        let frame = encode_binary_frame(session_id, payload);
        let (decoded_id, decoded_payload) = decode_binary_frame(&frame).unwrap();

        assert_eq!(decoded_id, session_id);
        assert_eq!(decoded_payload, payload);
    }

    #[test]
    fn test_binary_frame_empty_payload() {
        let session_id = "sess-x";
        let payload = b"";

        let frame = encode_binary_frame(session_id, payload);
        let (decoded_id, decoded_payload) = decode_binary_frame(&frame).unwrap();

        assert_eq!(decoded_id, session_id);
        assert!(decoded_payload.is_empty());
    }

    #[test]
    fn test_decode_binary_frame_empty_input() {
        assert!(decode_binary_frame(&[]).is_none());
    }

    #[test]
    fn test_decode_binary_frame_truncated() {
        // id_len says 10 but only 3 bytes follow
        assert!(decode_binary_frame(&[10, 0, 0, 0]).is_none());
    }

    #[test]
    fn test_decode_request() {
        let json_str = r#"{
            "type": "create",
            "shell": "/bin/zsh",
            "cols": 80,
            "rows": 24,
            "command": "",
            "dir": "/home/user",
            "paneId": "pane-1",
            "screenId": "screen-1"
        }"#;

        let jr: JsonRequest = serde_json::from_str(json_str).unwrap();
        let req = decode_request(jr);

        assert_eq!(req.msg_type, "create");
        assert_eq!(req.shell, "/bin/zsh");
        assert_eq!(req.cols, 80);
        assert_eq!(req.rows, 24);
        assert_eq!(req.dir, "/home/user");
        assert_eq!(req.pane_id, "pane-1");
        assert_eq!(req.screen_id, "screen-1");
        assert_eq!(req.request_id, "");
        assert!(!req.takeover && !req.redraw);
    }

    #[test]
    fn test_decode_size_flags() {
        let decode = |json: &str| decode_request(serde_json::from_str(json).unwrap());
        let resize = decode(r#"{"type":"resize","id":"s","cols":100,"rows":30,"redraw":true}"#);
        assert!(resize.redraw);
        assert_eq!((resize.cols, resize.rows), (100, 30));
        assert!(decode(r#"{"type":"activity","takeover":true}"#).takeover);
    }

    /// `upload` carries its request id, offset and chunk, and the first chunk
    /// the file's name and size.
    #[test]
    fn test_decode_upload_request() {
        let decode = |json: &str| decode_request(serde_json::from_str(json).unwrap());
        let first = decode(
            r#"{"type":"upload","requestId":"u1","offset":0,"data":"AAE=","name":"a b.pdf","size":3000000000}"#,
        );
        assert_eq!(first.msg_type, splitty_core::protocol::MSG_UPLOAD);
        assert_eq!(first.request_id, "u1");
        assert_eq!(first.name, "a b.pdf");
        assert_eq!(first.size, Some(3_000_000_000));
        assert_eq!(first.offset, 0);
        assert_eq!(first.data, b"AAE=");
        let next = decode(r#"{"type":"upload","requestId":"u1","offset":1048576,"data":""}"#);
        assert_eq!(next.offset, 1_048_576);
        assert_eq!(next.size, None);
        assert!(next.name.is_empty());
    }

    /// `save_image` carries its request id and the image's base64 text,
    /// which the dispatcher decodes.
    #[test]
    fn test_decode_save_image_request() {
        let json_str = r#"{"type":"save_image","requestId":"r1","data":"iVBORw0KGgo="}"#;

        let jr: JsonRequest = serde_json::from_str(json_str).unwrap();
        let req = decode_request(jr);

        assert_eq!(req.msg_type, splitty_core::protocol::MSG_SAVE_IMAGE);
        assert_eq!(req.request_id, "r1");
        assert_eq!(req.data, b"iVBORw0KGgo=");
        assert_eq!(req.id, "");
    }
}
