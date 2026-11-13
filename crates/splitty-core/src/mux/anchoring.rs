//! Anchoring viewer annotations by their quote in a document's source
//! (`viewer-annotations`, Anchoring By Quote).
//!
//! An annotation keeps the text it was made on, a little context on both
//! sides and the source lines of the passage. The passage is found again by
//! that quote: among several occurrences the one whose context matches best
//! wins, then the one nearest the recorded lines. A quote that does not occur
//! is looked for again with every run of whitespace as one space, which finds
//! a reflowed paragraph, and then as the passage it most likely became: spans
//! that start within 50 lines of the recorded start line, are between half
//! and twice as long as the quote, and score by the Dice coefficient of their
//! character trigrams with the quote's, whitespace collapsed and case folded.
//! The best span that scores at least 0.6 anchors the annotation as changed.
//! Spans start and end at word boundaries, or at any character of a script
//! written without spaces. The work per annotation and per version of a text
//! is bounded, so that a long line or a large document cannot stall the
//! server. Lengths are counted in characters (Unicode scalar values); ranges
//! are byte offsets of the source text.
use std::collections::HashMap;
use std::ops::Range;

use super::state::{AnnotationTarget, ViewerAnnotation};

/// The most occurrences of one quote that are compared, so that a very
/// common word cannot stall anchoring.
const MAX_OCCURRENCES: usize = 100_000;
/// How far from the recorded start line a changed passage may start.
const SIMILAR_LINES: u32 = 50;
/// The lowest score of a changed passage.
const SIMILAR_SCORE: f64 = 0.6;
/// A longer quote, in characters, is not looked for by similarity.
const SIMILAR_MAX_QUOTE: usize = 4_000;
/// The most characters of the text in which a changed passage may start.
const SIMILAR_MAX_REGION: usize = 65_536;
/// The most span ends one annotation's search for a changed passage compares.
const SIMILAR_STEPS_PER_QUOTE: usize = 1_000_000;
/// The most span ends the searches against one version of a text compare.
const SIMILAR_MAX_STEPS: usize = 4_000_000;

/// A document's source text as annotations are anchored in it: its bytes as
/// UTF-8 with U+FFFD for invalid sequences, without a leading byte order
/// mark, and with every line break as a line feed. Ranges of anchors count
/// bytes of this text.
pub fn viewer_source_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_owned()
    }
}

/// Where an annotation's passage is in a version of its document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Passage {
    /// A general comment names no passage.
    General,
    /// Neither the quote nor anything similar near its lines is in this
    /// version; the recorded quote and lines stand.
    Lost,
    Found {
        range: Range<usize>,
        start_line: u32,
        end_line: u32,
        /// The quote is not in this version: this is the passage it most
        /// likely became.
        changed: bool,
    },
}

/// Anchors any number of annotations against one version of a text. The
/// whitespace-collapsed copy is built only once a quote needs it, and the
/// searches for changed passages share one budget.
pub struct Anchorer<'a> {
    text: &'a str,
    /// The byte at which each line starts.
    line_starts: Vec<usize>,
    collapsed: Option<Collapsed>,
    steps: usize,
}

