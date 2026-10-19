//! List indent and outdent over the selected lines (R13).

use super::newline::list_line;
use super::parse::Parsed;
use super::text::{content_end, line_end, line_start};
use std::ops::Range;

/// Characters added before a list marker by Tab and removed (at most) by Shift+Tab.
const INDENT_STEP: usize = 2;

/// The result of a list indent or outdent: one contiguous replacement covering the selected
/// lines and the remapped selection. Applying it is one undo step that breaks typing
/// coalescing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndentEdit {
    /// Source range replaced: from the start of the first selected line to the end of the last
    /// one (its line terminator excluded). Empty when nothing changes.
    pub range: Range<usize>,
    /// Replacement text for `range`.
    pub text: String,
    /// The selection after the edit, as `start..end` with `start <= end`.
    pub selection: Range<usize>,
    /// Whether the selection's head precedes its anchor; preserved from the request.
    pub reversed: bool,
}

impl IndentEdit {
    /// Whether the edit changes nothing (an outdent where no selected line is indented).
    pub fn is_noop(&self) -> bool {
        self.range.is_empty() && self.text.is_empty()
    }

    /// `source` with the replacement applied.
    pub fn apply(&self, source: &str) -> String {
        let mut text = String::with_capacity(source.len() + self.text.len());
        text.push_str(&source[..self.range.start]);
        text.push_str(&self.text);
        text.push_str(&source[self.range.end..]);
        text
    }
}

/// One line's change: `removed` bytes at `position` replaced by `added` bytes.
struct Change {
    position: usize,
    removed: usize,
    added: usize,
}

