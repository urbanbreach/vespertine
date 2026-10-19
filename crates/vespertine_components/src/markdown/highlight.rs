//! Syntax coloring of fenced code bodies through the theme-independent TXT-007 highlighter (R8).

use super::parse::{CodeBlock, Parsed};
use super::{HighlightSpan, MAX_SYNTAX_SPANS};
use std::ops::Range;
use vespertine_richtext::syntax::{HighlightCache, Limits, resolve_alias};

/// Spans of every fenced code body of `source` with a known language, as exact source ranges.
pub(crate) fn compute(source: &str) -> Vec<HighlightSpan> {
    let parsed = Parsed::parse(source);
    let cache = HighlightCache::global();
    let mut spans = Vec::new();
    for block in parsed.blocks.iter().filter(|block| block.fenced) {
        if spans.len() >= MAX_SYNTAX_SPANS {
            break;
        }
        let Some(language) = resolve_alias(&block.info) else {
            continue;
        };
        let body = Body::assemble(source, block);
        if body.text.is_empty() {
            continue;
        }
        let Some(document) = cache.highlight_as(language, &body.text, Limits::default()) else {
            continue;
        };
        let mut line_start = 0;
        for (line, line_spans) in body.text.split_inclusive('\n').zip(document.lines().iter()) {
            for span in line_spans {
                body.map(
                    line_start + span.range.start..line_start + span.range.end,
                    |range| {
                        if spans.len() < MAX_SYNTAX_SPANS {
                            spans.push(HighlightSpan {
                                range,
                                kind: span.kind,
                            });
                        }
                    },
                );
            }
            line_start += line.len();
        }
    }
    spans
}

/// The body of a fence with container prefixes removed, and where each piece came from.
struct Body {
    text: String,
    /// `(start in text, source range)` of every segment, ascending.
    segments: Vec<(usize, Range<usize>)>,
}

impl Body {
    fn assemble(source: &str, block: &CodeBlock) -> Body {
        let mut text = String::new();
        let mut segments = Vec::with_capacity(block.segments.len());
        for segment in &block.segments {
            segments.push((text.len(), segment.clone()));
            text.push_str(&source[segment.clone()]);
        }
        Body { text, segments }
    }

