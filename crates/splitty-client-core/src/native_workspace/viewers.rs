//! Documents of viewer panes, read over the connection whose Screen holds
//! the pane (`viewer.read`), and the anchors and reviews of their
//! annotations, which that connection's server works out (`viewer.anchor`,
//! `viewer.review`). Each is asked for by the window while the pane is shown;
//! nothing here asks on its own.
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
/// Anchor and review requests under way on one connection at once.
const MAX_ASKS: usize = 16;

/// The anchors the server found for a pane's annotations in a version of its
/// document, in the order of the pane's list.
#[derive(Clone, Debug)]
pub struct ViewerAnchors {
    pub source: SourceToken,
    pub pane: String,
    pub version: String,
    pub anchors: Vec<crate::annotations::AnnotationAnchor>,
}

/// The review the server wrote for a pane, for the window's request `token`.
#[derive(Clone, Debug)]
pub struct ViewerReviewed {
    pub source: SourceToken,
    pub pane: String,
    pub token: u64,
    pub review: Result<crate::annotations::Review, String>,
}

/// What a request under way asked for.
#[derive(Debug)]
enum Ask {
    Anchors,
    Review { token: u64 },
}

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
    /// Anchor and review requests under way: request ID, pane, what.
    asks: Vec<(String, String, Ask)>,
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

    /// Ask for the anchors of `pane`'s annotations; an earlier request for
    /// them is forgotten.
    pub(super) fn ask_anchors(&mut self, client: &Client, pane: &str) -> std::io::Result<()> {
        self.asks
            .retain(|(_, held, ask)| !(held == pane && matches!(ask, Ask::Anchors)));
        self.room()?;
        let request = client.ask_viewer_anchors(pane)?;
        self.asks.push((request, pane.to_owned(), Ask::Anchors));
        Ok(())
    }

    /// Ask for `pane`'s review, answered under `token`.
    pub(super) fn ask_review(
        &mut self,
        client: &Client,
        pane: &str,
        token: u64,
        verdict: Option<crate::annotations::Verdict>,
        changed_during_wait: bool,
    ) -> std::io::Result<()> {
        self.room()?;
        let request = client.ask_viewer_review(pane, verdict, changed_during_wait)?;
        self.asks
            .push((request, pane.to_owned(), Ask::Review { token }));
        Ok(())
    }

    fn room(&self) -> std::io::Result<()> {
        if self.asks.len() >= MAX_ASKS {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Too many requests for annotations are under way",
            ));
        }
        Ok(())
    }

    /// Take the answer to an anchor or review request.
    fn answer(
        &mut self,
        source: SourceToken,
        value: &serde_json::Value,
        out: &mut Vec<Update>,
    ) -> bool {
        let Some(index) = value["requestId"]
            .as_str()
            .and_then(|id| self.asks.iter().position(|(request, ..)| request == id))
        else {
            return false;
        };
        let (_, pane, ask) = self.asks.remove(index);
        let failure = || {
            let error: String = value["error"]
                .as_str()
                .unwrap_or("The server did not answer")
                .chars()
                .filter(|c| !c.is_control())
                .take(256)
                .collect();
            error
        };
        match (ask, value["type"].as_str()) {
            (Ask::Anchors, Some("viewer.anchored")) => {
                let anchors = value["anchors"]
                    .as_array()
                    .filter(|anchors| anchors.len() <= splitty_core::mux::ANNOTATION_COUNT_LIMIT)
                    .and_then(|anchors| {
                        anchors
                            .iter()
                            .map(|anchor| serde_json::from_value(anchor.clone()).ok())
                            .collect::<Option<Vec<_>>>()
                    });
                if let (Some(version), Some(anchors)) = (value["version"].as_str(), anchors) {
                    out.push(Update::ViewerAnchors(ViewerAnchors {
                        source,
                        pane,
                        version: version.chars().take(256).collect(),
                        anchors,
                    }));
                }
            }
            (Ask::Anchors, _) => {}
            (Ask::Review { token }, kind) => {
                let review = match kind {
                    Some("viewer.reviewed") => {
                        let entries = value["entries"].as_array().and_then(|entries| {
                            entries
                                .iter()
                                .map(|entry| serde_json::from_value(entry.clone()).ok())
                                .collect::<Option<Vec<_>>>()
                        });
                        match (value["text"].as_str(), value["count"].as_u64(), entries) {
                            (Some(text), Some(count), Some(entries)) => {
                                Ok(crate::annotations::Review {
                                    text: text.to_owned(),
                                    count: usize::try_from(count).unwrap_or(usize::MAX),
                                    entries,
                                })
                            }
                            _ => Err("The server's review could not be read".to_owned()),
                        }
                    }
                    _ => Err(failure()),
                };
                out.push(Update::ViewerReview(ViewerReviewed {
                    source,
                    pane,
                    token,
                    review,
                }));
            }
        }
        true
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
        if let Some(value) = event.json().filter(|value| {
            matches!(
                value["type"].as_str(),
                Some("viewer.anchored" | "viewer.reviewed" | "viewer.failed")
            )
        }) {
            if self.answer(source, value, out) {
                return;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotations::AnchorState;
    use serde_json::json;

    fn owner() -> SourceToken {
        crate::workspace::SourceWorkspace::new(uuid::Uuid::new_v4()).source_token()
    }

    fn asked(asks: Vec<(&str, &str, Ask)>) -> Viewers {
        Viewers {
            reads: Vec::new(),
            asks: asks
                .into_iter()
                .map(|(request, pane, ask)| (request.to_owned(), pane.to_owned(), ask))
                .collect(),
        }
    }

    #[test]
    fn anchors_answer_the_request_for_them() {
        let mut viewers = asked(vec![("r1", "v1", Ask::Anchors)]);
        let mut out = Vec::new();
        let answer = json!({
            "type": "viewer.anchored", "requestId": "r1", "paneId": "v1", "version": "7",
            "anchors": [
                {"id": "a1", "state": "general"},
                {"id": "a2", "state": "anchored", "start": 4, "end": 9, "startLine": 2, "endLine": 2},
            ],
        });
        assert!(viewers.answer(owner(), &answer, &mut out));
        let [Update::ViewerAnchors(anchors)] = out.as_slice() else {
            panic!("expected anchors");
        };
        assert_eq!(
            (anchors.pane.as_str(), anchors.version.as_str()),
            ("v1", "7")
        );
        assert_eq!(anchors.anchors.len(), 2);
        assert_eq!(anchors.anchors[1].state, AnchorState::Anchored);
        assert!(viewers.asks.is_empty());
        // An answer nobody asked for passes through.
        assert!(!viewers.answer(owner(), &answer, &mut out));
    }

    #[test]
    fn malformed_anchors_are_dropped() {
        let mut viewers = asked(vec![("r1", "v1", Ask::Anchors)]);
        let mut out = Vec::new();
        let answer = json!({
            "type": "viewer.anchored", "requestId": "r1", "version": "7",
            "anchors": [{"id": "a1", "state": "somewhere"}],
        });
        assert!(viewers.answer(owner(), &answer, &mut out));
        assert!(out.is_empty());
        assert!(viewers.asks.is_empty());
    }

    #[test]
    fn a_review_answers_its_token() {
        let mut viewers = asked(vec![
            ("r1", "v1", Ask::Anchors),
            ("r2", "v1", Ask::Review { token: 5 }),
        ]);
        let mut out = Vec::new();
        let answer = json!({
            "type": "viewer.reviewed", "requestId": "r2", "paneId": "v1", "version": "7",
            "text": "# Review\n", "count": 1,
            "entries": [{"id": "a1", "kind": "comment", "body": "Why?"}],
        });
        assert!(viewers.answer(owner(), &answer, &mut out));
        let [Update::ViewerReview(reviewed)] = out.as_slice() else {
            panic!("expected a review");
        };
        assert_eq!(reviewed.token, 5);
        let review = reviewed.review.as_ref().unwrap();
        assert_eq!((review.text.as_str(), review.count), ("# Review\n", 1));
        assert_eq!(review.entries.len(), 1);
        assert_eq!(viewers.asks.len(), 1);
    }

    #[test]
    fn a_failed_review_reports_the_servers_reason() {
        let mut viewers = asked(vec![("r2", "v1", Ask::Review { token: 5 })]);
        let mut out = Vec::new();
        let answer = json!({
            "type": "viewer.failed", "requestId": "r2", "code": "unknown_pane",
            "error": "no such\u{1b} pane",
        });
        assert!(viewers.answer(owner(), &answer, &mut out));
        let [Update::ViewerReview(reviewed)] = out.as_slice() else {
            panic!("expected a review");
        };
        assert_eq!(reviewed.review.as_ref().unwrap_err(), "no such pane");
        // A review without its text is not taken for one.
        let mut viewers = asked(vec![("r3", "v1", Ask::Review { token: 6 })]);
        let mut out = Vec::new();
        let answer =
            json!({"type": "viewer.reviewed", "requestId": "r3", "count": 0, "entries": []});
        assert!(viewers.answer(owner(), &answer, &mut out));
        let [Update::ViewerReview(reviewed)] = out.as_slice() else {
            panic!("expected a review");
        };
        assert!(reviewed.review.is_err());
    }
}
