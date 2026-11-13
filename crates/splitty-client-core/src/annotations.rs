//! Annotations of a viewer pane, apart from any window: finding an
//! annotation's passage in the document by its quote, describing a selected
//! passage, and writing the review a terminal or a waiting process receives
//! (`viewer-annotations`).
pub use splitty_core::mux::{
    AnnotationKind, AnnotationReply, AnnotationStatus, AnnotationTarget, AnnotationView,
    ReplyAuthor, SentEntry, ViewerAnnotation,
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

/// Where an annotation's passage is in the document shown now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    Found {
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
}

fn line_of(text: &str, offset: usize) -> u32 {
    u32::try_from(text[..offset].bytes().filter(|byte| *byte == b'\n').count() + 1)
        .unwrap_or(u32::MAX)
}

fn found(text: &str, range: Range<usize>) -> Anchor {
    let start_line = line_of(text, range.start);
    let inner = text[range.clone()]
        .trim_end_matches('\n')
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    Anchor::Found {
        start_line,
        end_line: start_line.saturating_add(u32::try_from(inner).unwrap_or(u32::MAX)),
        range,
        changed: false,
    }
}

/// How many characters of the recorded context stand around a candidate.
fn context_score(text: &str, range: &Range<usize>, target: &AnnotationTarget) -> usize {
    let before = text[..range.start]
        .chars()
        .rev()
        .zip(target.prefix.chars().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let after = text[range.end..]
        .chars()
        .zip(target.suffix.chars())
        .take_while(|(a, b)| a == b)
        .count();
    before + after
}

/// The same text with every run of whitespace as one space, and for each of
/// its bytes the byte of the original it came from.
fn collapsed(text: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len());
    let mut in_space = false;
    for (index, c) in text.char_indices() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
                origin.push(index);
            }
            in_space = true;
        } else {
            in_space = false;
            out.push(c);
            origin.extend(std::iter::repeat_n(index, c.len_utf8()));
        }
    }
    (out, origin)
}