impl<'a> Anchorer<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        Self {
            text,
            line_starts,
            collapsed: None,
            steps: SIMILAR_MAX_STEPS,
        }
    }

    /// The 1-based line that holds the byte `offset`.
    fn line_of(&self, offset: usize) -> u32 {
        let line = self.line_starts.partition_point(|start| *start <= offset);
        u32::try_from(line.max(1)).unwrap_or(u32::MAX)
    }

    /// The first byte of the line `line`, or the text's end beyond its last.
    fn line_start(&self, line: u32) -> usize {
        let index = usize::try_from(line.max(1) - 1).unwrap_or(usize::MAX);
        self.line_starts
            .get(index)
            .copied()
            .unwrap_or(self.text.len())
    }

    /// A passage ends on the line of its last character.
    fn found(&self, range: Range<usize>, changed: bool) -> Passage {
        let last = self.text[range.start..range.end]
            .char_indices()
            .next_back()
            .map_or(range.start, |(at, _)| range.start + at);
        Passage::Found {
            start_line: self.line_of(range.start),
            end_line: self.line_of(last),
            range,
            changed,
        }
    }

    /// Find the passage of an annotation with `target`.
    pub fn anchor(&mut self, target: Option<&AnnotationTarget>) -> Passage {
        let Some(target) = target else {
            return Passage::General;
        };
        if target.quote.is_empty() {
            return Passage::Lost;
        }
        let text = self.text;
        if let Some(range) = best_occurrence(
            text,
            &target.quote,
            &target.prefix,
            &target.suffix,
            target.start_line,
            |offset| self.line_of(offset),
        ) {
            return self.found(range, false);
        }
        let (quote, _) = collapse(&target.quote);
        let quote = quote.trim_matches(' ');
        if quote.is_empty() {
            return Passage::Lost;
        }
        let collapsed = self.collapsed.take().unwrap_or_else(|| {
            let (text, origin) = collapse(text);
            Collapsed { text, origin }
        });
        let loose = best_occurrence(
            &collapsed.text,
            quote,
            &collapse(&target.prefix).0,
            &collapse(&target.suffix).0,
            target.start_line,
            |offset| self.line_of(collapsed.origin[offset]),
        )
        .map(|range| {
            // The collapsed quote starts and ends with a character that is
            // not whitespace, so both ends map to that very character.
            let last = collapsed.origin[range.end - 1];
            collapsed.origin[range.start]..last + char_len(text, last)
        });
        self.collapsed = Some(collapsed);
        if let Some(range) = loose {
            return self.found(range, false);
        }
        match self.similar(target) {
            Some(range) => self.found(range, true),
            None => Passage::Lost,
        }
    }

    /// The span the quote most likely became, near its recorded start line,
    /// or none that scores at least [`SIMILAR_SCORE`].
    ///
    /// Every start's spans share at most as many trigrams with the quote as
    /// the longest of them, which bounds their score; the starts are compared
    /// in the order of that bound, so that the search ends once no start left
    /// can beat the best span. The best score wins, then the start nearest the
    /// recorded line, then the earlier start, then the shorter span. The
    /// search also ends when its share of the budget is spent, with the best
    /// span so far.
    fn similar(&mut self, target: &AnnotationTarget) -> Option<Range<usize>> {
        let text = self.text;
        if self.steps == 0
            || text.is_empty()
            || target.quote.chars().nth(SIMILAR_MAX_QUOTE).is_some()
        {
            return None;
        }
        let (plain, _) = collapse(&target.quote);
        let quote: Vec<char> = plain.trim_matches(' ').chars().map(fold).collect();
        let quote_trigrams = quote.len().checked_sub(2).filter(|count| *count > 0)?;

        // The distinct trigrams of the quote and how often each occurs.
        let mut index: HashMap<u64, usize> = HashMap::new();
        let mut wanted: Vec<u32> = Vec::new();
        for window in quote.windows(3) {
            let slot = *index.entry(trigram(window)).or_insert_with(|| {
                wanted.push(0);
                wanted.len() - 1
            });
            wanted[slot] += 1;
        }

        // Where spans may start: the lines around the recorded start line,
        // and at most SIMILAR_MAX_REGION characters around it.
        let start_line = target.start_line;
        let mut region_start = self.line_start(start_line.saturating_sub(SIMILAR_LINES));
        let mut region_end = self.line_start(start_line.saturating_add(SIMILAR_LINES + 1));
        if region_start >= region_end {
            return None;
        }
        if text[region_start..region_end]
            .chars()
            .nth(SIMILAR_MAX_REGION)
            .is_some()
        {
            let centre = self.line_start(start_line).clamp(region_start, region_end);
            region_start = text[region_start..centre]
                .char_indices()
                .rev()
                .nth(SIMILAR_MAX_REGION / 2 - 1)
                .map_or(region_start, |(at, _)| region_start + at);
            region_end = text[region_start..region_end]
                .char_indices()
                .nth(SIMILAR_MAX_REGION)
                .map_or(region_end, |(at, _)| region_start + at);
        }

        let min_length = quote.len().div_ceil(2).max(3);
        let max_length = quote.len() * 2;
        let Normalized { codes, origin } = normalized(text, region_start, region_end, max_length);
        let length = codes.len();
        if length < min_length {
            return None;
        }

        // Per position, the slot of the quote's trigram that starts there.
        let positions = length.saturating_sub(2);
        let slots: Vec<Option<usize>> = (0..positions)
            .map(|at| index.get(&trigram(&codes[at..at + 3])).copied())
            .collect();
        // 1: a span may start here; 2: a span may end here.
        let mut boundary = vec![0_u8; length + 1];
        for at in 0..length {
            let space = codes[at] == ' ';
            let previous_space = at == 0 || codes[at - 1] == ' ';
            let unspaced = !space && is_unspaced(codes[at]);
            if !space && (previous_space || unspaced) {
                boundary[at] |= 1;
            }
            if !space && (at + 1 == length || codes[at + 1] == ' ' || unspaced) {
                boundary[at + 1] |= 2;
            }
        }

        // The bound of each start, from the trigrams the quote shares with
        // its longest span, kept in a window that slides along the starts.
        let min_trigrams = min_length - 2;
        let mut counts = vec![0_u32; wanted.len()];
        let mut starts: Vec<(usize, f64)> = Vec::new();
        let mut shared = 0_usize;
        let mut high = 0_usize;
        let mut start = 0;
        while start + min_length <= length && origin[start] < region_end {
            let top = positions.min(start + max_length - 2);
            high = high.max(start);
            while high < top {
                if let Some(slot) = slots[high] {
                    if counts[slot] < wanted[slot] {
                        shared += 1;
                    }
                    counts[slot] += 1;
                }
                high += 1;
            }
            if boundary[start] & 1 != 0 {
                let bound = ratio(2 * shared, quote_trigrams + shared.max(min_trigrams));
                if bound >= SIMILAR_SCORE {
                    starts.push((start, bound));
                }
            }
            if start < high {
                if let Some(slot) = slots[start] {
                    counts[slot] -= 1;
                    if counts[slot] < wanted[slot] {
                        shared -= 1;
                    }
                }
            }
            start += 1;
        }
        if starts.is_empty() {
            return None;
        }
        let mut order: Vec<(usize, f64, u32)> = starts
            .into_iter()
            .map(|(start, bound)| {
                let distance = self.line_of(origin[start]).abs_diff(start_line);
                (start, bound, distance)
            })
            .collect();
        order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));

        counts.fill(0);
        let mut touched: Vec<usize> = Vec::new();
        // The score, the distance and the start and end of the best span.
        let mut best: Option<(f64, u32, usize, usize)> = None;
        let allowed = self.steps.min(SIMILAR_STEPS_PER_QUOTE);
        let mut steps = allowed;
        for (start, bound, distance) in order {
            if steps == 0 || best.is_some_and(|(score, ..)| bound < score) {
                break;
            }
            let mut common = 0_usize;
            let last = length.min(start + max_length);
            for end in start + 3..=last {
                steps = steps.saturating_sub(1);
                if let Some(slot) = slots[end - 3] {
                    if counts[slot] == 0 {
                        touched.push(slot);
                    }
                    if counts[slot] < wanted[slot] {
                        common += 1;
                    }
                    counts[slot] += 1;
                }
                let span_trigrams = end - start - 2;
                // A longer span cannot do better than the best so far.
                let floor = best.map_or(SIMILAR_SCORE, |(score, ..)| score);
                if ratio(2 * quote_trigrams, quote_trigrams + span_trigrams) < floor {
                    break;
                }
                if end - start < min_length || boundary[end] & 2 == 0 {
                    continue;
                }
                let score = ratio(2 * common, quote_trigrams + span_trigrams);
                if score < SIMILAR_SCORE {
                    continue;
                }
                let better = best.is_none_or(|(held, held_distance, held_start, _)| {
                    score > held || (score == held && (distance, start) < (held_distance, held_start))
                });
                if better {
                    best = Some((score, distance, start, end));
                }
            }
            for slot in touched.drain(..) {
                counts[slot] = 0;
            }
        }
        self.steps -= allowed - steps;
        best.map(|(_, _, start, end)| {
            let last = origin[end - 1];
            origin[start]..last + char_len(text, last)
        })
    }
}

