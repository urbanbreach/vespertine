//! Grapheme, word and line boundary rules of the editing core (CMP-007 R6, R7, R9).
//!
//! All functions take UTF-8 byte offsets, treat offsets past the text as the text end, and
//! return offsets that lie on UAX #29 grapheme-cluster boundaries.

use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Clamps `offset` to the text and floors it onto a char boundary.
pub fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Clamps `offset` to the text and raises it onto a char boundary.
pub fn ceil_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

/// The first grapheme boundary at or before `offset`.
pub fn floor_grapheme_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    if offset == 0 || offset == text.len() {
        return offset;
    }
    // Only the cluster around the offset matters; start from a nearby line start so long
    // texts stay cheap.
    let scan_start = floor_char_boundary(text, offset.saturating_sub(64));
    let base = text[..scan_start]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0)
        .min(scan_start);
    let mut best = base;
    for (index, cluster) in text[base..].grapheme_indices(true) {
        let start = base + index;
        if start > offset {
            break;
        }
        best = start;
        if start + cluster.len() > offset {
            break;
        }
    }
    best
}

/// The first grapheme boundary at or after `offset`.
pub fn ceil_grapheme_boundary(text: &str, offset: usize) -> usize {
    let offset = ceil_char_boundary(text, offset);
    let floor = floor_grapheme_boundary(text, offset);
    if floor == offset {
        offset
    } else {
        next_grapheme(text, floor)
    }
}

/// The nearest grapheme boundary to `offset`; a tie rounds down.
pub fn snap_grapheme_boundary(text: &str, offset: usize) -> usize {
    let floor = floor_grapheme_boundary(text, offset);
    let ceil = ceil_grapheme_boundary(text, offset);
    if offset.min(text.len()) - floor <= ceil - offset.min(text.len()) {
        floor
    } else {
        ceil
    }
}

/// The grapheme boundary before `offset` (0 at the start).
pub fn prev_grapheme(text: &str, offset: usize) -> usize {
    let offset = floor_grapheme_boundary(text, offset);
    if offset == 0 {
        return 0;
    }
    let scan_start = floor_char_boundary(text, offset.saturating_sub(256));
    let base = text[..scan_start]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0)
        .min(scan_start);
    let mut previous = base;
    for (index, _) in text[base..offset].grapheme_indices(true) {
        previous = base + index;
    }
    previous
}

/// The grapheme boundary after `offset` (the text end at the end).
pub fn next_grapheme(text: &str, offset: usize) -> usize {
    let offset = floor_grapheme_boundary(text, offset);
    if offset >= text.len() {
        return text.len();
    }
    text[offset..]
        .graphemes(true)
        .next()
        .map(|cluster| offset + cluster.len())
        .unwrap_or(text.len())
}

fn is_period(ch: char) -> bool {
    matches!(ch, '.' | '\u{2024}' | '\u{fe52}' | '\u{ff0e}')
}

/// UAX #29 word segments with full stops split out of the segments that join letters across them
/// ("foo.bar" is "foo", ".", "bar"): punctuation counts as words in word movement (R7). A split
/// full stop keeps the combining marks of its grapheme cluster.
fn word_segments(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let starts_with_period = |grapheme: &str| grapheme.chars().next().is_some_and(is_period);
    text.split_word_bound_indices()
        .flat_map(move |(start, segment)| {
            let mut pieces: Vec<(usize, &str)> = Vec::new();
            if segment.chars().count() > 1 && segment.chars().any(is_period) {
                let mut piece_start = 0;
                for (index, grapheme) in segment.grapheme_indices(true) {
                    if starts_with_period(grapheme) {
                        if piece_start < index {
                            pieces.push((start + piece_start, &segment[piece_start..index]));
                        }
                        let end = index + grapheme.len();
                        pieces.push((start + index, &segment[index..end]));
                        piece_start = end;
                    }
                }
                if piece_start < segment.len() {
                    pieces.push((start + piece_start, &segment[piece_start..]));
                }
            } else {
                pieces.push((start, segment));
            }
            pieces
        })
}

fn is_blank(segment: &str) -> bool {
    segment.chars().all(char::is_whitespace)
}

/// Word left (R7): the start of the nearest preceding word segment that is not whitespace-only
/// and starts before `offset`; 0 when there is none.
pub fn word_left(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    let mut result = 0;
    for (start, segment) in word_segments(text) {
        if start >= offset {
            break;
        }
        if !is_blank(segment) {
            result = start;
        }
    }
    result
}

/// Word right (R7): the end of the nearest following word segment that is not whitespace-only
/// and ends after `offset`; the text end when there is none.
pub fn word_right(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    for (start, segment) in word_segments(text) {
        let end = start + segment.len();
        if end > offset && !is_blank(segment) {
            return end;
        }
    }
    text.len()
}

/// The UAX #29 word segment containing `offset` (R9); an empty range at the text end.
pub fn word_at(text: &str, offset: usize) -> Range<usize> {
    let offset = floor_char_boundary(text, offset);
    if offset >= text.len() {
        return text.len()..text.len();
    }
    for (start, segment) in word_segments(text) {
        let end = start + segment.len();
        if start <= offset && offset < end {
            return start..end;
        }
    }
    text.len()..text.len()
}

