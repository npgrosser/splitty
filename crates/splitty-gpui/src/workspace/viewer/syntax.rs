//! Syntax colors for the source view. Syntect names what it matched with
//! TextMate scopes; a few kinds of them are colored, from the terminal's
//! palette, so a document looks like the terminals beside it.
use std::{ops::Range, sync::LazyLock};

use super::SharedString;
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// Longer documents and lines stay plain: a parse has to end.
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Token {
    Comment,
    String,
    Number,
    Constant,
    Keyword,
    Function,
    Type,
    Tag,
    Attribute,
}

impl Token {
    /// The color of the token: an index into the terminal's 16 colors, or
    /// none for the muted text color.
    pub(super) fn ansi(self) -> Option<usize> {
        match self {
            Self::Comment => None,
            Self::String => Some(2),
            Self::Number | Self::Constant | Self::Attribute => Some(3),
            Self::Keyword => Some(5),
            Self::Function => Some(4),
            Self::Type => Some(6),
            Self::Tag => Some(1),
        }
    }

    fn of(stack: &ScopeStack) -> Option<Self> {
        // The innermost scope that is one of the kinds decides.
        stack.as_slice().iter().rev().find_map(|scope| {
            let name = scope.build_string();
            let starts = |prefix: &str| name.starts_with(prefix);
            if starts("comment") {
                Some(Self::Comment)
            } else if starts("string") {
                Some(Self::String)
            } else if starts("constant.numeric") {
                Some(Self::Number)
            } else if starts("constant") {
                Some(Self::Constant)
            } else if starts("entity.name.function")
                || starts("support.function")
                || starts("variable.function")
            {
                Some(Self::Function)
            } else if starts("entity.name.tag") {
                Some(Self::Tag)
            } else if starts("entity.other.attribute-name") {
                Some(Self::Attribute)
            } else if starts("entity.name") || starts("support.type") || starts("support.class") {
                Some(Self::Type)
            } else if starts("keyword") || starts("storage") || starts("markup.heading") {
                Some(Self::Keyword)
            } else {
                None
            }
        })
    }
}

pub(super) type LineSpans = Vec<(Range<usize>, Token)>;

fn syntax_for(name: &str, first_line: &str) -> Option<&'static SyntaxReference> {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let syntaxes = &*SYNTAXES;
    let found = file
        .rsplit_once('.')
        .and_then(|(_, extension)| syntaxes.find_syntax_by_extension(extension))
        .or_else(|| syntaxes.find_syntax_by_extension(file))
        .or_else(|| syntaxes.find_syntax_by_first_line(first_line))?;
    (found.name != "Plain Text").then_some(found)
}

/// The colored spans of every line of a document with this name, as byte
/// ranges of the lines shown; nothing for a language syntect does not know.
pub(super) fn highlight(name: &str, lines: &[SharedString]) -> Option<Vec<LineSpans>> {
    if lines.iter().map(|line| line.len() + 1).sum::<usize>() > MAX_BYTES {
        return None;
    }
    let syntax = syntax_for(name, lines.first().map(|line| line.as_ref()).unwrap_or(""))?;
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out = Vec::with_capacity(lines.len());
    let mut buffer = String::new();
    for line in lines {
        if line.len() > MAX_LINE_BYTES {
            out.push(Vec::new());
            continue;
        }
        buffer.clear();
        buffer.push_str(line);
        buffer.push('\n');
        let Ok(ops) = state.parse_line(&buffer, &SYNTAXES) else {
            return None;
        };
        let mut spans: LineSpans = Vec::new();
        let mut start = 0;
        let push = |range: Range<usize>, stack: &ScopeStack, spans: &mut LineSpans| {
            let end = range.end.min(line.len());
            if range.start >= end {
                return;
            }
            let Some(token) = Token::of(stack) else {
                return;
            };
            match spans.last_mut() {
                Some((last, kind)) if *kind == token && last.end == range.start => last.end = end,
                _ => spans.push((range.start..end, token)),
            }
        };
        for (offset, op) in ops {
            push(start..offset, &stack, &mut spans);
            start = offset;
            if stack.apply(&op).is_err() {
                return None;
            }
        }
        push(start..buffer.len(), &stack, &mut spans);
        out.push(spans);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<SharedString> {
        text.lines()
            .map(|line| SharedString::from(line.to_owned()))
            .collect()
    }

    #[test]
    fn a_known_language_is_colored_and_an_unknown_one_is_not() {
        let source = lines("// note\nfn main() {\n    let n = 42;\n    print(\"hi\");\n}");
        let spans = highlight("src/main.rs", &source).expect("Rust is known");
        assert_eq!(spans.len(), source.len());
        assert_eq!(spans[0], vec![(0..7, Token::Comment)]);
        let text = |line: usize, token: Token| {
            spans[line]
                .iter()
                .filter(|(_, kind)| *kind == token)
                .map(|(range, _)| &source[line][range.clone()])
                .collect::<Vec<_>>()
        };
        assert!(text(1, Token::Keyword).contains(&"fn"), "{:?}", spans[1]);
        assert_eq!(text(2, Token::Number), ["42"]);
        assert!(text(3, Token::String).concat().contains("hi"));
        assert!(highlight("notes.unknown-kind", &source).is_none());
        assert!(highlight("plain.txt", &source).is_none());
    }

    #[test]
    fn a_script_without_an_extension_is_known_by_its_first_line() {
        let source = lines("#!/bin/sh\necho \"hi\" # greet");
        let spans = highlight("run", &source).expect("a shell script");
        assert!(spans[1].iter().any(|(_, kind)| *kind == Token::Comment));
    }
}
