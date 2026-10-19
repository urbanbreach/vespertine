//! The text-search helper for hosts that search a document themselves (TXT-009 R13).

use std::ops::Range;

use super::matcher::{GroupMatcher, Query, byte_offsets_to_utf16, utf16_len};

/// Matches of `query` over a document made of `texts` joined by a single `\n`, as UTF-16 code
/// unit ranges of that document, in document order.
///
/// The lowercasing, mapping and whole-word rules are those of the declaration (R4-R5), and
/// each text is a logical group in the sense of R6: a match never crosses from one text into the
/// next (R13 does not say otherwise and the declaration it mirrors never matches across groups),
/// so a query containing `\n` finds nothing. An empty query finds nothing. The ranges can be fed
/// back as explicit ranges of a declaration over groups with the same texts and select exactly
/// what the query declaration paints.
pub fn find_ranges(
    texts: &[&str],
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> Vec<Range<usize>> {
    let query = Query::new(query, case_sensitive, whole_word);
    let mut ranges = Vec::new();
    let mut document_start = 0;
    for text in texts {
        if !query.is_empty() {
            let found = GroupMatcher::new((*text).to_owned()).find(&query);
            let offsets = byte_offsets_to_utf16(
                text,
                found.iter().flat_map(|range| [range.start, range.end]),
            );
            ranges.extend(
                offsets
                    .chunks_exact(2)
                    .map(|pair| document_start + pair[0]..document_start + pair[1]),
            );
        }
        document_start += utf16_len(text) + 1;
    }
    ranges
}

/// The active-match cursor of a finder: next and previous with wraparound, clamped when the
/// number of matches shrinks, reset when the query changes.
///
/// The index it reports is a position in the list returned by [`find_ranges`]; give it to a
/// declaration as `activeIndex` (together with the `matchIndexOffset` of the mounted window).
#[derive(Clone, Debug, Default)]
pub struct FindCursor {
    query: Option<(String, bool, bool)>,
    count: usize,
    index: Option<usize>,
}

impl FindCursor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tells the cursor the current query and number of matches. A query (or flag) different
    /// from the last one resets the cursor to "no active match"; otherwise a shrinking count
    /// clamps the index to the last match, and a count of zero clears it.
    pub fn update(&mut self, query: &str, case_sensitive: bool, whole_word: bool, count: usize) {
        let unchanged = self
            .query
            .as_ref()
            .is_some_and(|(q, c, w)| q == query && *c == case_sensitive && *w == whole_word);
        if !unchanged {
            self.query = Some((query.to_owned(), case_sensitive, whole_word));
            self.index = None;
        }
        self.count = count;
        self.index = match (self.index, count) {
            (_, 0) => None,
            (Some(index), count) => Some(index.min(count - 1)),
            (None, _) => None,
        };
    }

    /// Runs [`find_ranges`] and updates the cursor with the result.
    pub fn search(
        &mut self,
        texts: &[&str],
        query: &str,
        case_sensitive: bool,
        whole_word: bool,
    ) -> Vec<Range<usize>> {
        let ranges = find_ranges(texts, query, case_sensitive, whole_word);
        self.update(query, case_sensitive, whole_word, ranges.len());
        ranges
    }

    /// The active match index, if any.
    pub fn current(&self) -> Option<usize> {
        self.index
    }

    /// The active match's range out of the list that was last searched.
    pub fn active_range<'a>(&self, ranges: &'a [Range<usize>]) -> Option<&'a Range<usize>> {
        ranges.get(self.index?)
    }

    /// Moves to the next match, wrapping from the last to the first; from "no active match" it
    /// goes to the first. `None` when there are no matches.
    pub fn next(&mut self) -> Option<usize> {
        self.index = match (self.index, self.count) {
            (_, 0) => None,
            (None, _) => Some(0),
            (Some(index), count) => Some((index + 1) % count),
        };
        self.index
    }

    /// Moves to the previous match, wrapping from the first to the last; from "no active match"
    /// it goes to the last. `None` when there are no matches.
    pub fn previous(&mut self) -> Option<usize> {
        self.index = match (self.index, self.count) {
            (_, 0) => None,
            (None, count) | (Some(0), count) => Some(count - 1),
            (Some(index), _) => Some(index - 1),
        };
        self.index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::resolve::{GroupInput, Resolver, Segment};
    use crate::search::spec::{HighlightDeclaration, HighlightSpec};

    #[test]
    fn ranges_are_utf16_offsets_into_the_newline_joined_document() {
        // Document: "Red\n\u{1F600} red\nred" -> units: "Red" 0..3, "\n" 3, emoji 4..6,
        // space 6, "red" 7..10, "\n" 10, "red" 11..14.
        let texts = ["Red", "\u{1F600} red", "red"];
        assert_eq!(
            find_ranges(&texts, "red", false, false),
            vec![0..3, 7..10, 11..14]
        );
        assert_eq!(find_ranges(&texts, "red", true, false), vec![7..10, 11..14]);
        assert!(find_ranges(&texts, "", false, false).is_empty());
        assert!(find_ranges(&[], "red", false, false).is_empty());
    }

    #[test]
    fn empty_texts_still_occupy_a_line_of_the_document() {
        assert_eq!(find_ranges(&["", "a"], "a", false, false), vec![1..2]);
    }

    #[test]
    fn matches_never_cross_texts_and_follow_the_declaration_rules() {
        assert!(find_ranges(&["ab", "cd"], "bc", false, false).is_empty());
        assert!(find_ranges(&["ab", "cd"], "b\nc", false, false).is_empty());
        assert_eq!(
            find_ranges(&["foo_bar foo"], "foo", false, true),
            vec![8..11]
        );
        // U+0130 and the ligature obey R4.
        assert!(find_ranges(&["\u{130}"], "i", false, false).is_empty());
        assert_eq!(
            find_ranges(&["a\u{130}"], "i\u{307}", false, false),
            vec![1..2]
        );
        assert!(find_ranges(&["\u{FB00}"], "ff", false, false).is_empty());
    }

    fn group_texts(texts: &[&str]) -> Vec<GroupInput> {
        texts
            .iter()
            .enumerate()
            .map(|(id, text)| GroupInput::new(id as u64, [*text]))
            .collect()
    }

    /// UTF-16 document ranges painted by a declaration's matches.
    fn declaration_ranges(
        texts: &[&str],
        resolution: &crate::search::resolve::Resolution,
    ) -> Vec<Range<usize>> {
        let mut starts = Vec::new();
        let mut next = 0;
        for text in texts {
            starts.push(next);
            next += utf16_len(text) + 1;
        }
        resolution
            .matches
            .iter()
            .map(|resolved| {
                let first: &Segment = resolved.segments.first().unwrap();
                let last: &Segment = resolved.segments.last().unwrap();
                let group_start = starts[first.group_id as usize];
                let start =
                    group_start + utf16_len(&texts[first.group_id as usize][..first.range.start]);
                let group_start = starts[last.group_id as usize];
                let end = group_start + utf16_len(&texts[last.group_id as usize][..last.range.end]);
                start..end
            })
            .collect()
    }

    #[test]
    fn helper_and_declaration_agree_on_identical_inputs() {
        let texts = [
            "Hello Tom",
            "tom and TOM, tomato",
            "\u{1F600} tom\u{130} _tom_ tom",
            "",
            "foo_bar foo",
        ];
        let groups = group_texts(&texts);
        for query in ["tom", "TOM", "o", "foo", "i\u{307}", "\u{1F600}", "m"] {
            for case_sensitive in [false, true] {
                for whole_word in [false, true] {
                    let spec = HighlightSpec::new(query)
                        .with_case_sensitive(case_sensitive)
                        .with_whole_word(whole_word);
                    let resolution =
                        Resolver::new().resolve(&HighlightDeclaration::from_spec(spec), &groups);
                    let found = find_ranges(&texts, query, case_sensitive, whole_word);
                    assert_eq!(
                        found,
                        declaration_ranges(&texts, &resolution),
                        "query {query:?} cs={case_sensitive} ww={whole_word}"
                    );

                    // Fed back as explicit ranges the same segments are selected.
                    let explicit = HighlightSpec::from_ranges(found.clone());
                    let from_ranges = Resolver::new()
                        .resolve(&HighlightDeclaration::from_spec(explicit), &groups);
                    let segments = |r: &crate::search::resolve::Resolution| {
                        r.matches
                            .iter()
                            .map(|m| m.segments.clone())
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(segments(&from_ranges), segments(&resolution));
                }
            }
        }
    }

    #[test]
    fn cursor_wraps_in_both_directions() {
        let mut cursor = FindCursor::new();
        let texts = ["a a a"];
        let ranges = cursor.search(&texts, "a", false, false);
        assert_eq!(ranges.len(), 3);
        assert_eq!(cursor.current(), None);
        assert_eq!(cursor.active_range(&ranges), None);

        assert_eq!(cursor.next(), Some(0));
        assert_eq!(cursor.active_range(&ranges), Some(&(0..1)));
        assert_eq!(cursor.next(), Some(1));
        assert_eq!(cursor.next(), Some(2));
        assert_eq!(cursor.next(), Some(0), "wraps forward");
        assert_eq!(cursor.previous(), Some(2), "wraps backward");
        assert_eq!(cursor.previous(), Some(1));
    }

    #[test]
    fn previous_from_no_selection_goes_to_the_last_match() {
        let mut cursor = FindCursor::new();
        cursor.update("a", false, false, 4);
        assert_eq!(cursor.previous(), Some(3));
    }

    #[test]
    fn cursor_clamps_when_the_count_shrinks_and_clears_at_zero() {
        let mut cursor = FindCursor::new();
        cursor.update("a", false, false, 5);
        cursor.next();
        for _ in 0..4 {
            cursor.next();
        }
        assert_eq!(cursor.current(), Some(4));
        cursor.update("a", false, false, 2);
        assert_eq!(cursor.current(), Some(1));
        cursor.update("a", false, false, 6);
        assert_eq!(cursor.current(), Some(1), "growth keeps the index");
        cursor.update("a", false, false, 0);
        assert_eq!(cursor.current(), None);
        assert_eq!(cursor.next(), None);
        assert_eq!(cursor.previous(), None);
        // Matches reappearing under the same query do not select anything by themselves.
        cursor.update("a", false, false, 3);
        assert_eq!(cursor.current(), None);
    }

    #[test]
    fn a_new_query_or_flag_resets_the_cursor() {
        let mut cursor = FindCursor::new();
        cursor.update("a", false, false, 3);
        cursor.next();
        cursor.next();
        assert_eq!(cursor.current(), Some(1));
        cursor.update("b", false, false, 3);
        assert_eq!(cursor.current(), None);
        cursor.next();
        cursor.update("b", true, false, 3);
        assert_eq!(cursor.current(), None);
        cursor.next();
        cursor.update("b", true, true, 3);
        assert_eq!(cursor.current(), None);
    }
}