/// Anchor each of `annotations` in `text`, a document's source text.
pub fn anchor_annotations(text: &str, annotations: &[ViewerAnnotation]) -> Vec<Passage> {
    let mut anchorer = Anchorer::new(text);
    annotations
        .iter()
        .map(|annotation| anchorer.anchor(annotation.target.as_ref()))
        .collect()
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    // Counts of a document of at most some MiB: exact in an f64.
    numerator as f64 / denominator.max(1) as f64
}

fn char_len(text: &str, at: usize) -> usize {
    text[at..].chars().next().map_or(1, char::len_utf8)
}

fn trigram(window: &[char]) -> u64 {
    (u64::from(window[0]) << 42) | (u64::from(window[1]) << 21) | u64::from(window[2])
}

fn fold(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(single), None) => single,
        _ => c,
    }
}

/// Scripts written without spaces between words: Han, Hiragana, Katakana,
/// Thai, Lao, Khmer and Myanmar. A span may start and end at any of their
/// characters.
fn is_unspaced(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0E00..=0x0EFF // Thai, Lao
            | 0x1000..=0x109F // Myanmar
            | 0x1780..=0x17FF // Khmer
            | 0x19E0..=0x19FF // Khmer symbols
            | 0x2E80..=0x2FDF // CJK radicals
            | 0x3005 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303B
            | 0x3041..=0x309F // Hiragana
            | 0x30A0..=0x30FF // Katakana
            | 0x31F0..=0x31FF // Katakana extensions
            | 0x3400..=0x4DBF // CJK extension A
            | 0x4E00..=0x9FFF // CJK unified ideographs
            | 0xA9E0..=0xA9FF // Myanmar extended B
            | 0xAA60..=0xAA7F // Myanmar extended A
            | 0xF900..=0xFAFF // CJK compatibility ideographs
            | 0xFF66..=0xFF9D // halfwidth Katakana
            | 0x1B000..=0x1B16F // Kana supplement and extensions
            | 0x20000..=0x3FFFF // CJK extensions B and later
    )
}

