//! Annotations of a viewer pane, apart from any window: the anchors the
//! server found for a pane's annotations, describing a selected passage, and
//! how a review the server wrote reaches a terminal (`viewer-annotations`).
//! The server anchors the annotations and writes the review
//! (`viewer.anchor`, `viewer.review`); a window maps the anchors into what it
//! draws.
pub use splitty_core::mux::{
    is_pending, viewer_source_text, AnchorState, AnnotationAnchor, AnnotationKind, AnnotationReply,
    AnnotationStatus, AnnotationTarget, AnnotationView, ReplyAuthor, Review, SentEntry, Verdict,
    ViewerAnnotation,
};
use std::{io, ops::Range};
use uuid::Uuid;

/// The context kept before and after a quote, in characters.
const CONTEXT_CHARS: usize = 32;
/// The longest quote, remark, replacement or reply, in bytes.
pub const TEXT_LIMIT: usize = 16 * 1024;

fn hex_id(prefix: &str) -> String {
    format!("{prefix}{}", &Uuid::new_v4().simple().to_string()[..16])
}

/// A fresh annotation ID: `ann-` and 16 lowercase hex characters.
pub fn new_annotation_id() -> String {
    hex_id("ann-")
}

/// A fresh ID for a reply of the user.
pub fn new_reply_id() -> String {
    hex_id("rep-")
}

/// Where an annotation's passage is in the document shown now, as the
/// server anchored it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    Found {
        /// Bytes of the document's source text ([`viewer_source_text`]).
        range: Range<usize>,
        start_line: u32,
        end_line: u32,
        /// The quote is not in this version: this is the passage it most
        /// likely became.
        changed: bool,
    },
    /// The quote is not in this version; the recorded lines still stand.
    NotFound,
    /// A general comment names no passage.
    General,
    /// The server has not anchored the annotation in this version yet.
    Pending,
}

impl Anchor {
    /// The server's anchor, for the source text `text`; a range that is not
    /// one of the text is not trusted.
    pub fn from_wire(anchor: &AnnotationAnchor, text: &str) -> Self {
        match anchor.state {
            AnchorState::General => Self::General,
            AnchorState::Lost => Self::NotFound,
            AnchorState::Anchored | AnchorState::Changed => {
                match (anchor.start, anchor.end, anchor.start_line, anchor.end_line) {
                    (Some(start), Some(end), Some(start_line), Some(end_line))
                        if start <= end
                            && text.get(start..end).is_some()
                            && start_line <= end_line =>
                    {
                        Self::Found {
                            range: start..end,
                            start_line,
                            end_line,
                            changed: anchor.state == AnchorState::Changed,
                        }
                    }
                    _ => Self::NotFound,
                }
            }
        }
    }
}

/// The 1-based line of the byte `offset` of `text`.
fn line_of(text: &str, offset: usize) -> u32 {
    u32::try_from(text[..offset].bytes().filter(|byte| *byte == b'\n').count() + 1)
        .unwrap_or(u32::MAX)
}

/// Describe the passage `range` of a document's source as an annotation's
/// target: its text, the context around it and its lines; a passage ends on
/// the line of its last character.
pub fn target_for(text: &str, range: Range<usize>) -> io::Result<AnnotationTarget> {
    let invalid = |reason: &'static str| io::Error::new(io::ErrorKind::InvalidInput, reason);
    let quote = text
        .get(range.clone())
        .filter(|quote| !quote.trim().is_empty())
        .ok_or_else(|| invalid("Select a passage first"))?;
    if quote.len() > TEXT_LIMIT {
        return Err(invalid("The passage is too long to annotate"));
    }
    let prefix: Vec<char> = text[..range.start]
        .chars()
        .rev()
        .take(CONTEXT_CHARS)
        .collect();
    let last = quote
        .char_indices()
        .next_back()
        .map_or(range.start, |(at, _)| range.start + at);
    Ok(AnnotationTarget {
        view: AnnotationView::Source,
        quote: quote.to_owned(),
        prefix: prefix.into_iter().rev().collect(),
        suffix: text[range.end..].chars().take(CONTEXT_CHARS).collect(),
        start_line: line_of(text, range.start),
        end_line: line_of(text, last),
    })
}

/// How the user sends a review to a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewForm {
    Paste,
    File,
    Scratchpad,
}

/// What sending a review in a form does (`viewer-annotations`, Handing A
/// Review To A Terminal). The entries the review holds are marked as handed
/// over only once it went out: after the paste, after the file was stored
/// and its path inserted, after the scratchpad took it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewDelivery {
    /// Paste this text: the review without its final line breaks, so that
    /// nothing reads as Enter.
    Paste(String),
    /// Store the review as a file on the terminal's server and insert its
    /// quoted path. `instead_of_paste`: the user asked for a paste, but the
    /// program has not enabled bracketed paste and would run each line of
    /// the review as a command, which the window says.
    File {
        instead_of_paste: bool,
    },
    Scratchpad,
}

