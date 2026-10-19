//! Insertion and removal of references as pure edit computations (CMP-010 R10, R11).
//!
//! The editor applies the returned edit as one non-coalescing undo step; nothing here touches
//! editor state.

use super::reference::ReferenceDescriptor;
use std::ops::Range;

/// The editor state that gates reference edits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EditGate {
    /// The editor is read-only.
    pub read_only: bool,
    /// An input-method composition (a marked range) is active.
    pub composing: bool,
}

impl EditGate {
    /// Whether reference insertion and removal are allowed: not read-only and not composing.
    pub fn allows_reference_edit(&self) -> bool {
        !self.read_only && !self.composing
    }
}

/// A computed insertion: replace `range` of the old source with `text`, then put the caret at
/// `caret`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceInsertion {
    /// The source range to replace (the selection, or the completion token).
    pub range: Range<usize>,
    /// The text to insert: optional leading space, the link, optional trailing space.
    pub text: String,
    /// The link's range in the source after the edit.
    pub link: Range<usize>,
    /// The caret offset after the edit: after the link and any inserted or consumed trailing
    /// separator.
    pub caret: usize,
}

impl ReferenceInsertion {
    /// The source after applying the edit to `source`.
    pub fn apply(&self, source: &str) -> String {
        let mut out = String::with_capacity(source.len() + self.text.len());
        out.push_str(&source[..self.range.start]);
        out.push_str(&self.text);
        out.push_str(&source[self.range.end..]);
        out
    }
}

/// Characters after which no separating space is added before a link.
const NO_LEADING_SEPARATOR: &[char] = &['(', '[', '{', '*', '_', '~'];
/// Characters before which no separating space is added after a link.
const NO_TRAILING_SEPARATOR: &[char] =
    &[')', ']', '}', '*', '_', '~', ',', '.', ';', ':', '!', '?'];

fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r')
}

/// What follows a link, decided by the next character (R10).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Trailing {
    /// Nothing is inserted.
    Nothing,
    /// The next character is whitespace: nothing is inserted and the caret moves past it.
    Consume(char),
    /// One space is inserted.
    Space,
}

fn trailing_rule(next: Option<char>) -> Trailing {
    match next {
        Some(c) if is_line_break(c) => Trailing::Nothing,
        Some(c) if c.is_whitespace() => Trailing::Consume(c),
        Some(c) if NO_TRAILING_SEPARATOR.contains(&c) => Trailing::Nothing,
        _ => Trailing::Space,
    }
}

fn valid_range(source: &str, range: &Range<usize>) -> bool {
    range.start <= range.end
        && range.end <= source.len()
        && source.is_char_boundary(range.start)
        && source.is_char_boundary(range.end)
}

fn build(source: &str, range: Range<usize>, link: &str, leading: bool) -> ReferenceInsertion {
    let mut text = String::with_capacity(link.len() + 2);
    if leading {
        text.push(' ');
    }
    let link_start = range.start + text.len();
    text.push_str(link);
    let link_end = range.start + text.len();
    let mut caret = link_end;
    match trailing_rule(source[range.end..].chars().next()) {
        Trailing::Nothing => {}
        Trailing::Consume(c) => caret += c.len_utf8(),
        Trailing::Space => {
            text.push(' ');
            caret += 1;
        }
    }
    ReferenceInsertion {
        range,
        text,
        link: link_start..link_end,
        caret,
    }
}

/// `insert_reference(link)` (R10): replaces `selection` with `link`, adding a leading space when
/// the character before the selection exists and is neither whitespace nor one of `( [ { * _ ~`,
/// and a trailing separator by the next character: none before a line break, none (caret moves
/// past it) before other whitespace, none before `) ] } * _ ~ , . ; : ! ?`, otherwise one space
/// (also at the end of the text). The caret ends after the link and the consumed or inserted
/// separator.
///
/// Returns `None` when `gate` rejects the edit (read-only or composing) or when `selection` is
/// not a valid range of `source`.
pub fn insert_reference(
    source: &str,
    selection: Range<usize>,
    link: &str,
    gate: EditGate,
) -> Option<ReferenceInsertion> {
    if !gate.allows_reference_edit() || !valid_range(source, &selection) {
        return None;
    }
    let leading = source[..selection.start]
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_whitespace() && !NO_LEADING_SEPARATOR.contains(&c));
    Some(build(source, selection, link, leading))
}

/// `replace_reference(range, link)` (R10): replaces a completed completion token with `link`
/// using the same trailing rule as [`insert_reference`] and no leading separator.
pub fn replace_reference(
    source: &str,
    range: Range<usize>,
    link: &str,
    gate: EditGate,
) -> Option<ReferenceInsertion> {
    if !gate.allows_reference_edit() || !valid_range(source, &range) {
        return None;
    }
    Some(build(source, range, link, false))
}

