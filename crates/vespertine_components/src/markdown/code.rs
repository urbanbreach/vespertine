//! Code-context queries (R9, R10): is an editing action at an offset inside code?

use super::list::scan_item;
use super::parse::Parsed;
use super::text::{
    content_end, is_atx_heading, is_blank, is_thematic_break, line_end, line_start, quote_prefix,
};

/// Whether an editing action at byte `offset` of `source` is inside code, so completion triggers
/// and list continuation can be suppressed there (R9).
///
/// It is `false` for an offset beyond the source or not on a char boundary. It is `true` strictly
/// inside a parsed code block or inline code span, at the end of an indented code block, at the
/// end of a fenced block that is not closed, and inside an unfinished inline code run: an
/// unmatched backtick run open before the offset within the same paragraph, heading or list item.
/// This query is not subject to the processing limit.
pub fn in_code(source: &str, offset: usize) -> bool {
    if offset > source.len() || !source.is_char_boundary(offset) {
        return false;
    }
    in_code_parsed(&Parsed::parse(source), source, offset)
}

/// [`in_code`] over an existing parse; `offset` must be a valid char boundary.
pub(crate) fn in_code_parsed(parsed: &Parsed, source: &str, offset: usize) -> bool {
    parsed.block_containing(offset).is_some()
        || parsed.inside_inline_code(offset)
        || unfinished_inline_code(parsed, source, offset)
}

/// Whether an unmatched backtick run is open before `offset`.
fn unfinished_inline_code(parsed: &Parsed, source: &str, offset: usize) -> bool {
    let bytes = source.as_bytes();
    let mut open: Option<usize> = None;
    let mut index = block_start(parsed, source, offset);
    while index < offset {
        if let Some(span) = parsed.code_span_ending_after(index)
            && span.start <= index
        {
            index = span.end;
            continue;
        }
        if let Some(block) = parsed.block_covering(index) {
            index = block.range.end;
            continue;
        }
        match bytes[index] {
            b'\\' => index += 2,
            b'`' => {
                let run = bytes[index..]
                    .iter()
                    .take_while(|&&byte| byte == b'`')
                    .count();
                open = match open {
                    None => Some(run),
                    Some(length) if length == run => None,
                    keep => keep,
                };
                index += run;
            }
            _ => index += 1,
        }
    }
    open.is_some()
}

/// Start of the paragraph, heading or list item text that contains `offset`: the scan walks back
/// over lines until a blank line, a heading, a code block, or a line that begins a new block.
fn block_start(parsed: &Parsed, source: &str, offset: usize) -> usize {
    let mut start = line_start(source, offset);
    while start > 0 {
        let end = content_end(source, start, line_end(source, start));
        let previous_start = line_start(source, start - 1);
        let previous = &source[previous_start..content_end(source, previous_start, start - 1)];
        if begins_block(&source[start..end], previous)
            || ends_block(parsed, previous, previous_start..start)
        {
            break;
        }
        start = previous_start;
    }
    start
}

/// Whether `line` starts a new block when it follows `previous` (so it cannot continue the
/// paragraph of `previous`).
fn begins_block(line: &str, previous: &str) -> bool {
    let (quote_len, depth) = quote_prefix(line);
    if depth > quote_prefix(previous).1 {
        return true;
    }
    let rest = &line[quote_len..];
    let trimmed = rest.trim_start_matches([' ', '\t']);
    if is_atx_heading(rest)
        || is_thematic_break(trimmed)
        || trimmed.starts_with("```")
        || trimmed.starts_with("~~~")
    {
        return true;
    }
    let indent = rest.len() - trimmed.len();
    scan_item(rest.as_bytes(), indent, rest.len()).is_some()
}

