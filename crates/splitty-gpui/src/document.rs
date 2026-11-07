//! Native Markdown, inert resources, and source-based annotation targets.
use gpui_kit::{
    base::text::{RangeHighlight, RenderedText, TextView, TextViewState, TextViewStyle},
    prelude::*,
    *,
};
use serde_json::Value;
use splitty_client_core::AnnotationDraft;
use std::{cell::Cell, ops::Range, rc::Rc, sync::Arc};

pub const FIXTURE: &str = "# Native Markdown probe\n\nSelect a passage and choose **Annotate selection**. Marks are saved by the isolated service for this run.\n\n## Unicode and wrapping\n\nCafé — naïve — 日本語 — 👩🏽‍💻. This paragraph deliberately wraps when the pane becomes narrow. Selection and annotation ranges must stay on UTF-8 boundaries.\n\n> Native annotations follow the rendered glyph layout.\n\n```rust\nfn main() { println!(\"hello from native text\"); }\n```\n\n| Probe | State |\n| --- | --- |\n| Terminal | service-owned PTY |\n| Markdown | native selection |\n\n[Inert link](https://example.com) — links and document images perform no external action in this probe.\n";

pub struct DocumentView {
    pub text: Entity<TextViewState>,
    source: String,
    annotations: Vec<Value>,
    last_rendered: Option<RenderedText>,
    revision: u64,
    painted_revision: Rc<Cell<u64>>,
    mapped_marks: usize,
    pub status: String,
}
impl DocumentView {
    pub fn new(cx: &mut App) -> Self {
        Self {
            text: cx.new(|cx| {
                TextViewState::markdown("Waiting for service document…", cx)
                    .selectable(true)
                    .scrollable(true)
            }),
            source: String::new(),
            annotations: Vec::new(),
            last_rendered: None,
            revision: 1,
            painted_revision: Rc::new(Cell::new(0)),
            mapped_marks: 0,
            status: "Document not loaded".into(),
        }
    }
    pub fn loaded(&self) -> bool {
        !self.source.is_empty()
    }
    pub fn painted_current(&self) -> bool {
        self.loaded()
            && self.mapped_marks > 0
            && self
                .last_rendered
                .as_ref()
                .is_some_and(|text| text.source() == self.source)
            && self.painted_revision.get() == self.revision
    }
    pub fn count(&self) -> usize {
        self.annotations.len()
    }
    pub fn set_source(&mut self, source: String, cx: &mut App) {
        self.text
            .update(cx, |state, cx| state.set_text(&source, cx));
        self.source = source;
        self.last_rendered = None;
        self.status = "Select text to annotate".into();
    }
    pub fn snapshot(&mut self, annotations: &[Value]) -> bool {
        if self.annotations != annotations {
            self.annotations = annotations.to_vec();
            self.last_rendered = None;
            return true;
        }
        false
    }
    pub fn selection(&self, cx: &App) -> Result<AnnotationDraft, String> {
        let state = self.text.read(cx);
        let range = state
            .selected_source_range()
            .ok_or("Select a passage first")?;
        draft(&self.source, range)
    }
    pub fn smoke_annotation(&self) -> Result<AnnotationDraft, String> {
        let start = self.source.find("Café").ok_or("Smoke passage missing")?;
        draft(&self.source, start..start + "Café — naïve — 日本語".len())
    }
    pub fn refresh_marks(&mut self, cx: &mut App) -> bool {
        let rendered = self.text.read(cx).rendered_text();
        if self.last_rendered.as_ref() == Some(&rendered) {
            return false;
        }
        // Parsing is asynchronous. Never convert offsets against an older source.
        if rendered.source() != self.source {
            return false;
        }
        let highlights = self
            .annotations
            .iter()
            .filter_map(|annotation| {
                let target = annotation.get("target")?;
                let quote = target.get("quote")?.as_str()?;
                let prefix = target.get("prefix").and_then(Value::as_str).unwrap_or("");
                let suffix = target.get("suffix").and_then(Value::as_str).unwrap_or("");
                let range = anchor_exact(&self.source, quote, prefix, suffix)?;
                rendered
                    .range_for_source(range)
                    .map(|range| RangeHighlight::new(range, rgba(0xffc85755)))
            })
            .collect::<Vec<_>>();
        let count = highlights.len();
        match self
            .text
            .update(cx, |state, cx| state.set_range_highlights(highlights, cx))
        {
            Ok(()) => {
                self.mapped_marks = count;
                self.revision += 1;
                self.last_rendered = Some(rendered);
                true
            }
            Err(error) => {
                let status = error.to_string();
                let changed = self.status != status;
                self.status = status;
                self.mapped_marks = 0;
                changed
            }
        }
    }
    pub fn render(&self) -> impl IntoElement {
        let painted = self.painted_revision.clone();
        let revision = self.revision;
        div()
            .relative()
            .size_full()
            .child(
                TextView::new(&self.text)
                    .style(
                        TextViewStyle::default()
                            .with_dark(true)
                            .with_foreground(rgb(0xd9e2ef).into())
                            .with_muted_foreground(rgb(0xa9b9cb).into())
                            .with_link(rgb(0x82c7ff).into())
                            .with_selection(rgba(0x4278aa88).into())
                            .with_code_background(rgb(0x263547).into())
                            .with_border(rgb(0x405165).into())
                            .with_inline_code(HighlightStyle {
                                background_color: Some(rgb(0x263547).into()),
                                ..Default::default()
                            }),
                    )
                    .selectable(true)
                    .scrollable(true)
                    .on_link_click(|_, _, _, _| {})
                    // This resolver is authoritative, including data URLs; it never delegates to a URL loader.
                    .image_source(|_| ImageSource::Custom(Arc::new(|_, _| None)))
                    .size_full(),
            )
            .child(
                canvas(|_, _, _| (), move |_, (), _, _| painted.set(revision))
                    .absolute()
                    .size_full(),
            )
    }
}