/// Joins several links by single spaces so they can be inserted as one edit (R10).
pub fn join_links<S: AsRef<str>>(links: &[S]) -> String {
    let mut out = String::new();
    for (index, link) in links.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(link.as_ref());
    }
    out
}

/// A computed removal: the ascending, non-overlapping source ranges to delete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceRemoval {
    /// The ranges to delete: each chip plus, where the rule applies, one following space.
    pub removed: Vec<Range<usize>>,
}

impl ReferenceRemoval {
    /// Maps an offset of the old source to the new source: offsets after a removed range shift by
    /// its width, offsets inside one move to its start.
    pub fn map_offset(&self, offset: usize) -> usize {
        let mut shift = 0;
        for range in &self.removed {
            if offset >= range.end {
                shift += range.end - range.start;
            } else if offset > range.start {
                shift += offset - range.start;
            }
        }
        offset - shift
    }

    /// Shifts a selection by the removed widths.
    pub fn map_range(&self, range: Range<usize>) -> Range<usize> {
        self.map_offset(range.start)..self.map_offset(range.end)
    }

    /// The source after the removal.
    pub fn apply(&self, source: &str) -> String {
        let mut out = String::with_capacity(source.len());
        let mut cursor = 0;
        for range in &self.removed {
            out.push_str(&source[cursor..range.start]);
            cursor = range.end;
        }
        out.push_str(&source[cursor..]);
        out
    }
}

/// `remove_attachment_references(number)` (R11): removes every chip whose attachment number
/// matches, together with one following space when the chip stood at the start of the text or
/// after whitespace and a space follows it. Returns `None` when nothing matches. The editor
/// applies the removal as a single undo step, maps the selection with
/// [`ReferenceRemoval::map_range`], and rejects the call during read-only or composition
/// (see [`EditGate`]).
pub fn remove_attachment_references(
    source: &str,
    descriptors: &[ReferenceDescriptor],
    number: u32,
) -> Option<ReferenceRemoval> {
    let mut removed: Vec<Range<usize>> = Vec::new();
    for descriptor in descriptors
        .iter()
        .filter(|descriptor| descriptor.attachment == Some(number))
    {
        let range = &descriptor.range;
        if range.start > range.end
            || range.end > source.len()
            || !source.is_char_boundary(range.start)
            || !source.is_char_boundary(range.end)
        {
            continue;
        }
        let at_boundary = source[..range.start]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        let space_follows = source[range.end..].starts_with(' ');
        let next_chip_starts_here = descriptors
            .iter()
            .any(|other| other.range.start <= range.end && range.end < other.range.end);
        let end = if at_boundary && space_follows && !next_chip_starts_here {
            range.end + 1
        } else {
            range.end
        };
        removed.push(range.start..end);
    }
    removed.sort_by_key(|range| range.start);
    (!removed.is_empty()).then_some(ReferenceRemoval { removed })
}

#[cfg(test)]
mod tests {
    use super::super::reference::{ReferenceKind, test_support::descriptor};
    use super::*;

    const GATE: EditGate = EditGate {
        read_only: false,
        composing: false,
    };

    /// Inserts `LINK` at `caret` into `source` and returns the new source and caret.
    fn insert_at(source: &str, caret: usize) -> (String, usize) {
        let edit = insert_reference(source, caret..caret, "LINK", GATE).unwrap();
        (edit.apply(source), edit.caret)
    }

    #[test]
    fn next_character_classes_decide_the_trailing_separator() {
        // line break: nothing, caret directly after the link
        let (source, caret) = insert_at("\nrest", 0);
        assert_eq!(source, "LINK\nrest");
        assert_eq!(caret, 4);
        let (source, _) = insert_at("\r\nrest", 0);
        assert_eq!(source, "LINK\r\nrest");

        // whitespace: nothing inserted, caret passes the space
        let (source, caret) = insert_at(" rest", 0);
        assert_eq!(source, "LINK rest");
        assert_eq!(caret, 5);
        let (source, caret) = insert_at("\trest", 0);
        assert_eq!(source, "LINK\trest");
        assert_eq!(caret, 5);

        // closing punctuation: nothing
        for next in [")", "]", "}", "*", "_", "~", ",", ".", ";", ":", "!", "?"] {
            let source = format!("{next}x");
            let (out, caret) = insert_at(&source, 0);
            assert_eq!(out, format!("LINK{next}x"), "{next}");
            assert_eq!(caret, 4, "{next}");
        }

        // a letter: one space inserted
        let (source, caret) = insert_at("word", 0);
        assert_eq!(source, "LINK word");
        assert_eq!(caret, 5);

        // end of text: one space inserted
        let (source, caret) = insert_at("", 0);
        assert_eq!(source, "LINK ");
        assert_eq!(caret, 5);
    }

