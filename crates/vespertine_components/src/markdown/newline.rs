//! Enter list continuation (R11, R12) and the list lines of a document (R7).

use super::code::in_code_parsed;
use super::list::{ListPrefix, list_prefix};
use super::parse::Parsed;
use super::text::{content_end, line_end, line_start};
use std::ops::Range;

/// The list prefix of the line `start..end` (content end, no terminator) when it is a list line
/// outside code and outside a heading.
///
/// A heading that begins before the list marker (a setext underline or text line) always
/// disqualifies the line; an ATX heading after the marker (`- ## title`) disqualifies it only
/// when `allow_heading_after_marker` is false.
pub(crate) fn list_line(
    parsed: &Parsed,
    source: &str,
    start: usize,
    end: usize,
    allow_heading_after_marker: bool,
) -> Option<ListPrefix> {
    let prefix = list_prefix(&source[start..end])?;
    if in_code_parsed(parsed, source, start + prefix.marker_end) {
        return None;
    }
    if let Some(heading) = parsed.heading_overlapping(&(start..end)) {
        if !allow_heading_after_marker || heading.range.start < start + prefix.marker_offset {
            return None;
        }
    }
    Some(prefix)
}

/// The Markdown newline hook (R11): the edit Enter performs for a collapsed selection at `caret`
/// with no marked range, or `None` for the default newline (R12).
///
/// An edit is returned only when `caret` is a char boundary outside code (see [`in_code`]), the
/// current line (without a trailing carriage return) has a [`list_prefix`], the line is not
/// inside a heading, and `caret` is at or after the end of the prefix.
///
/// * Non-empty item: insert the line terminator the line already uses (`\r\n` when the line
///   ends in a carriage return, else `\n`) followed by [`ListPrefix::next_prefix`]; the range is
///   empty at `caret`.
/// * Empty item (only whitespace and maybe a task box after the marker): replace the line's text
///   (line start to line end without the carriage return) with
///   [`ListPrefix::outdented_prefix`].
///
/// Thematic breaks, indented-code markers, markers inside fences, setext underlines and code
/// lines inside quotes never continue. Not subject to the processing limit.
///
/// [`in_code`]: super::in_code
pub fn newline_edit(source: &str, caret: usize) -> Option<(Range<usize>, String)> {
    if caret > source.len() || !source.is_char_boundary(caret) {
        return None;
    }
    let start = line_start(source, caret);
    let terminator = line_end(source, caret);
    let end = content_end(source, start, terminator);
    if caret > end {
        return None;
    }
    let prefix = list_prefix(&source[start..end])?;
    if caret < start + prefix.prefix_end {
        return None;
    }
    let parsed = Parsed::parse(source);
    let prefix = list_line(&parsed, source, start, end, false)?;
    if in_code_parsed(&parsed, source, caret) {
        return None;
    }
    if prefix.is_empty {
        return Some((start..end, prefix.outdented_prefix));
    }
    let mut text = String::from(if end < terminator { "\r\n" } else { "\n" });
    text.push_str(&prefix.next_prefix);
    Some((caret..caret, text))
}

/// A list line of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListLine {
    /// Source offset of the start of the line.
    pub line_start: usize,
    /// The line's list prefix; its offsets are relative to `line_start`.
    pub prefix: ListPrefix,
}

