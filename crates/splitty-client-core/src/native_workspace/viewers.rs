//! Documents of viewer panes, read over the connection whose Screen holds
//! the pane (`viewer.read`). A read is asked for by the window while the
//! pane is shown; nothing here reads on its own.
use super::*;
use crate::document::{DocumentRead, ReadOutcome};
use crate::Client;

/// A change of a viewer pane's annotations, as the window asks for it.
#[derive(Clone, Debug)]
pub enum AnnotationOp {
    /// Add annotations, or replace those with the same ID.
    Put(Vec<crate::annotations::ViewerAnnotation>),
    Remove(Vec<String>),
    /// Add replies of the user to a thread, or replace those with the same ID.
    PutReplies {
        annotation_id: String,
        replies: Vec<crate::annotations::AnnotationReply>,
    },
    RemoveReplies {
        annotation_id: String,
        ids: Vec<String>,
    },
    /// Mark what a review held as handed over.
    MarkSent(Vec<crate::annotations::SentEntry>),
}
impl AnnotationOp {
    pub(super) fn screen_op(self, pane: &str) -> splitty_core::mux::ScreenOp {
        use splitty_core::{ids::PaneId, mux::ScreenOp};
        let pane_id = PaneId::from(pane);
        match self {
            Self::Put(annotations) => ScreenOp::PutAnnotations {
                pane_id,
                annotations,
            },
            Self::Remove(ids) => ScreenOp::RemoveAnnotations { pane_id, ids },
            Self::PutReplies {
                annotation_id,
                replies,
            } => ScreenOp::PutReplies {
                pane_id,
                annotation_id,
                replies,
            },
            Self::RemoveReplies { annotation_id, ids } => ScreenOp::RemoveReplies {
                pane_id,
                annotation_id,
                ids,
            },
            Self::MarkSent(entries) => ScreenOp::MarkSent { pane_id, entries },
        }
    }
}

/// The service's answer to a verdict on a waiting review.
#[derive(Clone, Debug)]
pub struct ReviewAnswered {
    pub source: SourceToken,
    pub pane: String,
    pub wait_id: String,
    pub accepted: bool,
}

/// Reads under way on one connection at once.
const MAX_READS: usize = 8;

/// What a read of a viewer pane's document brought.
#[derive(Clone, Debug)]
pub enum ViewerContent {
    Document {
        version: String,
        bytes: Arc<[u8]>,
    },
    /// The document is still the version the window has.
    Unchanged,
    /// The service's code (`too_large`, `not_a_file`, `changed`, …) and words.
    Failed {
        code: String,
        error: String,
    },
}

#[derive(Clone, Debug)]
pub struct ViewerUpdate {
    pub source: SourceToken,
    pub pane: String,
    pub content: ViewerContent,
}

#[derive(Default)]
pub(super) struct Viewers {
    reads: Vec<(String, DocumentRead)>,
}
impl Viewers {
    /// Read `pane`'s document; a read of the same pane under way is replaced.
    pub(super) fn read(
        &mut self,
        client: &Client,
        source: SourceToken,
        pane: &str,
        if_version: Option<&str>,
        out: &mut Vec<Update>,
    ) {
        self.reads.retain(|(held, _)| held != pane);
        let result = if self.reads.len() >= MAX_READS {
            Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Too many documents are being read",
            ))
        } else {
            client.read_document_if(pane, if_version)
        };
        match result {
            Ok(read) => self.reads.push((pane.to_owned(), read)),
            Err(error) => out.push(Update::Viewer(ViewerUpdate {
                source,
                pane: pane.to_owned(),
                content: ViewerContent::Failed {
                    code: "not_sent".into(),
                    error: error.to_string(),
                },
            })),
        }
    }

    /// Take an answer to one of the reads; other events pass through.
    pub(super) fn event(
        &mut self,
        client: &Client,
        source: SourceToken,
        event: &Event,
        out: &mut Vec<Update>,
    ) {
        if let Some(value) = event
            .json()
            .filter(|value| value["type"] == "viewer.review_answered")
        {
            if let (Some(pane), Some(wait_id)) =
                (value["paneId"].as_str(), value["waitId"].as_str())
            {
                out.push(Update::ReviewAnswered(ReviewAnswered {
                    source,
                    pane: pane.chars().take(256).collect(),
                    wait_id: wait_id.chars().take(256).collect(),
                    accepted: value["accepted"] == true,
                }));
            }
            return;
        }
        let Some(index) = self.reads.iter().position(|(_, read)| read.answers(event)) else {
            return;
        };
        let Some(outcome) = self.reads[index].1.settle(client, event) else {
            return;
        };
        let (pane, _) = self.reads.remove(index);
        out.push(Update::Viewer(ViewerUpdate {
            source,
            pane,
            content: match outcome {
                ReadOutcome::Document(document) => ViewerContent::Document {
                    version: document.version().to_owned(),
                    bytes: Arc::from(document.bytes()),
                },
                ReadOutcome::Unchanged => ViewerContent::Unchanged,
                ReadOutcome::Failed { code, error } => ViewerContent::Failed { code, error },
            },
        }));
    }
}