/// Start of the logical line containing `offset`.
pub fn line_start(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    text[..offset]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0)
}

/// Content end of the logical line containing `offset`: before the line feed and before the
/// carriage return of a trailing CR LF pair (R2, R7).
pub fn line_end(text: &str, offset: usize) -> usize {
    let raw = raw_line_end(text, offset);
    let start = line_start(text, offset);
    if raw > start && text.as_bytes()[raw - 1] == b'\r' && raw < text.len() {
        raw - 1
    } else {
        raw
    }
}

/// End of the logical line's raw range: the position of its line feed or the text end (a
/// trailing CR stays inside the line, R2).
pub fn raw_line_end(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    text[offset..]
        .find('\n')
        .map(|index| offset + index)
        .unwrap_or(text.len())
}

/// The raw logical line containing `offset` including its trailing line feed when it has one
/// (R8 triple click); never extends beyond the text end.
pub fn line_range_with_feed(text: &str, offset: usize) -> Range<usize> {
    let start = line_start(text, offset);
    let end = raw_line_end(text, offset);
    start..if end < text.len() { end + 1 } else { end }
}

/// Whether `range` of `text` is exactly one grapheme cluster.
pub fn is_single_grapheme(text: &str, range: Range<usize>) -> bool {
    range.start < range.end
        && text.get(range).is_some_and(|slice| {
            let mut clusters = slice.graphemes(true);
            clusters.next().is_some() && clusters.next().is_none()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_is_one_cluster() {
        let text = "a\r\nb";
        assert_eq!(next_grapheme(text, 1), 3);
        assert_eq!(prev_grapheme(text, 3), 1);
        assert_eq!(floor_grapheme_boundary(text, 2), 1);
        assert_eq!(ceil_grapheme_boundary(text, 2), 3);
    }

    #[test]
    fn zwj_emoji_and_combining_marks_are_single_clusters() {
        let family = "👩\u{200d}👩\u{200d}👧";
        let text = format!("x{family}e\u{301}y");
        let after_family = 1 + family.len();
        assert_eq!(next_grapheme(&text, 1), after_family);
        assert_eq!(prev_grapheme(&text, after_family), 1);
        let after_e = after_family + "e\u{301}".len();
        assert_eq!(next_grapheme(&text, after_family), after_e);
        assert_eq!(prev_grapheme(&text, after_e), after_family);
        // Offsets inside a cluster snap to its boundaries.
        assert_eq!(floor_grapheme_boundary(&text, 5), 1);
        assert_eq!(ceil_grapheme_boundary(&text, 5), after_family);
        assert_eq!(
            floor_grapheme_boundary(&text, after_family + 1),
            after_family
        );
    }

    #[test]
    fn word_moves_follow_uax29_segments() {
        let text = "foo.bar baz";
        assert_eq!(word_right(text, 0), 3);
        assert_eq!(word_right(text, 3), 4);
        assert_eq!(word_right(text, 4), 7);
        assert_eq!(word_right(text, 7), 11);
        assert_eq!(word_right(text, 11), 11);
        assert_eq!(word_left(text, 11), 8);
        assert_eq!(word_left(text, 8), 4);
        assert_eq!(word_left(text, 4), 3);
        assert_eq!(word_left(text, 3), 0);
        assert_eq!(word_left(text, 0), 0);
        assert_eq!(word_left("   ", 3), 0);
        assert_eq!(word_right("   ", 0), 3);
    }

    #[test]
    fn word_moves_keep_a_full_stop_with_its_combining_mark() {
        let text = ".\u{301}foo";
        assert_eq!(word_right(text, 0), 3);
        assert_eq!(word_left(text, 3), 0);
        assert_eq!(word_at(text, 0), 0..3);
        let text = "foo.\u{301}bar";
        assert_eq!(word_right(text, 3), 6);
        assert_eq!(word_left(text, 6), 3);
    }

    #[test]
    fn word_at_selects_the_containing_segment() {
        let text = "foo bar";
        assert_eq!(word_at(text, 1), 0..3);
        assert_eq!(word_at(text, 3), 3..4);
        assert_eq!(word_at(text, 5), 4..7);
        assert_eq!(word_at(text, 7), 7..7);
        assert_eq!(word_at("", 0), 0..0);
    }

    #[test]
    fn line_boundaries_and_crlf() {
        let text = "ab\r\ncd\nef";
        assert_eq!(line_start(text, 5), 4);
        assert_eq!(line_end(text, 0), 2);
        assert_eq!(raw_line_end(text, 0), 3);
        assert_eq!(line_end(text, 5), 6);
        assert_eq!(line_end(text, 8), 9);
        assert_eq!(line_range_with_feed(text, 0), 0..4);
        assert_eq!(line_range_with_feed(text, 8), 7..9);
    }

    #[test]
    fn single_grapheme_detection() {
        assert!(is_single_grapheme("e\u{301}", 0..3));
        assert!(!is_single_grapheme("ab", 0..2));
        assert!(!is_single_grapheme("ab", 1..1));
    }
}