/// Every logical line of `source` that is a list line outside code (R7), in document order. The
/// editor indents wrapped continuation rows of these lines by the shaped display width of the
/// prefix (see [`list_prefix_display_len`](super::list_prefix_display_len)). Not subject to the
/// processing limit.
pub fn list_lines(source: &str) -> Vec<ListLine> {
    let parsed = Parsed::parse(source);
    let mut lines = Vec::new();
    let mut start = 0;
    loop {
        let terminator = line_end(source, start);
        let end = content_end(source, start, terminator);
        if let Some(prefix) = list_line(&parsed, source, start, end, true) {
            lines.push(ListLine {
                line_start: start,
                prefix,
            });
        }
        if terminator >= source.len() {
            return lines;
        }
        start = terminator + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The edit at the end of `source`.
    fn at_end(source: &str) -> Option<(Range<usize>, String)> {
        newline_edit(source, source.len())
    }

    fn insertion(source: &str) -> String {
        let (range, text) = at_end(source).unwrap_or_else(|| panic!("no edit for {source:?}"));
        assert_eq!(range, source.len()..source.len(), "{source:?}");
        text
    }

    fn replacement(source: &str) -> String {
        let (range, text) = at_end(source).unwrap_or_else(|| panic!("no edit for {source:?}"));
        let line = source.rfind('\n').map_or(0, |index| index + 1);
        assert_eq!(range, line..source.len(), "{source:?}");
        text
    }

    #[test]
    fn non_empty_items_continue() {
        assert_eq!(insertion("9. item"), "\n10. ");
        assert_eq!(insertion("  - [x] done"), "\n  - [ ] ");
        assert_eq!(insertion("-\titem"), "\n-\t");
        assert_eq!(insertion("12)\titem"), "\n13)\t");
        assert_eq!(insertion("+   item"), "\n+   ");
        assert_eq!(insertion("> - item"), "\n> - ");
        assert_eq!(insertion(">> 9) item"), "\n>> 10) ");
        assert_eq!(insertion(">   - child"), "\n>   - ");
        assert_eq!(insertion("> > - [x] done"), "\n> > - [ ] ");
        assert_eq!(insertion("- [ ] todo"), "\n- [ ] ");
        assert_eq!(insertion("text\n\n1. first\n2. second"), "\n3. ");
    }

    #[test]
    fn the_new_item_takes_the_text_after_the_caret() {
        let source = "- abcd";
        assert_eq!(newline_edit(source, 4), Some((4..4, "\n- ".to_string())));
        assert_eq!(newline_edit(source, 2), Some((2..2, "\n- ".to_string())));
        assert_eq!(newline_edit(source, 1), None);
        assert_eq!(newline_edit(source, 0), None);
    }

    #[test]
    fn empty_items_outdent() {
        assert_eq!(replacement("- "), "");
        assert_eq!(replacement("- [ ]"), "");
        assert_eq!(replacement("1."), "");
        assert_eq!(replacement("- [X]\t"), "");
        assert_eq!(replacement("-"), "");
        assert_eq!(replacement("> - "), "> ");
        assert_eq!(replacement("- a\n  - b\n  - "), "");
        assert_eq!(replacement("- a\n  - b\n  - [ ]"), "");
        assert_eq!(replacement("- a\n  - b\n    - c\n    - "), "  ");
        assert_eq!(replacement("> - a\n>   - b\n>   - "), "> ");
        assert_eq!(replacement("> - a\n>   - b\n>   - [x]  "), "> ");
    }

    #[test]
    fn crlf_documents() {
        assert_eq!(
            newline_edit("- item\r\nnext", 6),
            Some((6..6, "\r\n- ".to_string()))
        );
        assert_eq!(
            newline_edit("- [ ]\r\nnext", 5),
            Some((0..5, String::new()))
        );
        assert_eq!(
            newline_edit("> - [x] done\r\n> next", 12),
            Some((12..12, "\r\n> - [ ] ".to_string()))
        );
        assert_eq!(newline_edit("- item\r\nnext", 7), None);
        assert_eq!(
            newline_edit("a\r\n- b\r\n- c", 6),
            Some((6..6, "\r\n- ".to_string()))
        );
        assert_eq!(
            newline_edit("a\r\n- b\r\n- c", 11),
            Some((11..11, "\n- ".to_string()))
        );
    }

    #[test]
    fn code_thematic_breaks_headings_and_setext_underlines_never_continue() {
        for source in [
            "```\n- item",
            "```\n1. item\n",
            "    - shell-command",
            ">     - literal",
            "- - -",
            "* * *",
            "___",
            "Title\n-",
            "- # heading",
            "- ## Caf\u{e9} ##",
            "~~~\n- a\n~~~\n- `unfinished",
            "text",
            "-text",
            "",
        ] {
            assert_eq!(at_end(source), None, "{source:?}");
        }
        assert_eq!(newline_edit("```\n- item", 99), None);
        assert_eq!(newline_edit("- caf\u{e9}", 6), None);
    }

    #[test]
    fn after_a_closed_fence_lists_continue() {
        assert_eq!(insertion("```\ncode\n```\n- item"), "\n- ");
    }

    #[test]
    fn the_size_limit_does_not_apply() {
        let mut source = "- item\n".repeat(20_000);
        source.push_str("- last");
        assert!(source.len() > super::super::DEFAULT_PROCESSING_LIMIT);
        assert_eq!(insertion(&source), "\n- ");
    }

    #[test]
    fn list_lines_skip_code_and_breaks() {
        let source = "- a\n```\n- b\n```\n    - c\n- - -\n  * d\n1. e\n- ## f\nTitle\n-\n";
        let starts: Vec<usize> = list_lines(source)
            .iter()
            .map(|line| line.line_start)
            .collect();
        let at = |text: &str| source.find(text).expect(text);
        assert_eq!(starts, [0, at("  * d"), at("1. e"), at("- ## f")]);
    }
}