/// Find an annotation's passage in `text` by its quote. Among several
/// occurrences the one whose surroundings match the recorded context best
/// wins, then the one nearest the recorded lines. A quote that is not there
/// is looked for again with runs of whitespace as one space, and then as the
/// passage it most likely became, which is anchored as changed.
pub fn anchor(text: &str, target: Option<&AnnotationTarget>) -> Anchor {
    let Some(target) = target else {
        return Anchor::General;
    };
    if target.quote.is_empty() {
        return Anchor::NotFound;
    }
    let best = |candidates: Vec<Range<usize>>| -> Option<Range<usize>> {
        candidates.into_iter().min_by_key(|range| {
            (
                std::cmp::Reverse(context_score(text, range, target)),
                line_of(text, range.start).abs_diff(target.start_line),
                range.start,
            )
        })
    };
    let exact: Vec<_> = text
        .match_indices(&target.quote)
        .map(|(start, quote)| start..start + quote.len())
        .collect();
    if let Some(range) = best(exact) {
        return found(text, range);
    }
    let (plain, origin) = collapsed(text);
    let (quote, _) = collapsed(&target.quote);
    let quote = quote.trim();
    if quote.is_empty() {
        return Anchor::NotFound;
    }
    let loose: Vec<_> = plain
        .match_indices(quote)
        .map(|(start, found)| {
            let last = start + found.len() - 1;
            let end = origin[last]
                + text[origin[last]..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
            origin[start]..end
        })
        .collect();
    if let Some(range) = best(loose) {
        return found(text, range);
    }
    match similar(text, target).map(|range| found(text, range)) {
        Some(Anchor::Found {
            range,
            start_line,
            end_line,
            ..
        }) => Anchor::Found {
            range,
            start_line,
            end_line,
            changed: true,
        },
        _ => Anchor::NotFound,
    }
}
/// How far from the recorded start line a changed passage may start.
const SIMILAR_LINES: u32 = 50;
/// The lowest score of a changed passage.
const SIMILAR_SCORE: f64 = 0.6;
/// A longer quote, in characters, is not looked for by similarity.
const SIMILAR_MAX_QUOTE: usize = 4_000;
/// The most bytes of the text in which a changed passage may start.
const SIMILAR_MAX_REGION: usize = 65_536;
/// The most span ends one search compares, so that it always ends soon.
const SIMILAR_STEPS: usize = 1_000_000;

/// Scripts written without spaces: a passage may start and end at any of
/// their characters.
fn unspaced(c: char) -> bool {
    matches!(u32::from(c), 0x0E00..=0x0E7F | 0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFF)
}

/// The passage the quote most likely became, near its recorded lines: the
/// span, at word boundaries and between half and twice as long as the quote,
/// whose character trigrams are most like the quote's (Dice coefficient,
/// whitespace collapsed and case folded), when that is similar enough.
fn similar(text: &str, target: &AnnotationTarget) -> Option<Range<usize>> {
    if target.quote.chars().count() > SIMILAR_MAX_QUOTE {
        return None;
    }
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
    let (plain, _) = collapsed(&target.quote);
    let quote: Vec<char> = plain.trim().chars().map(fold).collect();
    if quote.len() < 3 {
        return None;
    }
    // The region: the lines around the recorded start line, bounded.
    let first_line = target.start_line.saturating_sub(SIMILAR_LINES).max(1);
    let last_line = target.start_line.saturating_add(SIMILAR_LINES);
    let (mut region_start, mut region_end, mut centre) = (text.len(), text.len(), text.len());
    let mut offset = 0;
    for (line, part) in (1_u32..).zip(text.split_inclusive('\n')) {
        if line == first_line {
            region_start = offset;
        }
        if line == target.start_line {
            centre = offset;
        }
        offset += part.len();
        if line == last_line {
            region_end = offset;
            break;
        }
    }
    if region_start >= region_end {
        return None;
    }
    if region_end - region_start > SIMILAR_MAX_REGION {
        let centre = centre.clamp(region_start, region_end);
        region_start = region_start.max(centre.saturating_sub(SIMILAR_MAX_REGION / 2));
        region_end = region_end.min(region_start + SIMILAR_MAX_REGION);
        while !text.is_char_boundary(region_start) {
            region_start += 1;
        }
        while !text.is_char_boundary(region_end) {
            region_end -= 1;
        }
    }
    // The region with whitespace collapsed and case folded; for each of its
    // characters where it starts and ends in the text and its line.
    let mut codes = Vec::new();
    let mut spans: Vec<(usize, usize, u32)> = Vec::new();
    let mut line = line_of(text, region_start);
    let mut in_space = true;
    for (index, c) in text[region_start..region_end].char_indices() {
        let at = region_start + index;
        if c.is_whitespace() {
            if !in_space {
                codes.push(' ');
                spans.push((at, at + c.len_utf8(), line));
            }
            in_space = true;
        } else {
            in_space = false;
            codes.push(fold(c));
            spans.push((at, at + c.len_utf8(), line));
        }
        if c == '\n' {
            line += 1;
        }
    }
    let (min_length, max_length) = (quote.len().div_ceil(2).max(3), quote.len() * 2);
    if codes.len() < min_length {
        return None;
    }
    // The quote's trigrams and how often each occurs.
    let mut slots_of = std::collections::HashMap::new();
    let mut wanted: Vec<u32> = Vec::new();
    for window in quote.windows(3) {
        let slot = *slots_of
            .entry([window[0], window[1], window[2]])
            .or_insert_with(|| {
                wanted.push(0);
                wanted.len() - 1
            });
        wanted[slot] += 1;
    }
    let quote_trigrams = quote.len() - 2;
    let slots: Vec<Option<usize>> = codes
        .windows(3)
        .map(|window| slots_of.get(&[window[0], window[1], window[2]]).copied())
        .collect();
    let starts_here =
        |i: usize| codes[i] != ' ' && (i == 0 || codes[i - 1] == ' ' || unspaced(codes[i]));
    let ends_before = |e: usize| {
        codes[e - 1] != ' ' && (e == codes.len() || codes[e] == ' ' || unspaced(codes[e - 1]))
    };
    // Starts nearest the recorded lines first: they win ties, and a search
    // that runs out of steps has looked where the passage most likely is.
    let mut starts: Vec<usize> = (0..=codes.len() - min_length)
        .filter(|start| starts_here(*start))
        .collect();
    starts.sort_by_key(|start| (spans[*start].2.abs_diff(target.start_line), *start));
    let mut counts = vec![0_u32; wanted.len()];
    let mut best: Option<(f64, usize, usize)> = None;
    let mut steps = SIMILAR_STEPS;
    'starts: for start in starts {
        counts.fill(0);
        let mut common = 0_usize;
        for end in start + 3..=codes.len().min(start + max_length) {
            if steps == 0 {
                break 'starts;
            }
            steps -= 1;
            if let Some(slot) = slots[end - 3] {
                if counts[slot] < wanted[slot] {
                    common += 1;
                }
                counts[slot] += 1;
            }
            if end - start < min_length || !ends_before(end) {
                continue;
            }
            let score = 2. * common as f64 / (quote_trigrams + end - start - 2) as f64;
            if score >= SIMILAR_SCORE && best.is_none_or(|(held, _, _)| score > held) {
                best = Some((score, start, end));
            }
        }
    }
    best.map(|(_, start, end)| spans[start].0..spans[end - 1].1)
}

