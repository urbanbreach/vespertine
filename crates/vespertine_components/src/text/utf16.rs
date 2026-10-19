//! UTF-8 <-> UTF-16 offset conversion for the native text-input surface (CMP-007 R17).
//!
//! Internally every offset is a UTF-8 byte offset; ranges exchanged with the platform are in
//! UTF-16 code units. Conversion walks scalar values: the UTF-8 offset of a UTF-16 offset is the
//! byte length of the longest scalar prefix whose UTF-16 length does not exceed the offset (an
//! offset inside a surrogate pair rounds to the start of the pair); the reverse conversion is
//! the sum of the UTF-16 lengths of the scalars before the byte offset.

use std::ops::Range;

/// Length of `text` in UTF-16 code units.
pub fn len_utf16(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The UTF-16 offset of a UTF-8 byte offset (clamped to the text; an offset inside a scalar
/// counts the whole scalar before it only when it lies past the scalar start).
pub fn to_utf16(text: &str, offset: usize) -> usize {
    let mut utf16 = 0;
    let mut utf8 = 0;
    for ch in text.chars() {
        if utf8 >= offset {
            break;
        }
        utf8 += ch.len_utf8();
        utf16 += ch.len_utf16();
    }
    utf16
}

/// The UTF-8 byte offset of a UTF-16 offset: the longest scalar prefix whose UTF-16 length does
/// not exceed `offset16`.
pub fn from_utf16(text: &str, offset16: usize) -> usize {
    let mut utf16 = 0;
    let mut utf8 = 0;
    for ch in text.chars() {
        if utf16 + ch.len_utf16() > offset16 {
            break;
        }
        utf16 += ch.len_utf16();
        utf8 += ch.len_utf8();
    }
    utf8
}

/// Converts a UTF-8 range to UTF-16.
pub fn range_to_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    to_utf16(text, range.start)..to_utf16(text, range.end)
}

/// Converts a UTF-16 range to UTF-8. A reversed range is normalized (start <= end).
pub fn range_from_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    let a = from_utf16(text, range.start);
    let b = from_utf16(text, range.end);
    a.min(b)..a.max(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn astral_characters_count_two_units() {
        let text = "a🌍b";
        assert_eq!(len_utf16(text), 4);
        assert_eq!(to_utf16(text, 1), 1);
        assert_eq!(to_utf16(text, 5), 3);
        assert_eq!(to_utf16(text, 6), 4);
        assert_eq!(from_utf16(text, 1), 1);
        assert_eq!(from_utf16(text, 3), 5);
        assert_eq!(from_utf16(text, 4), 6);
    }

    #[test]
    fn offsets_inside_a_surrogate_pair_round_to_the_pair_start() {
        let text = "a🌍b";
        assert_eq!(from_utf16(text, 2), 1);
        assert_eq!(range_from_utf16(text, &(2..3)), 1..5);
        assert_eq!(from_utf16(text, 99), text.len());
    }

    #[test]
    fn bmp_multibyte_text_converts_by_scalar() {
        let text = "café 日本語";
        assert_eq!(to_utf16(text, 5), 4);
        assert_eq!(from_utf16(text, 4), 5);
        assert_eq!(range_to_utf16(text, &(0..text.len())), 0..8);
    }
}