pub fn draft(source: &str, range: Range<usize>) -> Result<AnnotationDraft, String> {
    let quote = source
        .get(range.clone())
        .filter(|text| !text.is_empty())
        .ok_or("Selection is not a nonempty UTF-8 range")?;
    if quote.len() > 16_384 {
        return Err("Selection exceeds the annotation limit".into());
    }
    let prefix: String = source[..range.start]
        .chars()
        .rev()
        .take(32)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let suffix: String = source[range.end..].chars().take(32).collect();
    let start_line = source[..range.start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let end_line = start_line + quote.bytes().filter(|byte| *byte == b'\n').count();
    Ok(AnnotationDraft {
        quote: quote.into(),
        prefix,
        suffix,
        start_line: u32::try_from(start_line).map_err(|_| "Too many lines")?,
        end_line: u32::try_from(end_line).map_err(|_| "Too many lines")?,
        body: "Native selection annotation".into(),
    })
}
fn anchor_exact(source: &str, quote: &str, prefix: &str, suffix: &str) -> Option<Range<usize>> {
    if quote.is_empty() {
        return None;
    }
    let mut candidates = source.match_indices(quote).filter(|(start, _)| {
        source[..*start].ends_with(prefix) && source[*start + quote.len()..].starts_with(suffix)
    });
    let (start, _) = candidates.next()?;
    candidates
        .next()
        .is_none()
        .then_some(start..start + quote.len())
}

#[cfg(test)]
mod tests {
    use super::{anchor_exact, draft};
    #[test]
    fn unicode_target_preserves_quote_context_and_lines() {
        let source = "前置\nCafé 👩🏽‍💻 日本語\nfin";
        let start = source.find("Café").expect("fixture");
        let end = source.find("\nfin").expect("fixture");
        let target = draft(source, start..end).expect("valid target");
        assert_eq!(target.quote, "Café 👩🏽‍💻 日本語");
        assert_eq!((target.start_line, target.end_line), (2, 2));
        assert_eq!(
            anchor_exact(source, &target.quote, &target.prefix, &target.suffix),
            Some(start..end)
        );
        assert!(draft(source, 1..2).is_err());
    }
    #[test]
    fn ambiguous_or_changed_quote_does_not_invent_a_mark() {
        assert_eq!(anchor_exact("same same", "same", "", ""), None);
        assert_eq!(anchor_exact("new text", "old text", "", ""), None);
    }
}