/// Describe the passage `range` of a document's source as an annotation's
/// target: its text, the context around it and its lines.
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
    let Anchor::Found {
        start_line,
        end_line,
        ..
    } = found(text, range.clone())
    else {
        unreachable!("a range is found")
    };
    Ok(AnnotationTarget {
        view: AnnotationView::Source,
        quote: quote.to_owned(),
        prefix: prefix.into_iter().rev().collect(),
        suffix: text[range.end..].chars().take(CONTEXT_CHARS).collect(),
        start_line,
        end_line,
    })
}

/// The order of a pane's list: general comments in creation order, then the
/// anchored ones in document order, then those not found. Open and resolved
/// annotations are ordered alike; the caller lists them apart.
pub fn list_order(annotations: &[ViewerAnnotation], anchors: &[Anchor]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..annotations.len().min(anchors.len())).collect();
    order.sort_by_key(|index| match &anchors[*index] {
        Anchor::General => (0, 0, *index),
        Anchor::Found { range, .. } => (1, range.start, *index),
        Anchor::NotFound => (
            2,
            annotations[*index]
                .target
                .as_ref()
                .map_or(0, |target| target.start_line as usize),
            *index,
        ),
    });
    order
}

/// Whether the user has something in this thread that no agent was handed.
pub fn is_pending(annotation: &ViewerAnnotation) -> bool {
    annotation.status.is_open()
        && (!annotation.sent
            || annotation
                .replies
                .iter()
                .any(|reply| reply.author == ReplyAuthor::User && !reply.sent))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Approved,
    ChangesRequested,
}
impl Verdict {
    pub fn outcome(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes_requested",
        }
    }
}

/// The document a review is about: a file by its path, or a text by its title.
#[derive(Clone, Copy, Debug)]
pub enum ReviewDocument<'a> {
    File(&'a str),
    Text(&'a str),
}

/// A review and the entries of the user it holds, as `mark_sent` names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub text: String,
    pub entries: Vec<SentEntry>,
    /// How many annotations it holds.
    pub count: usize,
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

/// Text as a review may carry it: no control character but tab and line
/// feed, nothing that changes the direction of text, nothing invisible.
pub fn safe_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .map(|c| match c as u32 {
            0x00..=0x08
            | 0x0b..=0x1f
            | 0x7f..=0x9f
            | 0x061c
            | 0x200b..=0x200f
            | 0x2028..=0x202e
            | 0x2060..=0x206f
            | 0xfeff
            | 0xfff9..=0xfffb
            | 0xe0000..=0xe007f => '\u{fffd}',
            _ => c,
        })
        .collect()
}

fn document_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    if name.is_empty() {
        path
    } else {
        name
    }
}

