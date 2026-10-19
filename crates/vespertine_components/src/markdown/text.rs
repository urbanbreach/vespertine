//! Byte-level line helpers shared by the Markdown layer.

use std::ops::Range;

/// Space or tab: the only whitespace Markdown markers and indentation use.
pub(crate) fn is_ws(byte: u8) -> bool {
    byte == b' ' || byte == b'\t'
}

/// Whitespace of a blank line: spaces, tabs, carriage returns, vertical tabs and form feeds.
/// A non-breaking space is not whitespace here.
pub(crate) fn is_blank(text: &str) -> bool {
    text.bytes()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | 0x0b | 0x0c))
}

/// Start of the logical line containing `pos` (`pos <= source.len()`).
pub(crate) fn line_start(source: &str, pos: usize) -> usize {
    source.as_bytes()[..pos]
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |index| index + 1)
}

/// Offset of the line feed ending the line containing `pos`, or the source length.
pub(crate) fn line_end(source: &str, pos: usize) -> usize {
    source.as_bytes()[pos..]
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or(source.len(), |index| pos + index)
}

/// End of the content of the line `start..end` without a trailing carriage return.
pub(crate) fn content_end(source: &str, start: usize, end: usize) -> usize {
    if end > start && source.as_bytes()[end - 1] == b'\r' {
        end - 1
    } else {
        end
    }
}

/// `end` without one trailing `\n` or `\r\n`.
pub(crate) fn trim_terminator(source: &str, end: usize) -> usize {
    let bytes = source.as_bytes();
    if end > 0 && bytes[end - 1] == b'\n' {
        if end > 1 && bytes[end - 2] == b'\r' {
            end - 2
        } else {
            end - 1
        }
    } else {
        end
    }
}

/// The logical lines covering `range`, as `start of first line..content end of last line`.
///
/// A non-empty range ending right after a line feed does not include the line that follows.
pub(crate) fn lines_covering(source: &str, range: Range<usize>) -> Range<usize> {
    let len = source.len();
    let (low, high) = if range.start <= range.end {
        (range.start, range.end)
    } else {
        (range.end, range.start)
    };
    let start = low.min(len);
    let end = high.clamp(start, len);
    let first = line_start(source, start);
    let probe = if end > start && source.as_bytes()[end - 1] == b'\n' {
        end - 1
    } else {
        end
    };
    let last_start = line_start(source, probe);
    let last_end = line_end(source, probe);
    first..content_end(source, last_start, last_end).max(first)
}

/// Whether two half-open ranges intersect; an empty `b` intersects `a` only strictly inside it.
pub(crate) fn intersects(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// Leading quote prefix of `line`: up to three spaces, `>`, one optional space or tab, repeated.
/// Returns its byte length and the quote depth.
pub(crate) fn quote_prefix(line: &str) -> (usize, usize) {
    let bytes = line.as_bytes();
    let mut pos = 0;
    let mut depth = 0;
    loop {
        let mut index = pos;
        while index < bytes.len() && bytes[index] == b' ' && index - pos < 3 {
            index += 1;
        }
        if index < bytes.len() && bytes[index] == b'>' {
            index += 1;
            if index < bytes.len() && is_ws(bytes[index]) {
                index += 1;
            }
            pos = index;
            depth += 1;
        } else {
            return (pos, depth);
        }
    }
}

/// Whether `text` is a thematic break: at least three identical `-`, `*` or `_` marks with
/// optional spaces or tabs between and around them, and nothing else.
pub(crate) fn is_thematic_break(text: &str) -> bool {
    let mut mark = None;
    let mut count = 0;
    for byte in text.bytes() {
        match byte {
            b' ' | b'\t' | b'\r' => {}
            b'-' | b'*' | b'_' => {
                if *mark.get_or_insert(byte) != byte {
                    return false;
                }
                count += 1;
            }
            _ => return false,
        }
    }
    count >= 3
}

/// Whether `text` (after optional indentation) opens an ATX heading: one to six `#` followed by
/// a space, a tab or the end of the text.
pub(crate) fn is_atx_heading(text: &str) -> bool {
    let bytes = text.trim_start_matches([' ', '\t']).as_bytes();
    let hashes = bytes.iter().take_while(|&&byte| byte == b'#').count();
    (1..=6).contains(&hashes)
        && bytes
            .get(hashes)
            .is_none_or(|&byte| is_ws(byte) || byte == b'\r')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thematic_breaks() {
        for text in ["---", "***", "___", "- - -", " * * * ", "-----", "_ _ _ _"] {
            assert!(is_thematic_break(text), "{text:?}");
        }
        for text in ["--", "-*-", "---a", "- - x", "\u{a0}---", "-\u{a0}-\u{a0}-"] {
            assert!(!is_thematic_break(text), "{text:?}");
        }
    }

    #[test]
    fn quote_prefixes() {
        assert_eq!(quote_prefix("> - a"), (2, 1));
        assert_eq!(quote_prefix(">> 9) a"), (3, 2));
        assert_eq!(quote_prefix("> > - a"), (4, 2));
        assert_eq!(quote_prefix(">   - a"), (2, 1));
        assert_eq!(quote_prefix("   > a"), (5, 1));
        assert_eq!(quote_prefix("    > a"), (0, 0));
        assert_eq!(quote_prefix("- a"), (0, 0));
    }

    #[test]
    fn covering_lines_exclude_the_line_after_a_trailing_line_feed() {
        let source = "ab\ncd\nef";
        assert_eq!(lines_covering(source, 1..1), 0..2);
        assert_eq!(lines_covering(source, 1..4), 0..5);
        assert_eq!(lines_covering(source, 0..3), 0..2);
        assert_eq!(lines_covering(source, 3..3), 3..5);
        assert_eq!(lines_covering(source, 8..8), 6..8);
        assert_eq!(lines_covering("a\r\nb", 0..0), 0..1);
    }

    #[test]
    fn atx_heading_openers() {
        assert!(is_atx_heading("# a"));
        assert!(is_atx_heading("  ###### a"));
        assert!(is_atx_heading("##"));
        assert!(!is_atx_heading("####### a"));
        assert!(!is_atx_heading("#a"));
        assert!(!is_atx_heading("a"));
    }
}