    #[test]
    fn previous_character_classes_decide_the_leading_separator() {
        // start of text: none
        assert_eq!(insert_at("", 0).0, "LINK ");
        // whitespace: none
        let (source, _) = insert_at("a ", 2);
        assert_eq!(source, "a LINK ");
        let (source, _) = insert_at("a\n", 2);
        assert_eq!(source, "a\nLINK ");
        // opening punctuation: none
        for previous in ["(", "[", "{", "*", "_", "~"] {
            let source = previous.to_string();
            let (out, _) = insert_at(&source, source.len());
            assert_eq!(out, format!("{previous}LINK "), "{previous}");
        }
        // a letter: one space
        let (source, caret) = insert_at("see", 3);
        assert_eq!(source, "see LINK ");
        assert_eq!(caret, 9);
        // other punctuation counts as non-whitespace
        let (source, _) = insert_at("a,", 2);
        assert_eq!(source, "a, LINK ");
    }

    #[test]
    fn insertion_replaces_the_selection() {
        let source = "open @fo now";
        let edit = insert_reference(source, 5..8, "LINK", GATE).unwrap();
        assert_eq!(edit.range, 5..8);
        assert_eq!(edit.apply(source), "open LINK now");
        assert_eq!(edit.link, 5..9);
        assert_eq!(edit.caret, 10);
        assert_eq!(&edit.apply(source)[edit.link], "LINK");
    }

    #[test]
    fn leading_separator_is_part_of_the_inserted_text() {
        let source = "see";
        let edit = insert_reference(source, 3..3, "LINK", GATE).unwrap();
        assert_eq!(edit.text, " LINK ");
        assert_eq!(edit.link, 4..8);
        assert_eq!(edit.caret, 9);
    }

    #[test]
    fn multibyte_neighbours_are_handled() {
        let source = "é";
        let edit = insert_reference(source, 2..2, "LINK", GATE).unwrap();
        assert_eq!(edit.apply(source), "é LINK ");
        let source = "éa";
        let edit = insert_reference(source, 0..0, "LINK", GATE).unwrap();
        assert_eq!(edit.apply(source), "LINK éa");
        let source = "\u{00A0}x";
        let edit = insert_reference(source, 0..0, "LINK", GATE).unwrap();
        // NBSP is whitespace: consumed, caret passes it (2 bytes).
        assert_eq!(edit.caret, 4 + 2);
        assert_eq!(edit.apply(source), "LINK\u{00A0}x");
    }

    #[test]
    fn insertion_is_rejected_when_read_only_or_composing() {
        let read_only = EditGate {
            read_only: true,
            composing: false,
        };
        let composing = EditGate {
            read_only: false,
            composing: true,
        };
        assert!(insert_reference("a", 1..1, "LINK", read_only).is_none());
        assert!(insert_reference("a", 1..1, "LINK", composing).is_none());
        assert!(replace_reference("a", 0..1, "LINK", read_only).is_none());
        assert!(replace_reference("a", 0..1, "LINK", composing).is_none());
        assert!(!read_only.allows_reference_edit());
        assert!(!composing.allows_reference_edit());
        assert!(GATE.allows_reference_edit());
    }

    #[test]
    fn invalid_selection_is_rejected() {
        assert!(insert_reference("é", 1..1, "LINK", GATE).is_none());
        assert!(insert_reference("ab", 1..5, "LINK", GATE).is_none());
        let reversed = Range { start: 2, end: 1 };
        assert!(insert_reference("ab", reversed, "LINK", GATE).is_none());
    }

    #[test]
    fn replace_reference_uses_the_trailing_rule_without_a_leading_separator() {
        let source = "see @fo";
        let edit = replace_reference(source, 4..7, "LINK", GATE).unwrap();
        assert_eq!(edit.apply(source), "see LINK ");
        assert_eq!(edit.caret, 9);

        let source = "x@fo";
        let edit = replace_reference(source, 1..4, "LINK", GATE).unwrap();
        assert_eq!(edit.apply(source), "xLINK ");

        let source = "see @fo, ok";
        let edit = replace_reference(source, 4..7, "LINK", GATE).unwrap();
        assert_eq!(edit.apply(source), "see LINK, ok");
        assert_eq!(edit.caret, 8);

        let source = "see @fo ok";
        let edit = replace_reference(source, 4..7, "LINK", GATE).unwrap();
        assert_eq!(edit.apply(source), "see LINK ok");
        assert_eq!(edit.caret, 9);
    }