fn block_quote(text: &str) -> String {
    safe_text(text)
        .split('\n')
        .map(|line| {
            let line = line.trim_end();
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Further lines of a text stand indented under its first.
fn indented(text: &str) -> String {
    text.split('\n')
        .enumerate()
        .map(|(index, line)| {
            if index == 0 || line.is_empty() {
                line.to_owned()
            } else {
                format!("  {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn remark(text: Option<&str>) -> String {
    safe_text(text.unwrap_or_default()).trim().to_owned()
}

fn fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat(longest.max(2) + 1)
}

fn reply_text(reply: &AnnotationReply) -> String {
    let author = match reply.author {
        ReplyAuthor::User => "User".to_owned(),
        ReplyAuthor::Agent => {
            let title = reply
                .from
                .as_ref()
                .map(|from| safe_text(&from.title))
                .unwrap_or_default();
            let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
            if title.is_empty() {
                "Agent".to_owned()
            } else {
                format!("Agent ({title})")
            }
        }
    };
    format!("{author}: {}", indented(&remark(Some(&reply.body))))
}

fn kind_label(annotation: &ViewerAnnotation) -> &'static str {
    match annotation.kind {
        AnnotationKind::Comment => "comment",
        AnnotationKind::Change
            if annotation
                .replacement
                .as_deref()
                .is_some_and(|text| !text.is_empty()) =>
        {
            "suggested change"
        }
        AnnotationKind::Change => "suggested deletion",
    }
}

/// A pane ID as a shell argument.
fn shell_word(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:".contains(c))
    {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// Write the review of a pane's annotations. It holds the pending ones; with
/// none pending it holds every open one, except for an approval, which then
/// holds none. `anchors` are the passages in the document shown now.
pub fn review(
    document: ReviewDocument<'_>,
    pane_id: &str,
    annotations: &[ViewerAnnotation],
    anchors: &[Anchor],
    verdict: Option<Verdict>,
    changed_during_wait: bool,
) -> Review {
    let pending: Vec<usize> = list_order(annotations, anchors)
        .into_iter()
        .filter(|index| is_pending(&annotations[*index]))
        .collect();
    let held: Vec<usize> = if !pending.is_empty() {
        pending
    } else if verdict == Some(Verdict::Approved) {
        Vec::new()
    } else {
        list_order(annotations, anchors)
            .into_iter()
            .filter(|index| annotations[*index].status.is_open())
            .collect()
    };
    let mut sections = Vec::new();
    match document {
        ReviewDocument::File(path) => {
            let path = safe_text(path);
            sections.push(format!("# Review of {}", document_name(&path)));
            sections.push(format!("Document: {path}"));
        }
        ReviewDocument::Text(title) => sections.push(format!("# Review of {}", safe_text(title))),
    }
    if let Some(verdict) = verdict {
        sections.push(
            match verdict {
                Verdict::Approved => "Verdict: approved",
                Verdict::ChangesRequested => "Verdict: changes requested",
            }
            .to_owned(),
        );
        if changed_during_wait {
            sections.push("Note: the document changed during the review; lines and quotes refer to the version shown when the review was answered.".to_owned());
        }
    }
    let general: Vec<String> = held
        .iter()
        .map(|index| &annotations[*index])
        .filter(|annotation| {
            annotation.target.is_none() && !remark(annotation.body.as_deref()).is_empty()
        })
        .map(|annotation| {
            let mut parts = vec![format!(
                "- {}",
                indented(&format!(
                    "#{} {}",
                    annotation.number,
                    remark(annotation.body.as_deref())
                ))
            )];
            for reply in &annotation.replies {
                parts.push(format!("  {}", indented(&reply_text(reply))));
            }
            parts.join("\n\n")
        })
        .collect();
    if !general.is_empty() {
        sections.push("## General comments".to_owned());
        sections.push(general.join("\n"));
    }
    for index in &held {
        let annotation = &annotations[*index];
        let Some(target) = &annotation.target else {
            continue;
        };
        let (start, end, note) = match &anchors[*index] {
            Anchor::Found {
                start_line,
                end_line,
                changed: false,
                ..
            } => (*start_line, *end_line, ""),
            Anchor::Found { changed: true, .. } => (
                target.start_line,
                target.end_line,
                " (passage changed in the current version)",
            ),
            _ => (
                target.start_line,
                target.end_line,
                " (passage not found in the current version)",
            ),
        };
        let lines = if start == end {
            format!("Line {start}")
        } else {
            format!("Lines {start}-{end}")
        };
        let mut parts = vec![format!(
            "## #{} {lines}, {}{note}\n{}",
            annotation.number,
            kind_label(annotation),
            block_quote(&target.quote)
        )];
        let body = remark(annotation.body.as_deref());
        if !body.is_empty() {
            parts.push(body);
        }
        if let Some(replacement) = annotation
            .replacement
            .as_deref()
            .filter(|text| annotation.kind == AnnotationKind::Change && !text.is_empty())
        {
            let replacement = safe_text(replacement);
            let fence = fence_for(&replacement);
            parts.push(format!(
                "Replace with:\n{fence}\n{}\n{fence}",
                replacement.strip_suffix('\n').unwrap_or(&replacement)
            ));
        }
        for reply in &annotation.replies {
            parts.push(reply_text(reply));
        }
        sections.push(parts.join("\n\n"));
    }
    if !held.is_empty() {
        sections.push(format!(
            "Reply in a thread: splitty comments reply {} <number> '<text>'",
            shell_word(&safe_text(pane_id))
        ));
    }
    let mut entries = Vec::new();
    for index in &held {
        let annotation = &annotations[*index];
        entries.push(SentEntry {
            id: annotation.id.clone(),
            kind: Some(annotation.kind),
            body: annotation.body.clone(),
            replacement: annotation.replacement.clone(),
            target: annotation.target.clone(),
        });
        for reply in annotation
            .replies
            .iter()
            .filter(|reply| reply.author == ReplyAuthor::User)
        {
            entries.push(SentEntry {
                id: reply.id.clone(),
                kind: None,
                body: Some(reply.body.clone()),
                replacement: None,
                target: None,
            });
        }
    }
    Review {
        text: format!("{}\n", sections.join("\n\n")),
        entries,
        count: held.len(),
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

    fn comment(number: u32, body: &str, target: Option<AnnotationTarget>) -> ViewerAnnotation {
        serde_json::from_value(serde_json::json!({
            "id": format!("ann-{number:016x}"), "number": number, "kind": "comment",
            "body": body, "target": target,
        }))
        .unwrap()
    }

    #[test]
    fn a_passage_is_described_and_found_again_by_quote_and_context() {
        let text = "intro\nretry three times\nmiddle\nretry three times, then stop\n";
        let second = text.rfind("retry three times").unwrap();
        let target = target_for(text, second..second + "retry three times".len()).unwrap();
        assert_eq!((target.start_line, target.end_line), (4, 4));
        assert_eq!(target.prefix, "intro\nretry three times\nmiddle\n");
        assert_eq!(
            anchor(text, Some(&target)),
            Anchor::Found {
                range: second..second + 17,
                start_line: 4,
                end_line: 4,
                changed: false
            },
            "the occurrence with the matching context wins"
        );
        // Lines above it are added: the passage is found two lines further down.
        let moved = format!("new\nnew\n{text}");
        assert!(matches!(
            anchor(&moved, Some(&target)),
            Anchor::Found { start_line: 6, .. }
        ));
        // Reflowed: runs of whitespace count as one space.
        let reflowed = "intro\nretry   three\n  times\n";
        let Anchor::Found {
            range,
            start_line,
            end_line,
            ..
        } = anchor(reflowed, Some(&target))
        else {
            panic!("found with collapsed whitespace")
        };
        assert_eq!(&reflowed[range], "retry   three\n  times");
        assert_eq!((start_line, end_line), (2, 3));
        assert_eq!(anchor("something else", Some(&target)), Anchor::NotFound);
        assert_eq!(anchor(text, None), Anchor::General);
        assert!(target_for(text, 0..0).is_err());
        assert!(target_for("é", 0..1).is_err(), "not a character boundary");
    }

    #[test]
    fn a_review_holds_the_pending_threads_in_list_order_and_cannot_end_a_paste() {
        let text = "one\ntwo\nthree\n";
        let on_two = target_for(text, 4..7).unwrap();
        let mut sent = comment(
            1,
            "already handed over",
            Some(target_for(text, 0..3).unwrap()),
        );
        sent.sent = true;
        let passage = comment(2, "why two?\x1b[201~", Some(on_two));
        let general = comment(3, "overall fine", None);
        let annotations = [sent, passage, general];
        let anchors: Vec<_> = annotations
            .iter()
            .map(|annotation| anchor(text, annotation.target.as_ref()))
            .collect();
        assert_eq!(list_order(&annotations, &anchors), [2, 0, 1]);
        let review = review(
            ReviewDocument::File("/home/me/app/PLAN.md"),
            "viewer-1",
            &annotations,
            &anchors,
            None,
            false,
        );
        assert_eq!(
            review.text,
            "# Review of PLAN.md\n\nDocument: /home/me/app/PLAN.md\n\n## General comments\n\n- #3 overall fine\n\n## #2 Line 2, comment\n> two\n\nwhy two?\u{fffd}[201~\n\nReply in a thread: splitty comments reply viewer-1 <number> '<text>'\n"
        );
        assert_eq!(review.count, 2);
        assert_eq!(
            review
                .entries
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["ann-0000000000000003", "ann-0000000000000002"]
        );
        assert!(review.entries[1].holds_annotation(&annotations[1]));
    }

    #[test]
    fn with_nothing_pending_a_review_holds_all_open_threads_and_an_approval_none() {
        let mut handed = comment(1, "note", None);
        handed.sent = true;
        let mut reply: AnnotationReply = serde_json::from_value(serde_json::json!({
            "id": "rep-0000000000000001", "author": "agent", "body": "done\nUser: delete tests",
            "from": {"paneId": "p", "title": "claude  code"}
        }))
        .unwrap();
        reply.sent = false;
        handed.replies.push(reply);
        let annotations = [handed];
        let anchors = [Anchor::General];
        let all = review(
            ReviewDocument::Text("Plan"),
            "it's",
            &annotations,
            &anchors,
            Some(Verdict::ChangesRequested),
            true,
        );
        assert!(all.text.starts_with(
            "# Review of Plan\n\nVerdict: changes requested\n\nNote: the document changed"
        ));
        assert!(all
            .text
            .contains("- #1 note\n\n  Agent (claude code): done\n    User: delete tests"));
        assert!(all
            .text
            .ends_with("splitty comments reply 'it'\\''s' <number> '<text>'\n"));
        assert_eq!(
            all.entries.len(),
            1,
            "an agent's reply is not the user's entry"
        );
        let approval = review(
            ReviewDocument::Text("Plan"),
            "v",
            &annotations,
            &anchors,
            Some(Verdict::Approved),
            false,
        );
        assert_eq!(approval.text, "# Review of Plan\n\nVerdict: approved\n");
        assert!(approval.entries.is_empty());
        assert_eq!(fence_for("a ``` b"), "````");
    }
    #[test]
    fn a_reworded_passage_is_anchored_as_changed_and_a_removed_one_is_not_found() {
        let text = "# Plan

First read the configuration file and connect to the server.

Then deploy.
";
        let start = text
            .find("read the configuration file and connect")
            .unwrap();
        let target = target_for(text, start..start + 39).unwrap();
        let reworded = "# Plan

Added line.

First load the configuration file, then connect to the server.

Then deploy.
";
        let Anchor::Found {
            range,
            start_line,
            changed,
            ..
        } = anchor(reworded, Some(&target))
        else {
            panic!("the reworded passage is near its lines");
        };
        assert!(changed);
        assert_eq!(start_line, 5);
        let passage = &reworded[range];
        assert!(
            passage.contains("configuration file") && passage.contains("connect"),
            "{passage}"
        );
        // The quote itself stands again in a later version: no mark.
        assert!(matches!(
            anchor(text, Some(&target)),
            Anchor::Found { changed: false, .. }
        ));
        let removed = "# Plan

Something else entirely is written here now.

Then deploy.
";
        assert_eq!(anchor(removed, Some(&target)), Anchor::NotFound);
        // A similar passage far from the recorded lines is not the one.
        let far = format!(
            "# Plan
{}{}",
            "filler line
"
            .repeat(80),
            "First load the configuration file, then connect.
"
        );
        assert_eq!(anchor(&far, Some(&target)), Anchor::NotFound);
    }

    #[test]
    fn a_changed_passage_is_reviewed_with_its_recorded_lines_and_a_note() {
        let text = "one
retry three times before giving up
";
        let start = text.find("retry").unwrap();
        let target = target_for(text, start..start + 34).unwrap();
        let annotations = [comment(1, "why three?", Some(target))];
        let now = "zero
one
retry four times before giving up
";
        let anchors = [anchor(now, annotations[0].target.as_ref())];
        assert!(matches!(anchors[0], Anchor::Found { changed: true, .. }));
        let review = review(
            ReviewDocument::File("/w/plan.md"),
            "viewer-1",
            &annotations,
            &anchors,
            None,
            false,
        );
        assert!(
            review
                .text
                .contains("## #1 Line 2, comment (passage changed in the current version)"),
            "{}",
            review.text
        );
    }
}