/// How `text`, a review, reaches a terminal whose program has or has not
/// enabled bracketed paste, sent in `form`.
pub fn review_delivery(text: &str, form: ReviewForm, bracketed_paste: bool) -> ReviewDelivery {
    let pasted = text.trim_end_matches('\n');
    match form {
        ReviewForm::Scratchpad => ReviewDelivery::Scratchpad,
        ReviewForm::File => ReviewDelivery::File {
            instead_of_paste: false,
        },
        ReviewForm::Paste if !bracketed_paste && pasted.contains('\n') => ReviewDelivery::File {
            instead_of_paste: true,
        },
        ReviewForm::Paste => ReviewDelivery::Paste(pasted.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As the web client does: a paste ends without Enter, and a review of
    /// several lines goes to a program without bracketed paste as a file.
    #[test]
    fn a_review_reaches_a_terminal_without_running_its_lines() {
        let review = "# Review of plan.md\n\n## #1 Line 3\nSplit it.\n\n";
        assert_eq!(
            review_delivery(review, ReviewForm::Paste, true),
            ReviewDelivery::Paste("# Review of plan.md\n\n## #1 Line 3\nSplit it.".into())
        );
        assert_eq!(
            review_delivery(review, ReviewForm::Paste, false),
            ReviewDelivery::File {
                instead_of_paste: true
            }
        );
        assert_eq!(
            review_delivery("One line.\n", ReviewForm::Paste, false),
            ReviewDelivery::Paste("One line.".into())
        );
        for bracketed in [false, true] {
            assert_eq!(
                review_delivery(review, ReviewForm::File, bracketed),
                ReviewDelivery::File {
                    instead_of_paste: false
                }
            );
            assert_eq!(
                review_delivery(review, ReviewForm::Scratchpad, bracketed),
                ReviewDelivery::Scratchpad
            );
        }
    }

    /// A selected passage is described by its quote, its context and its
    /// lines; the passage ends on the line of its last character.
    #[test]
    fn a_selected_passage_is_described_as_a_target() {
        let text = "intro\nretry three times\nmiddle\nretry three times, then stop\n";
        let second = text.rfind("retry three times").unwrap();
        let target = target_for(text, second..second + "retry three times".len()).unwrap();
        assert_eq!((target.start_line, target.end_line), (4, 4));
        assert_eq!(target.prefix, "intro\nretry three times\nmiddle\n");
        assert_eq!(target.suffix, ", then stop\n");
        let lines = target_for(text, 6..24).unwrap();
        assert_eq!(
            (lines.start_line, lines.end_line),
            (2, 2),
            "the line break ends line 2"
        );
        assert!(target_for(text, 0..0).is_err());
        assert!(target_for(text, 5..6).is_err(), "only whitespace");
        assert!(target_for("é", 0..1).is_err(), "not a character boundary");
    }

    /// The server's anchors become the window's, and a range that is not
    /// one of the window's text is not trusted.
    #[test]
    fn the_servers_anchors_are_taken_for_the_text_shown() {
        let anchor = |value: serde_json::Value| -> AnnotationAnchor {
            serde_json::from_value(value).unwrap()
        };
        let text = "one\nretry three times\n";
        assert_eq!(
            Anchor::from_wire(
                &anchor(
                    serde_json::json!({"id": "a", "state": "changed", "start": 4, "end": 9, "startLine": 2, "endLine": 2})
                ),
                text
            ),
            Anchor::Found {
                range: 4..9,
                start_line: 2,
                end_line: 2,
                changed: true
            }
        );
        assert_eq!(
            Anchor::from_wire(
                &anchor(serde_json::json!({"id": "a", "state": "general"})),
                text
            ),
            Anchor::General
        );
        assert_eq!(
            Anchor::from_wire(
                &anchor(serde_json::json!({"id": "a", "state": "lost"})),
                text
            ),
            Anchor::NotFound
        );
        for broken in [
            serde_json::json!({"id": "a", "state": "anchored", "start": 4, "end": 99, "startLine": 2, "endLine": 2}),
            serde_json::json!({"id": "a", "state": "anchored", "start": 9, "end": 4, "startLine": 2, "endLine": 2}),
            serde_json::json!({"id": "a", "state": "anchored", "start": 4, "end": 9}),
        ] {
            assert_eq!(Anchor::from_wire(&anchor(broken), text), Anchor::NotFound);
        }
        assert_eq!(
            Anchor::from_wire(
                &anchor(
                    serde_json::json!({"id": "a", "state": "anchored", "start": 1, "end": 2, "startLine": 1, "endLine": 1})
                ),
                "é"
            ),
            Anchor::NotFound,
            "not a character boundary"
        );
    }
}