/// Whether the line `previous` (occupying `span`, line feed included) ends its block: it is blank
/// (quote markers aside), a heading, a thematic break, or part of a code block or heading.
fn ends_block(parsed: &Parsed, previous: &str, span: std::ops::Range<usize>) -> bool {
    let rest = &previous[quote_prefix(previous).0..];
    if is_blank(rest) || is_atx_heading(rest) || is_thematic_break(rest) {
        return true;
    }
    let overlaps =
        |range: &std::ops::Range<usize>| range.start < span.end && range.end > span.start;
    let blocks = parsed
        .blocks
        .partition_point(|block| block.range.start < span.end);
    if blocks
        .checked_sub(1)
        .is_some_and(|index| overlaps(&parsed.blocks[index].range))
    {
        return true;
    }
    let headings = parsed
        .headings
        .partition_point(|heading| heading.range.start < span.end);
    headings
        .checked_sub(1)
        .is_some_and(|index| overlaps(&parsed.headings[index].range))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `in_code` at the end of `source`.
    fn at_end(source: &str) -> bool {
        in_code(source, source.len())
    }

    #[test]
    fn r10_required_cases() {
        assert!(at_end("``code "));
        assert!(!at_end("``literal ` backtick`` $skill"));
        assert!(!at_end("`code` $skill"));
        assert!(!at_end("```\ncode\n```"));
        assert!(at_end("````\ncode\n```"));
        assert!(at_end("`unfinished\nsoft line $skill"));
        assert!(at_end("# heading `unfinished $skill"));
        assert!(!at_end("say \\` $"));
    }

    #[test]
    fn unfinished_code_ends_at_block_boundaries() {
        assert!(!at_end("`unfinished\n\n"));
        assert!(!at_end("`unfinished\n   \n"));
        assert!(!at_end("`unfinished\r\n\r\n"));
        assert!(!at_end("`unfinished\r\n \t\r\n$skill"));
        assert!(!at_end("`unfinished\n# heading"));
        assert!(!at_end("`unfinished\n- item"));
        assert!(!at_end("`unfinished\n1. item"));
        assert!(!at_end("`unfinished\n> quote"));
        assert!(!at_end("`unfinished\n---\ntext"));
        assert!(!at_end("`unfinished\n```\ncode\n```\ntext"));
        // A fresh line right after a soft break continues the paragraph.
        assert!(at_end("`unfinished\n"));
        assert!(at_end("`unfinished\r\nsoft"));
        assert!(at_end("`unfinished\n    indented soft"));
    }

    #[test]
    fn quoted_and_listed_fences_follow_the_same_closing_rule() {
        assert!(!at_end("> ```rust\n> fn x() {}\n> ```"));
        assert!(at_end("> ```rust\n> fn x() {}"));
        assert!(at_end("> ````\n> x\n> ```"));
        assert!(!at_end("- ```\n  x\n  ```"));
        assert!(at_end("- ```\n  x"));
        assert!(at_end("```\n- item"));
        assert!(!at_end("~~~\nx\n~~~"));
        assert!(at_end("~~~\nx\n```"));
        // A quote ending after an unclosed fence is outside the fence on the next block.
        assert!(!in_code("> ```\n> x\n\nafter", "> ```\n> x\n\n".len()));
    }

    #[test]
    fn tight_lists() {
        assert!(!at_end("- `unfinished\n- $skill"));
        assert!(at_end("- `unfinished\n  soft $skill"));
        assert!(!at_end("1. `unfinished\n2. $skill"));
        assert!(!at_end("- a\n\n  `unfinished\n\n  $skill"));
    }

    #[test]
    fn strictly_inside_parsed_code() {
        let source = "a `code` b";
        assert!(!in_code(source, 2));
        assert!(in_code(source, 3));
        assert!(in_code(source, 7));
        assert!(!in_code(source, 8));
        let fenced = "x\n```\ncode\n```\ny";
        assert!(!in_code(fenced, 2));
        assert!(in_code(fenced, 6));
        assert!(in_code(fenced, 9));
        assert!(!in_code(fenced, fenced.len()));
    }

    #[test]
    fn indented_code_includes_its_end() {
        assert!(at_end("    - shell-command"));
        assert!(!at_end("    code\nplain"));
        assert!(in_code("    code\nplain", 8));
        assert!(!in_code("    code\nplain", 9));
        assert!(in_code("text\n\n    code\n", 14));
        assert!(!at_end("text\n\n    code\n"));
    }

    #[test]
    fn invalid_offsets_are_not_code() {
        let source = "`caf\u{e9}`";
        assert!(!in_code(source, source.len() + 1));
        assert!(!in_code(source, 5));
        assert!(in_code(source, 4));
        assert!(!in_code("", 0));
    }

    #[test]
    fn code_spans_skip_other_length_runs_and_escapes() {
        assert!(!at_end("`a``b` and `` c ` d `` end"));
        assert!(at_end("`a``b` and `c"));
        assert!(!at_end("\\`not code"));
        assert!(at_end("`a ``b"));
        assert!(!at_end("`a ``b` c"));
    }
}