/// How many characters of context around `range` match, counted outward.
fn context_score(text: &str, range: &Range<usize>, prefix: &str, suffix: &str) -> usize {
    let before = text[..range.start]
        .chars()
        .rev()
        .zip(prefix.chars().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let after = text[range.end..]
        .chars()
        .zip(suffix.chars())
        .take_while(|(a, b)| a == b)
        .count();
    before + after
}

/// The best occurrence of `quote` in `text`, overlapping ones included: the
/// most matching context, then the line nearest `start_line`, then the first.
fn best_occurrence(
    text: &str,
    quote: &str,
    prefix: &str,
    suffix: &str,
    start_line: u32,
    line_of: impl Fn(usize) -> u32,
) -> Option<Range<usize>> {
    let mut best: Option<(usize, u32, Range<usize>)> = None;
    let mut from = 0;
    for _ in 0..MAX_OCCURRENCES {
        let Some(found) = text.get(from..).and_then(|rest| rest.find(quote)) else {
            break;
        };
        let at = from + found;
        let range = at..at + quote.len();
        let score = context_score(text, &range, prefix, suffix);
        let distance = line_of(at).abs_diff(start_line);
        if best
            .as_ref()
            .is_none_or(|(held, held_distance, _)| score > *held || (score == *held && distance < *held_distance))
        {
            best = Some((score, distance, range));
        }
        from = at + char_len(text, at);
    }
    best.map(|(_, _, range)| range)
}

/// A text with every run of whitespace as one space.
struct Collapsed {
    text: String,
    /// For each byte of `text`, the byte of the original it came from (the
    /// start of its character); one more entry holds the original length.
    origin: Vec<usize>,
}

fn collapse(text: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len() + 1);
    let mut in_space = false;
    for (at, c) in text.char_indices() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
                origin.push(at);
            }
            in_space = true;
        } else {
            in_space = false;
            out.push(c);
            origin.extend(std::iter::repeat_n(at, c.len_utf8()));
        }
    }
    origin.push(text.len());
    (out, origin)
}