    #[test]
    fn several_references_are_joined_by_single_spaces() {
        assert_eq!(join_links(&["A", "B", "C"]), "A B C");
        assert_eq!(join_links::<&str>(&[]), "");
        let joined = join_links(&["A".to_owned(), "B".to_owned()]);
        let source = "compare ";
        let edit = insert_reference(source, 8..8, &joined, GATE).unwrap();
        assert_eq!(edit.apply(source), "compare A B ");
        assert_eq!(edit.caret, 12);
    }

    fn chip(range: Range<usize>, number: u32) -> ReferenceDescriptor {
        descriptor(range, ReferenceKind::Image, "x", "x", Some(number))
    }

    #[test]
    fn removal_takes_one_following_space_after_a_boundary() {
        // "compare [1] [2] tail" with 3-char chips.
        let source = "compare [1] [2] tail";
        let chips = [chip(8..11, 1), chip(12..15, 2)];
        let removal = remove_attachment_references(source, &chips, 1).unwrap();
        assert_eq!(removal.removed, vec![8..12]);
        assert_eq!(removal.apply(source), "compare [2] tail");
        let removal = remove_attachment_references(source, &chips, 2).unwrap();
        assert_eq!(removal.apply(source), "compare [1] tail");
    }

    #[test]
    fn removal_at_the_start_of_the_text_takes_the_space() {
        let source = "[1] rest";
        let removal = remove_attachment_references(source, &[chip(0..3, 1)], 1).unwrap();
        assert_eq!(removal.apply(source), "rest");
    }

    #[test]
    fn removal_keeps_spaces_when_the_chip_follows_a_non_space() {
        let source = "a[1] b";
        let removal = remove_attachment_references(source, &[chip(1..4, 1)], 1).unwrap();
        assert_eq!(removal.apply(source), "a b");
        // No following space: only the chip goes.
        let source = "a [1]b";
        let removal = remove_attachment_references(source, &[chip(2..5, 1)], 1).unwrap();
        assert_eq!(removal.apply(source), "a b");
        let source = "a [1]";
        let removal = remove_attachment_references(source, &[chip(2..5, 1)], 1).unwrap();
        assert_eq!(removal.apply(source), "a ");
    }

    #[test]
    fn removal_after_a_line_break_takes_a_following_space_only() {
        let source = "a\n[1]  b";
        let removal = remove_attachment_references(source, &[chip(2..5, 1)], 1).unwrap();
        assert_eq!(removal.apply(source), "a\n b");
        let source = "a\n[1]\nb";
        let removal = remove_attachment_references(source, &[chip(2..5, 1)], 1).unwrap();
        assert_eq!(removal.apply(source), "a\n\nb");
    }

    #[test]
    fn every_chip_of_the_number_is_removed() {
        let source = "[1] x [1] y [1]";
        let chips = [chip(0..3, 1), chip(6..9, 1), chip(12..15, 1)];
        let removal = remove_attachment_references(source, &chips, 1).unwrap();
        assert_eq!(removal.apply(source), "x y ");
        assert_eq!(removal.removed, vec![0..4, 6..10, 12..15]);
    }

    #[test]
    fn adjacent_chips_of_one_number_are_both_removed() {
        let source = "a [1] [1] b";
        let chips = [chip(2..5, 1), chip(6..9, 1)];
        let removal = remove_attachment_references(source, &chips, 1).unwrap();
        assert_eq!(removal.apply(source), "a b");
    }

    #[test]
    fn removal_of_an_unknown_number_removes_nothing() {
        assert!(remove_attachment_references("[1]", &[chip(0..3, 1)], 2).is_none());
        assert!(remove_attachment_references("text", &[], 1).is_none());
        let file = descriptor(0..3, ReferenceKind::File, "x", "x", None);
        assert!(remove_attachment_references("[1]", &[file], 1).is_none());
    }

    #[test]
    fn selection_is_shifted_by_the_removed_widths() {
        let source = "compare [1] [2] tail";
        let chips = [chip(8..11, 1), chip(12..15, 2)];
        let removal = remove_attachment_references(source, &chips, 1).unwrap();
        assert_eq!(removal.map_offset(0), 0);
        assert_eq!(removal.map_offset(8), 8);
        assert_eq!(removal.map_offset(10), 8);
        assert_eq!(removal.map_offset(12), 8);
        assert_eq!(removal.map_offset(13), 9);
        assert_eq!(removal.map_offset(source.len()), source.len() - 4);
        assert_eq!(removal.map_range(12..20), 8..16);
        // A caret at the end of the source lands at the end of the new source.
        assert_eq!(removal.map_offset(20), removal.apply(source).len());
    }
}