/// Indents (`outdent == false`) or outdents the selected lines (R13), or returns `None` (the key
/// propagates to ordinary focus traversal) when the selection is not a valid range of `source`
/// or any selected line is not a list line outside code.
///
/// The selected lines are the logical lines the selection touches; a non-empty selection ending
/// right after a line feed does not include the line that follows. Indent inserts two spaces
/// before each marker (after the quote prefix); outdent removes up to two whitespace characters
/// from the start of each marker's indentation. Each selection endpoint `o` moves by
/// `added - min(removed, o - position)` for every change at `position <= o`. `reversed` is
/// returned unchanged in the result. The caller applies the rest of R13's conditions (Markdown
/// mode, editable, no marked range, no completion popup). Not subject to the processing limit.
pub fn indent_edit(
    source: &str,
    selection: Range<usize>,
    reversed: bool,
    outdent: bool,
) -> Option<IndentEdit> {
    let (start, end) = if selection.start <= selection.end {
        (selection.start, selection.end)
    } else {
        (selection.end, selection.start)
    };
    if end > source.len() || !source.is_char_boundary(start) || !source.is_char_boundary(end) {
        return None;
    }
    let probe = if end > start && source.as_bytes()[end - 1] == b'\n' {
        end - 1
    } else {
        end
    };
    let parsed = Parsed::parse(source);
    let first = line_start(source, start);
    let mut changes = Vec::new();
    let mut line = first;
    let last_end = loop {
        let terminator = line_end(source, line);
        let prefix = list_line(
            &parsed,
            source,
            line,
            content_end(source, line, terminator),
            true,
        )?;
        if outdent {
            let indentation = prefix.marker_offset - prefix.indent_start;
            let removed = indentation.min(INDENT_STEP);
            if removed > 0 {
                changes.push(Change {
                    position: line + prefix.indent_start,
                    removed,
                    added: 0,
                });
            }
        } else {
            changes.push(Change {
                position: line + prefix.marker_offset,
                removed: 0,
                added: INDENT_STEP,
            });
        }
        if terminator >= source.len() || terminator + 1 > probe {
            break terminator;
        }
        line = terminator + 1;
    };
    let remap = |offset: usize| {
        let shift: isize = changes
            .iter()
            .take_while(|change| change.position <= offset)
            .map(|change| {
                change.added as isize - change.removed.min(offset - change.position) as isize
            })
            .sum();
        offset.saturating_add_signed(shift)
    };
    let selection = remap(start)..remap(end);
    if changes.is_empty() {
        return Some(IndentEdit {
            range: first..first,
            text: String::new(),
            selection,
            reversed,
        });
    }
    let mut text = String::with_capacity(last_end - first + changes.len() * INDENT_STEP);
    let mut cursor = first;
    for change in &changes {
        text.push_str(&source[cursor..change.position]);
        text.push_str(&"  "[..change.added]);
        cursor = change.position + change.removed;
    }
    text.push_str(&source[cursor..last_end]);
    Some(IndentEdit {
        range: first..last_end,
        text,
        selection,
        reversed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indent(source: &str, selection: Range<usize>, outdent: bool) -> Option<IndentEdit> {
        indent_edit(source, selection, false, outdent)
    }

    #[test]
    fn tab_over_three_lines_adds_two_spaces_and_remaps_the_selection() {
        let source = "- a\n- b\n- c";
        let edit = indent(source, 2..9, false).expect("list lines");
        assert_eq!(edit.apply(source), "  - a\n  - b\n  - c");
        // The start (inside line 1) moves by 2; the end (inside line 3) by all three shifts.
        assert_eq!(edit.selection, 4..15);
        assert_eq!(edit.range, 0..source.len());
        assert!(!edit.reversed);
    }

    #[test]
    fn selection_endpoints_before_a_marker_do_not_move_with_other_lines() {
        let source = "- a\n- b";
        // Collapsed caret at the very start of line 2 (before the marker at the same offset).
        let edit = indent(source, 4..4, false).expect("list line");
        assert_eq!(edit.apply(source), "- a\n  - b");
        assert_eq!(edit.selection, 6..6);
        let edit = indent(source, 0..0, false).expect("list line");
        assert_eq!(edit.apply(source), "  - a\n- b");
        assert_eq!(edit.selection, 2..2);
    }

    #[test]
    fn shift_tab_removes_up_to_two() {
        let source = "- a\n  - b\n    - c\n- d";
        let edit = indent(source, 0..source.len(), true).expect("list lines");
        assert_eq!(edit.apply(source), "- a\n- b\n  - c\n- d");
        let edit = indent(source, 0..1, true).expect("list line");
        assert!(edit.is_noop());
        let edit = indent(source, 5..5, true).expect("list line");
        assert_eq!(edit.apply(source), "- a\n- b\n    - c\n- d");
    }

    #[test]
    fn outdent_remaps_by_removed_widths() {
        let source = "- a\n    - b\n  - c";
        // Offsets inside the removed run move by only what lies before them.
        let edit = indent(source, 5..14, true).expect("list lines");
        assert_eq!(edit.apply(source), "- a\n  - b\n- c");
        assert_eq!(edit.selection, 4..10);
        let edit = indent(source, 5..5, true).expect("list line");
        assert_eq!(edit.selection, 4..4);
        let edit = indent(source, 11..11, true).expect("list line");
        assert_eq!(edit.selection, 9..9);
    }

    #[test]
    fn quote_prefixes_are_kept() {
        let source = "> - a\n>   - b";
        let edit = indent(source, 0..source.len(), false).expect("list lines");
        assert_eq!(edit.apply(source), ">   - a\n>     - b");
        let edit = indent(source, 0..source.len(), true).expect("list lines");
        assert_eq!(edit.apply(source), "> - a\n> - b");
    }

    #[test]
    fn a_non_list_line_leaves_the_source_unchanged() {
        assert_eq!(indent("- a\ntext\n- b", 0..12, false), None);
        assert_eq!(indent("- a\n\n- b", 0..8, true), None);
        assert_eq!(indent("text", 0..0, false), None);
        assert_eq!(indent("- a\n```\n- b\n```", 4..11, false), None);
        assert_eq!(indent("    - code", 5..5, false), None);
        assert_eq!(indent("- - -", 0..0, false), None);
        assert_eq!(indent("Title\n-", 6..7, false), None);
    }

    #[test]
    fn a_selection_ending_after_a_line_feed_excludes_the_next_line() {
        let source = "- a\n- b\ntext";
        let edit = indent(source, 0..8, false).expect("two list lines");
        assert_eq!(edit.apply(source), "  - a\n  - b\ntext");
        assert_eq!(edit.selection, 2..12);
        assert_eq!(indent(source, 0..9, false), None);
    }

    #[test]
    fn crlf_lines_keep_their_terminators() {
        let source = "- a\r\n- b";
        let edit = indent(source, 0..source.len(), false).expect("list lines");
        assert_eq!(edit.apply(source), "  - a\r\n  - b");
    }

    #[test]
    fn reversed_is_preserved() {
        let edit = indent_edit("- a", 0..3, true, false).expect("list line");
        assert!(edit.reversed);
        assert_eq!(edit.selection, 2..5);
        let (anchor, head) = (3, 0);
        let swapped = indent_edit("- a", anchor..head, true, false).expect("list line");
        assert_eq!(swapped.selection, 2..5);
    }

    #[test]
    fn outdent_without_indentation_is_a_noop() {
        let edit = indent("- a\n- b", 0..7, true).expect("list lines");
        assert!(edit.is_noop());
        assert_eq!(edit.selection, 0..7);
    }

    #[test]
    fn invalid_selections_are_rejected() {
        assert_eq!(indent("- a", 0..9, false), None);
        assert_eq!(indent("- \u{e9}", 3..3, false), None);
    }

    #[test]
    fn tab_and_shift_tab_round_trip() {
        let source = "- a\n  - b\n- c";
        let selection = 1..source.len() - 1;
        let tabbed = indent(source, selection.clone(), false).expect("list lines");
        let text = tabbed.apply(source);
        let back = indent(&text, tabbed.selection, true).expect("list lines");
        assert_eq!(back.apply(&text), source);
        assert_eq!(back.selection, selection);
    }
}