/// A run of text with whitespace collapsed to one space and case folded.
struct Normalized {
    codes: Vec<char>,
    /// The byte in the original text of each character.
    origin: Vec<usize>,
}

/// `text` from `from` on, normalized, until the original offset reaches
/// `until` and `extra` more characters follow, or to its end.
fn normalized(text: &str, from: usize, until: usize, extra: usize) -> Normalized {
    let mut codes = Vec::new();
    let mut origin = Vec::new();
    let mut in_space = false;
    let mut after_until = 0;
    for (index, c) in text[from..].char_indices() {
        let at = from + index;
        if c.is_whitespace() {
            if in_space {
                continue;
            }
            in_space = true;
            codes.push(' ');
        } else {
            in_space = false;
            codes.push(fold(c));
        }
        origin.push(at);
        if at >= until {
            after_until += 1;
            if after_until > extra {
                break;
            }
        }
    }
    Normalized { codes, origin }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::state::AnnotationView;

    /// The target of the first occurrence of `quote` in `text`, as a
    /// selection of it would give.
    fn selection(text: &str, quote: &str) -> AnnotationTarget {
        let start = text.find(quote).expect("the quote is in the text");
        let end = start + quote.len();
        let anchorer = Anchorer::new(text);
        let Passage::Found {
            start_line,
            end_line,
            ..
        } = anchorer.found(start..end, false)
        else {
            unreachable!()
        };
        let prefix: Vec<char> = text[..start].chars().rev().take(32).collect();
        AnnotationTarget {
            view: AnnotationView::Source,
            quote: quote.to_owned(),
            prefix: prefix.into_iter().rev().collect(),
            suffix: text[end..].chars().take(32).collect(),
            start_line,
            end_line,
        }
    }

    fn target(quote: &str, start_line: u32) -> AnnotationTarget {
        AnnotationTarget {
            view: AnnotationView::Source,
            quote: quote.to_owned(),
            prefix: String::new(),
            suffix: String::new(),
            start_line,
            end_line: start_line,
        }
    }

    fn anchor(text: &str, target: &AnnotationTarget) -> Passage {
        Anchorer::new(text).anchor(Some(target))
    }

    /// The text and lines of a found passage, and whether it changed.
    fn found(text: &str, passage: &Passage) -> (String, u32, u32, bool) {
        let Passage::Found {
            range,
            start_line,
            end_line,
            changed,
        } = passage
        else {
            panic!("not found: {passage:?}")
        };
        (text[range.clone()].to_owned(), *start_line, *end_line, *changed)
    }

    fn numbered(count: usize, from: usize) -> String {
        (from..from + count)
            .map(|line| format!("filler line {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // Ranges in the source (mux-protocol, Review Texts And Anchors).
    #[test]
    fn the_source_text_drops_a_byte_order_mark_and_keeps_only_line_feeds() {
        assert_eq!(
            viewer_source_text(b"\xef\xbb\xbfone\r\ntwo\rthree\n"),
            "one\ntwo\nthree\n"
        );
        assert_eq!(viewer_source_text(b"a\xffb"), "a\u{fffd}b");
        assert_eq!(viewer_source_text("x\u{feff}".as_bytes()), "x\u{feff}");
        let text = viewer_source_text("\u{feff}é\r\nretry".as_bytes());
        let Passage::Found { range, start_line, .. } = anchor(&text, &target("retry", 2)) else {
            panic!("found")
        };
        assert_eq!((range, start_line), (3..8, 2), "bytes after the mark and the folded break");
    }

    #[test]
    fn a_passage_ends_on_the_line_of_its_last_character() {
        let anchorer = Anchorer::new("one\ntwo\nthree");
        assert!(matches!(
            anchorer.found(0..4, false),
            Passage::Found { start_line: 1, end_line: 1, .. }
        ));
        assert!(matches!(
            anchorer.found(4..13, false),
            Passage::Found { start_line: 2, end_line: 3, .. }
        ));
    }

    // Lines inserted above the passage.
    #[test]
    fn a_passage_is_found_again_at_its_new_lines() {
        let before = format!(
            "{}\nThe cache is rebuilt on every change.\n{}",
            numbered(4, 1),
            numbered(3, 6)
        );
        let quote = selection(&before, "rebuilt on every change");
        assert_eq!(quote.start_line, 5);
        let after = format!("{}\n{before}", numbered(10, 100));
        assert_eq!(
            found(&after, &anchor(&after, &quote)),
            ("rebuilt on every change".into(), 15, 15, false)
        );
    }

    #[test]
    fn a_passage_that_is_gone_is_lost_and_found_again_once_it_is_back() {
        let text = "Step one.\nRetry three times.\nStep three.\n";
        let quote = selection(text, "Retry three times");
        assert_eq!(anchor("Step one.\nStep three.\n", &quote), Passage::Lost);
        let again = "Intro.\nStep one.\nRetry three times.\nStep three.\n";
        assert_eq!(found(again, &anchor(again, &quote)).1, 3);
    }

    // The same words twice.
    #[test]
    fn the_occurrence_with_the_recorded_context_wins_over_a_nearer_one() {
        let original = "intro\nThen deploy, run the tests again.\n";
        let quote = selection(original, "the tests");
        let changed = format!(
            "intro\nthe tests\n{}\nThen deploy, run the tests again.\n",
            numbered(20, 1)
        );
        let passage = anchor(&changed, &quote);
        assert_eq!(found(&changed, &passage).1, 23);
        // Another occurrence with the context, for the native window's case.
        let text = "intro\nretry three times\nmiddle\nretry three times, then stop\n";
        let second = text.rfind("retry three times").expect("twice");
        let mut quote = selection(text, "retry three times");
        quote.prefix = text[..second].chars().rev().take(32).collect::<Vec<_>>().into_iter().rev().collect();
        quote.suffix = ", then stop\n".into();
        quote.start_line = 4;
        let Passage::Found { range, .. } = anchor(text, &quote) else {
            panic!("found")
        };
        assert_eq!(range.start, second);
    }

    #[test]
    fn among_occurrences_with_equal_context_the_nearest_line_wins() {
        let text = ["x", "TODO", "x", "x", "x", "x", "TODO", "x"].join("\n");
        assert_eq!(found(&text, &anchor(&text, &target("TODO", 6))).1, 7);
    }

    #[test]
    fn overlapping_occurrences_are_all_compared() {
        let text = "aaaa";
        let mut quote = target("aa", 1);
        quote.prefix = "aa".into();
        let Passage::Found { range, .. } = anchor(text, &quote) else {
            panic!("found")
        };
        assert_eq!(range, 2..4);
    }

    // A reflowed paragraph.
    #[test]
    fn a_reflowed_paragraph_is_found_with_whitespace_as_one_space() {
        let original = "Intro.\nThe session list is cached and\ninvalidated on every change.\nEnd.";
        let quote = selection(original, "cached and\ninvalidated on every");
        let reflowed = "Intro.\nThe session list is cached  and invalidated\n   on every change.\nEnd.";
        assert_eq!(
            found(reflowed, &anchor(reflowed, &quote)),
            ("cached  and invalidated\n   on every".into(), 2, 3, false)
        );
        // From the native window's suite: a passage rewrapped onto two lines.
        let text = "intro\nretry three times\nmiddle\n";
        let quote = selection(text, "retry three times");
        let rewrapped = "intro\nretry   three\n  times\n";
        assert_eq!(
            found(rewrapped, &anchor(rewrapped, &quote)),
            ("retry   three\n  times".into(), 2, 3, false)
        );
        // NEL is whitespace, a byte order mark is not.
        let nel = "intro\nretry\u{85}three times\n";
        assert!(matches!(anchor(nel, &quote), Passage::Found { changed: false, .. }));
        assert!(!matches!(
            anchor("intro\nretry\u{feff}three times\n", &quote),
            Passage::Found { changed: false, .. }
        ));
    }

    #[test]
    fn an_empty_quote_and_a_general_comment() {
        assert_eq!(anchor("anything", &target("", 1)), Passage::Lost);
        assert_eq!(anchor("anything", &target(" \n ", 1)), Passage::Lost);
        assert_eq!(Anchorer::new("anything").anchor(None), Passage::General);
    }

    #[test]
    fn many_annotations_against_one_version() {
        let mut anchorer = Anchorer::new("alpha\nbeta\ngamma\n");
        let states: Vec<_> = ["alpha", "gamma", "delta"]
            .into_iter()
            .map(|quote| matches!(anchorer.anchor(Some(&target(quote, 1))), Passage::Found { .. }))
            .collect();
        assert_eq!(states, [true, true, false]);
    }

    const PLAN: &str = "# Plan\n\n1. Read the configuration from the file.\n2. Cache the session list and invalidate it on every change.\n3. Retry three times before giving up.\n4. Write the result to the log.\n";
    const QUOTE: &str = "Cache the session list and invalidate it on every change.";

    // The passage reworded.
    #[test]
    fn a_reworded_passage_is_anchored_as_changed() {
        let quote = selection(PLAN, QUOTE);
        let reworded = PLAN
            .replace("# Plan\n", "# Plan\n\nA short introduction.\n")
            .replace(QUOTE, "Cache the list of sessions and invalidate it on each change.");
        assert_eq!(
            found(&reworded, &anchor(&reworded, &quote)),
            (
                "Cache the list of sessions and invalidate it on each change.".into(),
                6,
                6,
                true
            )
        );
        // From the native window's suite.
        let text = "# Plan\n\nFirst read the configuration file and connect to the server.\n\nThen deploy.\n";
        let quote = selection(text, "read the configuration file and connect");
        let reworded = "# Plan\n\nAdded line.\n\nFirst load the configuration file, then connect to the server.\n\nThen deploy.\n";
        let (passage, start_line, _, changed) = found(reworded, &anchor(reworded, &quote));
        assert!(changed && start_line == 5, "{passage}");
        assert!(passage.contains("configuration file") && passage.contains("connect"));
        assert!(matches!(anchor(text, &quote), Passage::Found { changed: false, .. }));
    }

    #[test]
    fn case_is_folded_and_whitespace_collapsed_before_comparing() {
        let quote = selection(PLAN, QUOTE);
        let shouted = PLAN.replace(QUOTE, "CACHE the session  list and\ninvalidate it on any change.");
        assert_eq!(
            found(&shouted, &anchor(&shouted, &quote)),
            ("CACHE the session  list and\ninvalidate it on any change.".into(), 4, 5, true)
        );
    }

    // The passage removed.
    #[test]
    fn a_removed_passage_is_lost_when_nothing_similar_is_near() {
        let quote = selection(PLAN, QUOTE);
        let rewritten = PLAN.replace(&format!("2. {QUOTE}\n"), "2. Compute the list on demand.\n");
        assert_eq!(anchor(&rewritten, &quote), Passage::Lost);
        let removed = "# Plan\n\nSomething else entirely is written here now.\n\nThen deploy.\n";
        assert_eq!(anchor(removed, &quote), Passage::Lost);
    }

    #[test]
    fn a_changed_passage_is_looked_for_only_near_the_recorded_lines() {
        let quote = selection(PLAN, QUOTE);
        let reworded = "Cache the list of sessions and invalidate it on each change.";
        let far = format!("{}\n{reworded}\n", numbered(SIMILAR_LINES as usize + 10, 1));
        assert_eq!(anchor(&far, &quote), Passage::Lost);
        let near = format!("{}\n{reworded}\n", numbered(SIMILAR_LINES as usize - 10, 1));
        assert!(matches!(anchor(&near, &quote), Passage::Found { changed: true, .. }));
    }

    #[test]
    fn among_equally_similar_passages_the_nearest_wins() {
        let quote = target("retry three times before giving up", 20);
        let mut lines: Vec<String> = numbered(30, 1).split('\n').map(str::to_owned).collect();
        lines[4] = "Retry four times before giving up".into();
        lines[22] = "Retry four times before giving up".into();
        let text = lines.join("\n");
        let (_, start_line, _, changed) = found(&text, &anchor(&text, &quote));
        assert!(changed);
        assert_eq!(start_line, 23);
    }

    #[test]
    fn a_changed_passage_may_run_past_the_search_region() {
        // The passage starts on the region's last line and ends on the next.
        let quote = target("alpha beta gamma delta epsilon zeta", 1);
        let text = format!(
            "{}\nalpha beta gamma\ndelta epsilon theta\n",
            numbered(SIMILAR_LINES as usize, 2)
        );
        let (passage, start_line, end_line, changed) = found(&text, &anchor(&text, &quote));
        assert!(changed);
        assert_eq!((start_line, end_line), (51, 52), "{passage}");
    }

    #[test]
    fn a_quote_of_more_than_4000_characters_is_not_looked_for_by_similarity() {
        let long = "word ".repeat(SIMILAR_MAX_QUOTE / 5 + 1);
        let long = long.trim();
        let text = format!("{} other", &long[..long.len() - 10]);
        assert_eq!(anchor(&text, &target(long, 1)), Passage::Lost);
        let fits = &long[..3_000];
        let text = format!("{} other", &fits[..fits.len() - 10]);
        assert!(matches!(anchor(&text, &target(fits, 1)), Passage::Found { changed: true, .. }));
    }

    // A script without spaces.
    #[test]
    fn a_passage_of_a_script_without_spaces_starts_and_ends_at_any_character() {
        let text = "最初に設定ファイルを読み込んで接続する。次に進む。";
        let quote = target("設定ファイルを読み込んでから接続する", 1);
        assert_eq!(found(text, &anchor(text, &quote)).0, "設定ファイルを読み込んで接続する");
        // Lao, Khmer and Myanmar as well.
        for (text, quote, expected) in [
            ("ນີ້ແມ່ນການທົດສອບຂໍ້ຄວາມພາສາລາວ ແລະອື່ນໆ", "ການທົດສອບຂໍ້ຄວາມພາສາລາວຍາວ", "ການທົດສອບຂໍ້ຄວາມພາສາລາວ"),
            ("នេះជាការសាកល្បងអត្ថបទខ្មែរ និងផ្សេងទៀត", "ការសាកល្បងអត្ថបទខ្មែរវែង", "ការសាកល្បងអត្ថបទខ្មែរ"),
            ("ဒါဟာမြန်မာစာသားစမ်းသပ်မှုဖြစ်ပါတယ် နောက်ထပ်", "မြန်မာစာသားစမ်းသပ်မှုရှည်", "မြန်မာစာသားစမ်းသပ်မှု"),
        ] {
            let passage = anchor(text, &target(quote, 1));
            let (span, _, _, changed) = found(text, &passage);
            assert!(changed);
            assert!(span.starts_with(&expected[..expected.len().min(9)]), "{span}");
        }
    }

    #[test]
    fn a_large_document_or_a_long_line_does_not_stall_anchoring() {
        let words = ["cache", "session", "list", "retry", "change", "window", "server", "review", "thread", "passage"];
        let mut seed: u32 = 7;
        let mut next = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 8) & 0xffff
        };
        let mut prose = |chars: usize, width: usize| {
            let mut out = String::new();
            let mut column = 0;
            while out.len() < chars {
                let word = format!("{}{}", words[next() as usize % words.len()], next() % 100);
                if column + word.len() > width {
                    out.push('\n');
                    column = 0;
                } else {
                    out.push(' ');
                    column += word.len() + 1;
                }
                out.push_str(&word);
            }
            out
        };
        let large = prose(2 * 1024 * 1024, 80);
        let line = prose(512 * 1024, usize::MAX);
        let quotes: Vec<String> = (0..20)
            .map(|i| prose(SIMILAR_MAX_QUOTE - 100 - i, usize::MAX).trim().to_owned())
            .collect();
        for text in [&large, &line] {
            let mut anchorer = Anchorer::new(text);
            let started = std::time::Instant::now();
            for (i, quote) in quotes.iter().enumerate() {
                anchorer.anchor(Some(&target(quote, 1 + 500 * u32::try_from(i).unwrap_or(0))));
            }
            assert!(anchorer.steps <= SIMILAR_MAX_STEPS);
            assert!(started.elapsed() < std::time::Duration::from_secs(60));
        }
    }
}
