//! Literal matching (TXT-009 R4, R5, R10): Unicode default lowercasing with an offset map back to
//! the original text, case-sensitive matching, whole-word boundaries, and a per-group cache entry.

use std::cell::{Cell, OnceCell};
use std::ops::Range;

thread_local! {
    static SCANS: Cell<u64> = const { Cell::new(0) };
}

/// Number of group scans performed on this thread so far. Every [`GroupMatcher::find`] that
/// actually searches text increments it, so a test can prove that a change of colors,
/// `activeIndex`, `matchIndexOffset` or radius never rescans (R10).
pub fn scan_count() -> u64 {
    SCANS.with(Cell::get)
}

fn record_scan() {
    SCANS.with(|scans| scans.set(scans.get() + 1));
}

/// A literal query prepared once per specification (lowercased copy included).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Query {
    text: String,
    lowered: String,
    case_sensitive: bool,
    whole_word: bool,
}

impl Query {
    pub fn new(text: &str, case_sensitive: bool, whole_word: bool) -> Self {
        Self {
            text: text.to_owned(),
            lowered: if case_sensitive {
                String::new()
            } else {
                lowercase(text)
            },
            case_sensitive,
            whole_word,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Unicode default lowercasing, character by character (`char::to_lowercase`): no case folding
/// and no context-sensitive rules such as the final sigma.
fn lowercase(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// A word character for whole-word matching (R5).
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The case-lowered copy of a group's text, or a marker that lowering changed nothing.
enum Lowered {
    /// Lowering every character yields the character itself: search the original text, offsets
    /// map one to one.
    Same,
    Changed {
        text: String,
        /// For every byte of `text`, the byte offset of the original character it came from.
        origin: Vec<usize>,
    },
}

impl Lowered {
    fn build(text: &str) -> Self {
        let mut changed = false;
        let mut lowered = String::with_capacity(text.len());
        let mut origin = Vec::with_capacity(text.len());
        for (start, c) in text.char_indices() {
            let before = lowered.len();
            let mut lower = c.to_lowercase();
            // `to_lowercase` of a character is never empty.
            if lower.len() != 1 || lower.clone().next() != Some(c) {
                changed = true;
            }
            lowered.extend(&mut lower);
            origin.resize(origin.len() + (lowered.len() - before), start);
        }
        if changed {
            Self::Changed {
                text: lowered,
                origin,
            }
        } else {
            Self::Same
        }
    }
}

/// Reusable per-group cache entry: the group's text plus its lazily built lowercased copy and
/// offset map. A keystroke reuses it for a group whose text did not change (R10).
pub struct GroupMatcher {
    text: String,
    lowered: OnceCell<Lowered>,
}

impl GroupMatcher {
    pub fn new(text: String) -> Self {
        Self {
            text,
            lowered: OnceCell::new(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// All matches of `query` as byte ranges of the original text: non-overlapping, leftmost
    /// first, never empty. Counts as one scan (see [`scan_count`]).
    pub fn find(&self, query: &Query) -> Vec<Range<usize>> {
        if query.is_empty() || self.text.is_empty() {
            return Vec::new();
        }
        record_scan();
        if query.case_sensitive {
            return scan(
                &self.text,
                &query.text,
                &self.text,
                query.whole_word,
                |s, e| s..e,
            );
        }
        match self.lowered.get_or_init(|| Lowered::build(&self.text)) {
            Lowered::Same => scan(
                &self.text,
                &query.lowered,
                &self.text,
                query.whole_word,
                |s, e| s..e,
            ),
            Lowered::Changed { text, origin } => {
                // Per-byte map: a lowered offset maps to the start of the character it came from;
                // the end of the text maps to the end of the original.
                let original_len = self.text.len();
                scan(
                    text,
                    &query.lowered,
                    &self.text,
                    query.whole_word,
                    |s, e| {
                        let end = if e == text.len() {
                            original_len
                        } else {
                            origin[e]
                        };
                        origin[s]..end
                    },
                )
            }
        }
    }
}

/// Leftmost-first scan of `needle` in `haystack`. `map` converts a haystack range to a range of
/// `original`; candidates whose mapped range is empty, or that fail the whole-word test against
/// the neighbors in `original`, are discarded and scanning resumes one character after the
/// candidate's start so that overlapping alternatives are still found.
fn scan(
    haystack: &str,
    needle: &str,
    original: &str,
    whole_word: bool,
    map: impl Fn(usize, usize) -> Range<usize>,
) -> Vec<Range<usize>> {
    let mut matches = Vec::new();
    let mut from = 0;
    while let Some(found) = haystack[from..].find(needle) {
        let start = from + found;
        let end = start + needle.len();
        let range = map(start, end);
        if range.start < range.end && (!whole_word || at_word_boundaries(original, &range)) {
            matches.push(range);
            from = end;
        } else {
            // `needle` is non-empty so there is a character at `start`.
            from = start + haystack[start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    matches
}

fn at_word_boundaries(text: &str, range: &Range<usize>) -> bool {
    let before = text[..range.start].chars().next_back();
    let after = text[range.end..].chars().next();
    !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
}

/// Offset in UTF-16 code units of every byte offset in `offsets` (sorted ascending, on character
/// boundaries of `text`), in one pass.
pub fn byte_offsets_to_utf16(text: &str, offsets: impl IntoIterator<Item = usize>) -> Vec<usize> {
    let mut result = Vec::new();
    let (mut byte, mut units) = (0, 0);
    let mut chars = text.chars();
    for target in offsets {
        while byte < target {
            let Some(c) = chars.next() else { break };
            byte += c.len_utf8();
            units += c.len_utf16();
        }
        result.push(units);
    }
    result
}

/// Length of `text` in UTF-16 code units.
pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Byte offset of UTF-16 offset `units` in `text`; `None` when it is past the end or falls inside
/// a surrogate pair.
pub fn utf16_to_byte(text: &str, units: usize) -> Option<usize> {
    let mut seen = 0;
    for (byte, c) in text.char_indices() {
        if seen == units {
            return Some(byte);
        }
        if seen > units {
            return None;
        }
        seen += c.len_utf16();
    }
    (seen == units).then_some(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(text: &str, query: &str, case_sensitive: bool, whole_word: bool) -> Vec<Range<usize>> {
        GroupMatcher::new(text.to_owned()).find(&Query::new(query, case_sensitive, whole_word))
    }

    fn texts<'a>(text: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
        ranges.iter().map(|range| &text[range.clone()]).collect()
    }

    #[test]
    fn case_insensitive_by_default_and_case_sensitive_on_request() {
        let text = "Red red RED";
        assert_eq!(
            texts(text, &find(text, "red", false, false)),
            ["Red", "red", "RED"]
        );
        assert_eq!(texts(text, &find(text, "red", true, false)), ["red"]);
        assert_eq!(texts(text, &find(text, "RED", true, false)), ["RED"]);
        assert_eq!(texts(text, &find(text, "RED", false, false)).len(), 3);
    }

    #[test]
    fn matches_are_non_overlapping_and_leftmost_first() {
        assert_eq!(find("aaaa", "aa", false, false), vec![0..2, 2..4]);
        assert_eq!(find("aaa", "aa", true, false), vec![0..2]);
        assert_eq!(find("abab", "bab", false, false), vec![1..4]);
    }

    #[test]
    fn empty_query_and_empty_text_never_match() {
        assert!(find("abc", "", false, false).is_empty());
        assert!(find("", "a", false, false).is_empty());
    }

    #[test]
    fn lowercase_expansion_maps_back_to_original_offsets() {
        // U+0130 lowercases to "i" + U+0307 (3 bytes) from a 2-byte original.
        let text = "a\u{130}b";
        // The query "i" would match the "i" half of the expansion: the mapped range is empty.
        assert!(find(text, "i", false, false).is_empty());
        // "i" + U+0307 matches the whole character.
        assert_eq!(find(text, "i\u{307}", false, false), vec![1..3]);
        // Neighbors of the expansion still match, with original offsets.
        assert_eq!(find(text, "b", false, false), vec![3..4]);
        assert_eq!(find(text, "a", false, false), vec![0..1]);
        // A match covering the expansion and what follows.
        assert_eq!(find(text, "i\u{307}b", false, false), vec![1..4]);
        // Case-sensitive matching is by exact characters.
        assert!(find(text, "i\u{307}", true, false).is_empty());
        assert_eq!(find(text, "\u{130}", true, false), vec![1..3]);
    }

    #[test]
    fn match_ending_inside_an_expansion_keeps_the_original_prefix() {
        // The lowered "xi" ends inside the expansion of U+0130: the end maps to that
        // character's start, so only "x" is covered (non-empty, kept).
        assert_eq!(find("x\u{130}", "xi", false, false), vec![0..1]);
        // A match starting inside the expansion maps to the whole character.
        assert_eq!(find("\u{130}", "\u{307}", false, false), vec![0..2]);
    }

    #[test]
    fn ligature_is_not_case_folded() {
        // U+FB00 lowercases to itself; full case folding would make it "ff".
        assert!(find("\u{FB00}", "ff", false, false).is_empty());
        assert_eq!(find("\u{FB00}", "\u{FB00}", false, false), vec![0..3]);
        assert_eq!(find("\u{FB00}x", "x", false, false), vec![3..4]);
    }

    #[test]
    fn lowercasing_is_per_character_not_contextual() {
        // Final sigma: per-character lowercasing always yields U+03C3.
        assert_eq!(find("\u{3A3}", "\u{3C3}", false, false), vec![0..2]);
        assert!(find("\u{3A3}", "\u{3C2}", false, false).is_empty());
    }

    #[test]
    fn whole_word_uses_original_neighbors_and_underscores_are_word_characters() {
        let text = "foo_bar foo";
        assert_eq!(find(text, "foo", false, true), vec![8..11]);
        assert_eq!(find(text, "foo", false, false), vec![0..3, 8..11]);
        assert_eq!(
            find("foo.foo,foo", "foo", false, true),
            vec![0..3, 4..7, 8..11]
        );
        assert_eq!(
            find("xfoo foox", "foo", false, true),
            Vec::<Range<usize>>::new()
        );
        // Unicode alphanumerics are word characters.
        assert!(find("\u{E9}foo", "foo", false, true).is_empty());
        assert_eq!(find("5 foo", "foo", false, true), vec![2..5]);
        assert!(find("5foo", "foo", false, true).is_empty());
    }

    #[test]
    fn whole_word_rejection_still_finds_later_overlapping_candidates() {
        // "aa" at 0 is followed by "a" (rejected); the candidate at 1 is preceded by "a"
        // (rejected); the one at 4 is accepted.
        assert_eq!(find("aaa aa", "aa", false, true), vec![4..6]);
        // The first two candidates are glued to underscores; the last one passes.
        assert_eq!(find("a_a a", "a", false, true), vec![4..5]);
    }

    #[test]
    fn whole_word_after_an_expansion_checks_the_original_neighbor() {
        // Neighbor of "b" in the original is U+0130, a letter: not a boundary.
        assert!(find("\u{130}b", "b", false, true).is_empty());
        assert_eq!(find("\u{130} b", "b", false, true), vec![3..4]);
    }

    #[test]
    fn scans_are_counted_once_per_group_search() {
        let matcher = GroupMatcher::new("red green red".to_owned());
        let query = Query::new("red", false, false);
        let before = scan_count();
        matcher.find(&query);
        matcher.find(&Query::new("green", false, false));
        assert_eq!(scan_count() - before, 2);
        // An empty query does no scan.
        matcher.find(&Query::new("", false, false));
        assert_eq!(scan_count() - before, 2);
    }

    #[test]
    fn utf16_helpers_handle_astral_characters() {
        let text = "a\u{1F600}b";
        assert_eq!(utf16_len(text), 4);
        assert_eq!(utf16_to_byte(text, 0), Some(0));
        assert_eq!(utf16_to_byte(text, 1), Some(1));
        assert_eq!(utf16_to_byte(text, 2), None);
        assert_eq!(utf16_to_byte(text, 3), Some(5));
        assert_eq!(utf16_to_byte(text, 4), Some(6));
        assert_eq!(utf16_to_byte(text, 5), None);
        assert_eq!(byte_offsets_to_utf16(text, [0, 1, 5, 6]), vec![0, 1, 3, 4]);
    }
}