    /// Maps a range of the assembled text back to source ranges, one per segment it touches.
    fn map(&self, range: Range<usize>, mut emit: impl FnMut(Range<usize>)) {
        let first = self
            .segments
            .partition_point(|(start, source)| start + source.len() <= range.start);
        for (start, source) in &self.segments[first..] {
            if *start >= range.end {
                break;
            }
            let from = range.start.max(*start) - start;
            let to = range.end.min(start + source.len()) - start;
            if from < to {
                emit(source.start + from..source.start + to);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine_richtext::syntax::TokenKind;

    fn spans(source: &str) -> Vec<(String, TokenKind)> {
        compute(source)
            .into_iter()
            .map(|span| (source[span.range].to_string(), span.kind))
            .collect()
    }

    fn contains(spans: &[(String, TokenKind)], text: &str, kind: TokenKind) -> bool {
        spans.iter().any(|(t, k)| t == text && *k == kind)
    }

    #[test]
    fn keywords_land_on_exact_offsets_after_multibyte_characters() {
        let source =
            "```rust\nfn main() { let caf\u{e9} = 42; }\n```\n```python\ndef hello(): pass\n```";
        let found = compute(source);
        let fn_at = source.find("fn main").expect("fn");
        let def_at = source.find("def hello").expect("def");
        assert!(
            found
                .iter()
                .any(|span| span.range == (fn_at..fn_at + 2) && span.kind == TokenKind::Keyword)
        );
        assert!(
            found
                .iter()
                .any(|span| span.range == (def_at..def_at + 3) && span.kind == TokenKind::Keyword)
        );
        let let_at = source.find("let").expect("let");
        assert!(
            found
                .iter()
                .any(|span| span.range == (let_at..let_at + 3) && span.kind == TokenKind::Keyword)
        );
        let number_at = source.find("42").expect("number");
        assert!(
            found
                .iter()
                .any(|span| span.range == (number_at..number_at + 2)
                    && span.kind == TokenKind::Number)
        );
        for span in &found {
            assert!(source.is_char_boundary(span.range.start));
            assert!(source.is_char_boundary(span.range.end));
            assert!(!source[span.range.clone()].contains('`'));
        }
    }

    #[test]
    fn a_string_literal_is_colored_alone() {
        let source = "```rust\nlet name = \"literal\";\n```";
        let found = spans(source);
        let strings: Vec<_> = found
            .iter()
            .filter(|(_, kind)| *kind == TokenKind::String)
            .collect();
        assert_eq!(strings.len(), 1, "{found:?}");
        assert_eq!(strings[0].0, "\"literal\"");
    }

    #[test]
    fn quoted_fences_keep_multi_line_state_and_exclude_prefixes() {
        let source = "> ```rust\n> /* one\n> two */ let a = 1;\n> ```";
        let found = compute(source);
        assert!(!found.is_empty());
        let comment: Vec<_> = found
            .iter()
            .filter(|span| span.kind == TokenKind::Comment)
            .map(|span| &source[span.range.clone()])
            .collect();
        assert_eq!(comment, ["/* one", "two */"]);
        for span in &found {
            let text = &source[span.range.clone()];
            assert!(!text.contains('>') && !text.contains("```"), "{text:?}");
        }
    }

    #[test]
    fn listed_fences_keep_multi_line_state_and_exclude_prefixes() {
        let source = "- item\n\n  ```rust\n  /* one\n  two */ let a = 1;\n  ```\n";
        let found = compute(source);
        let comment: Vec<_> = found
            .iter()
            .filter(|span| span.kind == TokenKind::Comment)
            .map(|span| &source[span.range.clone()])
            .collect();
        assert_eq!(comment, ["/* one", "two */"]);
        let found = spans("> ```rust\n> let s = \"a\n> b\";\n> ```");
        assert!(contains(&found, "\"a", TokenKind::String));
        assert!(contains(&found, "b\"", TokenKind::String));
    }

    #[test]
    fn crlf_fences_exclude_the_carriage_return() {
        let source = "```rust\r\nfn main() {}\r\n```\r\n";
        let found = compute(source);
        assert!(!found.is_empty());
        assert!(
            found
                .iter()
                .all(|span| !source[span.range.clone()].contains('\r'))
        );
    }

    #[test]
    fn prose_inline_code_and_unknown_languages_are_neutral() {
        assert!(compute("plain `fn main() {}` text\n\n# fn heading\n").is_empty());
        assert!(compute("```\nfn main() {}\n```").is_empty());
        assert!(compute("```nonsense\nfn main() {}\n```").is_empty());
        assert!(compute("```rust,ignore\nfn main() {}\n```").is_empty());
        assert!(compute("    fn main() {}").is_empty());
    }

    #[test]
    fn the_language_is_the_first_word_of_the_info_string() {
        assert!(!compute("```RS title=x\nfn main() {}\n```").is_empty());
        assert!(!compute("``` python\ndef f(): pass\n```").is_empty());
    }

    #[test]
    fn an_unclosed_fence_is_highlighted_to_the_end() {
        assert!(!compute("```rust\nfn main() {}").is_empty());
    }

    #[test]
    fn spans_are_capped() {
        let mut source = String::from("```json\n");
        for _ in 0..9_000 {
            source.push_str("[1, 2, 3]\n");
        }
        source.push_str("```\n```json\n[1, 2]\n```\n");
        let found = compute(&source);
        assert_eq!(found.len(), MAX_SYNTAX_SPANS);
        assert!(
            found
                .windows(2)
                .all(|pair| pair[0].range.end <= pair[1].range.start)
        );
    }
}
