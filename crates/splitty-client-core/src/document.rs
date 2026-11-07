//! Versioned document assembly on the exact connection that owns the pane.
use crate::connection::{identifier, invalid, Client, Event};
use base64::Engine as _;
use serde_json::{json, Value};
use splitty_transport::budget::{ConnectionBudget, Lease};
use std::io;
use uuid::Uuid;

const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 1024 * 1024;

/// Retain this object while its bytes are used by a document view. The native
/// text engine may own its layout, but transport document storage stays charged.
pub struct Document {
    pub connection_id: Uuid,
    bytes: Vec<u8>,
    version: String,
    _lease: Lease,
}

impl Document {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// One pending read. A new connection needs a new read, even for the same pane.
pub struct DocumentRead {
    connection_id: Uuid,
    request_id: String,
    pane_id: String,
    document: Option<Document>,
    size: usize,
    finished: bool,
    _lease: Lease,
}

impl Client {
    pub fn read_document(&self, pane_id: &str) -> io::Result<DocumentRead> {
        identifier(pane_id)?;
        let lease = self
            .sender()
            .context()
            .budget
            .reserve(1024, false)
            .map_err(io::Error::other)?;
        let read = DocumentRead {
            connection_id: self.connection_id(),
            request_id: Uuid::new_v4().to_string(),
            pane_id: pane_id.into(),
            document: None,
            size: 0,
            finished: false,
            _lease: lease,
        };
        read.send(self)?;
        Ok(read)
    }
}

impl DocumentRead {
    /// Ignore unrelated events; reject another connection before inspecting
    /// any peer-controlled request ID. Successful partial chunks request the
    /// next versioned chunk, never mixing a changed file into retained bytes.
    pub fn accept(&mut self, client: &Client, event: &Event) -> io::Result<Option<Document>> {
        if client.connection_id() != self.connection_id || event.connection_id != self.connection_id
        {
            return Err(invalid("document belongs to another connection"));
        }
        let Some(value) = event.json() else {
            return Ok(None);
        };
        if value["requestId"].as_str() != Some(self.request_id.as_str()) {
            return Ok(None);
        }
        let budget = client.sender().context().budget;
        let result = self.consume(value, &budget);
        match result {
            Ok(Some(document)) => Ok(Some(document)),
            Ok(None) => {
                self.send(client)?;
                Ok(None)
            }
            Err(error) => {
                self.finished = true;
                self.document = None;
                Err(error)
            }
        }
    }

    fn send(&self, client: &Client) -> io::Result<()> {
        if self.finished {
            return Err(invalid("document read already finished"));
        }
        let offset = self.document.as_ref().map_or(0, |doc| doc.bytes.len());
        let version = self.document.as_ref().map(|doc| doc.version.as_str());
        client.command(0, || {
            json!({"type":"viewer.read", "requestId":self.request_id,
            "paneId":self.pane_id, "offset":offset, "version":version})
        })
    }

    fn consume(
        &mut self,
        value: &Value,
        budget: &ConnectionBudget,
    ) -> io::Result<Option<Document>> {
        if self.finished {
            return Err(invalid("document read already finished"));
        }
        match value["type"].as_str() {
            Some("viewer.failed") => return Err(invalid("service refused document read")),
            Some("viewer.chunk") => {}
            _ => return Err(invalid("unexpected document response")),
        }
        let size = value["size"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n <= MAX_DOCUMENT_BYTES)
            .ok_or_else(|| invalid("document exceeds limit"))?;
        let offset = value["offset"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| invalid("invalid document offset"))?;
        let version = value["version"]
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= 256)
            .ok_or_else(|| invalid("invalid document version"))?;
        let data = value["data"]
            .as_str()
            .filter(|v| v.len() <= MAX_CHUNK_BYTES.div_ceil(3) * 4)
            .ok_or_else(|| invalid("invalid document chunk"))?;
        let eof = value["eof"]
            .as_bool()
            .ok_or_else(|| invalid("missing document end flag"))?;
        if self.document.is_none() {
            if offset != 0 {
                return Err(invalid("document starts at wrong offset"));
            }
            let lease = budget
                .reserve(size + version.len(), false)
                .map_err(io::Error::other)?;
            self.document = Some(Document {
                connection_id: self.connection_id,
                bytes: Vec::with_capacity(size),
                version: version.into(),
                _lease: lease,
            });
            self.size = size;
        }
        let document = self.document.as_mut().expect("allocated document");
        if size != self.size || version != document.version || offset != document.bytes.len() {
            return Err(invalid("document changed during read"));
        }
        let _scratch = budget
            .reserve(base64::decoded_len_estimate(data.len()), false)
            .map_err(io::Error::other)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| invalid("invalid document encoding"))?;
        let end = offset
            .checked_add(bytes.len())
            .filter(|end| *end <= size)
            .ok_or_else(|| invalid("document chunk exceeds declared size"))?;
        if bytes.len() > MAX_CHUNK_BYTES || eof != (end == size) || (!eof && bytes.is_empty()) {
            return Err(invalid("inconsistent document chunk"));
        }
        document.bytes.extend_from_slice(&bytes);
        if eof {
            self.finished = true;
            Ok(self.document.take())
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_transport::budget::ProcessBudget;

    fn read(budget: &ConnectionBudget) -> DocumentRead {
        DocumentRead {
            connection_id: Uuid::new_v4(),
            request_id: "r".into(),
            pane_id: "p".into(),
            document: None,
            size: 0,
            finished: false,
            _lease: budget.reserve(1024, false).unwrap(),
        }
    }

    fn chunk(offset: usize, size: usize, version: &str, data: &[u8], eof: bool) -> Value {
        json!({"type":"viewer.chunk", "offset":offset, "size":size, "version":version,
            "data":base64::engine::general_purpose::STANDARD.encode(data), "eof":eof})
    }

    #[test]
    fn assembly_keeps_its_charge_and_rejects_changed_versions_offsets_and_sizes() {
        let budget = ConnectionBudget::default();
        let mut pending = read(&budget);
        assert!(pending
            .consume(&chunk(0, 4, "v1", b"ab", false), &budget)
            .unwrap()
            .is_none());
        for bad in [
            chunk(2, 4, "v2", b"cd", true),
            chunk(1, 4, "v1", b"cd", true),
            chunk(2, 5, "v1", b"cd", true),
            chunk(2, 4, "v1", b"cd", false),
        ] {
            assert!(pending.consume(&bad, &budget).is_err());
        }
        let document = pending
            .consume(&chunk(2, 4, "v1", b"cd", true), &budget)
            .unwrap()
            .unwrap();
        drop(pending);
        assert_eq!(document.bytes(), b"abcd");
        assert_eq!(budget.used(), 6);
        drop(document);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn announced_storage_is_reserved_before_document_allocation() {
        let budget = ConnectionBudget::new(ProcessBudget::new(4096, 0), 4096, 0);
        let mut pending = read(&budget);
        assert!(pending
            .consume(&chunk(0, 5000, "v", b"a", false), &budget)
            .is_err());
        assert!(pending.document.is_none());
        assert_eq!(budget.used(), 1024);
        assert!(pending
            .consume(&chunk(0, MAX_DOCUMENT_BYTES + 1, "v", b"a", false), &budget)
            .is_err());
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn reused_request_and_pane_do_not_admit_a_reply_from_another_connection() {
        let (runtime, path) = crate::appearance::local::tests::fixture();
        let service = crate::EmbeddedService::from_runtime(runtime);
        let mut old_client = service.connect().await.unwrap();
        let current_client = service.connect().await.unwrap();
        let old_read = old_client.read_document("same-pane").unwrap();
        let mut current_read = current_client.read_document("same-pane").unwrap();
        current_read.request_id.clone_from(&old_read.request_id);
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let event = old_client.next_event().await.unwrap().unwrap();
                if event
                    .json()
                    .is_some_and(|json| json["requestId"] == old_read.request_id)
                {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        assert!(current_read.accept(&current_client, &event).is_err());
        assert!(
            !current_read.finished,
            "foreign failure must not settle the current read"
        );
        assert!(current_read.document.is_none());
        assert!(current_read.accept(&old_client, &event).is_err());
        drop(event);
        drop(old_read);
        drop(current_read);
        old_client.finish().await.unwrap();
        current_client.finish().await.unwrap();
        service.close().await.unwrap();
        assert!(!path.exists());
    }
}
